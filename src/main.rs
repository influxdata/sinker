#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

use anyhow::Result;
use clap::Parser;
use kube::{
    api::{Patch, PatchParams},
    Api, CustomResource, CustomResourceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use tracing::{debug, error, info};

use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
#[derive(CustomResource, Debug, Serialize, Deserialize, Default, Clone, JsonSchema)]
#[kube(
    group = "sinker.mkm.pub",
    version = "v1alpha1",
    kind = "ResourceSync",
    namespaced
)]
#[kube(status = "ResourceSyncStatus")]
pub struct ResourceSyncSpec {
    source: ClusterResourceRef,
    destination: ClusterResourceRef,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
pub struct ResourceSyncStatus {
    demo: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
pub struct ClusterResourceRef {
    resource_ref: ResourceRef,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
pub struct ResourceRef {
    pub api_group: String,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        log_level,
        log_format,
        client,
        admin,
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

    rt.run().await?;
    Ok(())
}
