use std::collections::HashMap;
use std::sync::Arc;

use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_context::context::{Handle, RefContext};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::error;

use crate::controller::Context;
use crate::remote_watcher::{RemoteWatcher, RemoteWatcherKey};
use crate::resources::ResourceSync;

type ContextAndThreadHandle = (Handle, JoinHandle<()>);
type SyncMap<K, V> = Arc<Mutex<HashMap<K, V>>>;

pub struct RemoteWatcherManager {
    tctx: RefContext,
    watchers: SyncMap<RemoteWatcherKey, ContextAndThreadHandle>,
    sender: UnboundedSender<Result<ObjectRef<ResourceSync>, watcher::Error>>,
}

impl RemoteWatcherManager {
    pub fn new(
        tctx: RefContext,
    ) -> (
        Self,
        UnboundedReceiverStream<Result<ObjectRef<ResourceSync>, watcher::Error>>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let manager = RemoteWatcherManager {
            tctx,
            watchers: Arc::new(Mutex::new(HashMap::new())),
            sender,
        };

        (manager, UnboundedReceiverStream::new(receiver))
    }

    pub async fn add_if_not_exists(&self, key: &RemoteWatcherKey, ctx: Arc<Context>) {
        let mut watchers = self.watchers.lock().await;

        if watchers.get(key).is_some() {
            return;
        }

        let (tctx, handle) = RefContext::with_parent(&self.tctx, None);
        let watcher = RemoteWatcher::new(key.clone(), self.sender.clone(), tctx);

        let join_handle = tokio::spawn(async move { watcher.run(ctx).await });

        watchers.insert(key.clone(), (handle, join_handle));
    }

    pub async fn stop_and_remove_if_exists(&self, key: &RemoteWatcherKey) {
        let mut watchers = self.watchers.lock().await;

        if let Some(handles) = watchers.remove(key) {
            handles.0.cancel();

            if let Err(err) = handles.1.await {
                error!("Error stopping remote watcher: {}", err);
            }
        }
    }
}
