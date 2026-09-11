//! ResourceSync reconciliation metrics compatible with controller-runtime dashboards.
//!
//! The controller passes each complete reconciliation future through these collectors;
//! watched-object events and queued retries are not themselves reconciliation attempts.
//! Registration joins the existing admin registry, while cloned handles let concurrent
//! attempts contribute to the same process-wide series.

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
///
/// Series aggregate all ResourceSync objects handled by this process. Object names,
/// namespaces, and remote cluster identities are intentionally not metric labels.
#[derive(Clone, Debug)]
pub struct ControllerMetrics {
    active_workers: Gauge,
    reconcile_total: Family<[(&'static str, &'static str); 1], Counter>,
    reconcile_time: Histogram,
}

impl Default for ControllerMetrics {
    /// Create independent, zeroed collectors without adding them to a registry.
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
    /// Call once per registry: each call creates fresh collectors, rather than
    /// looking up earlier registrations. All series are exposed at zero immediately.
    /// The registry owns clones, so moving it into the admin server leaves the
    /// returned handles connected to the exposed series.
    ///
    /// Namespace, pod, service, and cluster labels belong to the scrape configuration,
    /// not the resources being reconciled.
    pub fn register(registry: &mut Registry) -> Self {
        let metrics = Self::default();
        // Scope this label to our collectors; Kubert's existing client and runtime
        // metrics in the parent registry must retain their own label sets.
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

    /// Measure a reconciler future and return its action or error unchanged.
    ///
    /// Timing starts on the first poll and includes time suspended at `.await`
    /// points, but excludes time waiting in the controller queue. Dropping an
    /// unpolled future records nothing. Cancellation and panic unwinding release
    /// the worker and record elapsed time, without a completed-result label.
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
        // Action has no public duration accessor; equality with its constructors
        // distinguishes waiting for events, an immediate retry, and a timed retry.
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
///
/// Each guard balances one worker increment. Keeping it inside the async body
/// ties cleanup to the lifetime of the attempt, including all suspended polls.
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
    use std::collections::BTreeMap;

    // Keep the expected public labels independent of the production RESULTS list.
    const OUTCOMES: [&str; 4] = ["success", "error", "requeue", "requeue_after"];

    fn encoded(registry: &Registry) -> String {
        let mut text = String::new();
        encode(&mut text, registry).expect("encode metrics");
        text
    }

    // Inspect the exposed series rather than collector handles: get_or_create
    // could otherwise create a missing series and conceal a registration defect.
    fn samples(registry: &Registry) -> BTreeMap<String, f64> {
        let mut samples = BTreeMap::new();
        for line in encoded(registry)
            .lines()
            .filter(|line| !line.starts_with('#'))
        {
            let (series, value) = line.split_once(' ').expect("metric sample");
            assert!(
                samples
                    .insert(series.to_owned(), value.parse().expect("numeric sample"))
                    .is_none(),
                "duplicate series: {series}"
            );
        }
        samples
    }

    // Result counts follow OUTCOMES. Observations can exceed their sum because
    // canceled and unwound attempts contribute elapsed time without an outcome.
    fn assert_state(registry: &Registry, active: u32, results: [u32; 4], observations: u32) {
        let samples = samples(registry);
        assert_eq!(
            samples["controller_runtime_active_workers{controller=\"resourcesync\"}"],
            f64::from(active)
        );
        let actual: BTreeMap<_, _> = samples
            .iter()
            .filter(|(name, _)| name.starts_with("controller_runtime_reconcile_total{"))
            .map(|(name, count)| (name.clone(), *count))
            .collect();
        let expected = OUTCOMES.into_iter().zip(results).map(|(outcome, count)| (
            format!("controller_runtime_reconcile_total{{controller=\"resourcesync\",result=\"{outcome}\"}}"),
            f64::from(count),
        )).collect();
        assert_eq!(actual, expected, "exactly the four public outcome series");
        assert_eq!(
            samples["controller_runtime_reconcile_time_seconds_count{controller=\"resourcesync\"}"],
            f64::from(observations)
        );
        assert_eq!(samples["controller_runtime_reconcile_time_seconds_bucket{controller=\"resourcesync\",le=\"+Inf\"}"], f64::from(observations));
    }

    fn duration_sum(registry: &Registry) -> f64 {
        samples(registry)
            ["controller_runtime_reconcile_time_seconds_sum{controller=\"resourcesync\"}"]
    }

    #[test]
    fn dashboard_series_exist_before_reconciliation_in_the_runtime_registry() {
        let mut registry = Registry::default();
        let _runtime_metrics = kubert::runtime::RuntimeMetrics::register(&mut registry);
        ControllerMetrics::register(&mut registry);
        let text = encoded(&registry);
        for declaration in [
            "# TYPE client_response_duration_seconds histogram",
            "# TYPE controller_runtime_active_workers gauge",
            "# TYPE controller_runtime_reconcile counter",
            "# TYPE controller_runtime_reconcile_time_seconds histogram",
        ] {
            assert!(
                text.lines().any(|line| line == declaration),
                "{declaration}"
            );
        }
        assert_state(&registry, 0, [0; 4], 0);
        assert_eq!(duration_sum(&registry), 0.0);
        let mut bounds = vec![];
        for (series, value) in samples(&registry) {
            if let Some(bound) = series.strip_prefix("controller_runtime_reconcile_time_seconds_bucket{controller=\"resourcesync\",le=\"") {
                let bound = bound.strip_suffix("\"}").expect("bucket labels");
                assert_eq!(value, 0.0, "initial bucket {bound}");
                bounds.push(bound.parse::<f64>().expect("bucket upper bound"));
            }
        }
        bounds.sort_by(f64::total_cmp);
        // The classic controller-runtime bucket contract, including the implicit +Inf bucket.
        assert_eq!(
            bounds,
            [
                0.005,
                0.01,
                0.025,
                0.05,
                0.1,
                0.15,
                0.2,
                0.25,
                0.3,
                0.35,
                0.4,
                0.45,
                0.5,
                0.6,
                0.7,
                0.8,
                0.9,
                1.0,
                1.25,
                1.5,
                1.75,
                2.0,
                2.5,
                3.0,
                3.5,
                4.0,
                4.5,
                5.0,
                6.0,
                7.0,
                8.0,
                9.0,
                10.0,
                15.0,
                20.0,
                25.0,
                30.0,
                40.0,
                50.0,
                60.0,
                f64::INFINITY,
            ]
        );
    }

    #[rstest]
    #[case::success(Ok(Action::await_change()), [1, 0, 0, 0])]
    #[case::error(Err("reconciliation failed"), [0, 1, 0, 0])]
    #[case::immediate_retry(Ok(Action::requeue(Duration::ZERO)), [0, 0, 1, 0])]
    #[case::smallest_timed_retry(Ok(Action::requeue(Duration::from_nanos(1))), [0, 0, 0, 1])]
    #[case::timed_retry(Ok(Action::requeue(Duration::from_millis(500))), [0, 0, 0, 1])]
    #[tokio::test]
    async fn completed_attempt_preserves_result_and_records_one_outcome(
        #[case] result: Result<Action, &'static str>,
        #[case] counts: [u32; 4],
    ) {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let before = Instant::now();
        let returned = metrics
            .instrument(async {
                assert_state(&registry, 1, [0; 4], 0);
                result.clone()
            })
            .await;
        let upper_bound = before.elapsed().as_secs_f64();
        assert_eq!(returned, result);
        assert_state(&registry, 0, counts, 1);
        assert!((0.0..=upper_bound).contains(&duration_sum(&registry)));
    }

    #[tokio::test]
    async fn mixed_outcomes_accumulate_across_clones_without_touching_another_registry() {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let clone = metrics.clone();
        let mut unrelated_registry = Registry::default();
        let _unrelated_metrics = ControllerMetrics::register(&mut unrelated_registry);
        let cases = [
            (Ok(Action::await_change()), [1, 0, 0, 0]),
            (Err("first failure"), [1, 1, 0, 0]),
            (
                Ok(Action::requeue(Duration::from_millis(500))),
                [1, 1, 0, 1],
            ),
            (Ok(Action::requeue(Duration::ZERO)), [1, 1, 1, 1]),
            (Err("second failure"), [1, 2, 1, 1]),
            (Ok(Action::await_change()), [2, 2, 1, 1]),
            (Ok(Action::requeue(Duration::ZERO)), [2, 2, 2, 1]),
            (Ok(Action::requeue(Duration::from_secs(9))), [2, 2, 2, 2]),
        ];
        for (index, (result, counts)) in cases.into_iter().enumerate() {
            let handles = if index % 2 == 0 { &metrics } else { &clone };
            assert_eq!(
                handles.instrument(future::ready(result.clone())).await,
                result
            );
            assert_state(&registry, 0, counts, counts.into_iter().sum());
            assert_state(&unrelated_registry, 0, [0; 4], 0);
            assert_eq!(duration_sum(&unrelated_registry), 0.0);
        }
    }

    #[rstest]
    #[case::success_then_cancel(Ok(Action::await_change()), [1, 0, 0, 0], false)]
    #[case::cancel_then_success(Ok(Action::await_change()), [1, 0, 0, 0], true)]
    #[case::error_then_cancel(Err("failed after awaiting work"), [0, 1, 0, 0], false)]
    #[case::cancel_then_error(Err("failed after awaiting work"), [0, 1, 0, 0], true)]
    #[tokio::test]
    async fn overlapping_attempts_track_workers_duration_and_cancellation(
        #[case] result: Result<Action, &'static str>,
        #[case] counts: [u32; 4],
        #[case] cancel_first: bool,
    ) {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let clone = metrics.clone();
        let (finish, wait) = tokio::sync::oneshot::channel();
        let mut first = Box::pin(metrics.instrument(async {
            wait.await.expect("complete first attempt");
            result.clone()
        }));
        let mut second = Box::pin(clone.instrument(future::pending::<Result<Action, ()>>()));
        assert_state(&registry, 0, [0; 4], 0);
        let before = Instant::now();
        assert!(poll!(first.as_mut()).is_pending());
        assert!(poll!(second.as_mut()).is_pending());
        let after_start = Instant::now();
        // Repeated Pending polls must not start additional attempts.
        assert!(poll!(first.as_mut()).is_pending());
        assert!(poll!(second.as_mut()).is_pending());
        assert_state(&registry, 2, [0; 4], 0);
        // Both attempts span this interval, making twice its length a lower bound
        // on their combined duration without relying on a scheduler sleep.
        let lower_bound = 2.0 * after_start.elapsed().as_secs_f64();

        if cancel_first {
            drop(second);
            assert_state(&registry, 1, [0; 4], 1);
            finish.send(()).expect("signal completion");
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), first)
                    .await
                    .expect("first completes"),
                result
            );
        } else {
            finish.send(()).expect("signal completion");
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), first)
                    .await
                    .expect("first completes"),
                result
            );
            assert_state(&registry, 1, counts, 1);
            drop(second);
        }
        // Each attempt fits within the outer interval, regardless of which ends first.
        let upper_bound = 2.0 * before.elapsed().as_secs_f64();
        assert_state(&registry, 0, counts, 2);
        let sum = duration_sum(&registry);
        assert!(
            (lower_bound..=upper_bound).contains(&sum),
            "sum {sum} outside {lower_bound}..={upper_bound} seconds"
        );
    }

    #[test]
    fn dropping_an_unpolled_future_does_not_start_an_attempt() {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let polled = std::cell::Cell::new(false);
        // This body must remain unexecuted: constructing a future is not an attempt.
        drop(metrics.instrument(async {
            polled.set(true);
            Ok::<_, ()>(Action::await_change())
        }));
        assert!(!polled.get());
        assert_state(&registry, 0, [0; 4], 0);
        assert_eq!(duration_sum(&registry), 0.0);
    }

    #[test]
    fn slow_attempt_records_seconds_and_the_infinite_bucket() {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        // Backdate the private guard to test long durations without waiting or
        // adding a production clock abstraction solely for tests.
        let started = Instant::now()
            .checked_sub(Duration::from_secs(75))
            .expect("earlier instant");
        metrics.active_workers.inc();
        let lower_bound = started.elapsed().as_secs_f64();
        drop(ReconcileAttempt {
            metrics: &metrics,
            started,
        });
        let upper_bound = started.elapsed().as_secs_f64();
        assert_state(&registry, 0, [0; 4], 1);
        assert!((lower_bound..=upper_bound).contains(&duration_sum(&registry)));
        for (series, value) in samples(&registry) {
            if series.starts_with("controller_runtime_reconcile_time_seconds_bucket{")
                && !series.contains("+Inf")
            {
                assert_eq!(
                    value, 0.0,
                    "75 seconds exceeds every finite bucket: {series}"
                );
            }
        }
    }

    #[tokio::test]
    async fn panic_unwinding_releases_worker_without_claiming_a_result() {
        let mut registry = Registry::default();
        let metrics = ControllerMetrics::register(&mut registry);
        let mut other = Box::pin(metrics.instrument(future::pending::<Result<Action, ()>>()));
        assert!(poll!(other.as_mut()).is_pending());
        let future = metrics.instrument(async {
            assert_state(&registry, 2, [0; 4], 0);
            panic!("reconciler panic");
            #[allow(unreachable_code)]
            Ok::<_, ()>(Action::await_change())
        });
        let panic = std::panic::AssertUnwindSafe(future)
            .catch_unwind()
            .await
            .expect_err("propagates the reconciler panic");
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"reconciler panic"));
        assert_state(&registry, 1, [0; 4], 1);
        drop(other);
        assert_state(&registry, 0, [0; 4], 2);
    }
}
