use futures::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, OwnerReference, Time};
use k8s_openapi::chrono::Utc;
use kube::api::DeleteParams;
use kube::api::Patch::Merge;
use kube::runtime::{predicates, reflector, WatchStreamExt};
use kube::{
    api::{ListParams, Patch, PatchParams},
    runtime::{
        controller::{Action, Controller},
        watcher,
    },
    Api, Client, Resource, ResourceExt,
};
use serde_json::json;
use std::string::ToString;
use std::{sync::Arc, time::Duration};
#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use util::{WithItemAdded, WithItemRemoved};

use crate::mapping::{apply_mappings, clone_resource};
use crate::remote_watcher_manager::RemoteWatcherManager;
use crate::resource_extensions::NamespacedApi;
use crate::resources::ResourceSyncStatus;
use crate::{requeue_after, resources::ResourceSync, util, Error, Result, FINALIZER};

static RESOURCE_SYNC_FAILING_CONDITION: &str = "ResourceSyncFailing";

pub struct Context {
    pub client: Client,
    pub remote_watcher_manager: RemoteWatcherManager,
}

macro_rules! apply_patch_params {
    () => {
        PatchParams::apply(&ResourceSync::group(&())).force()
    };
}

async fn reconcile_deleted_resource(
    resource_sync: Arc<ResourceSync>,
    name: &str,
    target_api: NamespacedApi,
    parent_api: &Api<ResourceSync>,
    ctx: Arc<Context>,
) -> Result<Action> {
    if !resource_sync.has_target_finalizer() {
        // We have already removed our finalizer, so nothing more needs to be done
        return Ok(Action::await_change());
    }

    let target_name = &resource_sync.spec.target.resource_ref.name;

    match target_api.get(target_name).await {
        Ok(target) if target.metadata.deletion_timestamp.is_some() => {
            resource_sync
                .start_remote_watches_if_not_watching(ctx)
                .await;
            Ok(Action::await_change())
        }
        Ok(target) => {
            let delete_type = match target.metadata.finalizers {
                Some(finalizers) if !finalizers.is_empty() => &DeleteParams::background(),
                _ => &DeleteParams::foreground(),
            };
            target_api.delete(target_name, delete_type).await?;

            resource_sync
                .start_remote_watches_if_not_watching(ctx)
                .await;
            Ok(Action::await_change())
        }
        Err(kube::Error::Api(err)) if err.code == 404 => {
            stop_watches_and_remove_resource_sync_finalizers(resource_sync, name, parent_api, ctx)
                .await
        }
        Err(err) => Err(err.into()),
    }
}

async fn stop_watches_and_remove_resource_sync_finalizers(
    resource_sync: Arc<ResourceSync>,
    name: &str,
    parent_api: &Api<ResourceSync>,
    ctx: Arc<Context>,
) -> Result<Action> {
    resource_sync.stop_remote_watches_if_watching(ctx).await;

    let patched_finalizers = resource_sync
        .finalizers_clone_or_empty()
        .with_item_removed(&FINALIZER.to_string());

    // Target has been deleted, remove the finalizer from the ResourceSync
    let patch = Merge(json!({
        "metadata": {
            "finalizers": patched_finalizers,
        },
    }));

    parent_api
        .patch(name, &PatchParams::default(), &patch)
        .await?;

    // We have removed our finalizer, so nothing more needs to be done
    Ok(Action::await_change())
}

async fn add_target_finalizer(
    resource_sync: Arc<ResourceSync>,
    name: &str,
    parent_api: &Api<ResourceSync>,
) -> Result<Action> {
    let patched_finalizers = resource_sync
        .finalizers_clone_or_empty()
        .with_push(FINALIZER.to_string());

    let patch = Merge(json!({
        "metadata": {
            "finalizers": patched_finalizers,
        },
    }));

    parent_api
        .patch(name, &PatchParams::default(), &patch)
        .await?;

    requeue_after!(Duration::from_millis(500))
}

async fn reconcile_normally(
    resource_sync: Arc<ResourceSync>,
    name: &str,
    source_api: NamespacedApi,
    target_api: NamespacedApi,
    ctx: Arc<Context>,
) -> Result<Action> {
    let target_namespace = &target_api.namespace;
    let target_ar = &target_api.ar;

    let source = source_api
        .get(&resource_sync.spec.source.resource_ref.name)
        .await
        .map_err(|e| {
            Error::ResourceNotFoundError(
                resource_sync.spec.source.resource_ref.name.clone(),
                source_api.ar.kind,
                e,
            )
        })?;
    debug!(?source, "got source object");

    let target_ref = &resource_sync.spec.target.resource_ref;

    let target = {
        let mut target = if resource_sync.spec.mappings.is_empty() {
            clone_resource(&source, target_ref, target_namespace.as_deref(), target_ar)?
        } else {
            apply_mappings(
                &source,
                target_ref,
                target_namespace.as_deref(),
                target_ar,
                &resource_sync,
            )?
        };

        // If the target is local then add an owner reference to it
        match resource_sync.spec.target.cluster.to_owned() {
            Some(_) => target,
            None => {
                target.owner_references_mut().push(OwnerReference {
                    api_version: ResourceSync::api_version(&()).to_string(),
                    kind: ResourceSync::kind(&()).to_string(),
                    name: name.to_owned(),
                    uid: resource_sync
                        .metadata
                        .uid
                        .to_owned()
                        .ok_or(Error::UIDRequired)?,
                    controller: Some(false),
                    block_owner_deletion: Some(true),
                });

                target
            }
        }
    };

    debug!(?target, "produced target object");

    let ssapply = apply_patch_params!();
    target_api
        .patch(&target_ref.name, &ssapply, &Patch::Apply(&target))
        .await?;

    resource_sync
        .start_remote_watches_if_not_watching(ctx)
        .await;

    info!(?name, ?target_ref, "successfully reconciled");

    Ok(Action::await_change())
}

// TODO: Until CDC finalizer is implemented (and even after that if a CDC is deleted using foreground propagation) CDC deletion could cause the cluster to be deleted or unreachable before we have a chance to clean up the resource(s) owned by the ResourceSync, we should be able to detect and handle this scenario gracefully
// TODO: Immutability on source/target (via CEL?)
// TODO: If secrets for remote clusters on target and source (when applicable) no longer exist then simply allow the ResourceSync to be deleted by removing the finalizer

async fn reconcile(resource_sync: Arc<ResourceSync>, ctx: Arc<Context>) -> Result<Action> {
    let name = resource_sync
        .metadata
        .name
        .to_owned()
        .ok_or(Error::NameRequired)?;
    let parent_api = resource_sync.api(ctx.client.clone());

    let result = reconcile_helper(
        Arc::clone(&resource_sync),
        Arc::clone(&ctx),
        &name,
        &parent_api,
    )
    .await;

    let status = match &result {
        Err(err) => {
            let sync_failing_condition = Condition {
                last_transition_time: sync_failing_transition_time(&(resource_sync.status)),
                message: err.to_string(),
                observed_generation: resource_sync.metadata.generation,
                reason: RESOURCE_SYNC_FAILING_CONDITION.to_string(),
                status: "True".to_string(),
                type_: RESOURCE_SYNC_FAILING_CONDITION.to_string(),
            };

            Some(ResourceSyncStatus {
                conditions: Some(vec![sync_failing_condition]),
            })
        }
        _ => None,
    };

    if status != resource_sync.status {
        parent_api
            .patch_status(
                &name,
                &PatchParams::default(),
                &Merge(json!({"status": status})),
            )
            .await?;
    }

    result
}

async fn reconcile_helper(
    resource_sync: Arc<ResourceSync>,
    ctx: Arc<Context>,
    name: &String,
    parent_api: &Api<ResourceSync>,
) -> Result<Action> {
    let resource_sync = Arc::clone(&resource_sync);

    info!(?name, "running reconciler");

    debug!(?resource_sync.spec, "got");
    let local_ns = resource_sync.namespace().ok_or(Error::NamespaceRequired)?;

    let (source_api, target_api) =
        match source_and_target_apis(&resource_sync, &ctx, local_ns).await {
            Ok(apis) => apis,
            Err(_)
                if resource_sync.has_force_delete_option_enabled()
                    && resource_sync.has_been_deleted() =>
            {
                debug!(?name, "force-deleting ResourceSync");
                return stop_watches_and_remove_resource_sync_finalizers(
                    resource_sync,
                    name,
                    parent_api,
                    ctx,
                )
                .await;
            }
            Err(err) => return Err(err),
        };

    match resource_sync {
        resource_sync if resource_sync.has_been_deleted() => {
            reconcile_deleted_resource(resource_sync, name, target_api, parent_api, ctx).await
        }
        resource_sync if !resource_sync.has_target_finalizer() => {
            add_target_finalizer(resource_sync, name, parent_api).await
        }
        _ => reconcile_normally(resource_sync, name, source_api, target_api, ctx).await,
    }
}

async fn source_and_target_apis(
    resource_sync: &Arc<ResourceSync>,
    ctx: &Arc<Context>,
    local_ns: String,
) -> Result<(NamespacedApi, NamespacedApi)> {
    let target_api = resource_sync
        .spec
        .target
        .api_for(ctx.client.clone(), &local_ns)
        .await?;
    let source_api = resource_sync
        .spec
        .source
        .api_for(ctx.client.clone(), &local_ns)
        .await?;

    Ok((source_api, target_api))
}

fn sync_failing_transition_time(status: &Option<ResourceSyncStatus>) -> Time {
    let now = Time(Utc::now());

    match status {
        None => now,
        Some(status) => match &status.conditions {
            None => now,
            Some(conditions) => {
                let sync_failing_condition = conditions
                    .iter()
                    .find(|c| c.type_ == RESOURCE_SYNC_FAILING_CONDITION);
                sync_failing_condition
                    .map(|c| c.last_transition_time.clone())
                    .unwrap_or(now)
            }
        },
    }
}

// TODO: Exponential Backoff using DefaultBackoff for watcher
fn error_policy(resource_sync: Arc<ResourceSync>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = resource_sync.name_any();
    warn!(?name, %error, "reconcile failed");
    // TODO(mkm): make error requeue duration configurable
    Action::requeue(Duration::from_secs(5))
}

pub async fn run(client: Client) -> Result<()> {
    let docs = Api::<ResourceSync>::all(client.clone());
    if let Err(e) = docs.list(&ListParams::default().limit(1)).await {
        error!("CRD is not queryable; {e:?}. Is the CRD installed?");
        std::process::exit(1);
    }

    let (reader, writer) = reflector::store();
    let resource_syncs = watcher(docs, watcher::Config::default().any_semantic())
        .default_backoff()
        .reflect(writer)
        .applied_objects()
        .predicate_filter(predicates::generation);

    let (remote_watcher_manager, remote_objects_trigger) =
        RemoteWatcherManager::new(client.clone());

    let ctx = Arc::new(Context {
        client,
        remote_watcher_manager,
    });

    Controller::for_stream(resource_syncs, reader)
        .reconcile_on(remote_objects_trigger)
        .shutdown_on_signal()
        .run(reconcile, error_policy, Arc::clone(&ctx))
        .filter_map(|x| async move { Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;

    ctx.remote_watcher_manager.stop_all().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{sync_failing_transition_time, RESOURCE_SYNC_FAILING_CONDITION};
    use crate::resources::ResourceSyncStatus;
    use chrono::{TimeDelta, TimeZone};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
    use once_cell::sync::Lazy;
    use rstest::rstest;

    static NOW: Lazy<Time> = Lazy::new(|| Time(chrono::Utc::now()));
    static EPOCH: Lazy<Time> = Lazy::new(|| Time(chrono::Utc.timestamp_opt(0, 0).unwrap()));

    #[rstest]
    #[case::none(None, &NOW)]
    #[case::no_conditions(Some(ResourceSyncStatus::default()), &NOW)]
    #[case::empty_conditions(Some(ResourceSyncStatus{conditions: Some(vec![])}), &NOW)]
    #[case::condition_already_present(
        Some(
            ResourceSyncStatus{
                conditions: Some(
                    vec![
                        Condition{
                            last_transition_time: EPOCH.clone(),
                            type_: RESOURCE_SYNC_FAILING_CONDITION.to_string(),
                            message: "".to_string(),
                            reason: "".to_string(),
                            observed_generation: None,
                            status: "".to_string()
                        }
                    ]
                )
            }
        ),
        &NOW
    )]
    #[tokio::test]
    async fn test_sync_failing_transition_time(
        #[case] status: Option<ResourceSyncStatus>,
        #[case] expected: &Time,
    ) {
        let result = sync_failing_transition_time(&status);
        let diff = result.0 - expected.0;

        assert!(diff.le(&TimeDelta::minutes(1)))
    }
}
