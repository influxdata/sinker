use kube::{
    core::{gvk::ParseGroupVersionError, GroupVersionKind, TypeMeta},
    CustomResource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    pub from_field_path: Option<String>,
    pub to_field_path: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSyncStatus {
    pub demo: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterResourceRef {
    pub resource_ref: ResourceRef,
    pub cluster: Option<ClusterRef>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRef {
    pub api_version: String,
    pub kind: String,
    pub name: String,
}

impl From<&ResourceRef> for TypeMeta {
    fn from(value: &ResourceRef) -> Self {
        TypeMeta {
            api_version: value.api_version.clone(),
            kind: value.kind.clone(),
        }
    }
}

impl TryFrom<&ResourceRef> for GroupVersionKind {
    type Error = ParseGroupVersionError;

    fn try_from(value: &ResourceRef) -> std::result::Result<Self, Self::Error> {
        let type_meta: TypeMeta = value.into();
        type_meta.try_into()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRef {
    pub namespace: Option<String>,
    pub kube_config: KubeConfig,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KubeConfig {
    pub secret_ref: SecretRef,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecretRef {
    pub name: String,
    pub key: String,
}
