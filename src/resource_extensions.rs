use std::ops::Deref;
use std::sync::Arc;

use k8s_openapi::api::core::v1::Secret;
use kube::api::{ApiResource, DynamicObject};
use kube::runtime::reflector::ObjectRef;
use kube::{discovery, Api, Client, Config, ResourceExt};
use tracing::debug;

use crate::controller::Context;
use crate::remote_watcher::RemoteWatcherKey;
use crate::resources::{ClusterRef, ClusterResourceRef, ResourceSync};
use crate::{Error, FINALIZER};

macro_rules! rs_watch {
    ($fn_name:ident, $method:ident) => {
        pub async fn $fn_name(&self, ctx: Arc<Context>) {
            self.spec.source.$method(Arc::clone(&ctx), &self).await;
            self.spec.target.$method(Arc::clone(&ctx), &self).await;
        }
    };
}

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

    pub fn api(&self, client: Client) -> Api<Self> {
        match self.namespace() {
            None => Api::all(client),
            Some(ns) => Api::namespaced(client, &ns),
        }
    }

    pub fn finalizers_clone_or_empty(&self) -> Vec<String> {
        match self.metadata.finalizers.as_ref() {
            Some(f) => f.clone(),
            None => vec![],
        }
    }

    rs_watch!(
        start_remote_watches_if_not_watching,
        start_watch_if_not_watching
    );
    rs_watch!(stop_remote_watches_if_watching, stop_watch_if_watching);
}

macro_rules! crr_watch {
    ($fn_name:ident, $method:ident) => {
        pub async fn $fn_name(&self, ctx: Arc<Context>, resource_sync: &ResourceSync) {
            ctx.remote_watcher_manager
                .$method(&self.remote_watcher_key(resource_sync))
                .await;
        }
    };
}

impl ClusterResourceRef {
    fn remote_watcher_key(&self, resource_sync: &ResourceSync) -> RemoteWatcherKey {
        RemoteWatcherKey {
            object: self.clone(),
            resource_sync: ObjectRef::from_obj(resource_sync),
        }
    }

    crr_watch!(start_watch_if_not_watching, add_if_not_exists);
    crr_watch!(stop_watch_if_watching, stop_and_remove_if_exists);
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
    client: Client,
) -> crate::Result<Client> {
    let client = match cluster_ref {
        None => client,
        Some(cluster_ref) => {
            let secrets: Api<Secret> = Api::namespaced(client, local_ns);
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
    client: Client,
) -> crate::Result<NamespacedApi> {
    let cluster_ref = cluster_resource_ref.cluster.as_ref();
    let client = cluster_client(cluster_ref, local_ns, client).await?;

    let resource_ref = &cluster_resource_ref.resource_ref;
    let (ar, _) = discovery::pinned_kind(&client, &resource_ref.try_into()?).await?;

    // if cluster_ref is a remote cluster and we don't specify a namespace in
    // the config, use the default for that client.
    // if cluster_ref is local, use local_ns.
    let (api, namespace) = match cluster_ref {
        Some(cluster) => match &cluster.namespace {
            Some(namespace) => (
                Api::namespaced_with(client, namespace, &ar),
                Some(namespace.to_owned()),
            ),
            None => (
                // assume cluster-scoped resource.
                // TODO: should someday handle "default namespace for the kubeconfig being used"
                Api::all_with(client, &ar),
                None,
            ),
        },
        None => (
            Api::namespaced_with(client, local_ns, &ar),
            Some(local_ns.to_owned()),
        ),
    };

    Ok(NamespacedApi { api, ar, namespace })
}

impl ClusterResourceRef {
    pub async fn api_for(&self, client: Client, local_ns: &str) -> crate::Result<NamespacedApi> {
        api_for(self, local_ns, client).await
    }
}
