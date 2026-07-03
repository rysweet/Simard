//! Unified Simard telemetry facade.
//!
//! One typed entry point for every operational metric. The facade dual-writes
//! into a bounded in-process [`registry`] (the source of truth read by
//! [`crate::status`]) and — once wired in `init_tracing` — into OpenTelemetry
//! instruments on an `SdkMeterProvider`. OTLP export is endpoint-gated
//! (`OTEL_EXPORTER_OTLP_ENDPOINT`), identical to traces, so the default
//! deployment stays fully local.
//!
//! See `docs/reference/telemetry-metrics.md` for the metric catalog and design.
//!
//! ```no_run
//! use simard::telemetry;
//! use simard::telemetry::names;
//!
//! telemetry::counter_add(names::DISTILL_RUNS, 1, &[(names::ATTR_RESULT, "ok")]);
//! telemetry::gauge_set(names::ENGINEER_ACTIVE, 2, &[]);
//! telemetry::histogram_record(names::DAEMON_CYCLE_DURATION_SECONDS, 1.4, &[]);
//! ```

pub mod names;
pub mod otel;
pub mod registry;
pub mod snapshot;

pub use otel::{init as init_metrics, shutdown as shutdown_metrics};
pub use registry::{capture, counter_add, gauge_set, histogram_record, overflow_count, reset};
pub use snapshot::MetricsSnapshot;

use std::path::Path;

/// Capture the current in-process registry and atomically flush it to
/// `<state_root>/telemetry/metrics_snapshot.json`.
///
/// The daemon calls this once per OODA cycle (and on shutdown) so out-of-process
/// readers — `simard status` and the TUI — see live daemon metrics without an
/// external OTLP collector. The write is atomic and `0600`; readers tolerate a
/// missing/corrupt file. Errors are returned, not panicked, so a flush failure
/// never disrupts the cycle.
pub fn flush_snapshot(state_root: impl AsRef<Path>) -> std::io::Result<()> {
    let snap = registry::capture();
    snapshot::write_atomic(&snapshot::snapshot_path(state_root.as_ref()), &snap)
}
