use std::collections::HashMap;
use std::sync::Arc;

use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher;
use kubert::client::Client;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_context::context::{Context, Handle};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error};

use crate::remote_watcher::{RemoteWatcher, RemoteWatcherKey};
use crate::resources::ResourceSync;

type ContextAndThreadHandle = (Handle, JoinHandle<()>);
type SyncMap<K, V> = Arc<Mutex<HashMap<K, V>>>;

pub struct RemoteWatcherManager {
    watchers: SyncMap<RemoteWatcherKey, ContextAndThreadHandle>,
    sender: UnboundedSender<Result<ObjectRef<ResourceSync>, watcher::Error>>,
    client: Client,
}

impl RemoteWatcherManager {
    pub fn new(
        client: Client,
    ) -> (
        Self,
        UnboundedReceiverStream<Result<ObjectRef<ResourceSync>, watcher::Error>>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let manager = RemoteWatcherManager {
            watchers: Arc::new(Mutex::new(HashMap::new())),
            sender,
            client,
        };

        (manager, UnboundedReceiverStream::new(receiver))
    }

    pub async fn add_if_not_exists(&self, key: &RemoteWatcherKey) {
        let mut watchers = self.watchers.lock().await;

        if watchers.get(key).is_some() {
            return;
        }

        debug!("Starting remote watcher for: {:#?}", key);

        let (ctx, handle) = Context::new();
        let watcher = RemoteWatcher::new(key.clone(), self.sender.clone(), self.client.clone());

        let join_handle = tokio::spawn(watcher.run(ctx));

        watchers.insert(key.clone(), (handle, join_handle));
    }

    pub async fn stop_and_remove_if_exists(&self, key: &RemoteWatcherKey) {
        let mut watchers = self.watchers.lock().await;

        if let Some(handles) = watchers.remove(key) {
            debug!("Stopping remote watcher for: {:#?}", key);

            handles.0.cancel();

            if let Err(err) = handles.1.await {
                error!("Error stopping remote watcher: {}", err);
            } else {
                debug!("Remote watcher stopped for: {:#?}", key);
            }
        }
    }

    pub async fn stop_all(&self) {
        let keys = {
            let watchers = self.watchers.lock().await;

            watchers.keys().cloned().collect::<Vec<_>>()
        };

        for key in keys {
            self.stop_and_remove_if_exists(&key).await;
        }
    }
}
