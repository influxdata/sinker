use std::{collections::BTreeMap, sync::Arc, time::Duration};

use futures::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::DeleteParams;
use kube::api::Patch::Merge;
use kube::{
    api::{ListParams, Patch, PatchParams},
    core::{DynamicObject, GroupVersionKind, ObjectMeta},
    discovery::{self, ApiResource},
    runtime::{
        controller::{Action, Controller},
        watcher,
    },
    Api, Client, Config, Resource, ResourceExt,
};
use serde_json::json;
use serde_json_path::JsonPath;
#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use util::{WithItemAdded, WithItemRemoved};

use crate::resource_extensions::NamespacedApi;
use crate::{
    mapping::set_field_path,
    requeue_after,
    resources::{ClusterRef, ClusterResourceRef, Mapping, ResourceSync, GVKN},
    util, Error, Result, FINALIZER,
};

pub struct Context {
    pub client: Client,
}

#[cfg(deleteme)]
async fn resource_fetcher(
    resource_ref: &ClusterResourceRef,
    local_ns: &str,
    ctx: Arc<Context>,
) -> Result<()> {
    let client = cluster_client(resource_ref.cluster.as_ref(), local_ns, ctx).await?;
    Ok(())
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

// copies data, annotations and labels from source
fn clone_resource(
    source: &DynamicObject,
    target_ref: &GVKN,
    target_namespace: Option<&str>,
    ar: &ApiResource,
) -> Result<DynamicObject> {
    let mut target = DynamicObject::new(&target_ref.name, ar).data(source.data.clone());
    target.metadata.namespace = target_namespace.map(String::from);

    target.metadata.annotations = source.metadata.annotations.clone().map(cleanup_annotations);
    target.metadata.labels = source.metadata.labels.clone();

    Ok(target)
}

// copies only fields explicitly selected in the sinks' spec.mappings
fn apply_mappings(
    source: &DynamicObject,
    target_ref: &GVKN,
    target_namespace: Option<&str>,
    ar: &ApiResource,
    sinker: &ResourceSync,
) -> Result<DynamicObject> {
    let mut template = DynamicObject::new(&target_ref.name, ar).data(json!({}));
    template.metadata.namespace = target_namespace.map(String::from);

    for mapping in &sinker.spec.mappings {
        let subtree = find_field_path(source, &mapping.from_field_path)?;
        debug!(?subtree, ?mapping.from_field_path, "from field path");

        let dbg = serde_json::to_string_pretty(&template)?;
        debug!(%dbg, "before");

        debug!(?subtree, ?mapping.to_field_path, "to field path");
        match mapping {
            Mapping {
                from_field_path: None,
                to_field_path: None,
            } => {
                // user must specify either from, to or both, but not neither.
                // leave the mapping array empty if that's what they want.
                return Err(Error::MappingEmpty);
            }
            Mapping {
                from_field_path: Some(_),
                to_field_path: None,
            } => {
                // this is like a clone_resource but the source is a subtree not the whole object.
                // likely copying the inner resource of a SinkerContainer into root.
                // we need to convert `subtree` into a DynamicObject that will work with our
                // existing `clone_resource` function, taking care to preserve the metadata
                // and not produce duplicate fields.
                let ar = get_ar_from_subtree(&subtree)?;
                let source_metadata = convert_metadata(&subtree["metadata"]);
                let mut subtree = subtree.clone();
                cleanup_subtree(&mut subtree);
                let mut source = DynamicObject::new(&subtree["metadata"]["name"].to_string(), &ar)
                    .data(subtree.clone());
                source.metadata.namespace = subtree["metadata"]["namespace"]
                    .as_str()
                    .map(str::to_string);
                source.metadata.annotations = source_metadata.annotations;
                source.metadata.labels = source_metadata.labels;
                template = clone_resource(&source, target_ref, target_namespace, &ar)?;
            }
            Mapping {
                from_field_path: _,
                to_field_path: Some(to_field_path),
            } => {
                if to_field_path.starts_with("metadata.") {
                    // DynamicObject's metadata is not a serde_json::value::Value,
                    // but we can convert to/from that and use the same code to update
                    // it as we do for spec/status in .data
                    let mut metadata = serde_json::value::to_value(template.metadata.clone())?;
                    set_field_path(
                        &mut metadata,
                        to_field_path.strip_prefix("metadata.").unwrap(),
                        subtree.clone(),
                    )?;
                    template.metadata = serde_json::value::from_value(metadata)?;
                } else {
                    set_field_path(&mut template.data, to_field_path, subtree.clone())?;
                }
            }
        }
        let dbg = serde_json::to_string_pretty(&template)?;
        debug!(%dbg, "after");
    }
    Ok(template)
}

fn map_conversion(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<BTreeMap<String, String>> {
    Some(
        map.iter()
            .map(|(k, v)| (k.to_string(), v.as_str().unwrap().to_string()))
            .collect::<BTreeMap<String, String>>(),
    )
}

// extract metadata that we can set on a DynamicObject from a serde_json value subtree.
fn convert_metadata(subtree: &serde_json::Value) -> ObjectMeta {
    let mut metadata = ObjectMeta {
        ..Default::default()
    };
    if let serde_json::Value::Object(map) = &subtree["annotations"] {
        metadata.annotations = map_conversion(map);
    }
    if let serde_json::Value::Object(map) = &subtree["labels"] {
        metadata.labels = map_conversion(map);
    }
    metadata
}

// extract GVKN from a k8s resource in an arbitrary serde_json subtree.
fn get_ar_from_subtree(subtree: &serde_json::Value) -> Result<ApiResource> {
    let api_version = subtree["apiVersion"]
        .as_str()
        .ok_or(Error::MalformedInnerResource(
            "failed to parse apiVersion".to_string(),
        ))?;
    // account for group-less resources by extracting version from the end first,
    // then group if there is a term remaining.
    let mut gv_terms = api_version.split('/').rev();
    let version = gv_terms
        .next()
        .ok_or(Error::MalformedInnerResource(
            "failed to parse apiVersion".to_string(),
        ))?
        .to_string();
    let group = gv_terms.next().unwrap_or("").to_string();
    let kind = subtree["kind"]
        .as_str()
        .ok_or(Error::MalformedInnerResource(
            "failed to parse kind".to_string(),
        ))?
        .to_string();
    Ok(ApiResource::from_gvk(&GroupVersionKind {
        group,
        version,
        kind,
    }))
}

// used when creating a DynamicObject from a k8s resource in arbitrary subtree. removes the
// fields that are already stored in the DynamicObject and we therefore don't want in .data.
// if we didn't do this then the resulting resource would have these fields twice.
fn cleanup_subtree(subtree: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = subtree {
        map.remove("apiVersion");
        map.remove("kind");
        map.remove("metadata");
    }
}

fn error_policy(sinker: Arc<ResourceSync>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = sinker.name_any();
    warn!(?name, %error, "reconcile failed");
    // TODO(mkm): make error requeue duration configurable
    Action::requeue(Duration::from_secs(5))
}

fn cleanup_annotations(mut annotations: BTreeMap<String, String>) -> BTreeMap<String, String> {
    annotations.remove("kubectl.kubernetes.io/last-applied-configuration");
    annotations
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

fn find_field_path<T>(resource: T, from_field_path: &Option<String>) -> Result<serde_json::Value>
where
    T: serde::Serialize,
{
    let resource_json = serde_json::to_value(&resource)?;
    let from_field_path = if let Some(from_field_path) = from_field_path {
        if from_field_path.is_empty() {
            "$".to_string()
        } else {
            format!("$.{}", from_field_path)
        }
    } else {
        "$".to_string()
    };
    let subtree = JsonPath::parse(&from_field_path)?
        .query(&resource_json)
        .at_most_one()
        .map_err(|_| Error::JsonPathExactlyOneValue(from_field_path.to_owned()))?;
    Ok(subtree.cloned().unwrap_or(json!(null)))
}

#[cfg(test)]
mod tests {
    use kube::core::{ApiResource, GroupVersionKind};

    use crate::resources::{Mapping, ResourceSyncSpec};

    use super::*;

    #[tokio::test]
    async fn test_get_ar_from_subtree() {
        let subtree = &json!({
            "apiVersion": "sinker.influxdata.io/v1alpha1",
            "kind": "SinkerContainer",
            "metadata": { "name": "test-sinker-container" },
            "spec": {
                "dummykey": "dummyvalue",
            },
        });
        let ar = get_ar_from_subtree(subtree).unwrap();
        assert_eq!(ar.group, "sinker.influxdata.io");
        assert_eq!(ar.version, "v1alpha1");
        assert_eq!(ar.kind, "SinkerContainer");
        assert_eq!(ar.api_version, "sinker.influxdata.io/v1alpha1");
    }

    #[tokio::test]
    async fn test_get_ar_from_subtree_nogroup() {
        let subtree = &json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": { "name": "test-config-map-1" },
            "data": {
                "dummykey": "dummyvalue",
            },
        });
        let ar = get_ar_from_subtree(subtree).unwrap();
        assert_eq!(ar.group, "");
        assert_eq!(ar.version, "v1");
        assert_eq!(ar.kind, "ConfigMap");
        assert_eq!(ar.api_version, "v1");
    }

    #[tokio::test]
    async fn test_clone_resource() {
        let resource_sync = ResourceSync::new(
            "sinker-test",
            ResourceSyncSpec {
                mappings: vec![],
                source: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "v1".to_string(),
                        kind: "ConfigMap".to_string(),
                        name: "test-configmap-1".to_string(),
                    },
                    cluster: None,
                },
                target: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "v1".to_string(),
                        kind: "ConfigMap".to_string(),
                        name: "test-configmap-2".to_string(),
                    },
                    cluster: None,
                },
            },
        );
        let dynamic_sc: DynamicObject = serde_json::from_str(
            &serde_json::to_string(&json!({
                "apiVersion": "v1",
                "kind": "ConfigMap",
                "metadata": { "name": "test-configmap-1" },
                "data": {
                    "dummykey": "dummyvalue",
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let expected = json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {
                "name": "test-configmap-2",
                "namespace": "default",
            },
            "data": {
                "dummykey": "dummyvalue",
            },
        });
        let ar = ApiResource::from_gvk(&GroupVersionKind {
            group: "".to_string(),
            version: "v1".to_string(),
            kind: "ConfigMap".to_string(),
        });
        let target = clone_resource(
            &dynamic_sc,
            &resource_sync.spec.target.resource_ref,
            Some("default"),
            &ar,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            serde_json::to_string(&expected).unwrap(),
        );
    }

    #[tokio::test]
    async fn test_clone_resource_cluster_scoped() {
        let resource_sync = ResourceSync::new(
            "sinker-test",
            ResourceSyncSpec {
                mappings: vec![],
                source: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "rbac.authorization.k8s.io/v1".to_string(),
                        kind: "ClusterRole".to_string(),
                        name: "test-clusterrole-1".to_string(),
                    },
                    cluster: None,
                },
                target: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "rbac.authorization.k8s.io/v1".to_string(),
                        kind: "ClusterRole".to_string(),
                        name: "test-clusterrole-2".to_string(),
                    },
                    cluster: None,
                },
            },
        );
        let dynamic_sc: DynamicObject = serde_json::from_str(
            &serde_json::to_string(&json!({
                "apiVersion": "rbac.authorization.k8s.io/v1",
                "kind": "ClusterRole",
                "metadata": { "name": "test-clusterrole-1" },
                "rules": [],
            }))
            .unwrap(),
        )
        .unwrap();
        let expected = json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "ClusterRole",
            "metadata": {
                "name": "test-clusterrole-2",
                "namespace": "default",
            },
            "rules": [],
        });
        let ar = ApiResource::from_gvk(&GroupVersionKind {
            group: "rbac.authorization.k8s.io".to_string(),
            version: "v1".to_string(),
            kind: "ClusterRole".to_string(),
        });
        let target = clone_resource(
            &dynamic_sc,
            &resource_sync.spec.target.resource_ref,
            Some("default"),
            &ar,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            serde_json::to_string(&expected).unwrap(),
        );
    }

    #[tokio::test]
    async fn test_apply_mappings() {
        let resource_sync = ResourceSync::new(
            "sinker-test",
            ResourceSyncSpec {
                mappings: vec![Mapping {
                    from_field_path: Some("spec.subtree1".to_string()),
                    to_field_path: Some("spec.subtree2".to_string()),
                }],
                source: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "sinker.influxdata.io/v1alpha1".to_string(),
                        kind: "SinkerContainer".to_string(),
                        name: "test-sinker-container-1".to_string(),
                    },
                    cluster: None,
                },
                target: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "sinker.influxdata.io/v1alpha1".to_string(),
                        kind: "SinkerContainer".to_string(),
                        name: "test-sinker-container-2".to_string(),
                    },
                    cluster: None,
                },
            },
        );
        let dynamic_sc: DynamicObject = serde_json::from_str(
            &serde_json::to_string(&json!({
                "apiVersion": "sinker.influxdata.io/v1alpha1",
                "kind": "SinkerContainer",
                "metadata": { "name": "test-sinker-container-1" },
                "spec": {
                    "subtree1": {
                        "key": "value",
                    },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let expected = json!({
            "apiVersion": "sinker.influxdata.io/v1alpha1",
            "kind": "SinkerContainer",
            "metadata": {
                "name": "test-sinker-container-2",
                "namespace": "default",
            },
            "spec": {
                "subtree2": {
                    "key": "value",
                },
            },
        });
        let ar = ApiResource::from_gvk(&GroupVersionKind {
            group: "sinker.influxdata.io".to_string(),
            version: "v1alpha1".to_string(),
            kind: "SinkerContainer".to_string(),
        });
        let target = apply_mappings(
            &dynamic_sc,
            &resource_sync.spec.target.resource_ref,
            Some("default"),
            &ar,
            &resource_sync,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            serde_json::to_string(&expected).unwrap(),
        );
    }

    #[tokio::test]
    async fn test_apply_mappings_to_metadata() {
        let resource_sync = ResourceSync::new(
            "sinker-test",
            ResourceSyncSpec {
                mappings: vec![
                    Mapping {
                        from_field_path: Some("spec.subtree1".to_string()),
                        to_field_path: Some("spec.subtree2".to_string()),
                    },
                    Mapping {
                        from_field_path: Some("metadata.labels".to_string()),
                        to_field_path: Some("metadata.labels".to_string()),
                    },
                ],
                source: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "sinker.influxdata.io/v1alpha1".to_string(),
                        kind: "SinkerContainer".to_string(),
                        name: "test-sinker-container-1".to_string(),
                    },
                    cluster: None,
                },
                target: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "sinker.influxdata.io/v1alpha1".to_string(),
                        kind: "SinkerContainer".to_string(),
                        name: "test-sinker-container-2".to_string(),
                    },
                    cluster: None,
                },
            },
        );
        let dynamic_sc: DynamicObject = serde_json::from_str(
            &serde_json::to_string(&json!({
                "apiVersion": "sinker.influxdata.io/v1alpha1",
                "kind": "SinkerContainer",
                "metadata": {
                    "labels": { "key": "value" },
                    "name": "test-sinker-container-1",
                },
                "spec": {
                    "subtree1": {
                        "key": "value",
                    },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let expected = json!({
            "apiVersion": "sinker.influxdata.io/v1alpha1",
            "kind": "SinkerContainer",
            "metadata": {
                "labels": { "key": "value" },
                "name": "test-sinker-container-2",
                "namespace": "default",
            },
            "spec": {
                "subtree2": {
                    "key": "value",
                },
            },
        });
        let ar = ApiResource::from_gvk(&GroupVersionKind {
            group: "sinker.influxdata.io".to_string(),
            version: "v1alpha1".to_string(),
            kind: "SinkerContainer".to_string(),
        });
        let target = apply_mappings(
            &dynamic_sc,
            &resource_sync.spec.target.resource_ref,
            Some("default"),
            &ar,
            &resource_sync,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            serde_json::to_string(&expected).unwrap(),
        );
    }

    #[tokio::test]
    async fn test_apply_mappings_from_sinkercontainer() {
        let resource_sync = ResourceSync::new(
            "sinker-test",
            ResourceSyncSpec {
                mappings: vec![Mapping {
                    from_field_path: Some("spec".to_string()),
                    to_field_path: None,
                }],
                source: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "sinker.influxdata.io/v1alpha1".to_string(),
                        kind: "SinkerContainer".to_string(),
                        name: "test-sinker-container".to_string(),
                    },
                    cluster: None,
                },
                target: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "v1".to_string(),
                        kind: "ConfigMap".to_string(),
                        name: "test-config-map-2".to_string(),
                    },
                    cluster: None,
                },
            },
        );
        let ar = ApiResource::from_gvk(&GroupVersionKind {
            group: "".to_string(),
            version: "v1".to_string(),
            kind: "ConfigMap".to_string(),
        });
        let expected = json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {
                "annotations": {
                    "key1": "value1",
                },
                "labels": {
                    "key2": "value2",
                },
                "name": "test-config-map-2",
                "namespace": "default",
            },
            "data": {
                "dummykey": "dummyvalue",
            },
        });
        let dynamic_sc: DynamicObject = serde_json::from_str(
            &serde_json::to_string(&json!({
                "apiVersion": "sinker.influxdata.io/v1alpha1",
                "kind": "SinkerContainer",
                "metadata": { "name": "test-sinker-container" },
                "spec": {
                    "apiVersion": "v1",
                    "kind": "ConfigMap",
                    "metadata": {
                        "annotations": { "key1": "value1" },
                        "labels": { "key2": "value2" },
                        "name": "test-config-map-1",
                    },
                    "data": {
                        "dummykey": "dummyvalue",
                    },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let target = apply_mappings(
            &dynamic_sc,
            &resource_sync.spec.target.resource_ref,
            Some("default"),
            &ar,
            &resource_sync,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            serde_json::to_string(&expected).unwrap(),
        );
    }

    #[tokio::test]
    async fn test_apply_mappings_from_sinkercontainer_clusterscoped() {
        let resource_sync = ResourceSync::new(
            "sinker-test",
            ResourceSyncSpec {
                mappings: vec![Mapping {
                    from_field_path: Some("spec".to_string()),
                    to_field_path: None,
                }],
                source: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "sinker.influxdata.io/v1alpha1".to_string(),
                        kind: "SinkerContainer".to_string(),
                        name: "test-sinker-container".to_string(),
                    },
                    cluster: None,
                },
                target: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "rbac.authorization.k8s.io/v1".to_string(),
                        kind: "ClusterRole".to_string(),
                        name: "test-clusterrole".to_string(),
                    },
                    cluster: None,
                },
            },
        );
        let ar = ApiResource::from_gvk(&GroupVersionKind {
            group: "rbac.authorization.k8s.io".to_string(),
            version: "v1".to_string(),
            kind: "ClusterRole".to_string(),
        });
        let expected = json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "ClusterRole",
            "metadata": {
                "name": "test-clusterrole",
                "namespace": "default",
            },
            "rules": [],
        });
        let dynamic_sc: DynamicObject = serde_json::from_str(
            &serde_json::to_string(&json!({
                "apiVersion": "sinker.influxdata.io/v1alpha1",
                "kind": "SinkerContainer",
                "metadata": { "name": "test-sinker-container" },
                "spec": {
                    "apiVersion": "rbac.authorization.k8s.io/v1",
                    "kind": "ClusterRole",
                    "metadata": {
                        "name": "test-clusterrole",
                    },
                    "rules": [],
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let target = apply_mappings(
            &dynamic_sc,
            &resource_sync.spec.target.resource_ref,
            Some("default"),
            &ar,
            &resource_sync,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            serde_json::to_string(&expected).unwrap(),
        );
    }

    #[tokio::test]
    async fn test_apply_mappings_to_sinkercontainer() {
        let resource_sync = ResourceSync::new(
            "sinker-test",
            ResourceSyncSpec {
                mappings: vec![Mapping {
                    from_field_path: None,
                    to_field_path: Some("spec".to_string()),
                }],
                source: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "v1".to_string(),
                        kind: "Deployment".to_string(),
                        name: "test-deployment".to_string(),
                    },
                    cluster: None,
                },
                target: ClusterResourceRef {
                    resource_ref: GVKN {
                        api_version: "sinker.influxdata.io/v1alpha1".to_string(),
                        kind: "SinkerContainer".to_string(),
                        name: "test-sinker-container".to_string(),
                    },
                    cluster: None,
                },
            },
        );
        let ar = ApiResource::from_gvk(&GroupVersionKind {
            group: "sinker.influxdata.io".to_string(),
            version: "v1alpha1".to_string(),
            kind: "SinkerContainer".to_string(),
        });
        let source_dep = json!({
            "apiVersion": "v1",
            "kind": "Deployment",
            "metadata": {
                "annotations": {
                    "key1": "value1",
                },
                "labels": {
                    "key2": "value2",
                },
                "name": "test-deployment",
                "namespace": "default",
            },
            "spec": {
                "dummykey": "dummyvalue",
            },
            "status": {
                "isgood": true,
            },
        });
        let expected = json!({
            "apiVersion": "sinker.influxdata.io/v1alpha1",
            "kind": "SinkerContainer",
            "metadata": { "name": "test-sinker-container", "namespace": "default" },
            "spec": source_dep,
        });
        let dynamic_sc: DynamicObject =
            serde_json::from_str(&serde_json::to_string(&json!(source_dep)).unwrap()).unwrap();
        let target = apply_mappings(
            &dynamic_sc,
            &resource_sync.spec.target.resource_ref,
            Some("default"),
            &ar,
            &resource_sync,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            serde_json::to_string(&expected).unwrap(),
        );
    }
}
