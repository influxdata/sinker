use std::collections::BTreeMap;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ApiResource, DynamicObject, GroupVersionKind};
use serde_json::json;
use serde_json_path::JsonPath;
use tracing::debug;

use crate::resources::{Mapping, ResourceSync, GVKN};
use crate::Error;

#[derive(thiserror::Error, Debug)]
pub enum AddToPathError {
    #[error("Value for field {0} must be an object")]
    ObjectRequired(serde_json::Value),
}

fn cleanup_annotations(mut annotations: BTreeMap<String, String>) -> BTreeMap<String, String> {
    annotations.remove("kubectl.kubernetes.io/last-applied-configuration");
    annotations
}

// copies data, annotations and labels from source
pub fn clone_resource(
    source: &DynamicObject,
    target_ref: &GVKN,
    target_namespace: Option<&str>,
    ar: &ApiResource,
) -> crate::Result<DynamicObject> {
    let mut target = DynamicObject::new(&target_ref.name, ar).data(source.data.clone());
    target.metadata.namespace = target_namespace.map(String::from);

    target.metadata.annotations = source.metadata.annotations.clone().map(cleanup_annotations);
    target.metadata.labels = source.metadata.labels.clone();

    Ok(target)
}

// copies only fields explicitly selected in the sinks' spec.mappings
pub fn apply_mappings(
    source: &DynamicObject,
    target_ref: &GVKN,
    target_namespace: Option<&str>,
    ar: &ApiResource,
    sinker: &ResourceSync,
) -> crate::Result<DynamicObject> {
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
fn get_ar_from_subtree(subtree: &serde_json::Value) -> crate::Result<ApiResource> {
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

fn set_field_path(
    root: &mut serde_json::Value,
    path: &str,
    leaf: serde_json::Value,
) -> Result<(), AddToPathError> {
    use serde_json::Value::Object;

    if let Object(map) = root {
        let mut map = map;
        let mut path = path;
        loop {
            match path.split_once('.') {
                // this is the leaf field, just add it to the map and we're done.
                None => {
                    map.insert(path.to_string(), leaf);
                    return Ok(());
                }
                // otherwise, we have to create an object that will hold the leaf or a subtree, and iterate on.
                Some((field, rest)) => {
                    map = match map.entry(field).or_insert(json!({})) {
                        Object(inner_map) => inner_map,
                        _ => return Err(AddToPathError::ObjectRequired(root.clone())),
                    };
                    path = rest;
                }
            };
        }
    } else {
        Err(AddToPathError::ObjectRequired(root.clone()))
    }
}

fn find_field_path<T>(
    resource: T,
    from_field_path: &Option<String>,
) -> crate::Result<serde_json::Value>
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
    use rstest::*;

    use crate::resources::{ClusterResourceRef, ResourceSyncSpec};

    use super::*;

    #[rstest]
    #[case("status", r#"{"spec":{},"status":"demo"}"#)]
    #[case("status.foo", r#"{"spec":{},"status":{"keep":1,"foo":"demo"}}"#)]
    #[case(
        "status.foo.bar",
        r#"{"spec":{},"status":{"keep":1,"foo":{"bar":"demo"}}}"#
    )]
    fn test_add_to_path(#[case] path: &str, #[case] expected: &str) {
        let mut root = json!({ "spec": {}, "status": {"keep": 1} });
        set_field_path(
            &mut root,
            path,
            serde_json::Value::String("demo".to_string()),
        )
        .unwrap();

        assert_eq!(serde_json::to_string(&root).unwrap(), expected);
    }

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
