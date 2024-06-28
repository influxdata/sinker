use std::sync::Arc;

use kube::{Api, ResourceExt};

use crate::resources::ResourceSync;
use crate::FINALIZER;

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

    pub fn api(&self, ctx: Arc<crate::controller::Context>) -> Api<Self> {
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
