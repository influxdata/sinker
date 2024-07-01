use std::fmt::Debug;
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
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
use serde::de::DeserializeOwned;
use serde::Serialize;
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

#[async_trait]
trait KubeApi<K: Clone + DeserializeOwned + Debug + Send + Sync, P: Serialize + Debug + Send + Sync>
where
    Self: Sync + Send,
{
    async fn do_patch(
        &self,
        name: &str,
        pp: &PatchParams,
        patch: &Patch<P>,
    ) -> kubert::client::Result<K>;
}

#[async_trait]
impl<K, P> KubeApi<K, P> for Api<K>
where
    K: Clone + DeserializeOwned + Debug + Send + Sync,
    P: Serialize + Debug + Send + Sync,
{
    async fn do_patch(
        &self,
        name: &str,
        pp: &PatchParams,
        patch: &Patch<P>,
    ) -> kubert::client::Result<K> {
        self.patch(name, pp, patch).await
    }
}

async fn reconcile_deleted_resource(
    resource_sync: Arc<ResourceSync>,
    name: &str,
    target_api: NamespacedApi,
    parent_api: Api<ResourceSync>,
) -> Result<Action> {
    if !resource_sync.has_target_finalizer() {
        // We have already removed our finalizer, so nothing more needs to be done
        return Ok(Action::await_change());
    }

    let target_name = &resource_sync.spec.target.resource_ref.name;

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
        Err(err) => Err(err.into()),
    }
}

async fn add_target_finalizer(
    resource_sync: Arc<ResourceSync>,
    name: &str,
    parent_api: Box<dyn KubeApi<ResourceSync, serde_json::Value>>,
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
        .do_patch(name, &PatchParams::default(), &patch)
        .await?;

    // For now we are watching all events for the ResourceSync, so the patch will trigger a reconcile
    Ok(Action::await_change())
}

async fn reconcile_normally(
    resource_sync: Arc<ResourceSync>,
    name: &str,
    source_api: NamespacedApi,
    target_api: NamespacedApi,
) -> Result<Action> {
    let target_namespace = &target_api.namespace;
    let target_ar = &target_api.ar;

    let source = source_api
        .get(&resource_sync.spec.source.resource_ref.name)
        .await?;
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

    let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
    target_api
        .patch(&target_ref.name, &ssapply, &Patch::Apply(&target))
        .await?;

    info!(?name, ?target_ref, "successfully reconciled");

    requeue_after!()
}

async fn reconcile(resource_sync: Arc<ResourceSync>, ctx: Arc<Context>) -> Result<Action> {
    let name = resource_sync
        .metadata
        .name
        .to_owned()
        .ok_or(Error::NameRequired)?;
    info!(?name, "running reconciler");

    debug!(?resource_sync.spec, "got");
    let local_ns = resource_sync.namespace().ok_or(Error::NamespaceRequired)?;

    let target_api = resource_sync
        .spec
        .target
        .api_for(Arc::clone(&ctx), &local_ns)
        .await?;
    let source_api = resource_sync
        .spec
        .source
        .api_for(Arc::clone(&ctx), &local_ns)
        .await?;
    let parent_api = resource_sync.api(Arc::clone(&ctx));

    match resource_sync {
        resource_sync if resource_sync.has_been_deleted() => {
            reconcile_deleted_resource(resource_sync, &name, target_api, parent_api).await
        }
        resource_sync if !resource_sync.has_target_finalizer() => {
            add_target_finalizer(resource_sync, &name, Box::new(parent_api)).await
        }
        _ => reconcile_normally(resource_sync, &name, source_api, target_api).await,
    }
}

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
    Controller::new(docs, watcher::Config::default().any_semantic())
        .shutdown_on_signal()
        .run(reconcile, error_policy, Arc::new(Context { client }))
        .filter_map(|x| async move { Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use core::fmt::Debug;
    use std::sync::Arc;

    use async_trait::async_trait;
    use kube::api::{Patch, PatchParams};
    use kube::runtime::controller::Action;
    use mockall::predicate::always;
    use mockall::{mock, predicate};
    use random_string::charsets::ALPHA_LOWER;
    use random_string::generate_rng;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    use crate::controller::{add_target_finalizer, KubeApi};
    use crate::resources::{ResourceSync, ResourceSyncSpec};
    use crate::FINALIZER;

    mock! {
         pub Api<K> {}
         #[async_trait]
         impl<K> KubeApi<K, serde_json::Value> for Api<K>
         where
             K: Clone + DeserializeOwned + Debug + Send + Sync,
         {
             async fn do_patch(
                 &self,
                 name: &str,
                 pp: &PatchParams,
                 patch: &Patch<serde_json::Value>,
             ) -> kubert::client::Result<K>;
         }
    }

    #[tokio::test]
    async fn test_add_target_finalizer() {
        let name = generate_rng(1..10, ALPHA_LOWER);
        let other_finalizer = &generate_rng(1..10, ALPHA_LOWER);

        let resource_sync = {
            let mut resource_sync = ResourceSync::new(&name, ResourceSyncSpec::default());
            _ = resource_sync
                .metadata
                .finalizers
                .insert(vec![other_finalizer.clone()]);

            resource_sync
        };

        let parent_api = {
            let mut parent_api: MockApi<ResourceSync> = MockApi::new();
            parent_api
                .expect_do_patch()
                .with(
                    predicate::eq(name.clone()),
                    always(),
                    predicate::eq(Patch::Merge(json!(
                            {
                                "metadata": {
                                    "finalizers": [other_finalizer, FINALIZER],
                                },
                            }
                    ))),
                )
                .returning(|_, _, _| Ok(ResourceSync::new("", ResourceSyncSpec::default())))
                .once();

            parent_api
        };

        let action = add_target_finalizer(Arc::from(resource_sync), &name, Box::new(parent_api))
            .await
            .unwrap();
        assert_eq!(action, Action::await_change());
    }
}
