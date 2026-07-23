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
    /// `true` when at least one corrupt-store quarantine in the live-store
    /// directory has an mtime at/after `fallback_window_start` (a fresh event).
    /// Retained historical snapshots (mtime before the window) never set this.
    pub quarantined: bool,
    /// Count of quarantines with mtime at/after `fallback_window_start` (the
    /// events that drive `quarantined` / failure). `quarantined ==
    /// (fresh_quarantines > 0)`.
    pub fresh_quarantines: u64,
    /// Count of quarantines present in the directory whose mtime is *before*
    /// the window — retained forensic snapshots the probe deliberately ignores.
    /// Lets operators distinguish "0 quarantines total" from "0 fresh + N
    /// retained" without shelling into the store directory.
    pub retained: u64,
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

/// Count quarantined corrupt cognitive-memory artifacts directly under
/// `state_root`, ignoring acknowledgement sidecars. Absent/unreadable dir ⇒ `0`
/// (nothing to quarantine).
///
/// A quarantine that carries a durable `.ack` sidecar (issue #4469) is treated
/// as "seen" and does NOT count — this is what lets a genuinely-stuck but
/// retained recovery asset clear the probe without deleting it. The `.ack`
/// sidecars themselves are never mistaken for quarantines, and a *fresh*
/// (unacknowledged) corruption event still counts because the marker is keyed
/// to the exact filename.
///
/// Test-only helper: total count of unacknowledged quarantined corrupt
/// cognitive-memory artifacts directly under `state_root`, regardless of
/// forensic-window age. Delegates to the production [`tally_quarantine_files`]
/// scan (summing *fresh* + *retained*) so the directory-scan and
/// acknowledgement logic lives in exactly one place — the test helper can never
/// drift from the production probe path (#4469 philosophy review S5). Compiled
/// only under `#[cfg(test)]`.
#[cfg(test)]
fn count_quarantine_files(state_root: &std::path::Path) -> u64 {
    // Any window start yields the same total: an artifact is either fresh
    // (mtime ≥ window) or retained (mtime < window), and this helper wants the
    // age-agnostic sum. Acknowledged artifacts and `.ack` sidecars are already
    // excluded by the production scan.
    let tally = tally_quarantine_files(state_root, Utc::now());
    tally.fresh + tally.retained
}

/// Tally quarantined corrupt cognitive-memory artifacts directly under `dir`,
/// split into *fresh* (mtime at/after `window_start`) and *retained* (mtime
/// before the window) counts, in a single directory scan.
///
/// This mirrors the `brains_llm_backed` probe's `fallback_window_start`
/// semantics (issue #4469): retained historical snapshots (the newest
/// `CORRUPT_DB_KEEP` forensic assets that cleanup deliberately keeps) predate
/// the deploy/observation window and must NOT be flagged as *current*
/// corruption, or the probe fails forever. A quarantine created at/after the
/// window is genuine post-deploy corruption and is still counted, so the probe
/// is not neutered into always passing.
///
/// Acknowledgement-aware (issue #4469): a quarantine carrying a durable `.ack`
/// sidecar is treated as "seen" and counts as neither fresh nor retained, so a
/// genuinely-stuck but acknowledged protected recovery asset can clear the
/// probe without being deleted. The `.ack` sidecar files themselves are never
/// counted as quarantines.
///
/// Absent/unreadable dir ⇒ `(0, 0)` (nothing to quarantine). Entries whose
/// mtime cannot be read are skipped entirely (fail-safe: never counted fresh,
/// and not surfaced as retained either since freshness is unknowable).
fn tally_quarantine_files(dir: &std::path::Path, window_start: DateTime<Utc>) -> QuarantineTally {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return QuarantineTally::default(),
    };
    let mut tally = QuarantineTally::default();
    for entry in entries.flatten() {
        // Borrow the lossy name instead of forcing a `String` per entry: most
        // entries fail the predicate below and are skipped, so avoid the
        // per-entry heap allocation.
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if !crate::cmd_cleanup::is_corrupt_quarantine_name(&name) {
            continue;
        }
        // #4469: `.ack` sidecars are never quarantines, and an acknowledged
        // artifact is "seen" — it counts as neither fresh nor retained so a
        // stuck protected recovery asset can converge without being deleted.
        if crate::self_deploy::quarantine_ack::is_ack_marker_name(&name)
            || crate::self_deploy::quarantine_ack::is_acknowledged(dir, &name)
        {
            continue;
        }
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if DateTime::<Utc>::from(mtime) >= window_start {
            tally.fresh += 1;
        } else {
            tally.retained += 1;
        }
    }
    tally
}

/// Fresh vs. retained quarantine counts from a single live-store directory scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct QuarantineTally {
    /// Quarantines with mtime at/after the window start (genuine fresh events).
    fresh: u64,
    /// Quarantines with mtime before the window (retained forensic snapshots).
    retained: u64,
}

/// Guarded autonomous auto-ack (#4469): if the #2550 protected recovery asset
/// under `state_root` is past the forensic window and not already acknowledged,
/// durably acknowledge it WITHOUT deleting the retained asset. Best-effort:
/// emits a structured tracing/OTel WARN and continues on any error (never
/// `print!`). Returns the acknowledged artifact basename when it fired, else
/// `None`.
///
/// The "protected recovery asset" selection and the forensic-window age gate are
/// single-sourced from [`crate::cmd_cleanup::disk`], so the probe and the cleanup
/// sweep can never disagree about which artifact is protected. Fresh corruption
/// (young, or not the protected asset) is never eligible and still reddens the
/// probe.
///
/// **What this does and does NOT do.** This is *defense-in-depth*, not the
/// primary convergence mechanism. Under the current callers the observation
/// window is recent ([`run_self_health_probe`] receives `now` from the
/// orchestrator, `now - 5m` from the operator CLI), so the aged protected asset
/// this path targets — mtime at least [`CORRUPT_DB_MAX_AGE_DAYS`] old — is
/// already counted `retained`, never `fresh`, and the `no_quarantine` probe
/// therefore already passes on it via the fresh-window semantics of
/// [`tally_quarantine_files`] (see
/// `no_quarantine_passes_with_only_historical_quarantines_in_state_dir`). The
/// auto-ack adds two guarantees on top: it drops the aged protected asset out of
/// the `retained` diagnostic, and it keeps the probe green even if a caller were
/// to pass an observation window *older* than the forensic age (which would
/// otherwise re-classify the aged asset as `fresh`). The escape hatch for a
/// genuinely-stuck *fresh* quarantine is the manual `--acknowledge-quarantine`
/// operator command, which the auto-ack deliberately does NOT replicate — this
/// path fires unattended and must never silence genuine fresh corruption.
///
/// Deliberate asymmetry with the manual
/// [`acknowledge`](crate::self_deploy::quarantine_ack::acknowledge) path: the
/// operator `--acknowledge-quarantine` CLI acknowledges *any* present, ackable
/// artifact the operator explicitly chooses, whereas this *automatic* probe path
/// narrows itself to the single aged #2550 protected recovery asset, precisely
/// because it fires unattended and must not silently acknowledge genuine fresh
/// corruption.
fn auto_ack_stuck_recovery_asset(state_root: &std::path::Path) -> Option<String> {
    let name = crate::cmd_cleanup::disk::aged_protected_recovery_asset(state_root)?;
    if crate::self_deploy::quarantine_ack::is_acknowledged(state_root, &name) {
        return None;
    }
    match crate::self_deploy::quarantine_ack::acknowledge(state_root, &name) {
        Ok(marker) => {
            // Both `artifact` and `marker` embed the untrusted quarantine
            // basename (the marker is `{name}.ack`); an on-disk filename may
            // contain a newline (a single `Component::Normal` on Unix), so log
            // BOTH via Debug (`?`) — which escapes control chars — to prevent
            // log-line forgery under the default non-JSON subscriber (#4469
            // security review, LOW-1/LOW-2).
            tracing::warn!(
                artifact = ?name,
                marker = ?marker,
                min_age_days = crate::cmd_cleanup::disk::CORRUPT_DB_MAX_AGE_DAYS,
                "self_deploy.quarantine.auto_ack: durably acknowledged aged #2550 \
                 protected recovery asset as defense-in-depth (#4469); artifact \
                 retained on disk"
            );
            Some(name)
        }
        Err(e) => {
            tracing::warn!(
                artifact = ?name,
                error = %e,
                "self_deploy.quarantine.auto_ack_failed: could not acknowledge aged \
                 protected recovery asset (#4469)"
            );
            None
        }
    }
}

/// Run the post-deploy probes against the live daemon and assemble a report.
///
/// Effectful: reads the running build commit, the live memory fact count, the
/// goal board, recent `brain_parse_failure` metrics, and the store quarantine
/// state. The `no_quarantine` probe additionally has an intentional *write*
/// side-effect — it may durably write one `.ack` sidecar via the guarded
/// `auto_ack_stuck_recovery_asset` auto-ack (aged #2550 protected recovery
/// asset only; #4469) so the deadlock can self-clear on the unattended
/// post-deploy path. Every probe degrades to `healthy: false` on its own error
/// rather than aborting the whole report, so the orchestrator always gets a
/// verdict to act on (and rolls back on any unhealthy probe).
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

    // Probe 5: no *fresh* quarantined corrupt cognitive-memory store. Scans the
    // SAME directory set the cleanup sweep reclaims — the top-level state root
    // AND the live-store subdir `<state_root>/state/` (where LadybugDB drops
    // corrupt snapshots next to the live `cognitive` store) — single-sourced
    // from `state_root::quarantine_scan_dirs`, so the probe and
    // `cmd_cleanup::disk` can never disagree on where quarantines live. Only
    // quarantines at/after the window start count as fresh, so retained
    // historical forensic snapshots don't fail the probe forever, but genuine
    // post-deploy corruption still does (issue #4469).
    //
    // #4469: before counting, run the guarded autonomous auto-ack against each
    // of those directories as *defense-in-depth*. The `no_quarantine` probe
    // already converges on retained (old) quarantines via the fresh-window
    // semantics below — only quarantines with mtime at/after `window_start`
    // count as fresh. The auto-ack additionally acknowledges the #2550 protected
    // recovery asset once it ages past the forensic window (durable `.ack`
    // sidecar, WITHOUT deleting it) so it drops out of the `retained` diagnostic
    // and the probe stays green even if a caller passes an observation window
    // older than the forensic age. It lives on the probe path (not just the
    // operator CLI) so it also fires for the orchestrator's unattended
    // post-deploy health check. Fresh corruption is never eligible for auto-ack;
    // a genuinely-stuck *fresh* quarantine is cleared by the operator's manual
    // `--acknowledge-quarantine` command.
    let mut quarantine_tally = QuarantineTally::default();
    for dir in crate::state_root::quarantine_scan_dirs() {
        // Best-effort guarded auto-ack: the return value (which artifact, if any,
        // was acked) is intentionally discarded here — success/failure is logged
        // internally via structured tracing/OTel WARN inside the helper, and the
        // subsequent tally re-scans the directory to reflect any new `.ack`.
        let _ = auto_ack_stuck_recovery_asset(&dir);
        let dir_tally = tally_quarantine_files(&dir, fallback_window_start);
        quarantine_tally.fresh += dir_tally.fresh;
        quarantine_tally.retained += dir_tally.retained;
    }
    let quarantined = quarantine_tally.fresh > 0;
    let no_quarantine = NoQuarantineProbe {
        healthy: !quarantined,
        quarantined,
        fresh_quarantines: quarantine_tally.fresh,
        retained: quarantine_tally.retained,
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

    /// Set a path's mtime to an absolute instant (test-only helper).
    fn set_mtime(path: &std::path::Path, when: std::time::SystemTime) {
        let times = std::fs::FileTimes::new().set_modified(when);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(times)
            .unwrap();
    }

    /// Root Cause A (issue #4469): only corrupt cognitive-memory artifacts whose
    /// mtime is at/after the window start are "fresh". Corrupt-name detection is
    /// unchanged from the old count; the window filter is the new behavior that
    /// lets the probe distinguish current corruption from retained snapshots.
    #[test]
    fn fresh_quarantine_scan_counts_only_corrupt_cognitive_files_within_window() {
        let dir = tempdir().unwrap();
        // A window that opened well in the past: files created "now" are fresh.
        let window = Utc::now() - chrono::Duration::seconds(300);

        std::fs::write(dir.path().join("cognitive.db"), b"x").unwrap();
        std::fs::write(dir.path().join("unrelated.corrupt-123"), b"x").unwrap();
        // Non-corrupt / non-cognitive names never count, even when fresh.
        assert_eq!(tally_quarantine_files(dir.path(), window).fresh, 0);

        std::fs::write(dir.path().join("cognitive.corrupt-20260101"), b"x").unwrap();
        std::fs::write(dir.path().join("cognitive_memory.corrupt-20260102"), b"x").unwrap();
        assert_eq!(tally_quarantine_files(dir.path(), window).fresh, 2);
    }

    /// Root Cause A, acceptance #1: retained *historical* snapshots (mtime before
    /// the window) are forensic recovery assets, NOT evidence of current
    /// corruption. The probe must ignore them — this is what lets the permanently
    /// failing `no_quarantine` probe finally clear. Mirrors the
    /// `brains_llm_backed` `fallback_window_start` semantics.
    #[test]
    fn fresh_quarantine_scan_ignores_historical_snapshots_before_window() {
        let dir = tempdir().unwrap();
        // The window opens "now"; every snapshot below predates it.
        let window = Utc::now();

        // Simulate the CORRUPT_DB_KEEP retained forensic snapshots that cleanup
        // deliberately keeps, all aged well before the observation window.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
        for i in 0..5 {
            let p = dir.path().join(format!("cognitive.corrupt-{i:04}"));
            std::fs::write(&p, b"forensic").unwrap();
            set_mtime(&p, old);
        }
        let tally = tally_quarantine_files(dir.path(), window);
        assert_eq!(
            tally.fresh, 0,
            "retained snapshots older than the window must not be counted fresh"
        );
        assert_eq!(
            tally.retained, 5,
            "all 5 out-of-window snapshots must be surfaced as retained"
        );
    }

    /// Root Cause A, acceptance #2: a genuinely fresh quarantine created within
    /// the window still counts — the probe must not be neutered into always
    /// passing. A post-deploy corruption event must still FAIL the probe.
    #[test]
    fn fresh_quarantine_scan_counts_new_corruption_within_window() {
        let dir = tempdir().unwrap();
        let window = Utc::now() - chrono::Duration::seconds(60);

        // A historical retained snapshot (ignored) ...
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 24 * 3600);
        let hist = dir.path().join("cognitive.corrupt-0000");
        std::fs::write(&hist, b"old").unwrap();
        set_mtime(&hist, old);

        // ... plus a fresh post-window quarantine that must be caught.
        std::fs::write(dir.path().join("cognitive.corrupt-9999"), b"fresh").unwrap();

        let tally = tally_quarantine_files(dir.path(), window);
        assert_eq!(
            tally.fresh, 1,
            "a fresh quarantine inside the window must still be flagged"
        );
        assert_eq!(
            tally.retained, 1,
            "the historical snapshot must be surfaced as retained, not fresh"
        );
    }

    /// Root Cause A boundary: the window filter is an INCLUSIVE lower bound
    /// (`mtime >= window_start`). A quarantine whose mtime lands exactly on the
    /// window start is a fresh, post-deploy event and must be counted; one a
    /// single second earlier is a retained historical snapshot. This pins the
    /// exact edge the fresh/retained split hinges on — the existing tests only
    /// cover instants clearly inside or clearly before the window. A
    /// whole-second instant is used so filesystem mtime truncation can never
    /// blur the boundary and make the assertion flaky.
    #[test]
    fn fresh_quarantine_scan_window_start_is_inclusive_lower_bound() {
        let dir = tempdir().unwrap();
        let boundary = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let window = DateTime::<Utc>::from(boundary);

        // Exactly on the boundary ⇒ fresh (inclusive `>=`).
        let on = dir.path().join("cognitive.corrupt-onboundary");
        std::fs::write(&on, b"x").unwrap();
        set_mtime(&on, boundary);

        // One second before the boundary ⇒ retained.
        let before = dir.path().join("cognitive.corrupt-beforeboundary");
        std::fs::write(&before, b"x").unwrap();
        set_mtime(&before, boundary - std::time::Duration::from_secs(1));

        let tally = tally_quarantine_files(dir.path(), window);
        assert_eq!(
            tally.fresh, 1,
            "a quarantine whose mtime equals window_start must count as fresh"
        );
        assert_eq!(
            tally.retained, 1,
            "a quarantine one second before window_start must be retained"
        );
    }

    #[test]
    fn fresh_quarantine_scan_missing_dir_is_zero() {
        let tally =
            tally_quarantine_files(std::path::Path::new("/no-such-dir-xyz-123"), Utc::now());
        assert_eq!(tally.fresh, 0);
        assert_eq!(tally.retained, 0);
    }

    // ── #4469: acknowledgement-aware quarantine scan ──
    // The `no_quarantine` probe must stop failing on a quarantine that carries a
    // durable `.ack` sidecar, so a genuinely-stuck (but protected/retained)
    // corrupt store can clear without deleting the recovery asset.

    #[test]
    fn quarantine_scan_ignores_acknowledged_artifact_and_its_marker() {
        let dir = tempdir().unwrap();
        // A quarantined corrupt store that has been durably acknowledged.
        std::fs::write(dir.path().join("cognitive.corrupt-20260101"), b"x").unwrap();
        std::fs::write(dir.path().join("cognitive.corrupt-20260101.ack"), b"").unwrap();
        // An acked artifact does not count, and the `.ack` sidecar itself is
        // never mistaken for a quarantine.
        assert_eq!(
            count_quarantine_files(dir.path()),
            0,
            "acknowledged quarantine (and its marker) must not fail the probe"
        );
    }

    #[test]
    fn quarantine_scan_still_flags_fresh_corruption_after_ack() {
        let dir = tempdir().unwrap();
        // Old, acknowledged quarantine.
        std::fs::write(dir.path().join("cognitive.corrupt-20260101"), b"x").unwrap();
        std::fs::write(dir.path().join("cognitive.corrupt-20260101.ack"), b"").unwrap();
        // A NEW corruption event — filename-keyed markers must not silence it.
        std::fs::write(dir.path().join("cognitive.corrupt-20260202"), b"x").unwrap();
        assert_eq!(
            count_quarantine_files(dir.path()),
            1,
            "fresh corruption must re-fail the probe despite an earlier ack"
        );
    }

    #[test]
    fn quarantine_scan_end_to_end_ack_clears_probe() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("cognitive.corrupt-20260101120000"), b"x").unwrap();
        assert_eq!(
            count_quarantine_files(dir.path()),
            1,
            "unacked = quarantined"
        );

        crate::self_deploy::quarantine_ack::acknowledge(
            dir.path(),
            "cognitive.corrupt-20260101120000",
        )
        .expect("acknowledge succeeds");
        assert_eq!(
            count_quarantine_files(dir.path()),
            0,
            "durable ack clears the no_quarantine probe"
        );
    }

    // ── #4469: guarded autonomous auto-ack of the stuck recovery asset ──
    // The #2550 protected recovery asset (largest quarantine ≥ 1 MB) is retained
    // forever yet keeps `no_quarantine` red, so it can never clear on its own.
    // Once it ages past the forensic window the probe auto-acks it — and only it.

    /// Backdate a path's mtime `days` into the past (plus slack).
    fn backdate(path: &std::path::Path, days: u64) {
        let when =
            std::time::SystemTime::now() - std::time::Duration::from_secs(days * 24 * 3600 + 3600);
        let times = std::fs::FileTimes::new().set_modified(when);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(times)
            .unwrap();
    }

    const PROTECT_MIN: u64 = crate::cmd_cleanup::disk::CORRUPT_DB_PROTECT_MIN_BYTES;
    const MAX_AGE_DAYS: u64 = crate::cmd_cleanup::disk::CORRUPT_DB_MAX_AGE_DAYS;

    #[test]
    fn auto_ack_clears_aged_protected_recovery_asset_and_retains_it() {
        let dir = tempdir().unwrap();
        let asset = dir.path().join("cognitive.corrupt-20260101120000");
        std::fs::write(&asset, vec![0u8; (PROTECT_MIN + 512) as usize]).unwrap();
        backdate(&asset, MAX_AGE_DAYS + 1);

        // Before: the aged protected asset keeps the probe red.
        assert_eq!(
            count_quarantine_files(dir.path()),
            1,
            "unacked = quarantined"
        );

        let acked = auto_ack_stuck_recovery_asset(dir.path());
        assert_eq!(
            acked.as_deref(),
            Some("cognitive.corrupt-20260101120000"),
            "auto-ack must fire for the aged protected asset"
        );
        // After: probe clears, artifact retained, sidecar written.
        assert_eq!(
            count_quarantine_files(dir.path()),
            0,
            "auto-ack clears the probe"
        );
        assert!(asset.is_file(), "the recovery asset must be retained");
        assert!(
            dir.path()
                .join("cognitive.corrupt-20260101120000.ack")
                .is_file()
        );
    }

    #[test]
    fn auto_ack_ignores_fresh_protected_asset() {
        let dir = tempdir().unwrap();
        // Large enough to be "protected", but INSIDE the forensic window.
        let asset = dir.path().join("cognitive.corrupt-20260101120000");
        std::fs::write(&asset, vec![0u8; (PROTECT_MIN + 512) as usize]).unwrap();
        backdate(&asset, MAX_AGE_DAYS.saturating_sub(2));

        assert_eq!(
            auto_ack_stuck_recovery_asset(dir.path()),
            None,
            "fresh asset not eligible"
        );
        assert_eq!(
            count_quarantine_files(dir.path()),
            1,
            "fresh quarantine still reddens"
        );
    }

    #[test]
    fn auto_ack_ignores_trivial_aged_quarantine() {
        let dir = tempdir().unwrap();
        // Aged, but below the protection floor — not the recovery asset.
        let small = dir.path().join("cognitive.corrupt-20260101120000");
        std::fs::write(&small, b"tiny").unwrap();
        backdate(&small, MAX_AGE_DAYS + 5);

        assert_eq!(
            auto_ack_stuck_recovery_asset(dir.path()),
            None,
            "a trivial (sub-floor) quarantine is never auto-acked"
        );
        assert_eq!(count_quarantine_files(dir.path()), 1);
    }

    #[test]
    fn auto_ack_is_idempotent() {
        let dir = tempdir().unwrap();
        let asset = dir.path().join("cognitive.corrupt-20260101120000");
        std::fs::write(&asset, vec![0u8; (PROTECT_MIN + 512) as usize]).unwrap();
        backdate(&asset, MAX_AGE_DAYS + 1);

        assert!(
            auto_ack_stuck_recovery_asset(dir.path()).is_some(),
            "first pass acks"
        );
        assert_eq!(
            auto_ack_stuck_recovery_asset(dir.path()),
            None,
            "already acknowledged ⇒ no repeat ack"
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
