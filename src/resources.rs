use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, CustomResourceValidation, JSONSchemaProps,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::{
    core::{gvk::ParseGroupVersionError, GroupVersionKind, TypeMeta},
    CustomResource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

static FORCE_DELETE_ANNOTATION: &str = "sinker.influxdata.io/force-delete";
static DISABLE_TARGET_DELETION_ANNOTATION: &str = "sinker.influxdata.io/disable-target-deletion";

impl ResourceSync {
    fn get_boolean_annotation_val(&self, annotation: &str) -> bool {
        self.metadata
            .annotations
            .as_ref()
            .map(|annotations| annotations.get(annotation))
            .unwrap_or_default()
            .cloned()
            .unwrap_or_default()
            .parse()
            .unwrap_or_default()
    }
    pub fn has_force_delete_option_enabled(&self) -> bool {
        self.get_boolean_annotation_val(FORCE_DELETE_ANNOTATION)
    }
    pub fn has_disable_target_deletion_option_enabled(&self) -> bool {
        self.get_boolean_annotation_val(DISABLE_TARGET_DELETION_ANNOTATION)
    }
}

#[derive(CustomResource, Debug, Serialize, Deserialize, Default, Clone, JsonSchema)]
#[kube(
    group = "sinker.influxdata.io",
    version = "v1alpha1",
    kind = "ResourceSync",
    namespaced
)]
#[kube(status = "ResourceSyncStatus")]
#[serde(rename_all = "camelCase")]
pub struct ResourceSyncSpec {
    pub source: ClusterResourceRef,
    pub target: ClusterResourceRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mappings: Vec<Mapping>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    /// If `None` then to_field_path cannot be `None`.
    pub from_field_path: Option<String>,
    /// If `None` then from_field_path cannot be `None`.
    pub to_field_path: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSyncStatus {
    pub conditions: Option<Vec<Condition>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterResourceRef {
    /// This is a reference to a resource that lives in the cluster specified by the sister cluster field.
    /// The resourceRef GVKN doesn't define the namespace explicitly. Instead, the namespace defends on the
    /// cluster reference.
    pub resource_ref: GVKN,
    /// A missing clusterRef means "this (local) cluster" and the namespace where resourceRef will be searched in
    /// is the namespace of the ResourceSync resource itself. A user cannot thus violate RBAC by referencing secrets
    /// in a namespace they don't have rights to by leveraging sinker.
    ///
    /// If a remote cluster reference is provided, then the namespace is taken from the cluster connection parameters.
    /// RBAC is still honoured because sinker can only access resources for which the provided token has rights to.
    pub cluster: Option<ClusterRef>,
}

/// This is a GVKN (apiVersion + kind + name) reference to a resource.
/// The namespace is given by the context where this reference belongs to.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GVKN {
    pub api_version: String,
    pub kind: String,
    pub name: String,
}

impl From<&GVKN> for TypeMeta {
    fn from(value: &GVKN) -> Self {
        TypeMeta {
            api_version: value.api_version.clone(),
            kind: value.kind.clone(),
        }
    }
}

impl TryFrom<&GVKN> for GroupVersionKind {
    type Error = ParseGroupVersionError;

    fn try_from(value: &GVKN) -> Result<Self, Self::Error> {
        let type_meta: TypeMeta = value.into();
        type_meta.try_into()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRef {
    /// If present, overrides the default namespace defined in the provided kubeConfig
    pub namespace: Option<String>,
    pub kube_config: KubeConfig,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KubeConfig {
    pub secret_ref: SecretRef,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecretRef {
    pub name: String,
    pub namespace: Option<String>,
    pub key: String,
}

#[derive(CustomResource, Debug, Serialize, Deserialize, Default, Clone, JsonSchema)]
#[kube(
    group = "sinker.influxdata.io",
    version = "v1alpha1",
    kind = "SinkerContainer",
    namespaced,
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct SinkerContainerSpec {}

const MANUAL_SCHEMA: &str = r#"
description: This is a handy generic resource container for use as ResourceSync sources or targets
type: object
properties:
  spec:
    description: This is an arbitrary object
    type: object
    x-kubernetes-preserve-unknown-fields: true
required:
- spec
"#;

impl SinkerContainer {
    pub fn crd_with_manual_schema() -> CustomResourceDefinition {
        use kube::CustomResourceExt;
        let schema: JSONSchemaProps = serde_yaml::from_str(MANUAL_SCHEMA).expect("invalid schema");

        let mut crd = <Self as CustomResourceExt>::crd();
        crd.spec.versions.iter_mut().for_each(|v| {
            v.schema = Some(CustomResourceValidation {
                open_api_v3_schema: Some(schema.clone()),
            })
        });
        crd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    macro_rules! gen_option_flag_tests {
        ($test_name:ident, $method:ident, $annotation_key:expr) => {
            #[rstest]
            #[case::no_annotations(
                ResourceSync {
                    metadata: Default::default(),
                    spec: Default::default(),
                    status: None,
                },
                false
            )]
            #[case::annotation_not_present(
                ResourceSync {
                    metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        annotations: Some(std::collections::BTreeMap::new()),
                        ..Default::default()
                    },
                    spec: Default::default(),
                    status: None,
                },
                false
            )]
            #[case::annotation_is_false(
                ResourceSync {
                    metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        annotations: Some(std::collections::BTreeMap::from([(
                            $annotation_key.to_string(),
                            false.to_string()
                        )])),
                        ..Default::default()
                    },
                    spec: Default::default(),
                    status: None,
                },
                false
            )]
            #[case::annotation_is_other(
                ResourceSync {
                    metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        annotations: Some(std::collections::BTreeMap::from([(
                            $annotation_key.to_string(),
                            "other".to_string()
                        )])),
                        ..Default::default()
                    },
                    spec: Default::default(),
                    status: None,
                },
                false
            )]
            #[case::annotation_is_true(
                ResourceSync {
                    metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        annotations: Some(std::collections::BTreeMap::from([(
                            $annotation_key.to_string(),
                            true.to_string()
                        )])),
                        ..Default::default()
                    },
                    spec: Default::default(),
                    status: None,
                },
                true
            )]
            #[tokio::test]
            async fn $test_name(
                #[case] resource_sync: ResourceSync,
                #[case] expected: bool,
            ) {
                assert_eq!(resource_sync.$method(), expected);
            }
        };
    }

    // Reuse the same cases for both annotation readers:
    gen_option_flag_tests!(
        test_resource_sync_has_force_delete_option_enabled,
        has_force_delete_option_enabled,
        FORCE_DELETE_ANNOTATION
    );

    gen_option_flag_tests!(
        test_resource_sync_has_disable_target_deletion_option_enabled,
        has_disable_target_deletion_option_enabled,
        DISABLE_TARGET_DELETION_ANNOTATION
    );
}
