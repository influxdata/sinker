use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::DeleteParams;
use kube::api::Patch::Merge;
use kube::{
    api::{ListParams, Patch, PatchParams},
    runtime::{
        controller::{Action, Controller},
        watcher,
    },
    Api, Client, Resource, ResourceExt,
};
use serde_json::json;
#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use util::{WithItemAdded, WithItemRemoved};

use crate::mapping::{apply_mappings, clone_resource};
use crate::resource_extensions::NamespacedApi;
use crate::{requeue_after, resources::ResourceSync, util, Error, Result, FINALIZER};

pub struct Context {
    pub client: Client,
}

async fn reconcile_deleted_resource(
    sinker: Arc<ResourceSync>,
    name: &str,
    target_api: NamespacedApi,
    parent_api: Api<ResourceSync>,
) -> Result<Action> {
    if !sinker.has_target_finalizer() {
        // We have already removed our finalizer, so nothing more needs to be done
        return Ok(Action::await_change());
    }

    let target_name = &sinker.spec.target.resource_ref.name;

    match target_api.get(target_name).await {
        Ok(target) if target.metadata.deletion_timestamp.is_some() => {
            // Target is being deleted, wait for it to be deleted
            // For now we need a requeue after, but in the future we should try to watch the target if we can
            requeue_after!()
        }
        Ok(_) => {
            target_api
                .delete(target_name, &DeleteParams::foreground())
                .await?;
            // Deleted target, wait for it to be deleted
            // For now we need a requeue after, but in the future we should try to watch the target if we can
            requeue_after!()
        }
        Err(kube::Error::Api(err)) if err.code == 404 => {
            let patched_finalizers = sinker
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
        Err(err) => Err(err.into()),
    }
}

async fn add_target_finalizer(
    sinker: Arc<ResourceSync>,
    name: &str,
    parent_api: Api<ResourceSync>,
) -> Result<Action> {
    let patched_finalizers = sinker
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

    // For now we are watching all events for the ResourceSync, so the patch will trigger a reconcile
    Ok(Action::await_change())
}

async fn reconcile_normally(
    sinker: Arc<ResourceSync>,
    name: &str,
    source_api: NamespacedApi,
    target_api: NamespacedApi,
) -> Result<Action> {
    let target_namespace = &target_api.namespace;
    let target_ar = &target_api.ar;

    let source = source_api
        .get(&sinker.spec.source.resource_ref.name)
        .await?;
    debug!(?source, "got source object");

    let target_ref = &sinker.spec.target.resource_ref;

    let target = {
        let mut target = if sinker.spec.mappings.is_empty() {
            clone_resource(&source, target_ref, target_namespace.as_deref(), target_ar)?
        } else {
            apply_mappings(
                &source,
                target_ref,
                target_namespace.as_deref(),
                target_ar,
                &sinker,
            )?
        };

        // If the target is local then add an owner reference to it
        match sinker.spec.target.cluster.to_owned() {
            Some(_) => target,
            None => {
                target.owner_references_mut().push(OwnerReference {
                    api_version: ResourceSync::api_version(&()).to_string(),
                    kind: ResourceSync::kind(&()).to_string(),
                    name: name.to_owned(),
                    uid: sinker.metadata.uid.to_owned().ok_or(Error::UIDRequired)?,
                    controller: Some(false),
                    block_owner_deletion: Some(true),
                });

                target
            }
        }
    };

    debug!(?target, "produced target object");

    let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
    target_api
        .patch(&target_ref.name, &ssapply, &Patch::Apply(&target))
        .await?;

    info!(?name, ?target_ref, "successfully reconciled");

    requeue_after!()
}

async fn reconcile(sinker: Arc<ResourceSync>, ctx: Arc<Context>) -> Result<Action> {
    let name = sinker.metadata.name.to_owned().ok_or(Error::NameRequired)?;
    info!(?name, "running reconciler");

    debug!(?sinker.spec, "got");
    let local_ns = sinker.namespace().ok_or(Error::NamespaceRequired)?;

    let target_api = sinker
        .spec
        .target
        .api_for(Arc::clone(&ctx), &local_ns)
        .await?;
    let source_api = sinker
        .spec
        .source
        .api_for(Arc::clone(&ctx), &local_ns)
        .await?;
    let parent_api = sinker.api(Arc::clone(&ctx));

    match sinker {
        sinker if sinker.has_been_deleted() => {
            reconcile_deleted_resource(sinker, &name, target_api, parent_api).await
        }
        sinker if !sinker.has_target_finalizer() => {
            add_target_finalizer(sinker, &name, parent_api).await
        }
        _ => reconcile_normally(sinker, &name, source_api, target_api).await,
    }
}

fn error_policy(sinker: Arc<ResourceSync>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = sinker.name_any();
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
    Controller::new(docs, watcher::Config::default().any_semantic())
        .shutdown_on_signal()
        .run(reconcile, error_policy, Arc::new(Context { client }))
        .filter_map(|x| async move { Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {}
