use std::time::Duration;

use backoff::backoff::Backoff;
use futures::{StreamExt, TryStreamExt};
use kube::api::WatchParams;
use kube::core::WatchEvent;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher;
use kube::runtime::watcher::DefaultBackoff;
use kube::Resource;
use kubert::client::Client;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::sleep;
use tokio_context::context::{Context, RefContext};
use tracing::{debug, error};

use crate::filters::Filterable;
use crate::resource_extensions::NamespacedApi;
use crate::resources::{ClusterResourceRef, ResourceSync};
use crate::{Error, Result};

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct RemoteWatcherKey {
    pub object: ClusterResourceRef,
    pub resource_sync: ObjectRef<ResourceSync>,
}

pub struct RemoteWatcher {
    key: RemoteWatcherKey,
    sender: UnboundedSender<Result<ObjectRef<ResourceSync>, watcher::Error>>,
    ctx: RefContext,
    client: Client,
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
    pub fn new(
        key: RemoteWatcherKey,
        sender: UnboundedSender<Result<ObjectRef<ResourceSync>, watcher::Error>>,
        ctx: RefContext,
        client: Client,
    ) -> Self {
        Self {
            key,
            sender,
            ctx,
            client,
        }
    }

    fn send_reconcile(&self) {
        if let Err(err) = self.sender.send(Ok(self.key.resource_sync.clone())) {
            error!("Error sending reconcile: {}", err);
        }
    }

    fn send_reconcile_on_success(&self, backoff: &mut DefaultBackoff) {
        backoff.reset();
        self.send_reconcile();
    }

    pub async fn run(&self) {
        let mut backoff = DefaultBackoff::default();

        let (mut ctx, handle) = Context::with_parent(&self.ctx, None);

        loop {
            tokio::select! {
                _ = ctx.done() => {
                    handle.cancel();
                    return;
                },
                Err(err) = self.start(&mut backoff) => {
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

    async fn start(&self, backoff: &mut DefaultBackoff) -> Result<()> {
        let local_ns = self
            .key
            .resource_sync
            .namespace
            .as_ref()
            .ok_or(Error::NamespaceRequired)?;
        let api = self
            .key
            .object
            .api_for(self.client.clone(), local_ns.as_str())
            .await?;

        let object_name = &self.key.object.resource_ref.name;
        let object = api.get(object_name).await?;

        let resource_version = object
            .metadata
            .resource_version
            .ok_or(Error::ResourceVersionRequired)?;

        // Send a reconcile once in case something changed before the rv we are watching from
        debug!(
            "Sending reconcile on start for object: {:#?} at ResourceVersion: {:#?}",
            self.key, resource_version
        );
        self.send_reconcile_on_success(backoff);

        self.watch(&api, object_name, &resource_version, backoff)
            .await
    }

    async fn watch(
        &self,
        api: &NamespacedApi,
        object_name: &str,
        resource_version: &str,
        backoff: &mut DefaultBackoff,
    ) -> Result<()> {
        let (mut ctx, handle) = Context::with_parent(&self.ctx, None);

        let watch_params = WatchParams::default().fields(&format!("metadata.name={}", object_name));
        let mut resource_version = resource_version.to_string();

        loop {
            tokio::select! {
                _ = ctx.done() => {
                    handle.cancel();
                    return Ok(());
                },
                stream = api.watch(&watch_params, &resource_version) => {
                    let mut stream = stream?.boxed();

                    while let Some(event) = stream.try_next().await? {
                        resource_version = match event {
                            WatchEvent::Added(obj) | WatchEvent::Modified(obj) | WatchEvent::Deleted(obj)  => {
                                match obj.was_last_modified_by(&ResourceSync::group(&())) {
                                    None => {
                                        debug!("Sending reconcile on watch event because it is impossible to determine if the object was last modified by us: {:#?}", self.key);
                                        self.send_reconcile_on_success(backoff);
                                    }
                                    Some(was_last_modified_by_us) if !was_last_modified_by_us => {
                                        debug!("Sending reconcile on watch event for externally modified object: {:#?}", self.key);
                                        self.send_reconcile_on_success(backoff);
                                    }
                                    _ => {
                                        debug!("Ignoring watch event for object modified by us: {:#?}", self.key);
                                    }
                                }

                                obj.metadata.resource_version.clone().ok_or(Error::ResourceVersionRequired)?
                            }
                            WatchEvent::Bookmark(bookmark) => {
                                let bookmark_rv = bookmark.metadata.resource_version.clone();

                                debug!("Bookmark event received for {:#?} at ResourceVersion {:#?}", self.key, bookmark_rv);
                                backoff.reset();

                                bookmark_rv
                            }
                            WatchEvent::Error(err) if err.code == 410 => {
                                debug!("ResourceVersion {:#?} is expired, so we restart from the beginning for Object {:#?}", resource_version, self.key);
                                "0".to_string()
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
