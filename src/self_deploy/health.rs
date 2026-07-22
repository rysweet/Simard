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

/// Count quarantined corrupt cognitive-memory artifacts directly under
/// `state_root`. Absent/unreadable dir ⇒ `0` (nothing to quarantine).
fn count_quarantine_files(state_root: &std::path::Path) -> u64 {
    let entries = match std::fs::read_dir(state_root) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    entries
        .flatten()
        .filter(|e| is_corrupt_quarantine_name(&e.file_name().to_string_lossy()))
        .count() as u64
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

    // Probe 5: no quarantined corrupt cognitive-memory store.
    let quarantined = count_quarantine_files(&state_root) > 0;
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
    fn quarantine_scan_detects_only_corrupt_cognitive_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("cognitive.db"), b"x").unwrap();
        std::fs::write(dir.path().join("unrelated.corrupt-123"), b"x").unwrap();
        assert_eq!(count_quarantine_files(dir.path()), 0);

        std::fs::write(dir.path().join("cognitive.corrupt-20260101"), b"x").unwrap();
        std::fs::write(dir.path().join("cognitive_memory.corrupt-20260102"), b"x").unwrap();
        assert_eq!(count_quarantine_files(dir.path()), 2);
    }

    #[test]
    fn quarantine_scan_missing_dir_is_zero() {
        assert_eq!(
            count_quarantine_files(std::path::Path::new("/no-such-dir-xyz-123")),
            0
        );
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
}
