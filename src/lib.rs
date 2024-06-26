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

    #[error("SerializationError: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("JsonPathError: {0}")]
    JsonPathError(#[from] serde_json_path::ParseError),

    #[error("Namespace is required")]
    NamespaceRequired,

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
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Expose all controller components used by main
pub mod controller;

mod mapping;
pub mod resources;
