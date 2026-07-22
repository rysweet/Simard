//! Post-deploy health probe (`simard self-health`): the gate between "swapped"
//! and "done". The report is healthy only when **every** probe is healthy; any
//! single failure fails the health check and triggers rollback.
//!
//! See `docs/concepts/reconcile-and-self-deploy.md` ("What 'healthy' means") and
//! `docs/reference/self-deploy-api.md#self-health-output`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;

/// Probe: the running build commit/version advanced to ≥ the target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionAdvancedProbe {
    pub healthy: bool,
    pub running: String,
    pub target: String,
}

/// Probe: cognitive-memory fact count ≥ the pre-deploy baseline (within tolerance).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryIntactProbe {
    pub healthy: bool,
    pub live_facts: u64,
    /// Baseline captured before the swap; `None` when no baseline was passed.
    pub baseline_facts: Option<u64>,
}

/// Probe: the goal board loads and the active-goal count is preserved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalBoardIntactProbe {
    pub healthy: bool,
    pub active_goals: usize,
}

/// Probe: zero `BrainJudgmentRecord.fallback == true` records over a probe cycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrainsLlmBackedProbe {
    pub healthy: bool,
    pub fallback_records: u64,
}

/// Probe: the cognitive-memory store quarantine flag is clear.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoQuarantineProbe {
    pub healthy: bool,
    pub quarantined: bool,
}

/// Probe: the PATH-resolved `simard` is the installed entrypoint (no stale
/// entrypoint / no foreign shadow). Distinct from `VersionAdvancedProbe`, which
/// compares git build commits; this asserts path identity against
/// `$SIMARD_HOME/bin/simard` and compares the binary's own `--version` string.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntrypointParityProbe {
    pub healthy: bool,
    /// Installed binary's own `--version`: `format!("simard {}", CARGO_PKG_VERSION)`.
    pub installed_version: String,
    /// `--version` reported by the `simard` resolved on `PATH`. Empty when
    /// `simard` could not be resolved or executed.
    pub path_version: String,
    /// The PATH-resolved `simard` path. Empty when unresolved.
    pub resolved_path: String,
    /// The canonicalized (`readlink -f`) target of `resolved_path`. Must equal
    /// `$SIMARD_HOME/bin/simard` for the probe to be healthy. Empty when unresolved.
    pub canonical_path: String,
    /// `true` when `canonical_path` does not equal the installed
    /// `$SIMARD_HOME/bin/simard` — a stale file or foreign shadow occupies PATH
    /// even if the version strings happen to match. Always fails the probe.
    pub path_mismatch: bool,
    /// `true` when a foreign (non-installer-owned) `simard` occupies the
    /// entrypoint path. Always fails the probe.
    pub foreign_shadow: bool,
}

/// The six post-deploy probes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfHealthProbes {
    pub version_advanced: VersionAdvancedProbe,
    pub memory_intact: MemoryIntactProbe,
    pub goal_board_intact: GoalBoardIntactProbe,
    pub brains_llm_backed: BrainsLlmBackedProbe,
    pub no_quarantine: NoQuarantineProbe,
    /// Additive field: an older orchestrator deserializing a newer report
    /// defaults this to the fail-closed (`healthy: false`) value.
    #[serde(default)]
    pub entrypoint_parity: EntrypointParityProbe,
}

impl SelfHealthProbes {
    /// `true` when every probe is healthy.
    pub fn all_healthy(&self) -> bool {
        self.version_advanced.healthy
            && self.memory_intact.healthy
            && self.goal_board_intact.healthy
            && self.brains_llm_backed.healthy
            && self.no_quarantine.healthy
            && self.entrypoint_parity.healthy
    }
}

/// The structured post-deploy health report. `healthy` is the logical AND of
/// every probe's `healthy`. Serialized verbatim by `simard self-health --json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfHealthReport {
    pub healthy: bool,
    pub probes: SelfHealthProbes,
}

impl SelfHealthReport {
    /// Assemble a report from probes, computing the top-level `healthy` as the
    /// logical AND of every probe (the documented invariant).
    pub fn compute(probes: SelfHealthProbes) -> Self {
        let healthy = probes.all_healthy();
        Self { healthy, probes }
    }

    /// Convenience accessor mirroring [`SelfHealthProbes::all_healthy`].
    pub fn is_healthy(&self) -> bool {
        self.healthy
    }
}

/// The build commit embedded in the **running** binary (set by `build.rs`).
fn running_commit() -> &'static str {
    env!("SIMARD_GIT_HASH")
}

/// Commit equality tolerant of abbreviation: equal, or one is a case-insensitive
/// prefix of the other (so an abbreviated target like `deadbeef` matches a full
/// running SHA `deadbeef…`). An `"unknown"` running commit never matches.
fn commits_compatible(running: &str, target: &str) -> bool {
    if running.is_empty() || target.is_empty() || running == "unknown" {
        return false;
    }
    let (r, t) = (running.to_ascii_lowercase(), target.to_ascii_lowercase());
    r == t || r.starts_with(&t) || t.starts_with(&r)
}

/// `true` for a quarantined corrupt cognitive-memory filename. Mirrors
/// `cmd_cleanup::disk::is_corrupt_quarantine_name`: both backend generations
/// leave a `cognitive*.corrupt-<ts>` artifact when a store is quarantined.
fn is_corrupt_quarantine_name(name: &str) -> bool {
    (name.starts_with("cognitive.") || name.starts_with("cognitive_memory."))
        && name.contains(".corrupt-")
}

/// Classification of a `cognitive*.corrupt-<ts>` artifact by file age (#4471).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuarantineClass {
    /// Freshly quarantined store (age < [`ACTIVE_QUARANTINE_MAX_AGE`]): a live
    /// corruption fault that MUST fail the probe and block self-deploy.
    Active,
    /// Aged forensic recovery artifact (age ≥ [`ACTIVE_QUARANTINE_MAX_AGE`]),
    /// retained by policy (#2420 / #2550). NOT a live fault; must NOT fail the
    /// probe and must NOT be deleted early.
    RetainedRecovery,
}

/// Age boundary between active corruption and a retained forensic artifact.
/// Strictly shorter than `disk::CORRUPT_DB_MAX_AGE_DAYS` (30d) so the two never
/// latch on the same boundary.
const ACTIVE_QUARANTINE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Classify one quarantine artifact by the age of its file metadata (#4471).
///
/// Fail-CLOSED: if the modified-time cannot be read, is in the future, or the
/// metadata errors, the artifact is treated as [`QuarantineClass::Active`] — the
/// safe verdict that blocks self-deploy rather than masking a live corruption.
/// Reads metadata ONLY; never opens, moves, or deletes the file.
fn classify_quarantine(path: &std::path::Path, now: std::time::SystemTime) -> QuarantineClass {
    let mtime = match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => t,
        // Missing/unreadable metadata ⇒ fail closed to Active.
        Err(_) => return QuarantineClass::Active,
    };
    match now.duration_since(mtime) {
        // A future mtime (mtime after `now`) yields Err ⇒ fail closed to Active.
        Err(_) => QuarantineClass::Active,
        Ok(age) if age >= ACTIVE_QUARANTINE_MAX_AGE => QuarantineClass::RetainedRecovery,
        Ok(_) => QuarantineClass::Active,
    }
}

/// Count ONLY [`QuarantineClass::Active`] artifacts under `state_root`, using
/// the injected `now` as the age reference (hermetic seam). Retained forensic
/// artifacts are excluded. Absent/unreadable dir ⇒ `0`. Directory-confined; does
/// not follow symlinks (SR-V4).
fn count_active_quarantine_files_at(
    state_root: &std::path::Path,
    now: std::time::SystemTime,
) -> u64 {
    let entries = match std::fs::read_dir(state_root) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    entries
        .flatten()
        .filter(|e| is_corrupt_quarantine_name(&e.file_name().to_string_lossy()))
        .filter(|e| classify_quarantine(&e.path(), now) == QuarantineClass::Active)
        .count() as u64
}

/// Count active-corruption quarantine artifacts under `state_root` as of now.
/// Thin wrapper over [`count_active_quarantine_files_at`] with the wall clock.
fn count_active_quarantine_files(state_root: &std::path::Path) -> u64 {
    count_active_quarantine_files_at(state_root, std::time::SystemTime::now())
}

/// Run the post-deploy probes against the live daemon and assemble a report.
///
/// Effectful: reads the running build commit, the live memory fact count, the
/// goal board, recent `brain_parse_failure` metrics, and the store quarantine
/// state. Every probe degrades to `healthy: false` on its own error rather than
/// aborting the whole report, so the orchestrator always gets a verdict to act
/// on (and rolls back on any unhealthy probe).
///
/// * `target_commit` — the commit the candidate was built from.
/// * `baseline_facts` — pre-deploy memory count (the orchestrator captures it);
///   `None` disables the comparison (the probe reports the live count only).
/// * `memory_count_tolerance` — allowed shortfall of `live_facts` below
///   `baseline_facts` before the memory probe fails.
/// * `fallback_window_start` — only `brain_parse_failure` metrics at/after this
///   instant count toward the "brains LLM-backed" probe, so historical failures
///   from the *previous* binary never fail a fresh deploy.
pub fn run_self_health_probe(
    mem: &dyn CognitiveMemoryOps,
    target_commit: &str,
    baseline_facts: Option<u64>,
    memory_count_tolerance: u64,
    fallback_window_start: DateTime<Utc>,
) -> SimardResult<SelfHealthReport> {
    let state_root = crate::state_root::simard_state_root();

    // Probe 1: version advanced to (>=) the target commit.
    let running = running_commit().to_string();
    let version_advanced = VersionAdvancedProbe {
        healthy: commits_compatible(&running, target_commit),
        running,
        target: target_commit.to_string(),
    };

    // Probe 2: memory fact count intact within tolerance.
    let memory_intact = match mem.get_statistics() {
        Ok(stats) => {
            let live = stats.total();
            let healthy = match baseline_facts {
                Some(baseline) => live + memory_count_tolerance >= baseline,
                None => true,
            };
            MemoryIntactProbe {
                healthy,
                live_facts: live,
                baseline_facts,
            }
        }
        Err(_) => MemoryIntactProbe {
            healthy: false,
            live_facts: 0,
            baseline_facts,
        },
    };

    // Probe 3: the goal board still loads.
    let goal_board_intact = match crate::goal_curation::load_goal_board(mem) {
        Ok(board) => GoalBoardIntactProbe {
            healthy: true,
            active_goals: board.active.len(),
        },
        Err(_) => GoalBoardIntactProbe {
            healthy: false,
            active_goals: 0,
        },
    };

    // Probe 4: zero brain parse-failures since the deploy window opened.
    let fallback_records =
        crate::self_metrics::query_metrics("brain_parse_failure", Some(fallback_window_start))
            .map(|v| v.len() as u64)
            .unwrap_or(0);
    let brains_llm_backed = BrainsLlmBackedProbe {
        healthy: fallback_records == 0,
        fallback_records,
    };

    // Probe 5: no ACTIVE quarantined corrupt cognitive-memory store. Retained
    // forensic recovery artifacts (#2420/#2550, aged ≥ 24h) are NOT active
    // faults and must not freeze self-deploy (#4469/#4471).
    let quarantined = count_active_quarantine_files(&state_root) > 0;
    let no_quarantine = NoQuarantineProbe {
        healthy: !quarantined,
        quarantined,
    };

    // Probe 6: the PATH-resolved `simard` is the installed entrypoint.
    let entrypoint_parity = probe_entrypoint_parity();

    Ok(SelfHealthReport::compute(SelfHealthProbes {
        version_advanced,
        memory_intact,
        goal_board_intact,
        brains_llm_backed,
        no_quarantine,
        entrypoint_parity,
    }))
}

/// Resolve the installed versioned binary path: `$SIMARD_HOME/bin/simard`, or
/// `$HOME/.simard/bin/simard` when `SIMARD_HOME` is unset.
fn resolve_installed_binary() -> Option<std::path::PathBuf> {
    if let Some(home) = std::env::var_os("SIMARD_HOME") {
        return Some(std::path::PathBuf::from(home).join("bin").join("simard"));
    }
    std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".simard/bin/simard"))
}

/// Resolve `simard` on `PATH` (first executable match), mirroring shell lookup.
fn resolve_simard_on_path() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join("simard");
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &std::path::Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
        _ => false,
    }
}

/// Run `<path> --version` (argv-only, no shell) and return the trimmed first
/// line of stdout, or an empty string on any failure.
fn version_string_of(path: &std::path::Path) -> String {
    match std::process::Command::new(path).arg("--version").output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}

/// Effectful probe: resolve `simard` on PATH, canonicalize it, exec `--version`,
/// and evaluate parity against the installed binary. Never calls the daemon.
fn probe_entrypoint_parity() -> EntrypointParityProbe {
    let installed_version = format!("simard {}", env!("CARGO_PKG_VERSION"));

    let installed_binary = resolve_installed_binary();
    let installed_canonical = installed_binary
        .as_deref()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.display().to_string())
        .or_else(|| installed_binary.as_ref().map(|p| p.display().to_string()))
        .unwrap_or_default();

    let resolved = resolve_simard_on_path();
    let resolved_path = resolved
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let canonical_path = resolved
        .as_deref()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let path_version = resolved
        .as_deref()
        .map(version_string_of)
        .unwrap_or_default();

    evaluate_entrypoint_parity(
        installed_version,
        installed_canonical,
        resolved_path,
        canonical_path,
        path_version,
    )
}

/// Pure parity evaluation: healthy iff a `simard` was resolved on PATH, it
/// canonicalizes to the installed binary (path identity), its `--version`
/// equals the installed version, and it is not a foreign shadow.
fn evaluate_entrypoint_parity(
    installed_version: String,
    installed_canonical: String,
    resolved_path: String,
    canonical_path: String,
    path_version: String,
) -> EntrypointParityProbe {
    let has_resolved = !resolved_path.is_empty();
    let has_version = !path_version.is_empty();
    let path_mismatch = canonical_path.is_empty()
        || installed_canonical.is_empty()
        || canonical_path != installed_canonical;
    let is_ours_banner = path_version.starts_with("simard ") || path_version == "simard";
    let foreign_shadow = has_resolved && (!has_version || !is_ours_banner);
    let version_match = has_version && path_version == installed_version;
    let healthy = has_resolved && has_version && !path_mismatch && version_match && !foreign_shadow;

    EntrypointParityProbe {
        healthy,
        installed_version,
        path_version,
        resolved_path,
        canonical_path,
        path_mismatch,
        foreign_shadow,
    }
}

#[cfg(test)]
mod probe_logic_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn commits_compatible_exact_and_prefix() {
        assert!(commits_compatible("deadbeefcafe", "deadbeefcafe"));
        assert!(commits_compatible("deadbeefcafe", "deadbeef")); // abbreviated target
        assert!(commits_compatible("DEADBEEF", "deadbeefcafe")); // case-insensitive
    }

    #[test]
    fn commits_incompatible_when_divergent_or_unknown() {
        assert!(!commits_compatible("deadbeef", "feedface"));
        assert!(!commits_compatible("unknown", "deadbeef"));
        assert!(!commits_compatible("", "deadbeef"));
        assert!(!commits_compatible("deadbeef", ""));
    }

    #[test]
    fn entrypoint_parity_healthy_on_path_identity_and_version_match() {
        let probe = evaluate_entrypoint_parity(
            "simard 0.35.0".to_string(),
            "/home/you/.simard/bin/simard".to_string(),
            "/home/you/.local/bin/simard".to_string(),
            "/home/you/.simard/bin/simard".to_string(),
            "simard 0.35.0".to_string(),
        );
        assert!(probe.healthy);
        assert!(!probe.path_mismatch);
        assert!(!probe.foreign_shadow);
    }

    #[test]
    fn entrypoint_parity_fails_on_version_skew() {
        // Path identity holds but the PATH binary reports an older version.
        let probe = evaluate_entrypoint_parity(
            "simard 0.35.0".to_string(),
            "/home/you/.simard/bin/simard".to_string(),
            "/home/you/.local/bin/simard".to_string(),
            "/home/you/.simard/bin/simard".to_string(),
            "simard 0.31.0".to_string(),
        );
        assert!(!probe.healthy, "version skew must fail the probe");
        assert!(!probe.foreign_shadow);
    }

    #[test]
    fn entrypoint_parity_fails_on_stale_file_same_version() {
        // The stale-file bug: version string matches but the resolved binary is
        // a different file — path identity catches what a version check misses.
        let probe = evaluate_entrypoint_parity(
            "simard 0.35.0".to_string(),
            "/home/you/.simard/bin/simard".to_string(),
            "/home/you/.local/bin/simard".to_string(),
            "/home/you/.local/bin/simard".to_string(),
            "simard 0.35.0".to_string(),
        );
        assert!(
            !probe.healthy,
            "path mismatch must fail even with matching version"
        );
        assert!(probe.path_mismatch);
    }

    #[test]
    fn entrypoint_parity_flags_foreign_shadow() {
        let probe = evaluate_entrypoint_parity(
            "simard 0.35.0".to_string(),
            "/home/you/.simard/bin/simard".to_string(),
            "/home/you/.local/bin/simard".to_string(),
            "/home/you/.local/bin/simard".to_string(),
            "othertool 1.2.3".to_string(),
        );
        assert!(!probe.healthy);
        assert!(
            probe.foreign_shadow,
            "non-`simard ` banner is a foreign shadow"
        );
    }

    #[test]
    fn entrypoint_parity_fails_when_unresolved() {
        let probe = evaluate_entrypoint_parity(
            "simard 0.35.0".to_string(),
            "/home/you/.simard/bin/simard".to_string(),
            String::new(),
            String::new(),
            String::new(),
        );
        assert!(!probe.healthy, "no `simard` on PATH must fail the probe");
    }

    #[test]
    fn entrypoint_parity_default_is_fail_closed() {
        assert!(!EntrypointParityProbe::default().healthy);
    }

    // ── quarantine classification (#4469 / #4471) ──────────────────────────
    // The old probe failed on ANY `cognitive*.corrupt-*` artifact, so retained
    // forensic recovery snapshots (#2420 / #2550, kept 30 days) held
    // `NoQuarantineProbe.healthy == false` for up to a month even on a fully
    // healthy build — freezing self-deploy and letting DeployDrift climb. The
    // classifier distinguishes ACTIVE corruption (<24h) from RETAINED forensic
    // artifacts (≥24h). Age logic is tested through an injected `now` so the
    // tests are hermetic (no mtime backdating, no clock coupling).

    use std::time::{Duration, SystemTime};

    /// A quarantine artifact created "now"; `classify_quarantine` is then probed
    /// with an injected reference time to place it in an age band deterministically.
    fn write_quarantine_file(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"corrupt-store-bytes").unwrap();
        path
    }

    #[test]
    fn classify_quarantine_recent_is_active() {
        let dir = tempdir().unwrap();
        let path = write_quarantine_file(dir.path(), "cognitive.corrupt-20260722");
        // Reference time == roughly the file's own mtime ⇒ age ≈ 0 < 24h.
        assert_eq!(
            classify_quarantine(&path, SystemTime::now()),
            QuarantineClass::Active,
            "a freshly created quarantine artifact is ACTIVE corruption"
        );
    }

    #[test]
    fn classify_quarantine_aged_is_retained_recovery() {
        let dir = tempdir().unwrap();
        let path = write_quarantine_file(dir.path(), "cognitive_memory.corrupt-20260601");
        // Look at the file from 48h in the future ⇒ age ≥ 24h ⇒ retained.
        let now = SystemTime::now() + Duration::from_secs(48 * 3600);
        assert_eq!(
            classify_quarantine(&path, now),
            QuarantineClass::RetainedRecovery,
            "an artifact older than ACTIVE_QUARANTINE_MAX_AGE is a retained forensic asset"
        );
    }

    #[test]
    fn classify_quarantine_boundary_uses_24h_threshold() {
        // Sanity-check the threshold constant is exactly 24h so the Active /
        // Retained boundary never overlaps the 30-day disk-retention window.
        assert_eq!(ACTIVE_QUARANTINE_MAX_AGE, Duration::from_secs(24 * 3600));

        let dir = tempdir().unwrap();
        let path = write_quarantine_file(dir.path(), "cognitive.corrupt-boundary");
        // Just under the threshold ⇒ Active; just over ⇒ Retained.
        let just_under = SystemTime::now() + (ACTIVE_QUARANTINE_MAX_AGE - Duration::from_secs(60));
        let just_over = SystemTime::now() + (ACTIVE_QUARANTINE_MAX_AGE + Duration::from_secs(60));
        assert_eq!(
            classify_quarantine(&path, just_under),
            QuarantineClass::Active
        );
        assert_eq!(
            classify_quarantine(&path, just_over),
            QuarantineClass::RetainedRecovery
        );
    }

    #[test]
    fn classify_quarantine_future_mtime_fails_closed_active() {
        let dir = tempdir().unwrap();
        let path = write_quarantine_file(dir.path(), "cognitive.corrupt-future");
        // Reference time BEFORE the file's mtime ⇒ negative age / duration error
        // ⇒ fail-closed to Active (block self-deploy, never mask live corruption).
        let now = SystemTime::now() - Duration::from_secs(2 * 3600);
        assert_eq!(
            classify_quarantine(&path, now),
            QuarantineClass::Active,
            "an unreadable/future/erroring mtime must classify as Active (fail-closed)"
        );
    }

    #[test]
    fn classify_quarantine_missing_file_fails_closed_active() {
        // A metadata error (missing file) fails closed to Active.
        assert_eq!(
            classify_quarantine(
                std::path::Path::new("/no-such-quarantine-file-xyz-123"),
                SystemTime::now()
            ),
            QuarantineClass::Active,
        );
    }

    #[test]
    fn count_active_excludes_retained_only_directory() {
        let dir = tempdir().unwrap();
        write_quarantine_file(dir.path(), "cognitive.corrupt-20260601");
        write_quarantine_file(dir.path(), "cognitive_memory.corrupt-20260515");
        // Viewed from 48h in the future, every artifact is aged (retained).
        let now = SystemTime::now() + Duration::from_secs(48 * 3600);
        assert_eq!(
            count_active_quarantine_files_at(dir.path(), now),
            0,
            "retained-only directory must have ZERO active artifacts (probe stays healthy)"
        );
    }

    #[test]
    fn count_active_counts_recent_corruption() {
        let dir = tempdir().unwrap();
        write_quarantine_file(dir.path(), "cognitive.corrupt-now-a");
        write_quarantine_file(dir.path(), "cognitive_memory.corrupt-now-b");
        // Non-quarantine files must never be counted.
        std::fs::write(dir.path().join("cognitive.db"), b"live").unwrap();
        std::fs::write(dir.path().join("unrelated.corrupt-1"), b"x").unwrap();
        assert_eq!(
            count_active_quarantine_files_at(dir.path(), SystemTime::now()),
            2,
            "two fresh quarantine artifacts are both ACTIVE"
        );
    }

    #[test]
    fn count_active_missing_dir_is_zero() {
        assert_eq!(
            count_active_quarantine_files_at(
                std::path::Path::new("/no-such-dir-active-xyz"),
                SystemTime::now()
            ),
            0
        );
    }

    #[test]
    fn no_quarantine_verdict_healthy_when_only_retained() {
        // The probe wiring keys `quarantined` off the ACTIVE count. With only
        // retained artifacts, the verdict is healthy so DeployDrift can drain.
        let dir = tempdir().unwrap();
        write_quarantine_file(dir.path(), "cognitive.corrupt-old");
        let now = SystemTime::now() + Duration::from_secs(48 * 3600);
        let quarantined = count_active_quarantine_files_at(dir.path(), now) > 0;
        assert!(
            !quarantined,
            "retained-only ⇒ not quarantined ⇒ NoQuarantineProbe.healthy == true"
        );
    }

    #[test]
    fn public_count_active_is_zero_for_empty_dir() {
        // The public (now-injecting) wrapper agrees with the seam on an empty dir.
        let dir = tempdir().unwrap();
        assert_eq!(count_active_quarantine_files(dir.path()), 0);
    }

    #[test]
    fn filename_parity_health_vs_disk() {
        // The `is_corrupt_quarantine_name` predicate is mirrored in health.rs and
        // cmd_cleanup::disk — the two must agree byte-for-byte so the disk-cleanup
        // retention contract is never diverged by this fix.
        let fixtures = [
            "cognitive.corrupt-20260101",
            "cognitive_memory.corrupt-20260102",
            "cognitive.corrupt-",
            "cognitive.db",
            "cognitive_memory.wal",
            "unrelated.corrupt-1",
            "cognitive",
            "prefix-cognitive.corrupt-1",
            "",
        ];
        for name in fixtures {
            assert_eq!(
                is_corrupt_quarantine_name(name),
                crate::cmd_cleanup::disk::is_corrupt_quarantine_name(name),
                "health.rs and disk.rs must classify {name:?} identically"
            );
        }
    }

    #[test]
    fn classifier_never_mutates_files() {
        // SR-D1: the classifier reads metadata only. It must never delete, move,
        // or rewrite a `*.corrupt-*` forensic recovery artifact (#2420 / #2550).
        let dir = tempdir().unwrap();
        let a = write_quarantine_file(dir.path(), "cognitive.corrupt-keep-a");
        let b = write_quarantine_file(dir.path(), "cognitive_memory.corrupt-keep-b");
        let before_a = std::fs::read(&a).unwrap();
        let before_b = std::fs::read(&b).unwrap();

        // Exercise both age bands over the same directory.
        let _ = count_active_quarantine_files_at(dir.path(), SystemTime::now());
        let _ = count_active_quarantine_files_at(
            dir.path(),
            SystemTime::now() + Duration::from_secs(72 * 3600),
        );

        assert!(a.exists() && b.exists(), "no artifact may be deleted");
        assert_eq!(
            std::fs::read(&a).unwrap(),
            before_a,
            "artifact bytes must be untouched"
        );
        assert_eq!(
            std::fs::read(&b).unwrap(),
            before_b,
            "artifact bytes must be untouched"
        );
    }
}
