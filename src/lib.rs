#![deny(rustdoc::broken_intra_doc_links, rustdoc::bare_urls, rust_2018_idioms)]

const FINALIZER: &str = "sinker.influxdata.io/target";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Kube Error: {0}")]
    KubeError(#[from] kube::Error),

    #[error("Parsing apiVersion and Kind: {0}")]
    ParseGroupVersionError(#[from] kube::core::gvk::ParseGroupVersionError),

    #[error("Error parsing kubeconfig from secret")]
    KubeconfigError(#[from] kube::config::KubeconfigError),

    #[error("error parsing kubeconfig from secret")]
    KubeconfigUtf8Error(#[source] std::str::Utf8Error),

    #[error("referenced key '{0}' does not exist in secret '{0}' in namespace '{0}'")]
    MissingKeyError(String, String, String),

    #[error("SerializationError: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("JsonPathError: {0}")]
    JsonPathError(#[from] serde_json_path::ParseError),

    #[error("Name is required")]
    NameRequired,

    #[error("UID is required")]
    UIDRequired,

    #[error("Namespace is required")]
    NamespaceRequired,

    #[error("Failed to acquire ResourceVersion")]
    ResourceVersionRequired,

    #[error(transparent)]
    AddToPathError(#[from] mapping::AddToPathError),

    #[error("JSONPath '{0}' produced no values")]
    JsonPathNoValues(String),

    #[error("JSONPath '{0}' didn't produce exactly one value")]
    JsonPathExactlyOneValue(String),

    #[error("Expected k8s resource at subtree: {0}")]
    MalformedInnerResource(String),

    #[error("Mapping block must contain from_field_path, to_field_path or both, cannot be empty")]
    MappingEmpty,

    #[error("Resource {0} of kind {1} not found: {2}")]
    ResourceNotFoundError(String, String, kube::Error),

    #[error("Referenced Kubeconfig Secret cannot be accessed due to namespace restrictions")]
    UnauthorizedKubeconfigAccess(),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Expose all controller components used by main
pub mod controller;

mod filters;
mod mapping;
pub mod metrics;
mod remote_watcher;
mod remote_watcher_manager;
mod resource_extensions;
pub mod resources;
mod util;

#[cfg(test)]
mod test_support {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use http::{Request, Response};
    use kube::client::Body;
    use kube::Client;
    use serde_json::{json, Value};

    use crate::resources::{ClusterResourceRef, ResourceSync, ResourceSyncSpec, GVKN};

    /// A finite, in-memory API script. Unexpected requests fail and are recorded, and
    /// `finish` checks that every scripted response was consumed.
    pub struct MockApi {
        pub client: Client,
        responses: Arc<Mutex<VecDeque<Response<Body>>>>,
        requests: Arc<Mutex<Vec<Request<Value>>>>,
    }

    impl MockApi {
        pub fn new(responses: Vec<Response<Body>>) -> Self {
            let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let service = tower::service_fn({
                let responses = Arc::clone(&responses);
                let requests = Arc::clone(&requests);
                move |request: Request<Body>| {
                    let responses = Arc::clone(&responses);
                    let requests = Arc::clone(&requests);
                    async move {
                        let (parts, body) = request.into_parts();
                        let bytes = body.collect_bytes().await?;
                        let body = if bytes.is_empty() {
                            Value::Null
                        } else {
                            serde_json::from_slice(&bytes).map_err(kube::Error::SerdeError)?
                        };
                        requests
                            .lock()
                            .expect("request lock poisoned")
                            .push(Request::from_parts(parts, body));
                        responses
                            .lock()
                            .expect("response lock poisoned")
                            .pop_front()
                            .ok_or_else(|| kube::Error::Service("unexpected API request".into()))
                    }
                }
            });
            Self {
                client: Client::new(service, "client-default"),
                responses,
                requests,
            }
        }

        pub fn finish(self, expected: &[(&str, &str)]) -> Vec<Request<Value>> {
            assert!(self
                .responses
                .lock()
                .expect("response lock poisoned")
                .is_empty());
            let requests =
                std::mem::take(&mut *self.requests.lock().expect("request lock poisoned"));
            let actual: Vec<_> = requests
                .iter()
                .map(|request| (request.method().as_str(), request.uri().path()))
                .collect();
            assert_eq!(actual, expected);
            requests
        }
    }

    pub fn response(code: u16, body: Value) -> Response<Body> {
        Response::builder()
            .status(code)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&body).expect("serialize response"),
            ))
            .expect("build response")
    }

    pub fn api_error(code: u16) -> Response<Body> {
        response(
            code,
            json!({
                "apiVersion": "v1", "kind": "Status", "status": "Failure",
                "code": code, "reason": "TestFailure", "message": "scripted API failure"
            }),
        )
    }

    pub fn discovery_response(kind: &str, plural: &str, namespaced: bool) -> Response<Body> {
        response(
            200,
            json!({
                "apiVersion": "v1", "kind": "APIResourceList", "groupVersion": "v1",
                "resources": [{"name": plural, "kind": kind, "namespaced": namespaced,
                    "verbs": ["get", "patch", "delete", "watch"]}]
            }),
        )
    }

    pub fn resource_sync() -> ResourceSync {
        let reference = |name: &str| ClusterResourceRef {
            resource_ref: GVKN {
                api_version: "v1".to_string(),
                kind: "ConfigMap".to_string(),
                name: name.to_string(),
            },
            cluster: None,
        };
        let mut sync = ResourceSync::new(
            "copy-config",
            ResourceSyncSpec {
                source: reference("source-config"),
                target: reference("target-config"),
                mappings: vec![],
            },
        );
        sync.metadata.namespace = Some("team-a".to_string());
        sync.metadata.uid = Some("sync-uid".to_string());
        sync.metadata.generation = Some(7);
        sync
    }
}
