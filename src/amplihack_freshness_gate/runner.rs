//! Production wiring for the amplihack freshness gate: the real
//! `amplihack update` subprocess runner (with an idle/liveness bound — never a
//! wall-clock mid-work kill), the system clock, the `self_metrics` sink, and the
//! two entry points the spawn path and daemon startup call.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::gate::{
    AmplihackUpdater, GateClock, GateConfig, GateOutcome, MetricSink, TRACE_TARGET,
    run_freshness_gate,
};

/// Idle/liveness bound for `amplihack update`: the subprocess is killed only if
/// it produces **no output at all** for this long. A build or network fetch that
/// is still emitting progress is never aborted — this is a liveness bound, not a
/// total-runtime deadline. Generous by design; an expiry here is surfaced as a
/// `failed` outcome, never a silent kill.
const DEFAULT_IDLE_BOUND_SECS: u64 = 900;

/// How often the monitor loop polls the child and the idle timer.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Runs the real `amplihack update` — the exact command the operator runs — with
/// an idle/liveness bound.
pub struct RealUpdater {
    idle_bound: Duration,
}

impl Default for RealUpdater {
    fn default() -> Self {
        Self {
            idle_bound: Duration::from_secs(DEFAULT_IDLE_BOUND_SECS),
        }
    }
}

impl AmplihackUpdater for RealUpdater {
    fn run_update(&self) -> Result<(), String> {
        run_amplihack_update(self.idle_bound)
    }
}

/// Spawn `amplihack update`, forward its output to `debug` tracing, and enforce
/// the idle bound. Returns `Ok(())` on a zero exit status, `Err(msg)` otherwise
/// (including a stalled subprocess that hit the idle bound).
fn run_amplihack_update(idle_bound: Duration) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let mut child = Command::new("amplihack")
        .arg("update")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn `amplihack update`: {e}"))?;

    let start = Instant::now();
    // Milliseconds since `start` of the most recent line of output. Both reader
    // threads and the monitor loop measure against the same `start`.
    let last_activity = Arc::new(AtomicU64::new(0));

    let mut pumps = Vec::new();
    if let Some(out) = child.stdout.take() {
        pumps.push(spawn_line_pump(
            out,
            Arc::clone(&last_activity),
            start,
            "stdout",
        ));
    }
    if let Some(err) = child.stderr.take() {
        pumps.push(spawn_line_pump(
            err,
            Arc::clone(&last_activity),
            start,
            "stderr",
        ));
    }

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(format!("failed to poll `amplihack update`: {e}")),
        }

        let now_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let idle_ms = now_ms.saturating_sub(last_activity.load(Ordering::Relaxed));
        if Duration::from_millis(idle_ms) > idle_bound {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "`amplihack update` produced no output for {}s (idle/liveness bound); killed as stalled",
                idle_bound.as_secs()
            ));
        }

        std::thread::sleep(POLL_INTERVAL);
    };

    for pump in pumps {
        let _ = pump.join();
    }

    if status.success() {
        Ok(())
    } else {
        Err(format!("`amplihack update` exited with {status}"))
    }
}

/// Read `stream` line by line, stamping `last_activity` and forwarding each line
/// to `debug` tracing so an operator can watch the update progress.
fn spawn_line_pump<R>(
    stream: R,
    last_activity: Arc<AtomicU64>,
    start: Instant,
    which: &'static str,
) -> std::thread::JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(text) => {
                    let stamp = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                    last_activity.store(stamp, Ordering::Relaxed);
                    tracing::debug!(target: TRACE_TARGET, stream = which, line = %text, "amplihack update output");
                }
                Err(_) => break,
            }
        }
    })
}

/// Real UNIX-epoch-seconds clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl GateClock for SystemClock {
    fn now_epoch_secs(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }
}

/// Records the failure metric through [`crate::self_metrics::record_metric`]. A
/// metrics write failure is logged (never panics the gate) — the warn/error
/// trace from the gate is the primary signal regardless.
#[derive(Debug, Clone, Copy, Default)]
pub struct SelfMetricsSink;

impl MetricSink for SelfMetricsSink {
    fn record(&self, name: &str, value: f64, context: &str) {
        if let Err(e) = crate::self_metrics::record_metric(name, value, context) {
            tracing::warn!(
                target: TRACE_TARGET,
                metric = name,
                error = %e,
                "failed to record amplihack update failure metric",
            );
        }
    }
}

/// Run the freshness gate against `state_root` with the production seams,
/// reading configuration from the environment. Used by the daemon startup pass
/// (which owns its resolved state root) and, indirectly, by the spawn path.
pub fn ensure_amplihack_fresh_in(state_root: &Path) -> GateOutcome {
    let config = GateConfig::from_env();
    run_freshness_gate(
        state_root,
        &config,
        &RealUpdater::default(),
        &SystemClock,
        &SelfMetricsSink,
    )
}

/// Run the freshness gate immediately before an engineer spawn, resolving the
/// state root the same way the rest of Simard does
/// ([`crate::state_root::simard_state_root`]).
pub fn ensure_amplihack_fresh() -> GateOutcome {
    ensure_amplihack_fresh_in(&crate::state_root::simard_state_root())
}
