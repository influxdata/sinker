use std::sync::Arc;
use std::time::Duration;

use futures::{StreamExt, TryStreamExt};
use kube::api::{DynamicObject, WatchParams};
use kube::core::WatchEvent;
use kube::runtime::reflector::ObjectRef;
use kube::Resource;
use tokio::sync::mpsc::Sender;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::error;

use crate::controller::Context;
use crate::filters::Filterable;
use crate::resource_extensions::NamespacedApi;
use crate::resources::{ClusterResourceRef, ResourceSync};
use crate::{Error, Result};

#[derive(Hash, PartialEq, Eq)]
pub struct RemoteWatcherKey {
    pub object: ClusterResourceRef,
    pub resource_sync: ObjectRef<ResourceSync>,
}

pub struct RemoteWatcher {
    key: RemoteWatcherKey,
    sender: Sender<ObjectRef<ResourceSync>>,
    canceled: Arc<RwLock<bool>>,
}

macro_rules! retry_sleep {
    ($($arg:tt)*) =>{
        error!($($arg)*);
        sleep(Duration::from_secs(5)).await;
    };
}

impl RemoteWatcher {
    pub fn new(key: RemoteWatcherKey, sender: Sender<ObjectRef<ResourceSync>>) -> Self {
        Self {
            key,
            sender,
            canceled: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn cancel(&mut self) {
        *self.canceled.write().await = true;
    }

    fn send_reconcile(&self) {
        if let Err(err) = self.sender.blocking_send(self.key.resource_sync.clone()) {
            error!("Error sending reconcile: {}", err);
        }
    }

    pub async fn run(&self, ctx: Arc<Context>) {
        while let Err(err) = self.start(Arc::clone(&ctx)).await {
            if *self.canceled.read().await {
                break;
            }

            self.send_reconcile();
            retry_sleep!("Error starting watch on remote object: {}", err);
        }
    }

    async fn start(&self, ctx: Arc<Context>) -> Result<()> {
        let local_ns = self
            .key
            .resource_sync
            .namespace
            .as_ref()
            .ok_or(Error::NamespaceRequired)?;
        let api = self
            .key
            .object
            .api_for(Arc::clone(&ctx), local_ns.as_str())
            .await?;

        let object_name = &self.key.object.resource_ref.name;
        let object = api.get(object_name).await?;

        let resource_version = object
            .metadata
            .resource_version
            .ok_or(Error::ResourceVersionRequired)?;

        // Send a reconcile once in case something changed before the rv we are watching from
        self.send_reconcile();

        self.watch(&api, object_name, &resource_version).await
    }

    async fn watch(
        &self,
        api: &NamespacedApi,
        object_name: &str,
        resource_version: &str,
    ) -> Result<()> {
        let watch_params = WatchParams::default().fields(&format!("metadata.name={}", object_name));
        let mut resource_version = resource_version.to_string();

        while !*self.canceled.read().await {
            let mut stream = api.watch(&watch_params, &resource_version).await?.boxed();

            while let Some(event) = stream.try_next().await? {
                match event {
                    WatchEvent::Added(obj) => {
                        resource_version = self.send(obj)?;
                    }
                    WatchEvent::Modified(obj) => {
                        resource_version = self.send(obj)?;
                    }
                    WatchEvent::Deleted(obj) => {
                        resource_version = self.send(obj)?;
                    }
                    WatchEvent::Bookmark(bookmark) => {
                        self.send_reconcile();
                        resource_version = bookmark.metadata.resource_version.clone();
                    }
                    WatchEvent::Error(err) => {
                        self.send_reconcile();
                        retry_sleep!("Error watching remote object: {}", err);
                    }
                }
            }
        }

        Ok(())
    }

    fn send(&self, obj: DynamicObject) -> Result<String> {
        obj.was_last_modified_by(&ResourceSync::group(&()))
            .is_some_and(|was| was)
            .then(|| self.send_reconcile());
        obj.metadata
            .resource_version
            .ok_or(Error::ResourceVersionRequired)
    }
}
