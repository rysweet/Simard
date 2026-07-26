//! Agentic disk reclamation — the deterministic Rust core (issue #2704).
//!
//! An **untrusted agent proposes** a candidate list (via the `disk-reclaim.yaml`
//! recipe); a **deterministic Rust executor disposes** — re-validating every
//! candidate through a non-bypassable guard immediately before deletion. The
//! delete primitive exists **only** inside the executor; no public path deletes
//! without passing [`guard::vet_candidate`].
//!
//! See `docs/concepts/agentic-disk-reclamation.md` for the design rationale and
//! `docs/reference/disk-reclaim-api.md` for the full API contract.
//!
//! # Module layout
//! - [`candidate`] — the `ReclaimCandidate` serde contract + marker parser.
//! - [`guard`] — the non-bypassable rail: `vet_candidate` → `Verdict`.
//! - [`daemon_dir`] — the protected daemon-directory union.
//! - [`executor`] — the largest-first, threshold-stop, TOCTOU-reasserting disposer.
//! - [`recipe`] — invoke the analysis recipe; strict parse; no fallback.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::SimardResult;

pub mod candidate;
pub mod daemon_dir;
pub mod executor;
pub mod guard;
pub mod prod;
pub mod recipe;

pub mod build_cache;

pub use build_cache::{
    EVICTABLE_CACHE_DIRS, build_cache_candidates, build_cache_leaf_dirs, target_debug_roots,
};
pub use candidate::{CandidateKind, MAX_CANDIDATES, ReclaimCandidate, parse_candidates};
pub use daemon_dir::resolve_daemon_working_dirs;
pub use executor::{
    PathRemover, RealPathRemover, ReclaimFailure, ReclaimReport, RemovedPath, SkippedPath,
    exec_reclaim,
};
pub use guard::{
    CachingSizeMeasurer, DuSizeMeasurer, GuardContext, ProtectedDenySet, ReclaimPrimitive,
    RejectReason, SizeMeasurer, TrackedWorktreeProbe, Verdict, WorktreeVerdict, vet_candidate,
};
pub use prod::{DerivingPathRemover, RealTrackedWorktreeProbe, main_worktree_of};
pub use recipe::{RecipeInvoker, RecipeRunnerInvoker, resolve_recipe_path, run_reclaim_recipe};

/// The hardcoded daemon working directory that is **always** protected — even if
/// the live service is relocated. Removing it crash-loops the daemon with
/// `status=200/CHDIR`.
pub const HARDCODED_PROTECTED_MAIN: &str = "/home/azureuser/src/Simard/worktrees/main";

/// Env var driving the `%-used` reclamation trigger threshold.
pub const RECLAIM_PCT_ENV: &str = "SIMARD_DISK_RECLAIM_PCT";

/// Env var gating whether the **daemon** self-heal path may actually delete
/// (kept off until recipe-step sandboxing is verified in production).
pub const DAEMON_APPLY_ENV: &str = "SIMARD_DISK_RECLAIM_DAEMON_APPLY";

/// Default reclamation trigger threshold (`%-used`) when the env is unset.
pub const DEFAULT_RECLAIM_PCT: u8 = 85;

/// Whether a reclamation run may perform destructive operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReclaimMode {
    /// Full analysis + guard vetting, zero destructive ops (the default).
    DryRun,
    /// Guarded reclamation (refused as root).
    Apply,
}

/// The hardcoded managed-repo set the recipe enumerates. **Not**
/// operator-configurable free-form — operators widen the *deny*-set via
/// `SIMARD_GIT_PROTECTED_REPOS`, never the allow-set.
pub fn managed_repos() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/home/azureuser/src/Simard"),
        PathBuf::from("/home/azureuser/src/amplihack-rs"),
        PathBuf::from("/home/azureuser/src/amplihack-memory-lib"),
    ]
}

/// Compute the reclamation allow-roots (the positive containment scope) for a
/// given Simard state root:
/// - `<state_root>/engineer-worktrees` (the `~/.simard` engineer worktrees),
/// - `<repo>/worktrees` for each managed repo,
/// - the shared cargo target dirs under the state root.
pub fn allow_roots(state_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![state_root.join("engineer-worktrees")];
    for repo in managed_repos() {
        roots.push(repo.join("worktrees"));
    }
    roots.push(state_root.join("cargo-target"));
    roots.push(state_root.join("shared-target"));
    roots
}

/// Read [`RECLAIM_PCT_ENV`], defaulting to [`DEFAULT_RECLAIM_PCT`] and clamping
/// to `[1, 99]`.
pub fn reclaim_pct_from_env() -> u8 {
    std::env::var(RECLAIM_PCT_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u8>().ok())
        .map(|v| v.clamp(1, 99))
        .unwrap_or(DEFAULT_RECLAIM_PCT)
}

/// Read [`DAEMON_APPLY_ENV`]. Returns [`ReclaimMode::Apply`] **only** when set to
/// `1`/`true` (case-insensitive); otherwise [`ReclaimMode::DryRun`]. This keeps
/// the daemon self-heal trigger disabled until recipe-step sandboxing is
/// verified. Governs the daemon path only — the CLI derives its mode from
/// `--apply`, never from this variable.
pub fn daemon_apply_from_env() -> ReclaimMode {
    match std::env::var(DAEMON_APPLY_ENV)
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("1") | Some("true") | Some("TRUE") | Some("True") | Some("yes") | Some("on") => {
            ReclaimMode::Apply
        }
        _ => ReclaimMode::DryRun,
    }
}

/// Which surface launched a reclamation run. Emitted as the `source` telemetry
/// attribute so an operator dry-run cannot be mistaken for daemon activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReclaimSource {
    /// The self-healing daemon maintenance trigger.
    Daemon,
    /// An operator `simard disk-reclaim` invocation.
    Cli,
}

impl ReclaimSource {
    /// Low-cardinality telemetry attribute value.
    pub fn as_str(self) -> &'static str {
        match self {
            ReclaimSource::Daemon => "daemon",
            ReclaimSource::Cli => "cli",
        }
    }
}

/// `true` iff the process's effective UID is 0 (root). Running the apply path as
/// root would nullify the path-ownership policy the rails rely on.
pub(crate) fn is_root() -> bool {
    // SAFETY: `geteuid` is always safe — it takes no arguments, reads no memory,
    // and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// The mode the guarded executor should actually run in. Refuses to *delete* as
/// root at the guarded core: an apply run under euid 0 is downgraded to dry-run
/// so the invariant holds for **every** caller of [`reclaim_candidates`] (CLI,
/// daemon, future callers), not just the adapters that pre-check. Running the
/// apply path as root would nullify the path-ownership policy the rails rely on.
fn effective_apply_mode(mode: ReclaimMode, is_root: bool) -> ReclaimMode {
    if mode == ReclaimMode::Apply && is_root {
        ReclaimMode::DryRun
    } else {
        mode
    }
}

/// Build the production guard context + remover and run the guarded executor over
/// an explicit candidate list, emitting telemetry. Shared by the recipe-driven
/// [`run_disk_reclaim`] and the operator `exec` form so the production wiring
/// (allow-roots, deny-set, live/worktree probes, remover) lives in exactly one
/// place. **Every** candidate is re-vetted by [`guard::vet_candidate`]; nothing
/// here can widen the rails.
pub fn reclaim_candidates(
    candidates: Vec<ReclaimCandidate>,
    state_root: &Path,
    mode: ReclaimMode,
    target_pct: u8,
    source: ReclaimSource,
) -> ReclaimReport {
    // Defense in depth: refuse to delete as root at the guarded core so every
    // entry point inherits the invariant — even a future caller that skips an
    // adapter-level pre-check. Downgrade apply -> dry-run rather than delete
    // under an euid that would nullify the path-ownership policy.
    let mode = {
        let effective = effective_apply_mode(mode, is_root());
        if effective != mode {
            tracing::warn!(
                "refusing disk-reclaim apply as root (euid 0); downgraded to dry-run \
                 — deletion would nullify the path-ownership policy"
            );
        }
        effective
    };

    // Deterministic build-cache leaves for the managed repos (issue #4810).
    // Threaded into BOTH the Rail-2 allow-scope and the guard's registered-leaf
    // allowlist so routine reclaim can evict `<repo>/target/debug/*` without
    // widening any other rail. The widening is expressed leaf-by-leaf (never
    // `target/`), so `starts_with` structurally forbids a wholesale-`target/debug`
    // candidate from ever being contained. Empty when nothing is built yet.
    let build_cache_leaves = build_cache::build_cache_leaf_dirs(&managed_repos());
    let mut allow = allow_roots(state_root);
    allow.extend(build_cache_leaves.iter().cloned());

    let protected = ProtectedDenySet::resolve(Path::new("/proc"));
    let live = crate::worktree_gc::ProcfsLiveProcessProbe::new();
    let wt = RealTrackedWorktreeProbe;
    // Cache `du` results for this run so the largest-first sort and the guard's
    // fresh-measure step don't each spawn `du` for the same path.
    let du = DuSizeMeasurer;
    let measurer = CachingSizeMeasurer::new(&du);
    let ctx = GuardContext {
        allow_roots: &allow,
        build_cache_leaves: &build_cache_leaves,
        protected: &protected,
        live_probe: &live,
        wt_probe: &wt,
        measurer: &measurer,
    };
    let remover = DerivingPathRemover {
        allow_roots: allow.clone(),
    };
    let disk = crate::disk_pressure::RealDiskStatProvider;

    let report = exec_reclaim(
        candidates, &ctx, mode, target_pct, &disk, state_root, &remover,
    );
    emit_reclaim_telemetry(&report, source);
    report
}

/// Production top-level orchestrator: invoke the analysis recipe, vet every
/// candidate through the non-bypassable guard, reclaim largest-first up to
/// `target_pct`, and emit telemetry. **No fallback** — a recipe/parse failure
/// surfaces as [`crate::error::SimardError::AdapterInvocationFailed`].
///
/// Apply mode is **refused when `geteuid() == 0`** (defense in depth; the CLI
/// pre-checks this too for its exit-2 mapping). The daemon path passes
/// `mode = Apply` only when [`daemon_apply_from_env`] returns it.
pub fn run_disk_reclaim(
    repo_root: &Path,
    state_root: &Path,
    home_override: Option<&Path>,
    mode: ReclaimMode,
    target_pct: u8,
    source: ReclaimSource,
) -> SimardResult<ReclaimReport> {
    if mode == ReclaimMode::Apply && is_root() {
        return Err(crate::error::SimardError::AdapterInvocationFailed {
            base_type: "disk-reclaim".to_string(),
            reason: "refusing --apply as root (euid 0) — would nullify the path-ownership policy"
                .to_string(),
        });
    }

    let invoker = RecipeRunnerInvoker {
        repo_root: repo_root.to_path_buf(),
        state_root: state_root.to_path_buf(),
        home_override: home_override.map(Path::to_path_buf),
    };
    let (candidates, _used_pct) = run_reclaim_recipe(&invoker)?;

    // Merge the deterministic build-cache leaves with the agent's proposal,
    // de-duplicating by path (issue #4810). The recipe contract is unchanged —
    // the agent may still nominate build caches and everything else — but the
    // deterministic set guarantees the regenerable leaves are always present
    // regardless of what the agent returns, removing steady-state relief's
    // dependence on non-deterministic LLM output. There is NO fallback: a recipe
    // failure still surfaces above as `AdapterInvocationFailed`; the deterministic
    // candidates never mask a broken recipe.
    let candidates = merge_dedup_by_path(candidates, build_cache_candidates(&managed_repos()));

    Ok(reclaim_candidates(
        candidates, state_root, mode, target_pct, source,
    ))
}

/// Merge two candidate lists, keeping the first occurrence of each path. The
/// recipe proposal wins on a path collision (its `kind`/`reason` are advisory and
/// re-derived by the guard anyway), and each deterministic leaf is appended only
/// when no candidate already targets that path.
fn merge_dedup_by_path(
    primary: Vec<ReclaimCandidate>,
    extra: Vec<ReclaimCandidate>,
) -> Vec<ReclaimCandidate> {
    let mut seen: std::collections::HashSet<PathBuf> =
        primary.iter().map(|c| c.path.clone()).collect();
    let mut out = primary;
    for candidate in extra {
        if seen.insert(candidate.path.clone()) {
            out.push(candidate);
        }
    }
    out
}

/// Whether the daemon self-heal trigger should fire: the measured home-partition
/// `%-used` is at or above the configured reclaim threshold. Kept as a named,
/// tested predicate so the trigger semantics live in one place rather than
/// buried inline in the daemon maintenance loop.
pub fn daemon_should_trigger(used_pct: u8, threshold_pct: u8) -> bool {
    used_pct >= threshold_pct
}

/// Emit the `simard.disk.reclaim.*` series for one run. Dry-run emits the gauges
/// and `candidates_skipped`; only actually-removed paths increment `bytes_freed`
/// / `paths_removed`. The agent's free-text `reason` is **never** an attribute —
/// only the closed [`RejectReason`] enum is.
pub fn emit_reclaim_telemetry(report: &ReclaimReport, source: ReclaimSource) {
    use crate::telemetry::{names, registry};

    let src = source.as_str();

    registry::gauge_set(
        names::DISK_RECLAIM_USED_PCT_BEFORE,
        i64::from(report.used_pct_before),
        &[(names::ATTR_SOURCE, src)],
    );
    registry::gauge_set(
        names::DISK_RECLAIM_USED_PCT_AFTER,
        i64::from(report.used_pct_after),
        &[(names::ATTR_SOURCE, src)],
    );

    if report.bytes_freed > 0 {
        registry::counter_add(
            names::DISK_RECLAIM_BYTES_FREED,
            report.bytes_freed,
            &[(names::ATTR_SOURCE, src)],
        );
    }

    for removed in &report.removed {
        registry::counter_add(
            names::DISK_RECLAIM_PATHS_REMOVED,
            1,
            &[
                (names::ATTR_SOURCE, src),
                (names::ATTR_KIND, kind_attr(removed.kind)),
            ],
        );
    }

    for skipped in &report.skipped {
        registry::counter_add(
            names::DISK_RECLAIM_CANDIDATES_SKIPPED,
            1,
            &[
                (names::ATTR_SOURCE, src),
                (names::ATTR_REASON, reason_attr(skipped.reject_reason)),
            ],
        );
    }
}

/// Fixed low-cardinality attribute string for a [`CandidateKind`].
fn kind_attr(kind: CandidateKind) -> &'static str {
    match kind {
        CandidateKind::TrackedWorktree => "tracked_worktree",
        CandidateKind::OrphanDir => "orphan_dir",
        CandidateKind::StaleBuildCache => "stale_build_cache",
    }
}

/// Fixed low-cardinality attribute string for a [`RejectReason`].
fn reason_attr(reason: RejectReason) -> &'static str {
    match reason {
        RejectReason::ProtectedPath => "protected_path",
        RejectReason::LiveProcess => "live_process",
        RejectReason::UncommittedOrUnpushed => "uncommitted_or_unpushed",
        RejectReason::ActiveWorktree => "active_worktree",
        RejectReason::OutsideAllowRoot => "outside_allow_root",
        RejectReason::UnknownPrState => "unknown_pr_state",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn clear() {
        // SAFETY: guarded by #[serial]; env is restored by each test.
        unsafe {
            std::env::remove_var(RECLAIM_PCT_ENV);
            std::env::remove_var(DAEMON_APPLY_ENV);
        }
    }

    #[test]
    #[serial(cognitive_memory)]
    fn reclaim_pct_defaults_to_85() {
        clear();
        assert_eq!(reclaim_pct_from_env(), 85);
    }

    #[test]
    #[serial(cognitive_memory)]
    fn reclaim_pct_reads_env() {
        clear();
        unsafe { std::env::set_var(RECLAIM_PCT_ENV, "70") };
        assert_eq!(reclaim_pct_from_env(), 70);
        clear();
    }

    #[test]
    #[serial(cognitive_memory)]
    fn reclaim_pct_clamps_out_of_range() {
        clear();
        unsafe { std::env::set_var(RECLAIM_PCT_ENV, "0") };
        assert_eq!(reclaim_pct_from_env(), 1, "0 clamps up to 1");
        unsafe { std::env::set_var(RECLAIM_PCT_ENV, "150") };
        // 150 does not fit in u8? It does (u8 max 255) → clamps to 99.
        assert_eq!(reclaim_pct_from_env(), 99, "150 clamps down to 99");
        clear();
    }

    #[test]
    #[serial(cognitive_memory)]
    fn reclaim_pct_ignores_garbage_and_uses_default() {
        clear();
        unsafe { std::env::set_var(RECLAIM_PCT_ENV, "not-a-number") };
        assert_eq!(reclaim_pct_from_env(), 85);
        clear();
    }

    #[test]
    #[serial(cognitive_memory)]
    fn daemon_apply_defaults_to_dry_run() {
        clear();
        assert_eq!(daemon_apply_from_env(), ReclaimMode::DryRun);
    }

    #[test]
    #[serial(cognitive_memory)]
    fn daemon_apply_only_on_explicit_true() {
        clear();
        for v in ["1", "true", "TRUE", "yes", "on"] {
            unsafe { std::env::set_var(DAEMON_APPLY_ENV, v) };
            assert_eq!(
                daemon_apply_from_env(),
                ReclaimMode::Apply,
                "{v} should enable apply",
            );
        }
        for v in ["0", "false", "no", "", "maybe"] {
            unsafe { std::env::set_var(DAEMON_APPLY_ENV, v) };
            assert_eq!(
                daemon_apply_from_env(),
                ReclaimMode::DryRun,
                "{v:?} must stay dry-run (fail-safe)",
            );
        }
        clear();
    }

    #[test]
    fn allow_roots_cover_engineer_and_managed_worktrees() {
        let roots = allow_roots(Path::new("/home/azureuser/.simard"));
        assert!(roots.contains(&PathBuf::from("/home/azureuser/.simard/engineer-worktrees")));
        assert!(roots.contains(&PathBuf::from("/home/azureuser/src/Simard/worktrees")));
        assert!(roots.contains(&PathBuf::from("/home/azureuser/src/amplihack-rs/worktrees")));
        assert!(roots.contains(&PathBuf::from(
            "/home/azureuser/src/amplihack-memory-lib/worktrees"
        )));
    }

    #[test]
    fn managed_repos_do_not_include_bare_home() {
        // Guard against an allow-root ever spanning all of $HOME.
        let repos = managed_repos();
        assert!(!repos.contains(&PathBuf::from("/home/azureuser")));
        assert_eq!(repos.len(), 3);
    }

    #[test]
    fn daemon_trigger_fires_at_or_above_threshold() {
        assert!(daemon_should_trigger(85, 85), "equal → fire");
        assert!(daemon_should_trigger(92, 85), "above → fire");
        assert!(!daemon_should_trigger(84, 85), "below → no fire");
        assert!(!daemon_should_trigger(0, 85));
        assert!(daemon_should_trigger(100, 85));
    }

    #[test]
    #[serial]
    fn emit_reclaim_telemetry_records_gauges_removed_and_skipped() {
        use crate::disk_reclaim::executor::{RemovedPath, SkippedPath};
        use crate::telemetry::{names, registry};

        let report = ReclaimReport {
            mode: ReclaimMode::Apply,
            used_pct_before: 91,
            used_pct_after: 83,
            target_pct: 85,
            bytes_freed: 4096,
            removed: vec![RemovedPath {
                path: PathBuf::from("/home/azureuser/.simard/engineer-worktrees/leftover"),
                kind: CandidateKind::OrphanDir,
                bytes: 4096,
                primitive: ReclaimPrimitive::RemoveDir,
            }],
            would_remove: vec![],
            skipped: vec![SkippedPath {
                path: PathBuf::from(super::HARDCODED_PROTECTED_MAIN),
                kind: CandidateKind::TrackedWorktree,
                reject_reason: RejectReason::ProtectedPath,
            }],
            failures: vec![],
        };

        emit_reclaim_telemetry(&report, ReclaimSource::Cli);
        let snap = registry::capture();

        let before = snap
            .gauges
            .iter()
            .find(|g| {
                g.name == names::DISK_RECLAIM_USED_PCT_BEFORE
                    && g.attrs
                        .iter()
                        .any(|(k, v)| k == names::ATTR_SOURCE && v == "cli")
            })
            .expect("used_pct_before gauge with source=cli");
        assert_eq!(before.value, 91);

        let after = snap
            .gauges
            .iter()
            .find(|g| {
                g.name == names::DISK_RECLAIM_USED_PCT_AFTER
                    && g.attrs
                        .iter()
                        .any(|(k, v)| k == names::ATTR_SOURCE && v == "cli")
            })
            .expect("used_pct_after gauge with source=cli");
        assert_eq!(after.value, 83);

        // paths_removed carries the kind attribute.
        assert!(
            snap.counters
                .iter()
                .any(|c| c.name == names::DISK_RECLAIM_PATHS_REMOVED
                    && c.attrs
                        .iter()
                        .any(|(k, v)| k == names::ATTR_KIND && v == "orphan_dir")),
            "expected a paths_removed counter tagged kind=orphan_dir",
        );
        // candidates_skipped carries the closed RejectReason enum, never free text.
        assert!(
            snap.counters
                .iter()
                .any(|c| c.name == names::DISK_RECLAIM_CANDIDATES_SKIPPED
                    && c.attrs
                        .iter()
                        .any(|(k, v)| k == names::ATTR_REASON && v == "protected_path")),
            "expected a candidates_skipped counter tagged reason=protected_path",
        );
    }

    #[test]
    fn effective_apply_mode_downgrades_apply_as_root() {
        // Apply as root is downgraded to dry-run at the guarded core (no deletion).
        assert_eq!(
            effective_apply_mode(ReclaimMode::Apply, true),
            ReclaimMode::DryRun,
            "apply as root -> dry-run (refuse deletion)"
        );
        // Apply as non-root is preserved.
        assert_eq!(
            effective_apply_mode(ReclaimMode::Apply, false),
            ReclaimMode::Apply,
            "apply as non-root is allowed"
        );
        // Dry-run as root stays dry-run (already safe — no deletion).
        assert_eq!(
            effective_apply_mode(ReclaimMode::DryRun, true),
            ReclaimMode::DryRun,
            "dry-run as root is unchanged"
        );
    }

    #[test]
    fn kind_and_reason_attrs_are_stable_snake_case() {
        assert_eq!(
            kind_attr(CandidateKind::TrackedWorktree),
            "tracked_worktree"
        );
        assert_eq!(kind_attr(CandidateKind::OrphanDir), "orphan_dir");
        assert_eq!(
            kind_attr(CandidateKind::StaleBuildCache),
            "stale_build_cache"
        );
        assert_eq!(reason_attr(RejectReason::ProtectedPath), "protected_path");
        assert_eq!(reason_attr(RejectReason::LiveProcess), "live_process");
        assert_eq!(
            reason_attr(RejectReason::UncommittedOrUnpushed),
            "uncommitted_or_unpushed"
        );
        assert_eq!(reason_attr(RejectReason::ActiveWorktree), "active_worktree");
        assert_eq!(
            reason_attr(RejectReason::OutsideAllowRoot),
            "outside_allow_root"
        );
        assert_eq!(
            reason_attr(RejectReason::UnknownPrState),
            "unknown_pr_state"
        );
    }

    #[test]
    fn merge_dedup_by_path_appends_only_new_paths() {
        let mk = |p: &str, kind: CandidateKind| ReclaimCandidate {
            path: PathBuf::from(p),
            kind,
            parent_repo: None,
            reason: None,
            est_bytes: None,
        };
        // The recipe proposal already includes `/a`; the deterministic set adds
        // `/a` (a collision, dropped) and `/b` (new, appended).
        let recipe = vec![mk("/a", CandidateKind::OrphanDir)];
        let deterministic = vec![
            mk("/a", CandidateKind::StaleBuildCache),
            mk("/b", CandidateKind::StaleBuildCache),
        ];

        let merged = merge_dedup_by_path(recipe, deterministic);
        let paths: Vec<_> = merged.iter().map(|c| c.path.clone()).collect();
        assert_eq!(
            paths,
            vec![PathBuf::from("/a"), PathBuf::from("/b")],
            "collision keeps the first (recipe) entry; only the new path is appended",
        );
        // The surviving `/a` is the recipe's, proving first-wins on collision.
        assert_eq!(merged[0].kind, CandidateKind::OrphanDir);
    }
}
