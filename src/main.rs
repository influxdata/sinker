#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use anyhow::Result;
use clap::Parser;
use kube::{
    api::{ListParams, Patch, PatchParams},
    Api, CustomResource, CustomResourceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    pub destination: ClusterResourceRef,
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
    pub api_group: Option<String>,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRef {
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
    pub namespace: Option<String>, // should we allow this?
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

    let ssapply = PatchParams::apply("crd_apply").force();
    crds.patch(
        "resourcesyncs.sinker.mkm.pub",
        &ssapply,
        &Patch::Apply(ResourceSync::crd()),
    )
    .await?;

    debug!("CRD applied");

    let rs: Api<ResourceSync> = Api::all(rt.client());
    let sinkers = rs.list(&ListParams::default()).await?;
    for sinker in sinkers {
        debug!(?sinker.spec, "got");
        let secret_ref = sinker.spec.source.cluster.unwrap().kube_config.secret_ref;
        let secret_name = secret_ref.name;

        let secrets: Api<Secret> = if let Some(ref namespace) = secret_ref.namespace {
            Api::namespaced(rt.client(), namespace)
        } else {
            Api::default_namespaced(rt.client())
        };

        let sec = secrets.get(&secret_name).await?;
        let len = sec.data.unwrap().get("value").unwrap().0.len();
        debug!(?len, "got secret ok");
    }

    if !keep_running {
        return Ok(());
    }
    rt.run().await?;
    Ok(())
}
