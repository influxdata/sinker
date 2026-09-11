use std::time::Duration;

use futures::{StreamExt, TryStreamExt};
use kube::api::WatchParams;
use kube::core::WatchEvent;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::utils::Backoff;
use kube::runtime::watcher::DefaultBackoff;
use kube::Client;
use kube::Resource;
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

    #[expect(
        clippy::result_large_err,
        reason = "Preserve the public Error variants without boxing"
    )]
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

    #[expect(
        clippy::result_large_err,
        reason = "Preserve the public Error variants without boxing"
    )]
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

    #[expect(
        clippy::result_large_err,
        reason = "Preserve the public Error variants without boxing"
    )]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{api_error, discovery_response, resource_sync, response, MockApi};
    use http::Response;
    use kube::client::Body;
    use rstest::rstest;
    use serde_json::{json, Value};
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    fn watcher(
        client: Client,
    ) -> (
        RemoteWatcher,
        mpsc::UnboundedReceiver<ObjectRef<ResourceSync>>,
    ) {
        let sync = resource_sync();
        let key = RemoteWatcherKey {
            object: sync.spec.source.clone(),
            resource_sync: ObjectRef::from_obj(&sync),
        };
        let (sender, receiver) = mpsc::unbounded_channel();
        (RemoteWatcher::new(key, sender, client), receiver)
    }

    fn watch_response(events: Vec<Value>) -> Response<Body> {
        let mut bytes = Vec::new();
        for event in events {
            serde_json::to_writer(&mut bytes, &event).expect("serialize watch event");
            bytes.push(b'\n');
        }
        Response::builder()
            .status(200)
            .body(Body::from(bytes))
            .expect("watch response")
    }

    fn object_event(event: &str, manager: Option<&str>, rv: Option<&str>) -> Value {
        let managed_fields = manager
            .map(|manager| vec![json!({"manager": manager, "time": "2024-01-01T00:00:00Z"})]);
        json!({"type": event, "object": {"apiVersion": "v1", "kind": "ConfigMap",
            "metadata": {"name": "source-config", "resourceVersion": rv, "managedFields": managed_fields}}})
    }

    #[rstest]
    #[case::added_external(object_event("ADDED", Some("external"), Some("11")), "11", true)]
    #[case::added_sinker(
        object_event("ADDED", Some("sinker.influxdata.io"), Some("11")),
        "11",
        false
    )]
    #[case::modified_external(object_event("MODIFIED", Some("external"), Some("12")), "12", true)]
    #[case::modified_sinker(
        object_event("MODIFIED", Some("sinker.influxdata.io"), Some("12")),
        "12",
        false
    )]
    #[case::unknown_ownership(object_event("MODIFIED", None, Some("13")), "13", true)]
    #[case::deleted_sinker(
        object_event("DELETED", Some("sinker.influxdata.io"), Some("14")),
        "14",
        true
    )]
    #[case::bookmark(json!({"type": "BOOKMARK", "object": {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"resourceVersion": "15"}}}), "15", false)]
    #[case::expired_version(json!({"type": "ERROR", "object": {"code": 410, "reason": "Expired", "message": "too old", "status": "Failure"}}), "0", false)]
    #[case::api_error(json!({"type": "ERROR", "object": {"code": 500, "reason": "InternalError", "message": "retry", "status": "Failure"}}), "10", true)]
    #[tokio::test]
    async fn watch_events_advance_versions_and_reconcile_when_needed(
        #[case] event: Value,
        #[case] expected_rv: &str,
        #[case] reconcile: bool,
    ) {
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            watch_response(vec![event]),
        ]);
        let (watcher, mut receiver) = watcher(mock.client.clone());
        let api = watcher
            .key
            .object
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("API discovery");
        let rv = timeout(
            Duration::from_secs(3),
            watcher.listen(
                &api,
                "10".into(),
                &WatchParams::default(),
                &mut DefaultBackoff::default(),
            ),
        )
        .await
        .expect("bounded watch stream")
        .expect("read events");
        assert_eq!(rv, expected_rv);
        if reconcile {
            assert_eq!(
                receiver.try_recv().expect("reconcile event"),
                watcher.key.resource_sync
            );
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1/namespaces/team-a/configmaps"),
        ]);
    }

    #[rstest]
    #[case::added("ADDED", false)]
    #[case::modified("MODIFIED", false)]
    #[case::deleted("DELETED", false)]
    #[case::added_after_events("ADDED", true)]
    #[case::modified_after_events("MODIFIED", true)]
    #[case::deleted_after_events("DELETED", true)]
    #[tokio::test]
    async fn object_events_require_resource_version(
        #[case] event: &str,
        #[case] earlier_events: bool,
    ) {
        let mut events = vec![];
        if earlier_events {
            events.push(object_event("ADDED", Some("external"), Some("11")));
            events.push(object_event("MODIFIED", None, Some("12")));
        }
        events.push(object_event(event, None, None));
        events.push(object_event("DELETED", None, Some("14")));
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            watch_response(events),
        ]);
        let (watcher, mut receiver) = watcher(mock.client.clone());
        let api = watcher
            .key
            .object
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("API discovery");
        let error = watcher
            .listen(
                &api,
                "10".into(),
                &WatchParams::default(),
                &mut DefaultBackoff::default(),
            )
            .await
            .expect_err("missing event version");
        assert!(matches!(error, Error::ResourceVersionRequired));
        if earlier_events {
            for _ in 0..2 {
                assert_eq!(
                    receiver.try_recv().expect("earlier reconcile"),
                    watcher.key.resource_sync
                );
            }
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1/namespaces/team-a/configmaps"),
        ]);
    }

    #[rstest]
    #[case::empty(vec![], "10", 0)]
    #[case::mixed(vec![
        object_event("ADDED", Some("external"), Some("11")),
        object_event("MODIFIED", Some("sinker.influxdata.io"), Some("12")),
        object_event("MODIFIED", None, Some("13")),
        object_event("DELETED", Some("sinker.influxdata.io"), Some("14")),
        object_event("ADDED", Some("sinker.influxdata.io"), Some("15")),
        object_event("MODIFIED", Some("external"), Some("16")),
        object_event("ADDED", None, Some("17")),
        object_event("DELETED", None, Some("18")),
        object_event("MODIFIED", Some("sinker.influxdata.io"), Some("19")),
    ], "19", 6)]
    #[case::bookmark_after_change(vec![
        object_event("MODIFIED", None, Some("11")),
        json!({"type": "BOOKMARK", "object": {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"resourceVersion": "20"}}}),
    ], "20", 1)]
    #[case::expired_after_change(vec![
        object_event("MODIFIED", None, Some("11")),
        json!({"type": "ERROR", "object": {"code": 410, "reason": "Expired", "message": "too old", "status": "Failure"}}),
    ], "0", 1)]
    #[tokio::test]
    async fn watch_sequences_preserve_versions_and_reconcile_counts(
        #[case] events: Vec<Value>,
        #[case] expected_version: &str,
        #[case] expected_reconciles: usize,
    ) {
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            watch_response(events),
        ]);
        let (watcher, mut receiver) = watcher(mock.client.clone());
        let api = watcher
            .key
            .object
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("API discovery");
        let version = timeout(
            Duration::from_secs(2),
            watcher.listen(
                &api,
                "10".into(),
                &WatchParams::default(),
                &mut DefaultBackoff::default(),
            ),
        )
        .await
        .expect("bounded watch stream")
        .expect("read events");
        assert_eq!(version, expected_version);
        for _ in 0..expected_reconciles {
            assert_eq!(
                receiver.try_recv().expect("reconcile event"),
                watcher.key.resource_sync
            );
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1/namespaces/team-a/configmaps"),
        ]);
    }

    #[rstest]
    #[case::discovery(false)]
    #[case::initial_get(true)]
    #[tokio::test]
    async fn start_propagates_api_errors_before_sending_reconcile(#[case] discovered: bool) {
        let mut responses = vec![];
        let mut expected = vec![("GET", "/api/v1")];
        if discovered {
            responses.push(discovery_response("ConfigMap", "configmaps", true));
            expected.push(("GET", "/api/v1/namespaces/team-a/configmaps/source-config"));
        }
        responses.push(api_error(403));
        let mock = MockApi::new(responses);
        let (watcher, mut receiver) = watcher(mock.client.clone());
        let error = watcher
            .start(&mut DefaultBackoff::default())
            .await
            .expect_err("API denied");
        assert!(matches!(error, Error::KubeError(kube::Error::Api(error)) if error.code == 403));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        mock.finish(&expected);
    }

    #[tokio::test]
    async fn start_reconciles_then_reconnects_from_last_event_with_name_selector() {
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            response(
                200,
                json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "source-config", "resourceVersion": "10"}}),
            ),
            watch_response(vec![object_event("MODIFIED", Some("external"), Some("11"))]),
            api_error(403),
        ]);
        let (watcher, mut receiver) = watcher(mock.client.clone());
        let error = timeout(
            Duration::from_secs(2),
            watcher.start(&mut DefaultBackoff::default()),
        )
        .await
        .expect("bounded reconnect")
        .expect_err("second watch denied");
        assert!(matches!(error, Error::KubeError(kube::Error::Api(error)) if error.code == 403));
        for _ in 0..2 {
            assert_eq!(
                receiver
                    .try_recv()
                    .expect("initial and external reconciles"),
                watcher.key.resource_sync
            );
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        let requests = mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1/namespaces/team-a/configmaps/source-config"),
            ("GET", "/api/v1/namespaces/team-a/configmaps"),
            ("GET", "/api/v1/namespaces/team-a/configmaps"),
        ]);
        for (request, rv) in [
            (&requests[2], "resourceVersion=10"),
            (&requests[3], "resourceVersion=11"),
        ] {
            let parameters: Vec<_> = request
                .uri()
                .query()
                .expect("watch query")
                .split('&')
                .collect();
            assert!(parameters.contains(&"watch=true"));
            assert!(parameters.contains(&"fieldSelector=metadata.name%3Dsource-config"));
            assert!(parameters.contains(&rv));
        }
    }

    #[tokio::test]
    async fn start_requires_namespace_before_api_access() {
        let mock = MockApi::new(vec![]);
        let (mut watcher, _receiver) = watcher(mock.client.clone());
        watcher.key.resource_sync.namespace = None;
        assert!(matches!(
            watcher
                .start(&mut DefaultBackoff::default())
                .await
                .expect_err("namespace required"),
            Error::NamespaceRequired
        ));
        mock.finish(&[]);
    }

    #[tokio::test]
    async fn start_requires_initial_resource_version_before_sending_reconcile() {
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            response(200, json!({"metadata": {"name": "source-config"}})),
        ]);
        let (watcher, mut receiver) = watcher(mock.client.clone());
        assert!(matches!(
            watcher
                .start(&mut DefaultBackoff::default())
                .await
                .expect_err("resource version required"),
            Error::ResourceVersionRequired
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1/namespaces/team-a/configmaps/source-config"),
        ]);
    }

    #[tokio::test]
    async fn failed_start_requests_reconciliation_and_can_be_cancelled() {
        let mock = MockApi::new(vec![]);
        let (mut watcher, mut receiver) = watcher(mock.client.clone());
        watcher.key.resource_sync.namespace = None;
        let expected = watcher.key.resource_sync.clone();
        let (ctx, handle) = Context::new();
        let task = tokio::spawn(watcher.run(ctx));
        let event = timeout(Duration::from_secs(2), receiver.recv()).await;
        handle.cancel();
        timeout(Duration::from_secs(3), task)
            .await
            .expect("join cancelled watch")
            .expect("watch task did not panic");
        assert_eq!(event.expect("failure triggers reconcile"), Some(expected));
        mock.finish(&[]);
    }

    #[tokio::test]
    async fn sending_to_closed_reconcile_channel_is_nonfatal() {
        let mock = MockApi::new(vec![]);
        let (watcher, receiver) = watcher(mock.client.clone());
        drop(receiver);
        watcher.send_reconcile_on_success(&mut DefaultBackoff::default());
        mock.finish(&[]);
    }

    #[tokio::test]
    async fn invalid_watch_json_returns_a_decode_error() {
        let mock = MockApi::new(vec![
            discovery_response("ConfigMap", "configmaps", true),
            Response::builder()
                .status(200)
                .body(Body::from(b"not-json\n".to_vec()))
                .expect("malformed stream"),
        ]);
        let (watcher, mut receiver) = watcher(mock.client.clone());
        let api = watcher
            .key
            .object
            .api_for(mock.client.clone(), "team-a")
            .await
            .expect("API discovery");
        let error = watcher
            .listen(
                &api,
                "10".into(),
                &WatchParams::default(),
                &mut DefaultBackoff::default(),
            )
            .await
            .expect_err("decode failure");
        assert!(matches!(
            error,
            Error::KubeError(kube::Error::SerdeError(_))
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        mock.finish(&[
            ("GET", "/api/v1"),
            ("GET", "/api/v1/namespaces/team-a/configmaps"),
        ]);
    }
}
