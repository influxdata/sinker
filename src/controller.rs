use futures::StreamExt;
use std::{sync::Arc, time::Duration};

use k8s_openapi::{
    api::core::v1::Secret,
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
};
use kube::{
    api::{ListParams, Patch, PatchParams},
    core::{DynamicObject, ObjectMeta},
    discovery,
    runtime::controller::{Action, Controller},
    Api, Client, Config, CustomResourceExt, Resource, ResourceExt,
};
use serde_json::json;

use crate::{mapping::set_field_path, resources::ResourceSync, Error, Result};

#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

struct Context {
    client: Client,
}

async fn reconcile(sinker: Arc<ResourceSync>, ctx: Arc<Context>) -> Result<Action> {
    let client = ctx.client.clone();

    let name = sinker.name_any();
    info!(?name, "running reconciler");

    let sinker_ns = sinker.namespace();

    debug!(?sinker.spec, "got");
    let cluster_ref = sinker.spec.source.cluster.as_ref();
    let secret_ref = &cluster_ref.unwrap().kube_config.secret_ref;

    let namespace = cluster_ref
        .and_then(|cluster_ref| cluster_ref.namespace.as_deref())
        .or(sinker_ns.as_deref())
        .ok_or(Error::NamespaceRequired)?;
    let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);

    let sec = secrets.get(&secret_ref.name).await?;

    let kube_config = kube::config::Kubeconfig::from_yaml(
        std::str::from_utf8(&sec.data.unwrap().get(&secret_ref.key).unwrap().0)
            .map_err(Error::KubeconfigUtf8Error)?,
    )?;
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
        annotations,
        labels: resource.metadata.labels,
        ..Default::default()
    };
    debug!(?resource, "patched remote object");

    let local_client = client.clone();
    let (ar, _) = discovery::pinned_kind(&local_client, &target_ref.try_into()?).await?;
    let api: Api<DynamicObject> = Api::namespaced_with(local_client.clone(), namespace, &ar);

    if sinker.spec.mappings.is_empty() {
        let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
        api.patch(&target_ref.name, &ssapply, &Patch::Apply(&resource))
            .await?;
    } else {
        let mut template = DynamicObject::new(&target_ref.name, &ar);
        template.metadata = ObjectMeta {
            name: Some(target_ref.name.clone()),
            namespace: sinker.namespace(),
            ..Default::default()
        };
        template.data = json!({});

        for mapping in &sinker.spec.mappings {
            let subtree = find_field_path(&resource, &mapping.from_field_path)?;
            debug!(?subtree, ?mapping.from_field_path, "from field path");

            let dbg = serde_json::to_string_pretty(&template)?;
            debug!(%dbg, "before");

            debug!(?subtree, ?mapping.to_field_path, "to field path");
            set_field_path(&mut template.data, &mapping.to_field_path, subtree.clone())?;
            let dbg = serde_json::to_string_pretty(&template)?;
            debug!(%dbg, "after");

            let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
            api.patch(&target_ref.name, &ssapply, &Patch::Apply(&template))
                .await?;
        }
    }

    Ok(Action::requeue(Duration::from_secs(5)))
}

fn error_policy(sinker: Arc<ResourceSync>, error: &Error, _ctx: Arc<Context>) -> Action {
    let name = sinker.name_any();
    warn!(?name, %error, "reconcile failed");
    Action::requeue(Duration::from_secs(5 * 60))
}

pub async fn run(client: Client) -> Result<()> {
    apply_crd(client.clone()).await?;

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
        [] => Err(Error::JsonPathNoValues(from_field_path.to_owned())),
        [&ref subtree] => Ok(subtree.to_owned()),
        _ => Err(Error::JsonPathExactlyOneValue(from_field_path.to_owned())),
    }
}

/// Applies the CRD to the k8s cluster.
async fn apply_crd(client: Client) -> Result<()> {
    let crds: Api<CustomResourceDefinition> = Api::all(client.clone());

    let ssapply = PatchParams::apply(&ResourceSync::group(&())).force();
    crds.patch(
        &ResourceSync::crd().metadata.name.unwrap(),
        &ssapply,
        &Patch::Apply(ResourceSync::crd()),
    )
    .await?;

    debug!("CRD applied");
    Ok(())
}
