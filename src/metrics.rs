//! ResourceSync reconciliation metrics compatible with controller-runtime dashboards.

use std::{future::Future, time::Duration, time::Instant};

use kube::runtime::controller::Action;
use prometheus_client::{
    metrics::{counter::Counter, family::Family, gauge::Gauge, histogram::Histogram},
    registry::Registry,
};

const RESULTS: [&str; 4] = ["success", "error", "requeue", "requeue_after"];

/// Shared metric handles for the `resourcesync` controller.
///
/// Register once in the registry served by the admin endpoint, then pass the
/// returned handles to [`crate::controller::run_with_metrics`]. Clones update the
/// same series. The default value collects metrics without exposing them.
#[derive(Clone, Debug)]
pub struct ControllerMetrics {
    active_workers: Gauge,
    reconcile_total: Family<[(&'static str, &'static str); 1], Counter>,
    reconcile_time: Histogram,
}

impl Default for ControllerMetrics {
    fn default() -> Self {
        let reconcile_total: Family<_, Counter> = Family::default();
        // Expose zero-valued outcomes before the first attempt, including outcomes
        // Sinker does not currently return, so rate queries have an initial sample.
        for result in RESULTS {
            reconcile_total
                .get_or_create(&[("result", result)])
                .inc_by(0);
        }
        Self {
            active_workers: Gauge::default(),
            reconcile_total,
            // Match controller-runtime's classic buckets, in seconds. The encoder
            // adds the +Inf bucket required by the dashboard's histogram fallback.
            reconcile_time: Histogram::new([
                0.005, 0.01, 0.025, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.6,
                0.7, 0.8, 0.9, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 6.0, 7.0,
                8.0, 9.0, 10.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 60.0,
            ]),
        }
    }
}

impl ControllerMetrics {
    /// Register the dashboard's metric families, labeled `controller="resourcesync"`.
    ///
    /// Call once per registry. Namespace, pod, service, and cluster labels belong
    /// to the scrape configuration, not the resources being reconciled.
    pub fn register(registry: &mut Registry) -> Self {
        let metrics = Self::default();
        let registry =
            registry.sub_registry_with_label(("controller".into(), "resourcesync".into()));
        registry.register(
            "controller_runtime_active_workers",
            "Number of ResourceSync reconciliations currently running",
            metrics.active_workers.clone(),
        );
        // prometheus-client appends _total when encoding counters.
        registry.register(
            "controller_runtime_reconcile",
            "Number of completed ResourceSync reconciliations by result",
            metrics.reconcile_total.clone(),
        );
        registry.register(
            "controller_runtime_reconcile_time_seconds",
            "Time spent reconciling a ResourceSync in seconds",
            metrics.reconcile_time.clone(),
        );
        metrics
    }

    pub(crate) async fn instrument<E>(
        &self,
        reconcile: impl Future<Output = Result<Action, E>>,
    ) -> Result<Action, E> {
        self.active_workers.inc();
        let _attempt = ReconcileAttempt {
            metrics: self,
            started: Instant::now(),
        };
        let result = reconcile.await;
        // Classify the returned result before kube applies its error policy: an
        // error retried after five seconds is still an error, not requeue_after.
        let outcome = match &result {
            Err(_) => "error",
            Ok(action) if *action == Action::await_change() => "success",
            Ok(action) if *action == Action::requeue(Duration::ZERO) => "requeue",
            Ok(_) => "requeue_after",
        };
        self.reconcile_total
            .get_or_create(&[("result", outcome)])
            .inc();
        result
    }
}

/// Release the worker and record elapsed time even if a polled future is canceled
/// or unwinds. Only completed attempts increment a result counter.
struct ReconcileAttempt<'a> {
    metrics: &'a ControllerMetrics,
    started: Instant,
}

impl Drop for ReconcileAttempt<'_> {
    fn drop(&mut self) {
        self.metrics
            .reconcile_time
            .observe(self.started.elapsed().as_secs_f64());
        self.metrics.active_workers.dec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{future, poll, FutureExt};
    use prometheus_client::encoding::text::encode;
    use rstest::rstest;

    fn encoded(registry: &Registry) -> String {
        let mut text = String::new();
        encode(&mut text, registry).expect("encode metrics");
        text
    }

    #[test]
    fn dashboard_series_exist_before_reconciliation_in_the_runtime_registry() {
        let mut registry = Registry::default();
        let _runtime_metrics = kubert::runtime::RuntimeMetrics::register(&mut registry);
        ControllerMetrics::register(&mut registry);
        let text = encoded(&registry);
        assert!(text.contains("# TYPE client_response_duration_seconds histogram\n"));
        assert!(text.contains("# TYPE controller_runtime_active_workers gauge\n"));
        assert!(text.contains("controller_runtime_active_workers{controller=\"resourcesync\"} 0\n"));
        assert!(text.contains("# TYPE controller_runtime_reconcile counter\n"));
        for outcome in RESULTS {
            assert!(text.contains(&format!(
                "controller_runtime_reconcile_total{{controller=\"resourcesync\",result=\"{outcome}\"}} 0\n"
            )));
        }
        assert!(text.contains("# TYPE controller_runtime_reconcile_time_seconds histogram\n"));
        for suffix in ["sum", "count"] {
            assert!(text.contains(&format!(
                "controller_runtime_reconcile_time_seconds_{suffix}{{controller=\"resourcesync\"}} 0"
            )));
        }
        for bound in ["0.005", "1.0", "60.0", "+Inf"] {
            assert!(text.contains(&format!(
                "controller_runtime_reconcile_time_seconds_bucket{{controller=\"resourcesync\",le=\"{bound}\"}} 0\n"
            )));
        }
    }

    #[rstest]
    #[case::success(Ok(Action::await_change()), "success")]
    #[case::error(Err("reconciliation failed"), "error")]
    #[case::immediate_retry(Ok(Action::requeue(Duration::ZERO)), "requeue")]
    #[case::timed_retry(Ok(Action::requeue(Duration::from_millis(500))), "requeue_after")]
    #[tokio::test]
    async fn completed_attempt_preserves_result_and_records_one_outcome(
        #[case] result: Result<Action, &'static str>,
        #[case] outcome: &str,
    ) {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let returned = metrics
            .instrument(async {
                assert_eq!(metrics.active_workers.get(), 1);
                result.clone()
            })
            .await;
        assert_eq!(returned, result);
        assert_eq!(metrics.active_workers.get(), 0);
        for label in RESULTS {
            assert_eq!(
                metrics
                    .reconcile_total
                    .get_or_create(&[("result", label)])
                    .get(),
                u64::from(label == outcome)
            );
        }
        let text = encoded(&registry);
        assert!(text.contains(
            "controller_runtime_reconcile_time_seconds_count{controller=\"resourcesync\"} 1\n"
        ));
        assert!(text.contains("controller_runtime_reconcile_time_seconds_bucket{controller=\"resourcesync\",le=\"+Inf\"} 1\n"));
    }

    #[tokio::test]
    async fn overlapping_attempts_track_workers_duration_and_cancellation() {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let clone = metrics.clone();
        let (finish, wait) = tokio::sync::oneshot::channel();
        let mut first = Box::pin(metrics.instrument(async {
            wait.await.expect("complete first attempt");
            Ok::<_, ()>(Action::await_change())
        }));
        let mut second = Box::pin(clone.instrument(future::pending::<Result<Action, ()>>()));
        assert_eq!(metrics.active_workers.get(), 0, "unpolled futures are idle");
        assert!(poll!(first.as_mut()).is_pending());
        assert!(poll!(second.as_mut()).is_pending());
        assert_eq!(metrics.active_workers.get(), 2);

        tokio::time::sleep(Duration::from_millis(20)).await;
        finish.send(()).expect("signal completion");
        assert_eq!(first.await, Ok(Action::await_change()));
        assert_eq!(metrics.active_workers.get(), 1);
        drop(second);
        assert_eq!(metrics.active_workers.get(), 0);
        assert_eq!(
            metrics
                .reconcile_total
                .get_or_create(&[("result", "success")])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .reconcile_total
                .get_or_create(&[("result", "error")])
                .get(),
            0
        );

        let text = encoded(&registry);
        assert!(text.contains(
            "controller_runtime_reconcile_time_seconds_count{controller=\"resourcesync\"} 2\n"
        ));
        let sum = text
            .lines()
            .find_map(|line| {
                line.strip_prefix(
                    "controller_runtime_reconcile_time_seconds_sum{controller=\"resourcesync\"} ",
                )
            })
            .expect("duration sum")
            .parse::<f64>()
            .expect("seconds");
        assert!(
            sum >= 0.04,
            "both attempts include time spent awaiting work: {sum}"
        );
    }

    #[tokio::test]
    async fn panic_unwinding_releases_worker_without_claiming_a_result() {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let future = metrics.instrument(async {
            panic!("reconciler panic");
            #[allow(unreachable_code)]
            Ok::<_, ()>(Action::await_change())
        });
        assert!(std::panic::AssertUnwindSafe(future)
            .catch_unwind()
            .await
            .is_err());
        assert_eq!(metrics.active_workers.get(), 0);
        for result in RESULTS {
            assert_eq!(
                metrics
                    .reconcile_total
                    .get_or_create(&[("result", result)])
                    .get(),
                0
            );
        }
        assert!(encoded(&registry).contains(
            "controller_runtime_reconcile_time_seconds_count{controller=\"resourcesync\"} 1\n"
        ));
    }
}
