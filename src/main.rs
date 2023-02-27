#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use anyhow::{anyhow, Result};
use clap::Parser;
use kube::{
    api::{ListParams, Patch, PatchParams},
    core::{gvk::ParseGroupVersionError, DynamicObject, GroupVersionKind, ObjectMeta, TypeMeta},
    discovery, Api, Config, CustomResource, CustomResourceExt, Resource, ResourceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[allow(unused_imports)]
use tracing::{debug, error, info};

use k8s_openapi::{
    api::core::v1::Secret,
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
};

#[derive(CustomResource, Debug, Serialize, Deserialize, Default, Clone, JsonSchema)]
#[kube(
    group = "sinker.mkm.pub",
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
    pub to_field_path: Option<String>,
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

#[derive(Clone, Parser)]
#[clap(version)]
struct Args {
    /// The tracing filter used for logs
    #[clap(long, env = "SINKER_LOG", default_value = "sinker=debug,warn")]
    log_level: kubert::LogFilter,

    /// The logging format
    #[clap(long, default_value = "plain")]
    log_format: kubert::LogFormat,

    #[clap(flatten)]
    client: kubert::ClientArgs,

    #[clap(flatten)]
    admin: kubert::AdminArgs,

    #[clap(long, env = "SINKER_KEEP_RUNNING")]
    keep_running: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        log_level,
        log_format,
        client,
        admin,
        keep_running,
    } = Args::parse();

    let rt = kubert::Runtime::builder()
        .with_log(log_level, log_format)
        .with_admin(admin)
        .with_client(client)
        .build()
        .await?;

    let crds: Api<CustomResourceDefinition> = Api::all(rt.client());

    let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
    crds.patch(
        &ResourceSync::crd().metadata.name.unwrap(),
        &ssapply,
        &Patch::Apply(ResourceSync::crd()),
    )
    .await?;

    debug!("CRD applied");

    let rs: Api<ResourceSync> = Api::all(rt.client());
    let sinkers = rs.list(&ListParams::default()).await?;
    for sinker in sinkers {
        let sinker_ns = sinker.namespace();

        debug!(?sinker.spec, "got");
        let cluster_ref = sinker.spec.source.cluster.as_ref();
        let secret_ref = &cluster_ref.unwrap().kube_config.secret_ref;

        let namespace = cluster_ref
            .and_then(|cluster_ref| cluster_ref.namespace.as_deref())
            .or(sinker_ns.as_deref())
            .ok_or(anyhow!("ResourceSync resource must have a namespace"))?;
        let secrets: Api<Secret> = Api::namespaced(rt.client(), namespace);

        let sec = secrets.get(&secret_ref.name).await?;

        let kube_config = kube::config::Kubeconfig::from_yaml(std::str::from_utf8(
            &sec.data.unwrap().get(&secret_ref.key).unwrap().0,
        )?)?;
        let config = Config::from_custom_kubeconfig(kube_config, &Default::default()).await?;
        let remote_client: kube::Client = kube::Client::try_from(config)?;

        let version = remote_client.apiserver_version().await?;
        debug!(?version, "remote cluster version");

        let source_ref = &sinker.spec.source.resource_ref;
        let (ar, _) = discovery::pinned_kind(&remote_client, &source_ref.try_into()?).await?;
        let api: Api<DynamicObject> = Api::namespaced_with(remote_client.clone(), namespace, &ar);
        let mut resource = api.get(&sinker.spec.source.resource_ref.name).await?;

        debug!(?resource, "got remote object");

        let target_ref = &sinker.spec.target.resource_ref;
        let annotations = resource.metadata.annotations.map(|mut annotations| {
            annotations.remove("kubectl.kubernetes.io/last-applied-configuration");
            annotations
        });

        resource.metadata = ObjectMeta {
            name: Some(target_ref.name.clone()),
            annotations: annotations,
            labels: resource.metadata.labels,
            ..Default::default()
        };
        debug!(?resource, "patched remote object");

        let local_client = rt.client();
        let (ar, _) = discovery::pinned_kind(&local_client, &target_ref.try_into()?).await?;
        let api: Api<DynamicObject> = Api::namespaced_with(local_client.clone(), namespace, &ar);

        if sinker.spec.mappings.is_empty() {
            let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
            api.patch(&target_ref.name, &ssapply, &Patch::Apply(&resource))
                .await?;
        } else {
            info!("TODO MAPPINGS");
            let mut template = DynamicObject::new(&target_ref.name, &ar);
            template.metadata = ObjectMeta {
                name: Some(target_ref.name.clone()),
                namespace: sinker.namespace(),
                ..Default::default()
            };
            assert_eq!(sinker.spec.mappings.len(), 1);
            let mapping = &sinker.spec.mappings[0];
            let resource_json = serde_json::to_value(&resource)?;
            let from_field_path = if let Some(from_field_path) = &mapping.from_field_path {
                format!("$.{}", from_field_path)
            } else {
                "$".to_string()
            };
            let subtree = jsonpath_lib::select(&resource_json, &from_field_path)?;
            assert_eq!(subtree.len(), 1);
            let subtree = subtree[0];
            debug!(?subtree);

            let mut parents = from_field_path
                .split(".")
                .skip(1)
                .collect::<Vec<_>>()
                .into_iter()
                .rev();
            let leaf = parents.next().unwrap();
            let mut parent = serde_json::Map::new();
            parent.insert(leaf.to_string(), subtree.clone());

            debug!(?leaf);
            for level in parents {
                debug!(?level, "level");
                let mut next = serde_json::Map::new();
                next.insert(level.to_string(), serde_json::Value::Object(parent));
                parent = next;
            }

            let root = serde_json::Value::Object(parent);
            template.data = root;

            let dbg = serde_json::to_string_pretty(&template)?;
            eprintln!("{}", dbg);

            let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
            api.patch(&target_ref.name, &ssapply, &Patch::Apply(&template))
                .await?;
        }
    }

    if !keep_running {
        return Ok(());
    }
    rt.run().await?;
    Ok(())
}

#[derive(thiserror::Error, Debug)]
enum AddToPathError {
    #[error("Value must be an object")]
    ObjectRequired(),
}

fn add_to_path(
    root: &mut serde_json::Value,
    path: &str,
    leaf: serde_json::Value,
) -> std::result::Result<(), AddToPathError> {
    use serde_json::Value::Object;

    if let Object(map) = root {
        let mut map = map;
        let mut path = path;
        loop {
            match path.split_once(".") {
                // this is the leaf field, just add it to the map and we're done.
                None => {
                    map.insert(path.to_string(), leaf);
                    return Ok(());
                }
                // otherwise, we have to create an object that will hold the leaf or a subtree, and iterate on.
                Some((field, rest)) => {
                    map.insert(field.to_string(), json!({}));
                    map = match map.get_mut(field).unwrap() {
                        Object(inner_map) => inner_map,
                        _ => unreachable!(), // we know it's an object, we just inserted it above as `json!{}`
                    };
                    path = rest;
                }
            };
        }
    } else {
        Err(AddToPathError::ObjectRequired())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_to_path() {
        let mut root = json!({"foo":[]});
        println!("before:\n{}", serde_json::to_string_pretty(&root).unwrap());

        add_to_path(
            &mut root,
            "foo.bar.baz",
            serde_json::Value::String("demo".to_string()),
        )
        .unwrap();

        println!("after:\n{}", serde_json::to_string_pretty(&root).unwrap());
    }
}
