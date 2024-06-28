use std::ops::Deref;
use std::sync::Arc;

use k8s_openapi::api::core::v1::Secret;
use kube::api::{ApiResource, DynamicObject};
use kube::{discovery, Api, Client, Config, ResourceExt};
use tracing::debug;

use crate::controller::Context;
use crate::resources::{ClusterRef, ClusterResourceRef, ResourceSync};
use crate::{Error, FINALIZER};

impl ResourceSync {
    pub fn has_been_deleted(&self) -> bool {
        self.metadata.deletion_timestamp.is_some()
    }

    pub fn has_target_finalizer(&self) -> bool {
        self.metadata
            .finalizers
            .as_ref()
            .map_or(false, |f| f.contains(&FINALIZER.to_string()))
    }

    pub fn api(&self, ctx: Arc<Context>) -> Api<Self> {
        match self.namespace() {
            None => Api::all(ctx.client.clone()),
            Some(ns) => Api::namespaced(ctx.client.clone(), &ns),
        }
    }

    pub fn finalizers_clone_or_empty(&self) -> Vec<String> {
        match self.metadata.finalizers.as_ref() {
            Some(f) => f.clone(),
            None => vec![],
        }
    }
}

/// An `Api` struct already contains the namespace but it doesn't expose an accessor for it.
/// This struct preserves a copy of the namespace we pass to the `Api` constructor when we create it.
pub struct NamespacedApi {
    pub ar: ApiResource,
    pub namespace: Option<String>,
    api: Api<DynamicObject>,
}

impl Deref for NamespacedApi {
    type Target = Api<DynamicObject>;

    fn deref(&self) -> &Self::Target {
        &self.api
    }
}

async fn cluster_client(
    cluster_ref: Option<&ClusterRef>,
    local_ns: &str,
    ctx: Arc<Context>,
) -> crate::Result<Client> {
    let client = match cluster_ref {
        None => ctx.client.clone(),
        Some(cluster_ref) => {
            let secrets: Api<Secret> = Api::namespaced(ctx.client.clone(), local_ns);
            let secret_ref = &cluster_ref.kube_config.secret_ref;
            let sec = secrets.get(&secret_ref.name).await?;

            let kube_config = kube::config::Kubeconfig::from_yaml(
                std::str::from_utf8(&sec.data.unwrap().get(&secret_ref.key).unwrap().0)
                    .map_err(Error::KubeconfigUtf8Error)?,
            )?;
            let mut config =
                Config::from_custom_kubeconfig(kube_config, &Default::default()).await?;

            if let Some(ref namespace) = cluster_ref.namespace {
                config.default_namespace = namespace.clone();
            }

            debug!(?config.cluster_url, "connecting to remote cluster");
            let remote_client = kube::Client::try_from(config)?;
            let version = remote_client.apiserver_version().await?;
            debug!(?version, "remote cluster version");

            remote_client
        }
    };
    Ok(client)
}

async fn api_for(
    cluster_resource_ref: &ClusterResourceRef,
    local_ns: &str,
    ctx: Arc<Context>,
) -> crate::Result<NamespacedApi> {
    let cluster_ref = cluster_resource_ref.cluster.as_ref();
    let client = cluster_client(cluster_ref, local_ns, ctx).await?;

    let resource_ref = &cluster_resource_ref.resource_ref;
    let (ar, _) = discovery::pinned_kind(&client, &resource_ref.try_into()?).await?;

    // if cluster_ref is a remote cluster and we don't specify a namespace in
    // the config, use the default for that client.
    // if cluster_ref is local, use local_ns.
    let (api, namespace) = match cluster_ref {
        Some(cluster) => match &cluster.namespace {
            Some(namespace) => (
                Api::namespaced_with(client.clone(), namespace, &ar),
                Some(namespace.to_owned()),
            ),
            None => (
                // assume cluster-scoped resource.
                // TODO: should someday handle "default namespace for the kubeconfig being used"
                Api::all_with(client.clone(), &ar),
                None,
            ),
        },
        None => (
            Api::namespaced_with(client.clone(), local_ns, &ar),
            Some(local_ns.to_owned()),
        ),
    };

    Ok(NamespacedApi { api, ar, namespace })
}

impl ClusterResourceRef {
    pub async fn api_for(&self, ctx: Arc<Context>, local_ns: &str) -> crate::Result<NamespacedApi> {
        api_for(self, local_ns, Arc::clone(&ctx)).await
    }
}
