use futures::StreamExt;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use k8s_openapi::api::core::v1::Secret;
use kube::{
    api::{ListParams, Patch, PatchParams},
    core::DynamicObject,
    discovery::{self, ApiResource},
    runtime::controller::{Action, Controller},
    Api, Client, Config, Resource, ResourceExt,
};
use serde_json::json;

use crate::{
    mapping::set_field_path,
    resources::{ClusterRef, ClusterResourceRef, ResourceSync, GVKN},
    Error, Result,
};

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

struct Context {
    client: Client,
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

async fn cluster_client(
    cluster_ref: Option<&ClusterRef>,
    local_ns: &str,
    ctx: Arc<Context>,
) -> Result<Client> {
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

/// An `Api` struct already contains the namespace but it doesn't expose an accessor for it.
/// This struct preserves a copy of the namespace we pass to the `Api` constructor when we create it.
struct NamespacedApi {
    api: Api<DynamicObject>,
    ar: ApiResource,
    namespace: String,
}

async fn api_for(
    cluster_resource_ref: &ClusterResourceRef,
    local_ns: &str,
    ctx: Arc<Context>,
) -> Result<NamespacedApi> {
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
                namespace.to_owned(),
            ),
            None => (
                Api::default_namespaced_with(client.clone(), &ar),
                client.default_namespace().to_owned(),
            ),
        },
        None => (
            Api::namespaced_with(client.clone(), local_ns, &ar),
            local_ns.to_owned(),
        ),
    };

    Ok(NamespacedApi { api, ar, namespace })
}

async fn reconcile(sinker: Arc<ResourceSync>, ctx: Arc<Context>) -> Result<Action> {
    let name = sinker.name_any();
    info!(?name, "running reconciler");

    debug!(?sinker.spec, "got");
    let local_ns = sinker.namespace().ok_or(Error::NamespaceRequired)?;

    let NamespacedApi { api, .. } =
        api_for(&sinker.spec.source, &local_ns, Arc::clone(&ctx)).await?;
    let source = api.get(&sinker.spec.source.resource_ref.name).await?;
    debug!(?source, "got source object");

    let target_ref = &sinker.spec.target.resource_ref;
    let NamespacedApi {
        api,
        ar,
        namespace: target_namespace,
    } = api_for(&sinker.spec.target, &local_ns, Arc::clone(&ctx)).await?;

    debug!(%target_namespace, "got client for target");

    let target = if sinker.spec.mappings.is_empty() {
        clone_resource(&source, target_ref, &target_namespace, &ar)?
    } else {
        apply_mappings(&source, target_ref, &target_namespace, &ar, &sinker)?
    };
    debug!(?target, "produced target object");

    let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
    api.patch(&target_ref.name, &ssapply, &Patch::Apply(&target))
        .await?;

    info!(?name, ?target_ref, "successfully reconciled");

    // TODO(mkm): make requeue duration configurable
    Ok(Action::requeue(Duration::from_secs(5)))
}

// copies data, annotations and labels from source
fn clone_resource(
    source: &DynamicObject,
    target_ref: &GVKN,
    target_namespace: &str,
    ar: &ApiResource,
) -> Result<DynamicObject> {
    let mut target = DynamicObject::new(&target_ref.name, ar)
        .within(target_namespace)
        .data(source.data.clone());

    target.metadata.annotations = source.metadata.annotations.clone().map(cleanup_annotations);
    target.metadata.labels = source.metadata.labels.clone();

    Ok(target)
}

// copies only fields explicitly selected in the sinks' spec.mappings
fn apply_mappings(
    source: &DynamicObject,
    target_ref: &GVKN,
    target_namespace: &str,
    ar: &ApiResource,
    sinker: &ResourceSync,
) -> Result<DynamicObject> {
    let mut template = DynamicObject::new(&target_ref.name, ar)
        .within(target_namespace)
        .data(json!({}));

    for mapping in &sinker.spec.mappings {
        let subtree = find_field_path(source, &mapping.from_field_path)?;
        debug!(?subtree, ?mapping.from_field_path, "from field path");

        let dbg = serde_json::to_string_pretty(&template)?;
        debug!(%dbg, "before");

        debug!(?subtree, ?mapping.to_field_path, "to field path");
        if mapping.to_field_path.starts_with("metadata.") {
            // DynamicObject's metadata is not a serde_json::value::Value,
            // but we can convert to/from that and use the same code to update
            // it as we do for spec/status in .data
            let mut metadata = serde_json::value::to_value(template.metadata.clone())?;
            set_field_path(
                &mut metadata,
                &mapping.to_field_path.strip_prefix("metadata.").unwrap(),
                subtree.clone(),
            )?;
            template.metadata = serde_json::value::from_value(metadata)?;
        } else {
            set_field_path(&mut template.data, &mapping.to_field_path, subtree.clone())?;
        }
        let dbg = serde_json::to_string_pretty(&template)?;
        debug!(%dbg, "after");
    }
    Ok(template)
}

fn error_policy(sinker: Arc<ResourceSync>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = sinker.name_any();
    warn!(?name, %error, "reconcile failed");
    // TODO(mkm): make error requeue duration configurable
    Action::requeue(Duration::from_secs(5 * 60))
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
    Controller::new(docs, ListParams::default())
        .shutdown_on_signal()
        .run(reconcile, error_policy, Arc::new(Context { client }))
        .filter_map(|x| async move { std::result::Result::ok(x) })
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
        format!("$.{}", from_field_path)
    } else {
        "$".to_string()
    };
    match jsonpath_lib::select(&resource_json, &from_field_path)?.as_slice() {
        [] => Ok(json!(null)),
        [subtree] => Ok((*subtree).clone()),
        _ => Err(Error::JsonPathExactlyOneValue(from_field_path.to_owned())),
    }
}
