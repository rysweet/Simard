//! Core of the amplihack freshness gate — configuration, the injectable
//! decision seams, the cross-process lock, the durable last-success state file,
//! and the [`run_freshness_gate`] decision itself.
//!
//! Everything here is deterministic and dependency-injected: the caller supplies
//! an [`AmplihackUpdater`], a [`GateClock`], and a [`MetricSink`], so the gate is
//! unit-testable with fakes and **no real network, subprocess, or clock**. The
//! production wiring lives in [`super::runner`].
//!
//! The on-disk contract (lock file, state file schema, trace target, outcome
//! tokens, and the `amplihack_update_failure` metric) is the one documented in
//! `docs/reference/amplihack-freshness-gate.md`.

use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// `flock(2)` advisory lock file that serializes `amplihack update` across
/// processes. Lives directly under the resolved state root.
pub const UPDATE_LOCK_FILENAME: &str = "amplihack-update.lock";

/// Durable record of the last successful update, so the TTL survives across
/// spawns and process restarts. Lives directly under the resolved state root.
pub const UPDATE_STATE_FILENAME: &str = "amplihack-update-state.json";

/// `tracing` target for every gate decision. Part of the operator contract —
/// the how-to greps for it literally.
pub const TRACE_TARGET: &str = "simard::amplihack_update";

/// Metric name emitted on any update failure.
pub const FAILURE_METRIC: &str = "amplihack_update_failure";

/// Master switch. Default ON per operator directive; `0` disables the gate.
pub const ENV_ENABLED: &str = "SIMARD_ENGINEER_AMPLIHACK_UPDATE";

/// Dedup window in seconds. Default [`DEFAULT_TTL_SECS`].
pub const ENV_TTL: &str = "SIMARD_AMPLIHACK_UPDATE_TTL_SECS";

/// Strict mode. `1` hard-blocks the spawn when the update fails.
pub const ENV_REQUIRE_FRESH: &str = "SIMARD_REQUIRE_FRESH_AMPLIHACK";

/// Default TTL: a successful update within this many seconds is reused.
pub const DEFAULT_TTL_SECS: i64 = 300;

/// The decision the gate reached for a single evaluation. Exactly one is
/// recorded per gate run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    /// Update executed and succeeded; a fresh timestamp was written.
    Ran,
    /// A successful update is within the TTL; the update was skipped.
    SkippedFresh,
    /// The update ran (or infra failed) and failed; default proceeds on the
    /// last-known-good install.
    Failed,
    /// The update failed **and** strict mode is on; the spawn is refused.
    Blocked,
    /// The gate is disabled via `SIMARD_ENGINEER_AMPLIHACK_UPDATE=0`.
    Disabled,
}

impl GateOutcome {
    /// The contract token used in traces and operator tooling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            GateOutcome::Ran => "ran",
            GateOutcome::SkippedFresh => "skipped-fresh",
            GateOutcome::Failed => "failed",
            GateOutcome::Blocked => "blocked",
            GateOutcome::Disabled => "disabled",
        }
    }

    /// Whether the caller should proceed to spawn the engineer. Only a strict
    /// [`GateOutcome::Blocked`] stops the spawn — every other outcome
    /// (including a surfaced [`GateOutcome::Failed`]) proceeds on the current
    /// install. This is honest, surfaced degradation, not a silent fallback.
    #[must_use]
    pub fn should_spawn(self) -> bool {
        !matches!(self, GateOutcome::Blocked)
    }
}

/// Effective gate configuration for one evaluation.
#[derive(Debug, Clone, Copy)]
pub struct GateConfig {
    /// Master switch. Default ON.
    pub enabled: bool,
    /// Dedup window in seconds.
    pub ttl_secs: i64,
    /// Strict mode: block the spawn on update failure.
    pub require_fresh: bool,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl_secs: DEFAULT_TTL_SECS,
            require_fresh: false,
        }
    }
}

impl GateConfig {
    /// Read the configuration from the environment. Defaults match the operator
    /// directive: gate ON, TTL [`DEFAULT_TTL_SECS`], strict mode OFF.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            enabled: parse_enabled(std::env::var(ENV_ENABLED).ok().as_deref()),
            ttl_secs: parse_ttl(std::env::var(ENV_TTL).ok().as_deref()),
            require_fresh: parse_require_fresh(std::env::var(ENV_REQUIRE_FRESH).ok().as_deref()),
        }
    }
}

/// Parse the master switch. Default ON; `0`/`false`/`off`/`no` disable.
pub(crate) fn parse_enabled(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        None => true,
    }
}

/// Parse strict mode. Default OFF; `1`/`true`/`on`/`yes` enable.
pub(crate) fn parse_require_fresh(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "on" | "yes")
    )
}

/// Parse the TTL. Non-negative integer seconds; anything invalid or negative
/// falls back to [`DEFAULT_TTL_SECS`].
pub(crate) fn parse_ttl(raw: Option<&str>) -> i64 {
    raw.and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&n| n >= 0)
        .unwrap_or(DEFAULT_TTL_SECS)
}

// ─────────────────────────── injection seams ───────────────────────────────

/// Runs the `amplihack update` command. Injected so the gate is testable with a
/// fake; production is [`super::runner::RealUpdater`].
pub trait AmplihackUpdater {
    /// Run one `amplihack update`. `Ok(())` on success, `Err(msg)` on any
    /// failure (network/build/install), where `msg` is a human-readable cause.
    fn run_update(&self) -> Result<(), String>;
}

/// Wall-clock source in UNIX epoch seconds. Injected so tests are deterministic;
/// production is [`super::runner::SystemClock`].
pub trait GateClock {
    /// Current time as UNIX epoch seconds.
    fn now_epoch_secs(&self) -> i64;
}

/// Sink for the failure metric. Injected so tests assert the metric without the
/// global `metrics.jsonl` side effect; production is
/// [`super::runner::SelfMetricsSink`].
pub trait MetricSink {
    /// Record a single metric occurrence.
    fn record(&self, name: &str, value: f64, context: &str);
}

// ─────────────────────────── durable state ─────────────────────────────────

/// On-disk shape of [`UPDATE_STATE_FILENAME`]. Single-field by contract so
/// operator tooling can read `last_success_epoch_secs` directly.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct UpdateState {
    last_success_epoch_secs: i64,
}

/// Read the last-success timestamp under the state root. Returns `None` when the
/// file is absent, unreadable, or unparseable (treated as "no prior success").
pub(crate) fn read_last_success(state_root: &Path) -> Option<i64> {
    let path = state_root.join(UPDATE_STATE_FILENAME);
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<UpdateState>(&raw)
        .ok()
        .map(|s| s.last_success_epoch_secs)
}

/// Persist a new last-success timestamp atomically (temp file + rename) so a
/// concurrent reader never observes a torn write.
pub(crate) fn write_last_success(state_root: &Path, epoch_secs: i64) -> std::io::Result<()> {
    let path = state_root.join(UPDATE_STATE_FILENAME);
    let tmp = state_root.join(format!("{UPDATE_STATE_FILENAME}.tmp"));
    let body = serde_json::to_string(&UpdateState {
        last_success_epoch_secs: epoch_secs,
    })
    .map_err(std::io::Error::other)?;
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

// ─────────────────────────── cross-process lock ────────────────────────────

/// RAII holder of the exclusive `flock(2)` over [`UPDATE_LOCK_FILENAME`]. The
/// advisory lock is released on drop (and by the OS on process death), so a
/// crash can never strand it.
pub(crate) struct UpdateLock {
    #[cfg(unix)]
    file: std::fs::File,
}

impl UpdateLock {
    /// Acquire the exclusive lock, blocking until it is held. Concurrent
    /// spawners serialize here. On non-unix targets this is a no-op holder (the
    /// project ships unix-only; the `cfg` gate keeps cross-target lint/doc
    /// passes green).
    #[cfg(unix)]
    pub(crate) fn acquire(state_root: &Path) -> Result<Self, String> {
        use std::os::unix::io::AsRawFd;

        let path = state_root.join(UPDATE_LOCK_FILENAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("open lock file {}: {e}", path.display()))?;
        // SAFETY: `flock` is FFI but well-defined for a valid fd + LOCK_EX.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            return Err(format!(
                "flock(LOCK_EX) on {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { file })
    }

    #[cfg(not(unix))]
    pub(crate) fn acquire(_state_root: &Path) -> Result<Self, String> {
        Ok(Self {})
    }
}

#[cfg(unix)]
impl Drop for UpdateLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: releasing our own held advisory lock; ignore the result since
        // the OS also releases on fd close.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

// ─────────────────────────── the decision ──────────────────────────────────

/// Run the freshness gate for one engineer spawn (or the startup pass).
///
/// Algorithm (see the reference doc for the authoritative contract):
/// 1. gate disabled ⇒ trace `disabled`, return [`GateOutcome::Disabled`];
/// 2. acquire the cross-process lock (the serialization point);
/// 3. **under the lock**, re-read the last-success timestamp;
/// 4. within TTL ⇒ [`GateOutcome::SkippedFresh`];
/// 5. otherwise run the update; on success write the timestamp and return
///    [`GateOutcome::Ran`];
/// 6. on failure (including infra: unwritable root, lock error, timestamp write)
///    log at warn/error, record the [`FAILURE_METRIC`], then branch on strict
///    mode: default [`GateOutcome::Failed`] (proceed), strict
///    [`GateOutcome::Blocked`] (refuse).
///
/// The lock is released on every path. No failure is ever silent.
pub fn run_freshness_gate(
    state_root: &Path,
    config: &GateConfig,
    updater: &dyn AmplihackUpdater,
    clock: &dyn GateClock,
    metrics: &dyn MetricSink,
) -> GateOutcome {
    let gate_start = Instant::now();

    if !config.enabled {
        tracing::info!(
            target: TRACE_TARGET,
            outcome = "disabled",
            "amplihack freshness gate disabled (SIMARD_ENGINEER_AMPLIHACK_UPDATE=0); spawning on the current install",
        );
        return GateOutcome::Disabled;
    }

    if let Err(e) = std::fs::create_dir_all(state_root) {
        return record_failure(
            config,
            metrics,
            &format!("state root {} unwritable: {e}", state_root.display()),
            0,
            gate_ms(gate_start),
        );
    }

    // Acquiring the lock is the serialization point; an error here is an infra
    // failure, surfaced (never swallowed) through the same failure branch.
    let _lock = match UpdateLock::acquire(state_root) {
        Ok(lock) => lock,
        Err(e) => {
            return record_failure(
                config,
                metrics,
                &format!("lock acquisition failed: {e}"),
                0,
                gate_ms(gate_start),
            );
        }
    };

    // Re-read the timestamp **under the lock** so a burst of spawners cannot all
    // decide "stale" and each rebuild.
    if let Some(ts) = read_last_success(state_root) {
        let age = clock.now_epoch_secs() - ts;
        if age >= 0 && age <= config.ttl_secs {
            tracing::info!(
                target: TRACE_TARGET,
                outcome = "skipped-fresh",
                ttl_secs = config.ttl_secs,
                age_secs = age,
                gate_duration_ms = gate_ms(gate_start),
                "amplihack update within TTL; skipping",
            );
            return GateOutcome::SkippedFresh;
        }
    }

    let update_start = Instant::now();
    let result = updater.run_update();
    let update_ms = gate_ms(update_start);

    match result {
        Ok(()) => {
            let now = clock.now_epoch_secs();
            if let Err(e) = write_last_success(state_root, now) {
                return record_failure(
                    config,
                    metrics,
                    &format!("update succeeded but persisting the timestamp failed: {e}"),
                    update_ms,
                    gate_ms(gate_start),
                );
            }
            tracing::info!(
                target: TRACE_TARGET,
                outcome = "ran",
                ttl_secs = config.ttl_secs,
                update_duration_ms = update_ms,
                gate_duration_ms = gate_ms(gate_start),
                "amplihack update ran successfully; engineer will run on the fresh install",
            );
            GateOutcome::Ran
        }
        Err(e) => record_failure(config, metrics, &e, update_ms, gate_ms(gate_start)),
    }
}

/// Elapsed milliseconds since `start`, saturated into `u64`.
fn gate_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Surface an update failure: record the [`FAILURE_METRIC`] **and** emit a
/// warn/error trace (never one without the other), then resolve the strict-mode
/// branch. Used for real update failures and infra failures alike.
fn record_failure(
    config: &GateConfig,
    metrics: &dyn MetricSink,
    error: &str,
    update_duration_ms: u64,
    gate_duration_ms: u64,
) -> GateOutcome {
    if config.require_fresh {
        let context = format!(
            "amplihack update failed ({error}); spawn blocked (SIMARD_REQUIRE_FRESH_AMPLIHACK=1)"
        );
        metrics.record(FAILURE_METRIC, 1.0, &context);
        tracing::error!(
            target: TRACE_TARGET,
            outcome = "blocked",
            require_fresh = true,
            ttl_secs = config.ttl_secs,
            update_duration_ms,
            gate_duration_ms,
            error,
            "amplihack update failed and strict freshness is required; blocking engineer spawn",
        );
        GateOutcome::Blocked
    } else {
        let context =
            format!("amplihack update failed ({error}); proceeding on last-known-good install");
        metrics.record(FAILURE_METRIC, 1.0, &context);
        tracing::warn!(
            target: TRACE_TARGET,
            outcome = "failed",
            require_fresh = false,
            ttl_secs = config.ttl_secs,
            update_duration_ms,
            gate_duration_ms,
            error,
            "amplihack update failed; proceeding on last-known-good install (staleness surfaced, not silent)",
        );
        GateOutcome::Failed
    }
}
