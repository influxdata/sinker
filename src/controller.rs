use futures::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, OwnerReference, Time};
use k8s_openapi::jiff::Timestamp;
use kube::api::DeleteParams;
use kube::api::Patch::Merge;
use kube::runtime::{predicates, reflector, PredicateConfig, WatchStreamExt};
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
use crate::metrics::ControllerMetrics;
use crate::remote_watcher_manager::RemoteWatcherManager;
use crate::resource_extensions::NamespacedApi;
use crate::resources::ResourceSyncStatus;
use crate::{requeue_after, resources::ResourceSync, util, Error, Result, FINALIZER};

const RESOURCE_SYNC_FAILING_CONDITION: &str = "ResourceSyncFailing";
const RESOURCE_SYNC_SUCCEEDED_REASON: &str = "ResourceSyncSucceeded";
const RESOURCE_SYNC_PREDICATE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Context {
    pub client: Client,
    pub remote_watcher_manager: RemoteWatcherManager,
}

macro_rules! apply_patch_params {
    () => {
        PatchParams::apply(&ResourceSync::group(&())).force()
    };
}

#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
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

    if resource_sync.has_disable_target_deletion_option_enabled() {
        return stop_watches_and_remove_resource_sync_finalizers(
            resource_sync,
            name,
            parent_api,
            ctx,
        )
        .await;
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

#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
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

#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
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

#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
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

// TODO: If secrets for remote clusters on target and source (when applicable) no longer exist then simply allow the ResourceSync to be deleted by removing the finalizer

#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
async fn reconcile_with_metrics(
    resource_sync: Arc<ResourceSync>,
    ctx: Arc<Context>,
    metrics: ControllerMetrics,
) -> Result<Action> {
    // Include early validation, finalizers, and status requests in the attempt.
    metrics.instrument(reconcile(resource_sync, ctx)).await
}

#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
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

    // Always write the status, and compute it from the live object rather than the reflector
    // cache. The cache can lag our own previous patch: deciding off it can skip the write and
    // leave the condition latched, and carrying its stale condition over corrupts
    // lastTransitionTime. Skip the get when we won't write; the object may already be gone.
    let live_status = if result.is_err() || !resource_sync.has_been_deleted() {
        parent_api.get_status(&name).await?.status
    } else {
        None
    };

    if let Some(status) = reconcile_status(&resource_sync, &live_status, &result) {
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

#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
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

#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
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

fn reconcile_status(
    resource_sync: &ResourceSync,
    live_status: &Option<ResourceSyncStatus>,
    result: &Result<Action>,
) -> Option<ResourceSyncStatus> {
    match result {
        Err(err) => Some(ResourceSyncStatus {
            conditions: Some(vec![sync_failing_condition(
                resource_sync,
                live_status,
                "True",
                RESOURCE_SYNC_FAILING_CONDITION,
                err.to_string(),
            )]),
        }),
        // A successful reconcile must reset the condition to False rather than leave the last
        // failure latched.
        Ok(_) if !resource_sync.has_been_deleted() => Some(ResourceSyncStatus {
            conditions: Some(vec![sync_failing_condition(
                resource_sync,
                live_status,
                "False",
                RESOURCE_SYNC_SUCCEEDED_REASON,
                "Sync succeeded".to_string(),
            )]),
        }),
        // None means don't write: a deleted resource's finalizer may already be gone, so a status
        // patch could 404.
        Ok(_) => None,
    }
}

fn sync_failing_condition(
    resource_sync: &ResourceSync,
    live_status: &Option<ResourceSyncStatus>,
    status: &str,
    reason: &str,
    message: String,
) -> Condition {
    Condition {
        last_transition_time: sync_failing_transition_time(live_status, status),
        message,
        observed_generation: resource_sync.metadata.generation,
        reason: reason.to_string(),
        status: status.to_string(),
        type_: RESOURCE_SYNC_FAILING_CONDITION.to_string(),
    }
}

// The transition time is only carried over while the condition value is unchanged; a True<->False
// flip records a new transition.
fn sync_failing_transition_time(status: &Option<ResourceSyncStatus>, new_status: &str) -> Time {
    let now = Time(Timestamp::now());

    status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .and_then(|conditions| {
            conditions
                .iter()
                .find(|c| c.type_ == RESOURCE_SYNC_FAILING_CONDITION)
        })
        .filter(|c| c.status == new_status)
        .map(|c| c.last_transition_time.clone())
        .unwrap_or(now)
}

// TODO: Exponential Backoff using DefaultBackoff for watcher
fn error_policy(resource_sync: Arc<ResourceSync>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = resource_sync.name_any();
    warn!(?name, %error, "reconcile failed");
    // TODO(mkm): make error requeue duration configurable
    Action::requeue(Duration::from_secs(5))
}

/// Run the ResourceSync controller without exporting reconciliation metrics.
/// Use [`run_with_metrics`] to share the admin server's registry.
#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
pub async fn run(client: Client) -> Result<()> {
    run_with_metrics(client, ControllerMetrics::default()).await
}

/// Run the ResourceSync controller using metrics registered with the admin server.
#[expect(
    clippy::result_large_err,
    reason = "Preserve the public Error variants without boxing"
)]
pub async fn run_with_metrics(client: Client, metrics: ControllerMetrics) -> Result<()> {
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
        .predicate_filter(
            predicates::generation,
            PredicateConfig::default().ttl(RESOURCE_SYNC_PREDICATE_TTL),
        );

    let (remote_watcher_manager, remote_objects_trigger) =
        RemoteWatcherManager::new(client.clone());

    let ctx = Arc::new(Context {
        client,
        remote_watcher_manager,
    });

    Controller::for_stream(resource_syncs, reader)
        .reconcile_on(remote_objects_trigger)
        .shutdown_on_signal()
        .run(
            move |resource_sync, ctx| reconcile_with_metrics(resource_sync, ctx, metrics.clone()),
            error_policy,
            Arc::clone(&ctx),
        )
        .filter_map(|x| async move { Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;

    ctx.remote_watcher_manager.stop_all().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        reconcile, reconcile_deleted_resource, reconcile_helper, reconcile_normally,
        reconcile_with_metrics, Context, ControllerMetrics, RemoteWatcherManager,
    };
    use super::{
        reconcile_status, sync_failing_transition_time, RESOURCE_SYNC_FAILING_CONDITION,
        RESOURCE_SYNC_SUCCEEDED_REASON,
    };
    use crate::resources::{ResourceSync, ResourceSyncStatus};
    use crate::test_support::{
        api_error, discovery_response, resource_sync as sync_fixture, response, MockApi,
    };
    use crate::FINALIZER;
    use crate::{Error, Result};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
    use k8s_openapi::jiff::Timestamp;
    use kube::runtime::controller::Action;
    use once_cell::sync::Lazy;
    use rstest::rstest;
    use serde_json::json;
    use std::{sync::Arc, time::Duration};

    #[tokio::test]
    async fn resource_sync_predicate_accepts_new_uids_and_generation_changes() {
        use super::RESOURCE_SYNC_PREDICATE_TTL;
        use futures::{stream, StreamExt};
        use kube::runtime::{predicates, watcher, PredicateConfig, WatchStreamExt};

        let mut original = sync_fixture();
        original.metadata.generation = Some(1);
        let mut metadata_only = original.clone();
        metadata_only.metadata.resource_version = Some("2".into());
        metadata_only.status = Some(ResourceSyncStatus::default());
        let mut recreated = original.clone();
        recreated.metadata.uid = Some("replacement-uid".into());
        let mut updated = recreated.clone();
        updated.metadata.generation = Some(2);
        let mut missing_generation = updated.clone();
        missing_generation.metadata.generation = None;

        let mut events = stream::iter([
            Ok(original.clone()),
            Ok(metadata_only),
            Err(watcher::Error::NoResourceVersion),
            Ok(recreated.clone()),
            Ok(recreated.clone()),
            Ok(updated.clone()),
            Ok(missing_generation.clone()),
            Ok(missing_generation.clone()),
        ])
        .predicate_filter(
            predicates::generation,
            PredicateConfig::default().ttl(RESOURCE_SYNC_PREDICATE_TTL),
        )
        .collect::<Vec<_>>()
        .await;

        assert_eq!(events.len(), 6);
        assert!(matches!(
            events.remove(1),
            Err(watcher::Error::NoResourceVersion)
        ));
        let objects: Vec<_> = events
            .into_iter()
            .map(|event| json!(event.expect("object event")))
            .collect();
        assert_eq!(
            objects,
            [
                json!(original),
                json!(recreated),
                json!(updated),
                json!(missing_generation),
                json!(missing_generation)
            ]
        );
    }

    fn context(client: kube::Client) -> Arc<Context> {
        let (remote_watcher_manager, _events) = RemoteWatcherManager::new(client.clone());
        Arc::new(Context {
            client,
            remote_watcher_manager,
        })
    }

    async fn stop_watches(ctx: &Context) {
        tokio::time::timeout(
            Duration::from_secs(2),
            ctx.remote_watcher_manager.stop_all(),
        )
        .await
        .expect("watchers cancel and join");
    }

    const SYNC_PATH: &str =
        "/apis/sinker.influxdata.io/v1alpha1/namespaces/team-a/resourcesyncs/copy-config";
    const STATUS_PATH: &str =
        "/apis/sinker.influxdata.io/v1alpha1/namespaces/team-a/resourcesyncs/copy-config/status";
    const SOURCE_PATH: &str = "/api/v1/namespaces/team-a/configmaps/source-config";
    const TARGET_PATH: &str = "/api/v1/namespaces/team-a/configmaps/target-config";

    fn assert_reconcile_metrics(registry: &prometheus_client::registry::Registry, outcome: &str) {
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, registry).expect("encode metrics");
        assert!(text.contains("controller_runtime_active_workers{controller=\"resourcesync\"} 0\n"));
        assert!(text.contains(
            "controller_runtime_reconcile_time_seconds_count{controller=\"resourcesync\"} 1\n"
        ));
        for result in ["success", "error", "requeue", "requeue_after"] {
            let count = u64::from(result == outcome);
            assert!(text.contains(&format!(
                "controller_runtime_reconcile_total{{controller=\"resourcesync\",result=\"{result}\"}} {count}\n"
            )));
        }
    }

    #[tokio::test]
    async fn initialization_preserves_finalizers_and_writes_live_success_status() {
        let mut sync = sync_fixture();
        sync.metadata.finalizers = Some(vec!["example.com/other".into()]);
        sync.status = status_with_condition("True");
        let mut live = sync.clone();
        live.status = status_with_condition("False");
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
            response(200, json!(sync)),
            response(200, json!(live)),
            response(200, json!(live)),
        ]);
        let mut registry = Default::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let result = reconcile_with_metrics(Arc::new(sync), context(mock.client.clone()), metrics)
            .await
            .expect("initialize sync");
        assert_eq!(result, Action::requeue(Duration::from_millis(500)));
        assert_reconcile_metrics(&registry, "requeue_after");
        let requests = mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1"),
            ("PATCH", SYNC_PATH),
            ("GET", STATUS_PATH),
            ("PATCH", STATUS_PATH),
        ]);
        assert_eq!(
            requests[2].body(),
            &json!({"metadata": {"finalizers": ["example.com/other", FINALIZER]}})
        );
        assert_eq!(
            requests[2].headers()["content-type"],
            "application/merge-patch+json"
        );
        let status: ResourceSyncStatus =
            serde_json::from_value(requests[4].body()["status"].clone()).expect("status patch");
        let condition = single_condition(Some(status));
        assert_eq!(condition.last_transition_time, *EPOCH);
        assert_eq!(condition.observed_generation, Some(7));
        assert_eq!(condition.status, "False");
        assert_eq!(condition.message, "Sync succeeded");
    }

    #[tokio::test]
    async fn deleted_missing_target_removes_only_our_finalizer_and_skips_status() {
        let mut sync = sync_fixture();
        sync.metadata.deletion_timestamp = Some(EPOCH.clone());
        sync.metadata.finalizers = Some(vec![
            FINALIZER.into(),
            "example.com/keep".into(),
            FINALIZER.into(),
        ]);
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
            api_error(404),
            response(200, json!(sync)),
        ]);
        let mut registry = Default::default();
        let metrics = ControllerMetrics::register(&mut registry);
        assert_eq!(
            reconcile_with_metrics(Arc::new(sync), context(mock.client.clone()), metrics)
                .await
                .expect("cleanup"),
            Action::await_change()
        );
        assert_reconcile_metrics(&registry, "success");
        let requests = mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1"),
            ("GET", TARGET_PATH),
            ("PATCH", SYNC_PATH),
        ]);
        assert_eq!(
            requests[3].body(),
            &json!({"metadata": {"finalizers": ["example.com/keep"]}})
        );
    }

    #[tokio::test]
    async fn source_failure_is_returned_and_written_to_status() {
        let mut sync = sync_fixture();
        sync.metadata.finalizers = Some(vec![FINALIZER.into()]);
        let mut live = sync.clone();
        live.status = status_with_condition("True");
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
            api_error(404),
            response(200, json!(live)),
            response(200, json!(live)),
        ]);
        let mut registry = Default::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let error = reconcile_with_metrics(Arc::new(sync), context(mock.client.clone()), metrics)
            .await
            .expect_err("source absent");
        assert_reconcile_metrics(&registry, "error");
        let message = error.to_string();
        assert!(
            matches!(error, Error::ResourceNotFoundError(name, kind, kube::Error::Api(error))
            if name == "source-config" && kind == "ConfigMap" && error.code == 404)
        );
        let requests = mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1"),
            ("GET", SOURCE_PATH),
            ("GET", STATUS_PATH),
            ("PATCH", STATUS_PATH),
        ]);
        let condition = &requests[4].body()["status"]["conditions"][0];
        assert_eq!(condition["status"], "True");
        assert_eq!(condition["message"], message);
        assert_eq!(condition["observedGeneration"], 7);
        assert_eq!(condition["lastTransitionTime"], json!(*EPOCH));
    }

    #[rstest]
    #[case::deleting_target_api_failure(true, true, false, true)]
    #[case::deleting_source_api_failure(true, true, true, true)]
    #[case::disabled_force_delete(true, false, false, false)]
    #[case::active_sync(false, true, false, false)]
    #[tokio::test]
    async fn force_delete_only_bypasses_api_resolution_for_deleting_syncs(
        #[case] deleted: bool,
        #[case] force: bool,
        #[case] source_failure: bool,
        #[case] removed: bool,
    ) {
        let mut sync = sync_fixture();
        sync.metadata.deletion_timestamp = deleted.then(|| EPOCH.clone());
        sync.metadata.finalizers = Some(vec![FINALIZER.into(), "example.com/keep".into()]);
        sync.metadata.annotations = Some(std::collections::BTreeMap::from([(
            crate::resources::FORCE_DELETE_ANNOTATION.into(),
            force.to_string(),
        )]));
        let mut responses = vec![];
        let mut expected = vec![];
        if source_failure {
            responses.push(discovery_response("ConfigMap", "configmaps", true));
            expected.push(("GET", "/api/v1"));
        }
        responses.push(api_error(403));
        expected.push(("GET", "/api/v1"));
        if removed {
            responses.push(response(200, json!(sync)));
            expected.push(("PATCH", SYNC_PATH));
        }
        let mock = MockApi::new(responses);
        let parent_api = sync.api(mock.client.clone());
        let result = reconcile_helper(
            Arc::new(sync),
            context(mock.client.clone()),
            &"copy-config".into(),
            &parent_api,
        )
        .await;
        if removed {
            assert_eq!(result.expect("force cleanup"), Action::await_change());
        } else {
            assert!(
                matches!(result.expect_err("API resolution failure"), Error::KubeError(kube::Error::Api(error)) if error.code == 403)
            );
        }
        let requests = mock.finish(&expected);
        if removed {
            assert_eq!(
                requests.last().expect("finalizer patch").body(),
                &json!({"metadata": {"finalizers": ["example.com/keep"]}})
            );
        }
    }

    #[rstest]
    #[case::no_finalizers(None, false, Some("Foreground"))]
    #[case::empty_finalizers(Some(vec![]), false, Some("Foreground"))]
    #[case::target_finalizer(Some(vec!["example.com/target"]), false, Some("Background"))]
    #[case::already_deleting(Some(vec!["example.com/target"]), true, None)]
    #[tokio::test]
    async fn target_deletion_waits_for_absence(
        #[case] finalizers: Option<Vec<&str>>,
        #[case] deleting: bool,
        #[case] propagation: Option<&str>,
    ) {
        let mut sync = sync_fixture();
        sync.metadata.finalizers = Some(vec![FINALIZER.into()]);
        sync.metadata.deletion_timestamp = Some(EPOCH.clone());
        let target = json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {
            "name": "target-config", "finalizers": finalizers, "deletionTimestamp": deleting.then(|| EPOCH.clone())}});
        let mut responses = vec![
            discovery_response("ConfigMap", "configmaps", true),
            response(200, target.clone()),
        ];
        let mut expected = vec![("GET", "/api/v1"), ("GET", TARGET_PATH)];
        if propagation.is_some() {
            responses.push(response(200, target));
            expected.push(("DELETE", TARGET_PATH));
        }
        let mock = MockApi::new(responses);
        let ctx = context(mock.client.clone());
        let _cancelled =
            crate::remote_watcher_manager::tests::park_watchers(&ctx.remote_watcher_manager, &sync)
                .await;
        let parent = sync.api(mock.client.clone());
        let target_api = sync
            .spec
            .target
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("target API");
        let result = reconcile_deleted_resource(
            Arc::new(sync),
            "copy-config",
            target_api,
            &parent,
            Arc::clone(&ctx),
        )
        .await;
        stop_watches(&ctx).await;
        assert_eq!(result.expect("request deletion"), Action::await_change());
        let requests = mock.finish(&expected);
        if let Some(propagation) = propagation {
            assert_eq!(requests[2].body()["propagationPolicy"], propagation);
        }
    }

    #[rstest]
    #[case::without_our_finalizer(false, false)]
    #[case::deletion_disabled(true, true)]
    #[tokio::test]
    async fn cleanup_can_skip_target_requests(#[case] finalizer: bool, #[case] disabled: bool) {
        let mut sync = sync_fixture();
        sync.metadata.finalizers = Some(if finalizer {
            vec![FINALIZER.into()]
        } else {
            vec!["example.com/other".into()]
        });
        sync.metadata.annotations = Some(std::collections::BTreeMap::from([(
            crate::resources::DISABLE_TARGET_DELETION_ANNOTATION.into(),
            disabled.to_string(),
        )]));
        let mut responses = vec![discovery_response("ConfigMap", "configmaps", true)];
        let mut expected = vec![("GET", "/api/v1")];
        if disabled {
            responses.push(response(200, json!(sync)));
            expected.push(("PATCH", SYNC_PATH));
        }
        let mock = MockApi::new(responses);
        let parent = sync.api(mock.client.clone());
        let target = sync
            .spec
            .target
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("target API");
        assert_eq!(
            reconcile_deleted_resource(
                Arc::new(sync),
                "copy-config",
                target,
                &parent,
                context(mock.client.clone())
            )
            .await
            .expect("cleanup"),
            Action::await_change()
        );
        let requests = mock.finish(&expected);
        if disabled {
            assert_eq!(requests[1].body(), &json!({"metadata": {"finalizers": []}}));
        }
    }

    #[rstest]
    #[case::whole_resource(false, false)]
    #[case::mapped_resource(true, false)]
    #[case::remote_target(false, true)]
    #[case::mapped_remote_target(true, true)]
    #[tokio::test]
    async fn target_apply_uses_forced_field_manager_and_local_ownership(
        #[case] mapped: bool,
        #[case] remote: bool,
    ) {
        let mut sync = sync_fixture();
        if mapped {
            sync.spec.mappings = vec![crate::resources::Mapping {
                from_field_path: Some("data.original".into()),
                to_field_path: Some("data.copied".into()),
            }];
        }
        let source = json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "source-config"}, "data": {"original": "value"}});
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
            response(200, source.clone()),
            response(200, source),
        ]);
        let source_api = sync
            .spec
            .source
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("source API");
        let target_api = sync
            .spec
            .target
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("target API");
        // API resolution is tested separately; this flag determines target ownership.
        if remote {
            sync.spec.target.cluster = Some(Default::default());
        }
        let ctx = context(mock.client.clone());
        let _cancelled =
            crate::remote_watcher_manager::tests::park_watchers(&ctx.remote_watcher_manager, &sync)
                .await;
        let result = reconcile_normally(
            Arc::new(sync),
            "copy-config",
            source_api,
            target_api,
            Arc::clone(&ctx),
        )
        .await;
        stop_watches(&ctx).await;
        assert_eq!(result.expect("apply target"), Action::await_change());
        let requests = mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1"),
            ("GET", SOURCE_PATH),
            ("PATCH", TARGET_PATH),
        ]);
        let patch = &requests[3];
        let query = patch.uri().query().expect("apply parameters");
        assert!(query.split('&').any(|part| part == "force=true"));
        assert!(query
            .split('&')
            .any(|part| part == "fieldManager=sinker.influxdata.io"));
        assert_eq!(
            patch.headers()["content-type"],
            "application/apply-patch+yaml"
        );
        assert_eq!(patch.body()["metadata"]["name"], "target-config");
        assert_eq!(patch.body()["metadata"]["namespace"], "team-a");
        assert_eq!(
            patch.body()["data"],
            if mapped {
                json!({"copied": "value"})
            } else {
                json!({"original": "value"})
            }
        );
        if remote {
            assert!(patch.body()["metadata"].get("ownerReferences").is_none());
        } else {
            assert_eq!(
                patch.body()["metadata"]["ownerReferences"],
                json!([{
                "apiVersion": "sinker.influxdata.io/v1alpha1", "kind": "ResourceSync", "name": "copy-config",
                "uid": "sync-uid", "controller": false, "blockOwnerDeletion": true}])
            );
        }
    }

    #[test]
    fn deletion_errors_still_update_status_and_ignore_unrelated_conditions() {
        let mut live = status_with_condition("True").expect("status fixture");
        live.conditions.as_mut().expect("conditions")[0].type_ = "OtherCondition".into();
        let mut sync = resource_sync(true, None);
        sync.metadata.generation = Some(9);
        let before = Timestamp::now();
        let condition = single_condition(reconcile_status(
            &sync,
            &Some(live),
            &Err(Error::NamespaceRequired),
        ));
        let after = Timestamp::now();
        assert_eq!(condition.status, "True");
        assert_eq!(condition.observed_generation, Some(9));
        assert_eq!(condition.message, "Namespace is required");
        assert!((before..=after).contains(&condition.last_transition_time.0));
    }

    #[rstest]
    #[case::missing_name(true)]
    #[case::missing_namespace(false)]
    #[tokio::test]
    async fn reconciliation_requires_identity_before_api_access(#[case] missing_name: bool) {
        let mut sync = sync_fixture();
        let mock = MockApi::new(vec![]);
        let ctx = context(mock.client.clone());
        if missing_name {
            sync.metadata.name = None;
            assert!(matches!(
                reconcile(Arc::new(sync), ctx)
                    .await
                    .expect_err("name required"),
                Error::NameRequired
            ));
        } else {
            sync.metadata.namespace = None;
            let parent = sync.api(mock.client.clone());
            assert!(matches!(
                reconcile_helper(Arc::new(sync), ctx, &"copy-config".into(), &parent)
                    .await
                    .expect_err("namespace required"),
                Error::NamespaceRequired
            ));
        }
        mock.finish(&[]);
    }

    #[rstest]
    #[case::get_target(false)]
    #[case::delete_target(true)]
    #[tokio::test]
    async fn force_delete_does_not_bypass_target_request_errors(#[case] delete: bool) {
        let mut sync = sync_fixture();
        sync.metadata.deletion_timestamp = Some(EPOCH.clone());
        sync.metadata.finalizers = Some(vec![FINALIZER.into()]);
        sync.metadata.annotations = Some(std::collections::BTreeMap::from([(
            crate::resources::FORCE_DELETE_ANNOTATION.into(),
            "true".into(),
        )]));
        let mut responses = vec![discovery_response("ConfigMap", "configmaps", true)];
        let mut expected = vec![("GET", "/api/v1"), ("GET", TARGET_PATH)];
        if delete {
            responses.push(response(
                200,
                json!({"metadata": {"name": "target-config"}}),
            ));
            expected.push(("DELETE", TARGET_PATH));
        }
        responses.push(api_error(403));
        let mock = MockApi::new(responses);
        let target = sync
            .spec
            .target
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("target API");
        let parent = sync.api(mock.client.clone());
        let error = reconcile_deleted_resource(
            Arc::new(sync),
            "copy-config",
            target,
            &parent,
            context(mock.client.clone()),
        )
        .await
        .expect_err("target request denied");
        assert!(matches!(error, Error::KubeError(kube::Error::Api(error)) if error.code == 403));
        mock.finish(&expected);
    }

    #[rstest]
    #[case::missing_owner_uid(false)]
    #[case::apply_denied(true)]
    #[tokio::test]
    async fn target_write_failures_are_returned(#[case] uid_present: bool) {
        let mut sync = sync_fixture();
        if !uid_present {
            sync.metadata.uid = None;
        }
        let mut responses = vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
            response(200, json!({"data": {"key": "value"}})),
        ];
        let mut expected = vec![("GET", "/api/v1"), ("GET", "/api/v1"), ("GET", SOURCE_PATH)];
        if uid_present {
            responses.push(api_error(409));
            expected.push(("PATCH", TARGET_PATH));
        }
        let mock = MockApi::new(responses);
        let ctx = context(mock.client.clone());
        let source = sync
            .spec
            .source
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("source API");
        let target = sync
            .spec
            .target
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("target API");
        let result = reconcile_normally(
            Arc::new(sync),
            "copy-config",
            source,
            target,
            Arc::clone(&ctx),
        )
        .await;
        stop_watches(&ctx).await;
        let error = result.expect_err("target write failure");
        if uid_present {
            assert!(
                matches!(error, Error::KubeError(kube::Error::Api(error)) if error.code == 409)
            );
        } else {
            assert!(matches!(error, Error::UIDRequired));
        }
        mock.finish(&expected);
    }

    #[tokio::test]
    async fn invalid_mappings_skip_target_write_and_report_failure() {
        let mut sync = sync_fixture();
        sync.metadata.finalizers = Some(vec![FINALIZER.into()]);
        sync.spec.mappings = vec![
            crate::resources::Mapping {
                from_field_path: Some("data.key".into()),
                to_field_path: Some("data.copied".into()),
            },
            crate::resources::Mapping::default(),
            crate::resources::Mapping {
                from_field_path: Some("invalid[".into()),
                to_field_path: Some("data.later".into()),
            },
        ];
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
            response(200, json!({"data": {"key": "value"}})),
            response(200, json!(sync)),
            response(200, json!(sync)),
        ]);
        let ctx = context(mock.client.clone());
        let error = reconcile(Arc::new(sync), Arc::clone(&ctx)).await;
        stop_watches(&ctx).await;
        assert!(matches!(
            error.expect_err("empty mapping"),
            Error::MappingEmpty
        ));
        let requests = mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1"),
            ("GET", SOURCE_PATH),
            ("GET", STATUS_PATH),
            ("PATCH", STATUS_PATH),
        ]);
        let condition = &requests[4].body()["status"]["conditions"][0];
        assert_eq!(condition["status"], "True");
        assert_eq!(condition["reason"], RESOURCE_SYNC_FAILING_CONDITION);
        assert_eq!(condition["message"], Error::MappingEmpty.to_string());
    }

    #[rstest]
    #[case::adding(false)]
    #[case::removing(true)]
    #[tokio::test]
    async fn finalizer_patch_errors_are_returned(#[case] deleting: bool) {
        let mut sync = sync_fixture();
        sync.metadata.finalizers = Some(vec!["example.com/keep".into()]);
        if deleting {
            sync.metadata.deletion_timestamp = Some(EPOCH.clone());
            sync.metadata
                .finalizers
                .as_mut()
                .expect("finalizers")
                .push(FINALIZER.into());
        }
        let mut responses = vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
        ];
        let mut expected = vec![("GET", "/api/v1"), ("GET", "/api/v1")];
        if deleting {
            responses.push(api_error(404));
            expected.push(("GET", TARGET_PATH));
        }
        responses.push(api_error(409));
        expected.push(("PATCH", SYNC_PATH));
        let mock = MockApi::new(responses);
        let ctx = context(mock.client.clone());
        let mut cancelled =
            crate::remote_watcher_manager::tests::park_watchers(&ctx.remote_watcher_manager, &sync)
                .await;
        let parent = sync.api(mock.client.clone());
        let result = reconcile_helper(
            Arc::new(sync),
            Arc::clone(&ctx),
            &"copy-config".into(),
            &parent,
        )
        .await;
        // Cleanup stops both watches before attempting the finalizer patch, even if it fails.
        let cancellations_before_cleanup: Vec<_> =
            std::iter::from_fn(|| cancelled.try_recv().ok()).collect();
        stop_watches(&ctx).await;
        let error = result.expect_err("finalizer patch conflict");
        assert!(matches!(error, Error::KubeError(kube::Error::Api(error)) if error.code == 409));
        assert_eq!(
            cancellations_before_cleanup.len(),
            if deleting { 2 } else { 0 }
        );
        if deleting {
            assert_ne!(
                cancellations_before_cleanup[0].object,
                cancellations_before_cleanup[1].object
            );
        }
        let requests = mock.finish(&expected);
        let finalizers = if deleting {
            json!(["example.com/keep"])
        } else {
            json!(["example.com/keep", FINALIZER])
        };
        assert_eq!(
            requests.last().expect("finalizer patch").body(),
            &json!({"metadata": {"finalizers": finalizers}})
        );
    }

    #[rstest]
    #[case::status_read(false)]
    #[case::status_write(true)]
    #[tokio::test]
    async fn status_api_errors_are_returned(#[case] write: bool) {
        let sync = sync_fixture();
        let mut responses = vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
            response(200, json!(sync)),
        ];
        let mut expected = vec![
            ("GET", "/api/v1"),
            ("GET", "/api/v1"),
            ("PATCH", SYNC_PATH),
            ("GET", STATUS_PATH),
        ];
        if write {
            responses.push(response(200, json!(sync)));
            expected.push(("PATCH", STATUS_PATH));
        }
        responses.push(api_error(403));
        let mock = MockApi::new(responses);
        let mut registry = Default::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let error = reconcile_with_metrics(Arc::new(sync), context(mock.client.clone()), metrics)
            .await
            .expect_err("status request denied");
        assert_reconcile_metrics(&registry, "error");
        assert!(matches!(error, Error::KubeError(kube::Error::Api(error)) if error.code == 403));
        mock.finish(&expected);
    }

    #[tokio::test]
    async fn early_validation_failure_counts_as_a_reconciliation_error() {
        let mut sync = sync_fixture();
        sync.metadata.name = None;
        let mock = MockApi::new(vec![]);
        let mut registry = Default::default();
        let metrics = ControllerMetrics::register(&mut registry);
        assert!(matches!(
            reconcile_with_metrics(Arc::new(sync), context(mock.client.clone()), metrics).await,
            Err(Error::NameRequired)
        ));
        assert_reconcile_metrics(&registry, "error");
        mock.finish(&[]);
    }

    #[rstest]
    #[case::status_read("GET")]
    #[case::status_write("PATCH")]
    #[tokio::test]
    async fn metrics_include_pending_status_requests(#[case] blocked_method: &'static str) {
        let sync = sync_fixture();
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            discovery_response("ConfigMap", "configmaps", true),
            response(200, json!(sync)),
            response(200, json!(sync)),
            response(200, json!(sync)),
        ]);
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (resume, resumed) = tokio::sync::oneshot::channel();
        let mut gate = Some((started, resumed));
        let client = mock.client.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let gate = if request.method() == blocked_method && request.uri().path() == STATUS_PATH
            {
                Some(gate.take().expect("one blocked status request"))
            } else {
                None
            };
            let client = client.clone();
            async move {
                if let Some((started, resumed)) = gate {
                    started.send(()).expect("notify status request");
                    resumed.await.expect("resume status request");
                }
                client.send(request).await
            }
        });
        let ctx = context(kube::Client::new(service, "client-default"));
        let mut registry = Default::default();
        let metrics = ControllerMetrics::register(&mut registry);
        // Keep the future owned here so assertion failures cancel it, rather than
        // leaving a detached reconciliation task waiting for a response.
        let mut future = Box::pin(reconcile_with_metrics(Arc::new(sync), ctx, metrics));
        tokio::select! {
            result = &mut future => panic!("reconciliation completed before status resumed: {result:?}"),
            reached = tokio::time::timeout(Duration::from_secs(2), waiting) => {
                reached.expect("status request starts").expect("status notification");
            }
        }
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).expect("encode metrics");
        assert!(text.contains("controller_runtime_active_workers{controller=\"resourcesync\"} 1\n"));
        assert!(text.contains(
            "controller_runtime_reconcile_time_seconds_count{controller=\"resourcesync\"} 0\n"
        ));
        for outcome in ["success", "error", "requeue", "requeue_after"] {
            assert!(text.contains(&format!(
                "controller_runtime_reconcile_total{{controller=\"resourcesync\",result=\"{outcome}\"}} 0\n"
            )));
        }
        resume.send(()).expect("release status response");
        let result = tokio::time::timeout(Duration::from_secs(2), future)
            .await
            .expect("reconciliation completes")
            .expect("successful initialization");
        assert_eq!(result, Action::requeue(Duration::from_millis(500)));
        assert_reconcile_metrics(&registry, "requeue_after");
        mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1"),
            ("PATCH", SYNC_PATH),
            ("GET", STATUS_PATH),
            ("PATCH", STATUS_PATH),
        ]);
    }

    #[tokio::test]
    async fn reconciliation_errors_retry_after_five_seconds() {
        let mock = MockApi::new(vec![]);
        assert_eq!(
            super::error_policy(
                Arc::new(sync_fixture()),
                &Error::NamespaceRequired,
                context(mock.client.clone())
            ),
            Action::requeue(Duration::from_secs(5))
        );
        mock.finish(&[]);
    }

    static EPOCH: Lazy<Time> = Lazy::new(|| Time(Timestamp::UNIX_EPOCH));

    fn status_with_condition(status: &str) -> Option<ResourceSyncStatus> {
        Some(ResourceSyncStatus {
            conditions: Some(vec![Condition {
                last_transition_time: EPOCH.clone(),
                type_: RESOURCE_SYNC_FAILING_CONDITION.to_string(),
                message: "".to_string(),
                reason: "".to_string(),
                observed_generation: None,
                status: status.to_string(),
            }]),
        })
    }

    #[rstest]
    #[case::none(None, "True", None)]
    #[case::no_conditions(Some(ResourceSyncStatus::default()), "True", None)]
    #[case::empty_conditions(Some(ResourceSyncStatus{conditions: Some(vec![])}), "True", None)]
    #[case::still_failing_keeps_time(status_with_condition("True"), "True", Some(&*EPOCH))]
    #[case::still_succeeding_keeps_time(status_with_condition("False"), "False", Some(&*EPOCH))]
    #[case::failure_after_success_transitions(status_with_condition("False"), "True", None)]
    #[case::success_after_failure_transitions(status_with_condition("True"), "False", None)]
    fn test_sync_failing_transition_time(
        #[case] status: Option<ResourceSyncStatus>,
        #[case] new_status: &str,
        #[case] expected: Option<&Time>,
    ) {
        let before = Timestamp::now();
        let result = sync_failing_transition_time(&status, new_status);
        let after = Timestamp::now();
        if let Some(expected) = expected {
            assert_eq!(&result, expected);
        } else {
            assert!((before..=after).contains(&result.0));
        }
    }

    #[rstest]
    #[case::first(0)]
    #[case::middle(1)]
    #[case::last(2)]
    fn transition_time_finds_the_sync_condition_among_other_conditions(#[case] position: usize) {
        let expected = Time(Timestamp::from_second(1_700_000_000).expect("fixed timestamp"));
        let mut matching = single_condition(status_with_condition("True"));
        matching.last_transition_time = expected.clone();
        let mut unrelated = matching.clone();
        unrelated.type_ = "Unrelated".into();
        unrelated.last_transition_time = EPOCH.clone();
        let mut another = unrelated.clone();
        another.type_ = "AnotherCondition".into();
        another.status = "False".into();
        let mut conditions = vec![unrelated, another];
        conditions.insert(position, matching);
        assert_eq!(
            sync_failing_transition_time(
                &Some(ResourceSyncStatus {
                    conditions: Some(conditions)
                }),
                "True"
            ),
            expected,
        );
    }

    fn resource_sync(deleted: bool, status: Option<ResourceSyncStatus>) -> ResourceSync {
        ResourceSync {
            metadata: ObjectMeta {
                deletion_timestamp: deleted.then(|| EPOCH.clone()),
                ..Default::default()
            },
            spec: Default::default(),
            status,
        }
    }

    fn single_condition(status: Option<ResourceSyncStatus>) -> Condition {
        let conditions = status.unwrap().conditions.unwrap();
        assert_eq!(conditions.len(), 1);
        conditions.into_iter().next().unwrap()
    }

    #[test]
    fn test_reconcile_status_err_sets_failing_true() {
        let rs = resource_sync(false, None);
        let result: Result<Action> = Err(Error::NameRequired);

        let condition = single_condition(reconcile_status(&rs, &rs.status, &result));

        assert_eq!(condition.type_, RESOURCE_SYNC_FAILING_CONDITION);
        assert_eq!(condition.status, "True");
        assert_eq!(condition.reason, RESOURCE_SYNC_FAILING_CONDITION);
        assert_eq!(condition.message, Error::NameRequired.to_string());
    }

    #[test]
    fn test_reconcile_status_ok_resets_failing_to_false() {
        let rs = resource_sync(false, status_with_condition("True"));
        let result: Result<Action> = Ok(Action::await_change());

        let condition = single_condition(reconcile_status(&rs, &rs.status, &result));

        assert_eq!(condition.type_, RESOURCE_SYNC_FAILING_CONDITION);
        assert_eq!(condition.status, "False");
        assert_eq!(condition.reason, RESOURCE_SYNC_SUCCEEDED_REASON);
    }

    #[test]
    fn test_reconcile_status_uses_live_status_for_transition_time() {
        // The cache still says True but the live object has already transitioned to False; the
        // carried-over time must come from the live condition.
        let rs = resource_sync(false, status_with_condition("True"));
        let live_status = status_with_condition("False");
        let result: Result<Action> = Ok(Action::await_change());

        let condition = single_condition(reconcile_status(&rs, &live_status, &result));

        assert_eq!(condition.status, "False");
        assert_eq!(condition.last_transition_time, *EPOCH);
    }

    #[test]
    fn test_reconcile_status_ok_deleted_skips_status_write() {
        let rs = resource_sync(true, status_with_condition("True"));
        let result: Result<Action> = Ok(Action::await_change());

        assert_eq!(reconcile_status(&rs, &rs.status, &result), None);
    }
}
