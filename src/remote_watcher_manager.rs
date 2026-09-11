use std::collections::HashMap;
use std::sync::Arc;

use kube::runtime::reflector::ObjectRef;
use kube::Client;
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
    sender: UnboundedSender<ObjectRef<ResourceSync>>,
    client: Client,
}

macro_rules! stop_and_remove_if_exists {
    ($watchers:expr, $key:expr) => {
        if let Some(handles) = $watchers.remove($key) {
            debug!("Stopping remote watcher for: {:#?}", $key);

            handles.0.cancel();

            if let Err(err) = handles.1.await {
                error!("Error stopping remote watcher: {}", err);
            } else {
                debug!("Remote watcher stopped for: {:#?}", $key);
            }
        }
    };
}

impl RemoteWatcherManager {
    pub fn new(client: Client) -> (Self, UnboundedReceiverStream<ObjectRef<ResourceSync>>) {
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

        stop_and_remove_if_exists!(watchers, key);
    }

    pub async fn stop_all(&self) {
        let mut watchers = self.watchers.lock().await;
        let keys = watchers.keys().cloned().collect::<Vec<_>>();

        for key in keys {
            stop_and_remove_if_exists!(watchers, &key);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_support::{resource_sync, MockApi};
    use std::time::Duration;
    use tokio::time::timeout;

    // Keep controller request tests independent of background discovery while still
    // exercising the manager's existing-watch and cancellation paths.
    pub async fn park_watchers(
        manager: &RemoteWatcherManager,
        sync: &ResourceSync,
    ) -> mpsc::UnboundedReceiver<RemoteWatcherKey> {
        let (sender, receiver) = mpsc::unbounded_channel();
        for object in [&sync.spec.source, &sync.spec.target] {
            let key = RemoteWatcherKey {
                object: object.clone(),
                resource_sync: ObjectRef::from_obj(sync),
            };
            let (mut ctx, handle) = Context::new();
            let sender = sender.clone();
            let task_key = key.clone();
            let task = tokio::spawn(async move {
                ctx.done().await;
                sender
                    .send(task_key)
                    .expect("cancellation receiver remains open");
            });
            assert!(manager
                .watchers
                .lock()
                .await
                .insert(key, (handle, task))
                .is_none());
        }
        receiver
    }

    #[tokio::test]
    async fn removal_cancels_and_joins_only_the_requested_watcher() {
        let mock = MockApi::new(vec![]);
        let (manager, _events) = RemoteWatcherManager::new(mock.client.clone());
        let sync = resource_sync();
        let mut cancelled = park_watchers(&manager, &sync).await;
        let source = RemoteWatcherKey {
            object: sync.spec.source.clone(),
            resource_sync: ObjectRef::from_obj(&sync),
        };
        let target = RemoteWatcherKey {
            object: sync.spec.target.clone(),
            resource_sync: ObjectRef::from_obj(&sync),
        };
        // A duplicate addition must keep the existing task, without starting API discovery.
        manager.add_if_not_exists(&source).await;
        timeout(
            Duration::from_secs(2),
            manager.stop_and_remove_if_exists(&source),
        )
        .await
        .expect("stop source");
        assert_eq!(
            cancelled
                .try_recv()
                .expect("source joined after cancellation"),
            source
        );
        assert!(matches!(
            cancelled.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        {
            let watchers = manager.watchers.lock().await;
            assert_eq!(watchers.len(), 1);
            assert!(watchers.contains_key(&target));
        }
        manager.stop_and_remove_if_exists(&source).await;
        timeout(Duration::from_secs(2), manager.stop_all())
            .await
            .expect("stop remaining watcher");
        assert_eq!(
            cancelled
                .try_recv()
                .expect("target joined after cancellation"),
            target
        );
        assert!(manager.watchers.lock().await.is_empty());
        assert!(matches!(
            cancelled.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        manager.stop_all().await;
        mock.finish(&[]);
    }

    #[tokio::test]
    async fn stop_all_cancels_and_joins_every_owner_of_the_same_resource() {
        let mock = MockApi::new(vec![]);
        let (manager, _events) = RemoteWatcherManager::new(mock.client.clone());
        let first = resource_sync();
        let mut second = first.clone();
        second.metadata.name = Some("other-sync".into());
        let mut first_cancelled = park_watchers(&manager, &first).await;
        let mut second_cancelled = park_watchers(&manager, &second).await;
        assert_eq!(manager.watchers.lock().await.len(), 4);
        timeout(Duration::from_secs(2), manager.stop_all())
            .await
            .expect("stop all watchers");
        assert!(manager.watchers.lock().await.is_empty());
        for receiver in [&mut first_cancelled, &mut second_cancelled] {
            let keys = [
                receiver.try_recv().expect("first cancellation"),
                receiver.try_recv().expect("second cancellation"),
            ];
            assert_ne!(keys[0].object, keys[1].object);
            assert!(matches!(
                receiver.try_recv(),
                Err(mpsc::error::TryRecvError::Disconnected)
            ));
        }
        mock.finish(&[]);
    }

    #[tokio::test]
    async fn new_watcher_can_be_cancelled_before_it_starts_network_work() {
        let mock = MockApi::new(vec![]);
        let (manager, _events) = RemoteWatcherManager::new(mock.client.clone());
        let sync = resource_sync();
        let key = RemoteWatcherKey {
            object: sync.spec.source.clone(),
            resource_sync: ObjectRef::from_obj(&sync),
        };
        manager.add_if_not_exists(&key).await;
        manager.add_if_not_exists(&key).await;
        assert_eq!(manager.watchers.lock().await.len(), 1);
        timeout(
            Duration::from_secs(2),
            manager.stop_and_remove_if_exists(&key),
        )
        .await
        .expect("cancel new watcher");
        assert!(manager.watchers.lock().await.is_empty());
        mock.finish(&[]);
    }

    #[tokio::test]
    async fn cleanup_removes_aborted_tasks_and_continues() {
        let mock = MockApi::new(vec![]);
        let (manager, _events) = RemoteWatcherManager::new(mock.client.clone());
        let sync = resource_sync();
        let mut cancelled = park_watchers(&manager, &sync).await;
        let source = RemoteWatcherKey {
            object: sync.spec.source.clone(),
            resource_sync: ObjectRef::from_obj(&sync),
        };
        manager
            .watchers
            .lock()
            .await
            .get(&source)
            .expect("source task")
            .1
            .abort();
        timeout(Duration::from_secs(2), manager.stop_all())
            .await
            .expect("join aborted and live tasks");
        assert!(manager.watchers.lock().await.is_empty());
        assert_eq!(
            cancelled.try_recv().expect("live target cancelled").object,
            sync.spec.target
        );
        assert!(matches!(
            cancelled.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        mock.finish(&[]);
    }
}
