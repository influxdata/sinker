use std::ops::Deref;
use std::sync::Arc;

use crate::controller::Context;
use crate::remote_watcher::RemoteWatcherKey;
use crate::resources::{
    ClusterRef, ClusterResourceRef, ResourceSync, ALLOWED_NAMESPACES_ANNOTATION,
};
use crate::Error::UnauthorizedKubeconfigAccess;
use crate::{Error, FINALIZER};
use k8s_openapi::api::core::v1::Secret;
use kube::api::{ApiResource, DynamicObject};
use kube::discovery::Scope::*;
use kube::runtime::reflector::ObjectRef;
use kube::{discovery, Api, Client, Config, ResourceExt};
use regex::Regex;
use tracing::debug;

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
            .is_some_and(|f| f.contains(&FINALIZER.to_string()))
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
    let client =
        match cluster_ref {
            None => client,
            Some(cluster_ref) => {
                let secret_ns = cluster_ref
                    .kube_config
                    .secret_ref
                    .namespace
                    .as_deref()
                    .unwrap_or(local_ns);
                let secrets: Api<Secret> = Api::namespaced(client, secret_ns);
                let secret_ref = &cluster_ref.kube_config.secret_ref;
                let sec = secrets.get(&secret_ref.name).await.map_err(|e| {
                    match secret_ns == local_ns {
                        true => crate::Error::from(e),
                        false => {
                            debug!(
                                "error accessing kubeconfig secret in remote namespace: {}",
                                e
                            );
                            UnauthorizedKubeconfigAccess()
                        }
                    }
                })?;

                if secret_ns != local_ns {
                    verify_kubeconfig_secret_access(local_ns, &sec)?;
                }

                let kube_config = kube::config::Kubeconfig::from_yaml(
                    std::str::from_utf8(
                        &sec.data
                            .unwrap()
                            .get(&secret_ref.key)
                            .ok_or_else(|| {
                                Error::MissingKeyError(
                                    secret_ref.key.clone(),
                                    secret_ref.name.clone(),
                                    secret_ns.to_string(),
                                )
                            })?
                            .0,
                    )
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

fn verify_kubeconfig_secret_access(local_ns: &str, sec: &Secret) -> crate::Result<()> {
    let allowed_namespaces = sec
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(ALLOWED_NAMESPACES_ANNOTATION))
        .ok_or(UnauthorizedKubeconfigAccess())?;
    let re = Regex::new(allowed_namespaces).map_err(|e| {
        debug!("invalid regex in allowed namespaces annotation: {}", e);
        UnauthorizedKubeconfigAccess()
    })?;
    match re.is_match(local_ns) {
        true => Ok(()),
        false => Err(UnauthorizedKubeconfigAccess()),
    }
}

async fn api_for(
    cluster_resource_ref: &ClusterResourceRef,
    local_ns: &str,
    client: Client,
) -> crate::Result<NamespacedApi> {
    let cluster_ref = cluster_resource_ref.cluster.as_ref();
    let client = cluster_client(cluster_ref, local_ns, client).await?;

    let resource_ref = &cluster_resource_ref.resource_ref;
    let (ar, capabilities) = discovery::pinned_kind(&client, &resource_ref.try_into()?).await?;

    let namespace = match capabilities.scope {
        Cluster => None,
        Namespaced => match cluster_ref {
            None => Some(local_ns.to_owned()),
            Some(cluster) => cluster.namespace.to_owned(),
        },
    };

    let api = match capabilities.scope {
        Cluster => Api::all_with(client, &ar),
        Namespaced => match &namespace {
            None => Api::default_namespaced_with(client, &ar),
            Some(ns) => Api::namespaced_with(client, ns, &ar),
        },
    };

    Ok(NamespacedApi { api, ar, namespace })
}

impl ClusterResourceRef {
    pub async fn api_for(&self, client: Client, local_ns: &str) -> crate::Result<NamespacedApi> {
        api_for(self, local_ns, client).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{api_error, discovery_response, resource_sync, response, MockApi};
    use futures::future::join_all;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use rand::{distr::Alphanumeric, rngs::StdRng, Rng, SeedableRng};
    use rstest::rstest;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[rstest]
    #[case::absent(None, false)]
    #[case::empty(Some(vec![]), false)]
    #[case::unrelated(Some(vec!["example.com/other"]), false)]
    #[case::sinker(Some(vec!["example.com/other", FINALIZER]), true)]
    fn finalizers_are_detected_and_cloned(
        #[case] finalizers: Option<Vec<&str>>,
        #[case] expected: bool,
    ) {
        let mut sync = resource_sync();
        sync.metadata.finalizers =
            finalizers.map(|values| values.into_iter().map(String::from).collect());
        assert_eq!(sync.has_target_finalizer(), expected);
        let original = sync.metadata.finalizers.clone();
        let mut cloned = sync.finalizers_clone_or_empty();
        assert_eq!(cloned, original.clone().unwrap_or_default());
        cloned.push("new/finalizer".into());
        assert_eq!(sync.metadata.finalizers, original);
    }

    #[rstest]
    #[case::unanchored("team", "prefix-team-suffix", true)]
    #[case::anchored("^team$", "prefix-team-suffix", false)]
    #[case::empty_pattern("", "team-a", true)]
    #[case::alternatives("^(team-a|team-b)$", "team-b", true)]
    fn namespace_annotation_uses_regex_matching(
        #[case] pattern: &str,
        #[case] namespace: &str,
        #[case] allowed: bool,
    ) {
        let result =
            verify_kubeconfig_secret_access(namespace, &secret_with_annotation(Some(pattern)));
        if allowed {
            result.expect("matching namespace authorized");
        } else {
            assert!(matches!(
                result.expect_err("nonmatching namespace denied"),
                UnauthorizedKubeconfigAccess()
            ));
        }
    }

    #[tokio::test]
    async fn local_client_and_resource_sync_api_do_not_require_discovery() {
        let mock = MockApi::new(vec![]);
        let client = cluster_client(None, "team-a", mock.client.clone())
            .await
            .expect("local client");
        assert_eq!(client.default_namespace(), "client-default");
        let mut sync = resource_sync();
        assert_eq!(
            sync.api(client.clone()).resource_url(),
            "/apis/sinker.influxdata.io/v1alpha1/namespaces/team-a/resourcesyncs"
        );
        sync.metadata.namespace = None;
        assert_eq!(
            sync.api(client).resource_url(),
            "/apis/sinker.influxdata.io/v1alpha1/resourcesyncs"
        );
        mock.finish(&[]);
    }

    #[rstest]
    #[case::namespaced(
        "ConfigMap",
        "configmaps",
        true,
        Some("team-a"),
        "/api/v1/namespaces/team-a/configmaps"
    )]
    #[case::cluster_scoped("Namespace", "namespaces", false, None, "/api/v1/namespaces")]
    #[tokio::test]
    async fn local_api_uses_discovered_scope(
        #[case] kind: &str,
        #[case] plural: &str,
        #[case] namespaced: bool,
        #[case] namespace: Option<&str>,
        #[case] url: &str,
    ) {
        let mock = MockApi::new(vec![discovery_response(kind, plural, namespaced)]);
        let mut reference = resource_sync().spec.source;
        reference.resource_ref.kind = kind.into();
        let api = reference
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("discover local API");
        assert_eq!(api.namespace.as_deref(), namespace);
        assert_eq!(api.ar.kind, kind);
        assert_eq!(api.resource_url(), url);
        mock.finish(&[("GET", "/api/v1")]);
    }

    fn cluster_ref(namespace: Option<&str>) -> ClusterRef {
        use crate::resources::{KubeConfig, SecretRef};
        ClusterRef {
            namespace: Some("workloads".into()),
            kube_config: KubeConfig {
                secret_ref: SecretRef {
                    name: "credentials".into(),
                    namespace: namespace.map(String::from),
                    key: "config".into(),
                },
            },
        }
    }

    #[rstest]
    #[case::implicit_local(None, "/api/v1/namespaces/team-a/secrets/credentials", false)]
    #[case::explicit_local(Some("team-a"), "/api/v1/namespaces/team-a/secrets/credentials", false)]
    #[case::cross_namespace(
        Some("credential-store"),
        "/api/v1/namespaces/credential-store/secrets/credentials",
        true
    )]
    #[tokio::test]
    async fn secret_read_errors_hide_cross_namespace_details(
        #[case] namespace: Option<&str>,
        #[case] path: &str,
        #[case] unauthorized: bool,
    ) {
        let mock = MockApi::new(vec![api_error(403)]);
        let error = cluster_client(Some(&cluster_ref(namespace)), "team-a", mock.client.clone())
            .await
            .map(|_| ())
            .expect_err("secret read fails");
        if unauthorized {
            assert!(matches!(error, UnauthorizedKubeconfigAccess()));
        } else {
            assert!(
                matches!(error, Error::KubeError(kube::Error::Api(error)) if error.code == 403)
            );
        }
        mock.finish(&[("GET", path)]);
    }

    #[rstest]
    #[case::same_namespace_without_annotation(None, None, true)]
    #[case::same_namespace_ignores_denial(None, Some("^denied$"), true)]
    #[case::cross_namespace_missing_annotation(Some("credential-store"), None, false)]
    #[case::cross_namespace_denied(Some("credential-store"), Some("^denied$"), false)]
    #[case::cross_namespace_allowed(Some("credential-store"), Some("^team-a$"), true)]
    #[tokio::test]
    async fn secret_authorization_precedes_key_lookup(
        #[case] namespace: Option<&str>,
        #[case] annotation: Option<&str>,
        #[case] allowed: bool,
    ) {
        let mut secret = secret_with_annotation(annotation);
        secret.data = Some(BTreeMap::new());
        let mock = MockApi::new(vec![response(200, json!(secret))]);
        let error = cluster_client(Some(&cluster_ref(namespace)), "team-a", mock.client.clone())
            .await
            .map(|_| ())
            .expect_err("missing key or authorization");
        if allowed {
            assert!(matches!(error, Error::MissingKeyError(key, name, ns)
                if key == "config" && name == "credentials" && ns == namespace.unwrap_or("team-a")));
        } else {
            assert!(matches!(error, UnauthorizedKubeconfigAccess()));
        }
        let path = format!(
            "/api/v1/namespaces/{}/secrets/credentials",
            namespace.unwrap_or("team-a")
        );
        mock.finish(&[("GET", &path)]);
    }

    #[rstest]
    #[case::invalid_utf8(vec![0xff], "utf8")]
    #[case::invalid_yaml(b"[unterminated".to_vec(), "yaml")]
    #[case::missing_context(b"apiVersion: v1\nkind: Config\nclusters: []\ncontexts: []\nusers: []\n".to_vec(), "context")]
    #[tokio::test]
    async fn invalid_kubeconfigs_return_typed_errors(
        #[case] bytes: Vec<u8>,
        #[case] failure: &str,
    ) {
        let secret = Secret {
            data: Some(BTreeMap::from([(
                "config".into(),
                k8s_openapi::ByteString(bytes),
            )])),
            ..Default::default()
        };
        let mock = MockApi::new(vec![response(200, json!(secret))]);
        let error = cluster_client(Some(&cluster_ref(None)), "team-a", mock.client.clone())
            .await
            .map(|_| ())
            .expect_err("invalid kubeconfig");
        match failure {
            "utf8" => assert!(
                matches!(error, Error::KubeconfigUtf8Error(error) if error.valid_up_to() == 0)
            ),
            "yaml" | "context" => assert!(matches!(error, Error::KubeconfigError(_))),
            _ => unreachable!(),
        }
        mock.finish(&[("GET", "/api/v1/namespaces/team-a/secrets/credentials")]);
    }

    #[test]
    fn watcher_keys_distinguish_owners_and_resource_connections() {
        let sync = resource_sync();
        let reference = sync.spec.source.clone();
        let key = reference.remote_watcher_key(&sync);
        assert_eq!(key.object, reference);
        assert_eq!(key.resource_sync.name, "copy-config");
        assert_eq!(key.resource_sync.namespace.as_deref(), Some("team-a"));
        let mut other_sync = sync.clone();
        other_sync.metadata.namespace = Some("team-b".into());
        assert_ne!(key, reference.remote_watcher_key(&other_sync));
        other_sync = sync.clone();
        other_sync.metadata.name = Some("other-sync".into());
        assert_ne!(key, reference.remote_watcher_key(&other_sync));
        let mut remote = reference.clone();
        remote.cluster = Some(cluster_ref(None));
        assert_ne!(key, remote.remote_watcher_key(&sync));
        assert_ne!(key, sync.spec.target.remote_watcher_key(&sync));
    }

    fn secret_with_annotation(value: Option<&str>) -> Secret {
        Secret {
            metadata: ObjectMeta {
                annotations: value.map(|v| {
                    BTreeMap::from([(ALLOWED_NAMESPACES_ANNOTATION.to_string(), v.to_string())])
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[rstest]
    fn errs_when_annotation_missing() {
        let sec = secret_with_annotation(None);

        let res = verify_kubeconfig_secret_access("dev", &sec);

        assert!(matches!(res, Err(UnauthorizedKubeconfigAccess())));
    }

    #[rstest]
    fn errs_when_annotation_key_absent() {
        let sec = Secret {
            metadata: ObjectMeta {
                annotations: Some(BTreeMap::from([(
                    "other-annotation".to_string(),
                    "^.*$".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };

        let res = verify_kubeconfig_secret_access("ns1", &sec);

        assert!(matches!(res, Err(UnauthorizedKubeconfigAccess())));
    }

    #[rstest]
    #[case::unbalanced_paren("(")]
    #[case::dangling_escape("\\")]
    fn errs_on_invalid_regex(#[case] pattern: &str) {
        let sec = secret_with_annotation(Some(pattern));

        let res = verify_kubeconfig_secret_access("random", &sec);

        assert!(matches!(res, Err(UnauthorizedKubeconfigAccess())));
    }

    #[rstest]
    fn allows_matching_namespace_randomized() {
        let pattern = "^ns-[a-z0-9]{4}$";
        let sec = secret_with_annotation(Some(pattern));
        let mut rng = StdRng::seed_from_u64(42);

        for _ in 0..8 {
            let tail: String = (0..4)
                .map(|_| rng.sample(Alphanumeric) as char)
                .map(|c| c.to_ascii_lowercase())
                .collect();
            let ns = format!("ns-{}", tail);

            let res = verify_kubeconfig_secret_access(&ns, &sec);

            assert!(res.is_ok(), "{} should match {}", ns, pattern);
        }
    }

    #[rstest]
    fn denies_non_matching_namespace_randomized() {
        let pattern = "^team-[0-9]{2}$";
        let sec = secret_with_annotation(Some(pattern));
        let mut rng = StdRng::seed_from_u64(84);

        for _ in 0..6 {
            let ns = format!("proj-{}", rng.random_range(10_u8..99_u8));

            let res = verify_kubeconfig_secret_access(&ns, &sec);

            assert!(matches!(res, Err(UnauthorizedKubeconfigAccess())));
        }
    }

    #[tokio::test]
    async fn concurrent_checks_are_independent() {
        let allow_secret = secret_with_annotation(Some("^ok-[a-z]{2}$"));
        let deny_secret = secret_with_annotation(Some("^deny$"));
        let mut rng = StdRng::seed_from_u64(7);

        let inputs: Vec<_> = (0..6)
            .map(|i| {
                let expect_ok = i % 2 == 0;
                let sec = if expect_ok {
                    allow_secret.clone()
                } else {
                    deny_secret.clone()
                };
                let ns = if expect_ok {
                    let part: String = (0..2)
                        .map(|_| char::from(rng.random_range(b'a'..=b'z')))
                        .collect();
                    format!("ok-{}", part)
                } else {
                    format!("bad-{}", rng.random::<u8>())
                };
                (ns, expect_ok, sec)
            })
            .collect();

        let handles: Vec<_> = inputs
            .into_iter()
            .map(|(ns, expect_ok, sec)| {
                tokio::spawn(async move {
                    let res = verify_kubeconfig_secret_access(&ns, &sec);
                    (ns, expect_ok, res)
                })
            })
            .collect();

        let outcomes = join_all(handles).await;

        for outcome in outcomes {
            let (ns, expect_ok, res) = outcome.expect("task panicked");
            if expect_ok {
                assert!(
                    res.is_ok(),
                    "seed 7: namespace {ns} should be authorized: {res:?}"
                );
            } else {
                assert!(matches!(res, Err(UnauthorizedKubeconfigAccess())));
            }
        }
    }
}
