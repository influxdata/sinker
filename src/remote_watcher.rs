use std::time::Duration;

use futures::{StreamExt, TryStreamExt};
use kube::api::WatchParams;
use kube::core::WatchEvent;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::utils::Backoff;
use kube::runtime::watcher::DefaultBackoff;
use kube::Resource;
use kubert::client::Client;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::sleep;
use tokio_context::context::Context;
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
    sender: UnboundedSender<ObjectRef<ResourceSync>>,
    client: Client,
}

macro_rules! send_reconcile_on_fail {
    ($self:expr, $backoff:expr, $($arg:tt)*) =>{
        $self.send_reconcile();
        error!($($arg)*);
        sleep($backoff.next().unwrap_or(Duration::from_secs(5))).await;
    };
}

macro_rules! rv_for {
    ($obj:expr) => {
        $obj.metadata
            .resource_version
            .clone()
            .ok_or(Error::ResourceVersionRequired)?
    };
}

// TODO: There may be some other error types that should not be triggering reconciles
// TODO: Could also process mappings to ignore changes to fields that we don't care about, but it's far more complex for a lot less benefit

impl RemoteWatcher {
    pub fn new(
        key: RemoteWatcherKey,
        sender: UnboundedSender<ObjectRef<ResourceSync>>,
        client: Client,
    ) -> Self {
        Self {
            key,
            sender,
            client,
        }
    }

    fn send_reconcile(&self) {
        if let Err(err) = self.sender.send(self.key.resource_sync.clone()) {
            error!("Error sending reconcile: {}", err);
        }
    }

    fn send_reconcile_on_success(&self, backoff: &mut DefaultBackoff) {
        backoff.reset();
        self.send_reconcile();
    }

    pub async fn run(self, mut ctx: Context) {
        let mut backoff = DefaultBackoff::default();

        loop {
            tokio::select! {
                biased;

                _ = ctx.done() => {
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
            "Sending reconcile on start at ResourceVersion {:#?} for object: {:#?}",
            resource_version, self.key
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
        let watch_params = WatchParams::default().fields(&format!("metadata.name={}", object_name));
        let mut resource_version = resource_version.to_string();

        loop {
            resource_version = self
                .listen(api, resource_version, &watch_params, backoff)
                .await?;
        }
    }

    async fn listen(
        &self,
        api: &NamespacedApi,
        mut resource_version: String,
        watch_params: &WatchParams,
        backoff: &mut DefaultBackoff,
    ) -> Result<String> {
        debug!(
            "Started watch at ResourceVersion {:#?} for remote object: {:#?}",
            resource_version, self.key
        );

        let mut stream = api.watch(watch_params, &resource_version).await?.boxed();

        while let Some(event) = stream.try_next().await? {
            resource_version = match event {
                WatchEvent::Deleted(obj) => {
                    let event_rv = rv_for!(obj);

                    debug!("Sending reconcile on watch event at ResourceVersion {:#?} for deleted object: {:#?}", event_rv, self.key);
                    self.send_reconcile_on_success(backoff);

                    event_rv
                }
                WatchEvent::Added(obj) | WatchEvent::Modified(obj) => {
                    let event_rv = rv_for!(obj);

                    match obj.was_last_modified_by(&ResourceSync::group(&())) {
                        None => {
                            debug!("Sending reconcile on watch event at ResourceVersion {:#?} because it is impossible to determine if the object was last modified by us for object: {:#?}", event_rv, self.key);
                            self.send_reconcile_on_success(backoff);
                        }
                        Some(was_last_modified_by_us) if !was_last_modified_by_us => {
                            debug!("Sending reconcile on watch event at ResourceVersion {:#?} for externally modified object: {:#?}", event_rv, self.key);
                            self.send_reconcile_on_success(backoff);
                        }
                        _ => {
                            debug!("Ignoring watch event at ResourceVersion {:#?} for object modified by us: {:#?}", event_rv, self.key);
                        }
                    }

                    event_rv
                }
                WatchEvent::Bookmark(bookmark) => {
                    let bookmark_rv = bookmark.metadata.resource_version.clone();

                    debug!(
                        "Bookmark event received at ResourceVersion {:#?} for object: {:#?}",
                        bookmark_rv, self.key
                    );
                    backoff.reset();

                    bookmark_rv
                }
                WatchEvent::Error(err) if err.code == 410 => {
                    debug!("ResourceVersion {:#?} is expired, so we restart from the beginning for object: {:#?}", resource_version, self.key);
                    "0".to_string()
                }
                WatchEvent::Error(err) => {
                    send_reconcile_on_fail!(self, backoff, "Error watching remote object: {}", err);

                    resource_version
                }
            }
        }

        Ok(resource_version)
    }
}
