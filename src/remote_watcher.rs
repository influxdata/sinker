use std::sync::Arc;
use std::time::Duration;

use backoff::backoff::Backoff;
use futures::{StreamExt, TryStreamExt};
use kube::api::WatchParams;
use kube::core::WatchEvent;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher::DefaultBackoff;
use kube::Resource;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio_context::context::Context as TokioContext;
use tokio_context::context::RefContext;
use tracing::{debug, error};

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
}

macro_rules! send_reconcile_on_fail {
    ($self:expr, $backoff:expr, $($arg:tt)*) =>{
        $self.send_reconcile();
        error!($($arg)*);
        sleep($backoff.next_backoff().unwrap_or(Duration::from_secs(5))).await;
    };
}

// TODO: May not want to trigger reconcile on all errors or bookmarks

impl RemoteWatcher {
    pub fn new(key: RemoteWatcherKey, sender: Sender<ObjectRef<ResourceSync>>) -> Self {
        Self { key, sender }
    }

    fn send_reconcile(&self) {
        if let Err(err) = self.sender.blocking_send(self.key.resource_sync.clone()) {
            error!("Error sending reconcile: {}", err);
        }
    }

    fn send_reconcile_on_success(&self, backoff: &mut DefaultBackoff) {
        backoff.reset();
        self.send_reconcile();
    }

    pub async fn run(&self, ctx: Arc<Context>, tctx: &RefContext) {
        let mut backoff = DefaultBackoff::default();

        let (mut tctx_local, handle) = TokioContext::with_parent(tctx, None);

        loop {
            tokio::select! {
                _ = tctx_local.done() => {
                    handle.cancel();
                    return;
                },
                Err(err) = self.start(Arc::clone(&ctx), tctx, &mut backoff) => {
                    send_reconcile_on_fail!(
                        self,
                        &mut backoff,
                        "Error starting watch on remote object: {}",
                        err
                    );
                }
            }
        }
    }

    async fn start(
        &self,
        ctx: Arc<Context>,
        tctx: &RefContext,
        backoff: &mut DefaultBackoff,
    ) -> Result<()> {
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
        debug!("Sending reconcile on start");
        self.send_reconcile_on_success(backoff);

        self.watch(&api, object_name, &resource_version, tctx, backoff)
            .await
    }

    async fn watch(
        &self,
        api: &NamespacedApi,
        object_name: &str,
        resource_version: &str,
        tctx: &RefContext,
        backoff: &mut DefaultBackoff,
    ) -> Result<()> {
        let (mut tctx, handle) = TokioContext::with_parent(tctx, None);

        let watch_params = WatchParams::default().fields(&format!("metadata.name={}", object_name));
        let mut resource_version = resource_version.to_string();

        loop {
            tokio::select! {
                _ = tctx.done() => {
                    handle.cancel();
                    return Ok(());
                },
                stream = api.watch(&watch_params, &resource_version) => {
                    let mut stream = stream?.boxed();

                    while let Some(event) = stream.try_next().await? {
                        resource_version = match event {
                            WatchEvent::Added(obj) | WatchEvent::Modified(obj) | WatchEvent::Deleted(obj)  => {
                                obj.was_last_modified_by(&ResourceSync::group(&())).is_some_and(|was| was).then(|| {
                                    debug!("Sending reconcile on watch event for externally modified object");
                                    self.send_reconcile_on_success(backoff);
                                });

                                obj.metadata.resource_version.clone().ok_or(Error::ResourceVersionRequired)?
                            }
                            WatchEvent::Bookmark(bookmark) => {
                                debug!("Sending reconcile on watch bookmark");
                                self.send_reconcile_on_success(backoff);

                                bookmark.metadata.resource_version.clone()
                            }
                            WatchEvent::Error(err) => {
                                send_reconcile_on_fail!(
                                    self,
                                    backoff,
                                    "Error watching remote object: {}",
                                    err
                                );

                                resource_version
                            }
                        }
                    }
                }
            }
        }
    }
}