//! Resource-aware engineer **admission** gate (issue #2706).
//!
//! # The problem
//!
//! The AIMD controller upstream bounds the *number* of concurrent engineers,
//! but it is blind to what those engineers consume: each one allocates a git
//! worktree and runs parallel `cargo` builds. Count-control is not
//! resource-admission — 40+ worktrees with piled-up build caches drove disk to
//! 91% and an `ENOSPC` kills recipes. This module adds the missing axis: before
//! admitting another engineer, weigh the HOST resource picture (disk %,
//! build-cache/worktree sizes, load average, in-flight builds).
//!
//! # Where the intelligence lives
//!
//! The decision is a **structured-reasoning brain step**
//! ([`OodaBrain::decide_resource_admission`]) driven by a hot-reloadable recipe
//! (`ooda-resource-admission.yaml`) — NOT a pile of hardcoded thresholds. It
//! mirrors the dependency/overlap gate ([`super::admission`]): gather structured
//! context → call a reasoner → apply the decision, with only a THIN
//! deterministic rail guarding the one irreversible outcome. The brain reasons
//! **repeatedly**, once per admission cycle ("repeated execution of structured
//! thought").
//!
//! # The rails
//!
//! | Rail | Guard | On fire |
//! | --- | --- | --- |
//! | **Disk-ceiling (hard)** | `disk_used_pct >= ceiling` AND the resolved action would SPAWN (`Admit`) | Deterministic `Defer` — overrides the brain. The `ENOSPC` guard: a spawn is the only action that CONSUMES disk, so blocking `Admit` past the ceiling makes the irreversible out-of-space state unreachable. Inert when disk % is unknown. |
//! | **Brain-error (fail-CLOSED)** | `decide_resource_admission` returns `Err` | `Defer` + loud `error!`. Unlike the overlap gate (fail-open), a resource-brain error skips the spawn: the reasoning was supposed to run and broke, so be conservative. Benign (retried next cycle); the kill-switch is the escape hatch. |
//!
//! The disk-ceiling rail is the load-bearing safety control. It lives HERE, in
//! Rust — never in the (user-writable, hot-reloadable) recipe — so editing the
//! prompt can change *scheduling quality* below the ceiling but can NEVER let the
//! daemon spawn another build past it. It can only ever be MORE conservative than
//! the brain (turn an `Admit` into a `Defer`): `Defer`/`ReclaimFirst` (which do
//! not spawn) pass through untouched, so the brain can still trigger recovery via
//! reclaim even when disk is already tight.

use std::path::Path;
use std::sync::LazyLock;

use crate::disk_pressure::check::DiskStatProvider;
use crate::disk_pressure::{exceeds_admission_ceiling, used_pct};
use crate::ooda_brain::{
    BrainJudgmentRecord, OodaBrain, ResourceAdmissionCtx, ResourceAdmissionDecision, prompt_store,
    push_brain_judgment,
};

/// Prompt-asset name for the resource-admission reasoning prompt. Registered in
/// [`crate::ooda_brain::prompt_store`] (embedded fallback + hot-reload) so the
/// judgment record can stamp the exact prompt version that produced a decision.
pub const RESOURCE_ADMISSION_PROMPT_NAME: &str = "ooda_resource_admission.md";

/// Metric name for one resource-admission decision, appended to `metrics.jsonl`.
pub const RESOURCE_ADMISSION_DECISION_METRIC: &str = "resource_admission_decision";

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// The applied resource-admission result the spawn seam acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResourceAdmissionOutcome {
    /// Proceed to worktree allocation + spawn (the existing path, unchanged).
    Admit,
    /// Skip this cycle, benignly (no worktree, no failure counted). `detail` is
    /// the benign skip outcome the seam returns as `make_outcome(action, true, ..)`.
    Defer { detail: String },
    /// Reclaim disk first (the seam invokes the disk-health capability
    /// best-effort), then skip this cycle benignly. Retried next round against
    /// the freed space.
    ReclaimFirst { detail: String },
}

/// The result of evaluating the rails over a ctx: the seam outcome plus the raw
/// decision + `fallback` flag the caller records for observability. Mirrors
/// [`super::admission::AdmissionEvaluation`]; keeping observability OUT of the
/// pure evaluator means the hermetic rail tests write no metric files.
#[derive(Debug, Clone)]
pub(crate) struct ResourceAdmissionEvaluation {
    pub outcome: ResourceAdmissionOutcome,
    pub decision: ResourceAdmissionDecision,
    pub fallback: bool,
}

/// Environment kill-switch: `SIMARD_RESOURCE_ADMISSION=off` disables the
/// **reasoning** gate (no gather, no brain call — candidates skip straight to the
/// hard rail). The deterministic disk-ceiling rail and the byte-level
/// `MIN_FREE_GB` precheck still run, so disabling the reasoning never disables
/// the ENOSPC guarantee. Secure default is **ON**.
///
/// Memoized (read once at first use) so the operations guarantee — "changing
/// this mid-run has no effect; restart to change" — is literally true. Only the
/// explicit value `off` (case-insensitive) disables; any unknown value keeps the
/// gate enabled so a typo can never silently disable it.
pub fn resource_admission_enabled() -> bool {
    static ENABLED: LazyLock<bool> = LazyLock::new(|| {
        resource_admission_enabled_from(std::env::var("SIMARD_RESOURCE_ADMISSION").ok().as_deref())
    });
    *ENABLED
}

/// Pure kill-switch classifier (testable without touching the process env).
fn resource_admission_enabled_from(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => !v.trim().eq_ignore_ascii_case("off"),
        None => true,
    }
}

/// The pure seam core: apply the rails to an already-assembled
/// [`ResourceAdmissionCtx`]. **Pure** — mutates no board, allocates no worktree,
/// writes no metric, invokes no reclaim, pushes no judgment. `enabled = false`
/// means the reasoning gate is off: the brain is NOT consulted and only the hard
/// disk-ceiling rail decides. Returns the outcome plus the decision/fallback for
/// the caller to record. Hermetically testable with a stub brain and an injected
/// resource picture.
pub(crate) fn evaluate_resource_admission(
    ctx: &ResourceAdmissionCtx,
    brain: &dyn OodaBrain,
    enabled: bool,
) -> ResourceAdmissionEvaluation {
    let hard_block = ctx
        .disk_used_pct
        .map(|u| exceeds_admission_ceiling(u, ctx.admission_ceiling_pct))
        .unwrap_or(false);

    // Kill-switch: reasoning disabled ⇒ hard rail ONLY (no brain call). A spawn
    // past the ceiling is still refused; otherwise admit.
    if !enabled {
        let (outcome, decision) = if hard_block {
            let d = rail_defer_decision(ctx);
            (
                ResourceAdmissionOutcome::Defer {
                    detail: rail_defer_detail(ctx),
                },
                d,
            )
        } else {
            (
                ResourceAdmissionOutcome::Admit,
                ResourceAdmissionDecision::Admit {
                    rationale:
                        "resource-admission reasoning disabled (SIMARD_RESOURCE_ADMISSION=off); \
                                deterministic disk-ceiling rail only"
                            .into(),
                },
            )
        };
        return ResourceAdmissionEvaluation {
            outcome,
            decision,
            fallback: true,
        };
    }

    // Reason (fail-CLOSED on error). Unlike the overlap gate, a resource-brain
    // Err DEFERS (does not admit): the reasoning was supposed to run and broke,
    // so be conservative. Benign — retried next cycle.
    let (decision, fallback) = match brain.decide_resource_admission(ctx) {
        Ok(d) => (d, false),
        Err(e) => {
            tracing::error!(
                target: "simard::ooda_brain",
                goal = %ctx.goal_id,
                error = %e,
                "decide_resource_admission FAILED — failing CLOSED to Defer (issue #2706)",
            );
            (
                ResourceAdmissionDecision::Defer {
                    rationale: format!(
                        "fail-closed (issue #2706): resource-admission brain error — deferring \
                         this cycle: {e}"
                    ),
                },
                true,
            )
        }
    };

    // Disk-ceiling (HARD, deterministic). A spawn (`Admit`) is the only action
    // that CONSUMES disk; refuse it past the ceiling regardless of what the brain
    // (or a compromised prompt) said. `Defer`/`ReclaimFirst` do not spawn, so
    // they pass through untouched — the brain can still trigger recovery even
    // when disk is tight. This is the control that survives a broken/compromised
    // reasoner and keeps ENOSPC unreachable.
    if hard_block && matches!(decision, ResourceAdmissionDecision::Admit { .. }) {
        let d = rail_defer_decision(ctx);
        tracing::warn!(
            target: "simard::ooda_brain",
            goal = %ctx.goal_id,
            disk_used_pct = ?ctx.disk_used_pct,
            ceiling_pct = ctx.admission_ceiling_pct,
            "resource-admission: disk-ceiling rail — deferring admission (overrides brain)",
        );
        return ResourceAdmissionEvaluation {
            outcome: ResourceAdmissionOutcome::Defer {
                detail: rail_defer_detail(ctx),
            },
            decision: d,
            fallback: true,
        };
    }

    let outcome = apply_resource_decision(&decision);
    ResourceAdmissionEvaluation {
        outcome,
        decision,
        fallback,
    }
}

/// Translate a brain decision into the seam's [`ResourceAdmissionOutcome`].
fn apply_resource_decision(decision: &ResourceAdmissionDecision) -> ResourceAdmissionOutcome {
    match decision {
        ResourceAdmissionDecision::Admit { .. } => ResourceAdmissionOutcome::Admit,
        ResourceAdmissionDecision::Defer { .. } => ResourceAdmissionOutcome::Defer {
            detail: defer_detail(decision),
        },
        ResourceAdmissionDecision::ReclaimFirst { .. } => ResourceAdmissionOutcome::ReclaimFirst {
            detail: reclaim_detail(decision),
        },
    }
}

/// The deterministic disk-ceiling Defer decision recorded when the rail fires.
fn rail_defer_decision(ctx: &ResourceAdmissionCtx) -> ResourceAdmissionDecision {
    let used = ctx
        .disk_used_pct
        .map(|p| format!("{p:.0}"))
        .unwrap_or_default();
    ResourceAdmissionDecision::Defer {
        rationale: format!(
            "disk-ceiling rail: {used}% used >= {:.0}% ceiling — refusing to admit another \
             engineer (ENOSPC guard, overrides reasoning)",
            ctx.admission_ceiling_pct
        ),
    }
}

/// Human-readable benign skip detail for the disk-ceiling rail.
fn rail_defer_detail(ctx: &ResourceAdmissionCtx) -> String {
    let used = ctx
        .disk_used_pct
        .map(|p| format!("{p:.0}"))
        .unwrap_or_else(|| "?".into());
    format!(
        "spawn deferred (issue #2706): disk-ceiling rail — {used}% used >= {:.0}% ceiling \
         (ENOSPC guard)",
        ctx.admission_ceiling_pct
    )
}

/// Human-readable benign skip detail for a brain `Defer`.
fn defer_detail(decision: &ResourceAdmissionDecision) -> String {
    format!(
        "spawn deferred (issue #2706): resource pressure — {}",
        decision.rationale()
    )
}

/// Human-readable benign skip detail for a `ReclaimFirst` (the seam reclaims,
/// then skips this cycle; retried next OODA round with the freed headroom).
fn reclaim_detail(decision: &ResourceAdmissionDecision) -> String {
    format!(
        "spawn reclaim-first (issue #2706): {}",
        decision.rationale()
    )
}

/// Record one resource-admission decision for observability (issue #2706):
///
/// 1. Push a [`BrainJudgmentRecord`] (phase `ResourceAdmission`) carrying the
///    disk %, ceiling and in-flight count. The deterministic rail override, the
///    kill-switch path, and the fail-closed brain-error defer all set
///    `fallback = true`.
/// 2. Emit the [`RESOURCE_ADMISSION_DECISION_METRIC`] to `metrics.jsonl` (the
///    numeric `value` is the disk %, or `-1.0` when unknown, so an unknown
///    reading is distinguishable from a genuine `0%`).
///
/// Best-effort: a metric write error never affects control flow.
pub(crate) fn record_resource_admission(
    goal_id: &str,
    decision: &ResourceAdmissionDecision,
    disk_used_pct: Option<f64>,
    ceiling_pct: f64,
    worktree_count: Option<u32>,
    in_flight_engineers: u32,
    fallback: bool,
) {
    push_brain_judgment(BrainJudgmentRecord::from_resource_admission(
        goal_id,
        decision,
        disk_used_pct,
        ceiling_pct,
        in_flight_engineers,
        fallback,
        prompt_store::current_version(RESOURCE_ADMISSION_PROMPT_NAME),
    ));
    let value = disk_used_pct.unwrap_or(-1.0);
    let _ = crate::self_metrics::record_metric(
        RESOURCE_ADMISSION_DECISION_METRIC,
        value,
        &resource_admission_metric_context(
            goal_id,
            decision,
            disk_used_pct,
            ceiling_pct,
            worktree_count,
            in_flight_engineers,
        ),
    );
}

/// Build the bounded metric context string for a resource-admission decision —
/// carries the resource reasoning so `simard metrics query` / status can audit
/// *why* a spawn was deferred or reclaimed.
fn resource_admission_metric_context(
    goal_id: &str,
    decision: &ResourceAdmissionDecision,
    disk_used_pct: Option<f64>,
    ceiling_pct: f64,
    worktree_count: Option<u32>,
    in_flight_engineers: u32,
) -> String {
    let disk = disk_used_pct
        .map(|p| format!("{p:.0}"))
        .unwrap_or_else(|| "unknown".to_string());
    let worktrees = worktree_count
        .map(|c| c.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "goal_id={goal_id} decision={} disk={disk}%/ceiling={ceiling_pct:.0}% \
         worktrees={worktrees} in_flight={in_flight_engineers} rationale={}",
        decision.variant_label(),
        decision.rationale(),
    )
}

/// Assemble the full [`ResourceAdmissionCtx`] for the host. **Pure &
/// best-effort**: every probe is absent-tolerant and degrades to `None`/`0`.
/// Never panics, never blocks. `state_root` is the engineer-worktree root — the
/// filesystem the ceiling governs — and `disk` is the injectable stat provider
/// (real in production, a fake in tests) so the whole gate is hermetic.
pub(crate) fn gather_resource_admission_ctx<P: DiskStatProvider + ?Sized>(
    state_root: &Path,
    goal_id: &str,
    disk: &P,
    ceiling_pct: f64,
) -> ResourceAdmissionCtx {
    let stat = disk.stat(state_root).ok();
    let disk_used_pct = stat.as_ref().and_then(used_pct);
    let disk_free_gb = stat.as_ref().map(|s| s.free_bytes as f64 / GIB);
    let disk_total_gb = stat.as_ref().map(|s| s.total_bytes as f64 / GIB);
    let (load_avg_1, load_avg_5, load_avg_15) = read_load_avgs();

    ResourceAdmissionCtx {
        goal_id: goal_id.to_string(),
        disk_used_pct,
        disk_free_gb,
        disk_total_gb,
        // A full recursive walk of the worktree tree is slowest exactly under the
        // pressure this gate detects, so it is left best-effort `None` on the hot
        // path — `disk_used_pct` (cheap `statvfs`) is the dominant signal.
        build_cache_bytes: None,
        worktree_count: count_engineer_worktrees(state_root),
        load_avg_1,
        load_avg_5,
        load_avg_15,
        cpu_count: std::thread::available_parallelism()
            .ok()
            .map(|n| n.get() as u32),
        in_flight_engineers: crate::ooda_brain::count_live_engineer_claims(state_root),
        // Optional: threaded from adaptive-scaling config in a follow-up; degrades
        // to None so the prompt renders "unknown".
        aimd_current_max: None,
        admission_ceiling_pct: ceiling_pct,
    }
}

/// Count the engineer-worktree directories under
/// `<state_root>/engineer-worktrees/`. `None` when the directory is
/// unreadable/absent.
fn count_engineer_worktrees(state_root: &Path) -> Option<u32> {
    let dir = state_root.join(crate::engineer_worktree::WORKTREES_SUBDIR);
    let entries = std::fs::read_dir(&dir).ok()?;
    let n = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .count();
    Some(n as u32)
}

/// Read the 1/5/15-minute load averages from `/proc/loadavg`. All `None`
/// off-Linux or on any read/parse error.
fn read_load_avgs() -> (Option<f64>, Option<f64>, Option<f64>) {
    let Ok(text) = std::fs::read_to_string("/proc/loadavg") else {
        return (None, None, None);
    };
    let mut it = text.split_whitespace();
    (
        it.next().and_then(|s| s.parse().ok()),
        it.next().and_then(|s| s.parse().ok()),
        it.next().and_then(|s| s.parse().ok()),
    )
}

/// The full resource-admission gate as invoked by the spawn seam: kill-switch →
/// (gather → reason) → hard rail → record → outcome. Wires the memoized
/// [`resource_admission_enabled`] and [`crate::disk_pressure::configured_admission_ceiling_pct`].
/// The returned `ReclaimFirst` is executed by the CALLER (which owns the
/// daemon `repo_root` the reclaim recipe needs) — keeping the potentially
/// minutes-long shell-out off this hot path.
pub(crate) fn run_resource_admission_gate<P: DiskStatProvider + ?Sized>(
    state_root: &Path,
    goal_id: &str,
    brain: &dyn OodaBrain,
    disk: &P,
) -> ResourceAdmissionOutcome {
    run_resource_admission_gate_with(
        state_root,
        goal_id,
        brain,
        disk,
        resource_admission_enabled(),
        crate::disk_pressure::configured_admission_ceiling_pct(),
    )
}

/// Testable core of [`run_resource_admission_gate`] with `enabled`/`ceiling_pct`
/// injected so the whole gather → reason → rail → record path is hermetic.
pub(crate) fn run_resource_admission_gate_with<P: DiskStatProvider + ?Sized>(
    state_root: &Path,
    goal_id: &str,
    brain: &dyn OodaBrain,
    disk: &P,
    enabled: bool,
    ceiling_pct: f64,
) -> ResourceAdmissionOutcome {
    // Reasoning off ⇒ NO gather, NO brain call — a minimal ctx carrying only the
    // rail inputs (disk + ceiling). Reasoning on ⇒ full best-effort gather.
    let ctx = if enabled {
        gather_resource_admission_ctx(state_root, goal_id, disk, ceiling_pct)
    } else {
        let stat = disk.stat(state_root).ok();
        ResourceAdmissionCtx {
            goal_id: goal_id.to_string(),
            disk_used_pct: stat.as_ref().and_then(used_pct),
            admission_ceiling_pct: ceiling_pct,
            ..Default::default()
        }
    };

    let eval = evaluate_resource_admission(&ctx, brain, enabled);
    record_resource_admission(
        goal_id,
        &eval.decision,
        ctx.disk_used_pct,
        ceiling_pct,
        ctx.worktree_count,
        ctx.in_flight_engineers,
        eval.fallback,
    );
    eval.outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk_pressure::check::DiskStat;
    use crate::error::{SimardError, SimardResult};
    use crate::ooda_brain::{
        BrainPhase, EngineerLifecycleCtx, EngineerLifecycleDecision, take_brain_judgments,
        with_brain_judgment_scope,
    };

    /// A hermetic resource-admission brain: returns the injected decision (or an
    /// error). The lifecycle path is never taken by these tests.
    struct StubResourceBrain(SimardResult<ResourceAdmissionDecision>);

    impl OodaBrain for StubResourceBrain {
        fn decide_engineer_lifecycle(
            &self,
            _ctx: &EngineerLifecycleCtx,
        ) -> SimardResult<EngineerLifecycleDecision> {
            unreachable!("resource-admission tests never take the lifecycle path")
        }

        fn decide_per_goal_cycle(
            &self,
            _ctx: &crate::ooda_brain::PerGoalCycleCtx,
        ) -> SimardResult<crate::ooda_brain::PerGoalAction> {
            unreachable!("resource-admission tests never take the per-goal-cycle path")
        }

        fn decide_resource_admission(
            &self,
            _ctx: &ResourceAdmissionCtx,
        ) -> SimardResult<ResourceAdmissionDecision> {
            match &self.0 {
                Ok(d) => Ok(d.clone()),
                Err(_) => Err(SimardError::AdapterInvocationFailed {
                    base_type: "recipe-resource-admission-brain".into(),
                    reason: "stub injected error".into(),
                }),
            }
        }
    }

    /// A brain that does NOT override `decide_resource_admission` — it inherits
    /// the fail-open trait default (`Admit`). Used to prove the default admits
    /// below the ceiling but the hard rail still fires at it.
    struct DefaultBrain;
    impl OodaBrain for DefaultBrain {
        fn decide_engineer_lifecycle(
            &self,
            _ctx: &EngineerLifecycleCtx,
        ) -> SimardResult<EngineerLifecycleDecision> {
            unreachable!()
        }
        fn decide_per_goal_cycle(
            &self,
            _ctx: &crate::ooda_brain::PerGoalCycleCtx,
        ) -> SimardResult<crate::ooda_brain::PerGoalAction> {
            unreachable!()
        }
    }

    /// A brain that PANICS if its resource method is called — proves the
    /// kill-switch path never consults the brain.
    struct NeverCalledBrain;
    impl OodaBrain for NeverCalledBrain {
        fn decide_engineer_lifecycle(
            &self,
            _ctx: &EngineerLifecycleCtx,
        ) -> SimardResult<EngineerLifecycleDecision> {
            unreachable!()
        }
        fn decide_per_goal_cycle(
            &self,
            _ctx: &crate::ooda_brain::PerGoalCycleCtx,
        ) -> SimardResult<crate::ooda_brain::PerGoalAction> {
            unreachable!()
        }
        fn decide_resource_admission(
            &self,
            _ctx: &ResourceAdmissionCtx,
        ) -> SimardResult<ResourceAdmissionDecision> {
            panic!("kill-switch path must NOT call the brain")
        }
    }

    /// Fake `DiskStatProvider` returning synthetic `(free, total)` — the whole
    /// gate becomes hermetic (no real `statvfs`).
    struct FakeDisk {
        free: u64,
        total: u64,
    }
    impl DiskStatProvider for FakeDisk {
        fn stat(&self, _p: &Path) -> Result<DiskStat, std::io::Error> {
            Ok(DiskStat {
                free_bytes: self.free,
                total_bytes: self.total,
            })
        }
    }
    /// Fake provider whose stat always errors (unknown disk ⇒ rail inert).
    struct ErrDisk;
    impl DiskStatProvider for ErrDisk {
        fn stat(&self, _p: &Path) -> Result<DiskStat, std::io::Error> {
            Err(std::io::Error::other("statvfs failed"))
        }
    }

    fn stub(d: ResourceAdmissionDecision) -> StubResourceBrain {
        StubResourceBrain(Ok(d))
    }
    fn err_brain() -> StubResourceBrain {
        StubResourceBrain(Err(SimardError::AdapterInvocationFailed {
            base_type: "x".into(),
            reason: "y".into(),
        }))
    }
    fn admit(r: &str) -> ResourceAdmissionDecision {
        ResourceAdmissionDecision::Admit {
            rationale: r.into(),
        }
    }
    fn defer(r: &str) -> ResourceAdmissionDecision {
        ResourceAdmissionDecision::Defer {
            rationale: r.into(),
        }
    }
    fn reclaim(r: &str) -> ResourceAdmissionDecision {
        ResourceAdmissionDecision::ReclaimFirst {
            rationale: r.into(),
        }
    }

    /// Build a ctx with an explicit disk % and ceiling. Other probe fields are
    /// irrelevant to the rail/seam logic under test.
    fn ctx(disk_used_pct: Option<f64>, ceiling: f64) -> ResourceAdmissionCtx {
        ResourceAdmissionCtx {
            goal_id: "cand".into(),
            disk_used_pct,
            admission_ceiling_pct: ceiling,
            ..Default::default()
        }
    }

    fn eval(c: &ResourceAdmissionCtx, b: &dyn OodaBrain) -> ResourceAdmissionEvaluation {
        evaluate_resource_admission(c, b, true)
    }

    // ── The seam: brain decision → outcome ─────────────────────────────────

    #[test]
    fn stub_admit_below_ceiling_proceeds() {
        let out = eval(&ctx(Some(40.0), 90.0), &stub(admit("headroom"))).outcome;
        assert_eq!(out, ResourceAdmissionOutcome::Admit);
    }

    #[test]
    fn stub_defer_yields_skip_outcome() {
        let out = eval(&ctx(Some(70.0), 90.0), &stub(defer("load high"))).outcome;
        match out {
            ResourceAdmissionOutcome::Defer { detail } => {
                assert!(detail.contains("deferred"), "detail: {detail}");
                assert!(detail.contains("load high"), "carries reason: {detail}");
            }
            other => panic!("expected Defer, got {other:?}"),
        }
    }

    #[test]
    fn stub_reclaim_first_yields_reclaim_outcome() {
        let out = eval(&ctx(Some(80.0), 90.0), &stub(reclaim("free stale caches"))).outcome;
        assert!(matches!(out, ResourceAdmissionOutcome::ReclaimFirst { .. }));
    }

    // ── Disk-ceiling rail: the ENOSPC guard ────────────────────────────────

    // The load-bearing safety test: disk at/over the ceiling ⇒ a brain that says
    // Admit is OVERRIDDEN to Defer. A prompt (even a compromised one) can never
    // spawn another build past the ceiling.
    #[test]
    fn hard_rail_overrides_admit() {
        let e = eval(
            &ctx(Some(95.0), 90.0),
            &stub(admit("parallelize aggressively")),
        );
        match e.outcome {
            ResourceAdmissionOutcome::Defer { detail } => {
                assert!(detail.contains("disk-ceiling"), "names the rail: {detail}");
            }
            other => panic!("disk-ceiling rail must Defer an Admit, got {other:?}"),
        }
        assert_eq!(e.decision.variant_label(), "defer");
        assert!(e.fallback, "deterministic rail override marks fallback");
    }

    #[test]
    fn hard_rail_inert_below_ceiling() {
        let out = eval(&ctx(Some(70.0), 90.0), &stub(admit("ok"))).outcome;
        assert_eq!(out, ResourceAdmissionOutcome::Admit);
    }

    // The rail is more-conservative-only: it blocks a spawn (Admit) but must NOT
    // suppress ReclaimFirst — reclaim RECOVERS space (resolution #1).
    #[test]
    fn hard_rail_passes_reclaim_first_through() {
        let out = eval(&ctx(Some(97.0), 90.0), &stub(reclaim("reclaim now"))).outcome;
        assert!(matches!(out, ResourceAdmissionOutcome::ReclaimFirst { .. }));
    }

    #[test]
    fn hard_rail_leaves_defer_untouched() {
        let out = eval(&ctx(Some(99.0), 90.0), &stub(defer("brain pressure"))).outcome;
        assert!(matches!(out, ResourceAdmissionOutcome::Defer { .. }));
    }

    // ── Issue #2935: raising the count ceiling to 24 must NOT bypass gates ───

    // Raising the per-cycle count ceiling to 24 is a COUNT axis; the disk-ceiling
    // rail is an independent RESOURCE axis. Even when the daemon may spawn up to
    // 24 engineers, a candidate that would push disk past the ceiling is still
    // deterministically deferred — the ENOSPC guard is untouched by the higher cap.
    #[test]
    fn count_cap_24_does_not_bypass_disk_ceiling_rail() {
        let e = eval(
            &ctx(Some(95.0), 90.0),
            &stub(admit("count cap is 24 — parallelize aggressively")),
        );
        match e.outcome {
            ResourceAdmissionOutcome::Defer { detail } => {
                assert!(detail.contains("disk-ceiling"), "names the rail: {detail}");
            }
            other => panic!(
                "disk-ceiling rail must still Defer a spawn past the ceiling at count cap 24, got {other:?}"
            ),
        }
    }

    // Memory pressure is a SOFT signal the brain reasons about. Even at count cap
    // 24, when the reasoner Defers under memory pressure the seam yields a benign
    // skip: the raised count ceiling never forces a spawn under pressure.
    #[test]
    fn count_cap_24_still_defers_under_memory_pressure() {
        let out = eval(
            &ctx(Some(50.0), 90.0),
            &stub(defer("memory pressure: MemAvailable critically low")),
        )
        .outcome;
        match out {
            ResourceAdmissionOutcome::Defer { detail } => {
                assert!(
                    detail.contains("memory pressure"),
                    "carries the reason: {detail}"
                );
            }
            other => panic!("expected Defer under memory pressure at count cap 24, got {other:?}"),
        }
    }

    #[test]
    fn unknown_disk_admits_on_reasoning() {
        // total=0 ⇒ used_pct None ⇒ rail inert ⇒ brain Admit honored.
        let out = eval(&ctx(None, 90.0), &stub(admit("independent"))).outcome;
        assert_eq!(
            out,
            ResourceAdmissionOutcome::Admit,
            "unknown disk fails open"
        );
    }

    #[test]
    fn ceiling_boundary_admit_override() {
        // At exactly the ceiling ⇒ Admit overridden to Defer.
        let at = eval(&ctx(Some(90.0), 90.0), &stub(admit("x"))).outcome;
        assert!(
            matches!(at, ResourceAdmissionOutcome::Defer { .. }),
            "at ceiling ⇒ Defer"
        );
        // One below ⇒ Admit honored.
        let below = eval(&ctx(Some(89.9), 90.0), &stub(admit("x"))).outcome;
        assert_eq!(below, ResourceAdmissionOutcome::Admit);
    }

    // ── Brain-error: fail-CLOSED (distinct from the overlap gate) ───────────

    #[test]
    fn brain_error_fails_closed_below_ceiling() {
        let e = eval(&ctx(Some(50.0), 90.0), &err_brain());
        match e.outcome {
            ResourceAdmissionOutcome::Defer { detail } => {
                assert!(detail.contains("deferred"), "detail: {detail}");
            }
            other => panic!("brain error must fail CLOSED to Defer, got {other:?}"),
        }
        assert_eq!(e.decision.variant_label(), "defer");
        assert!(e.fallback, "fail-closed path marks fallback");
    }

    // ENOSPC unreachable even when the brain is BROKEN: past the ceiling, a brain
    // error still Defers (both the fail-closed policy AND the rail agree).
    #[test]
    fn brain_error_past_ceiling_defers() {
        let out = eval(&ctx(Some(96.0), 90.0), &err_brain()).outcome;
        assert!(matches!(out, ResourceAdmissionOutcome::Defer { .. }));
    }

    // ── Default trait method ───────────────────────────────────────────────

    #[test]
    fn default_brain_admits_below_but_rail_fires_at_ceiling() {
        // The defaulted trait method returns Admit.
        let below = eval(&ctx(Some(30.0), 90.0), &DefaultBrain).outcome;
        assert_eq!(below, ResourceAdmissionOutcome::Admit);
        // Hard rail still fires at the ceiling over the defaulted Admit.
        let at = eval(&ctx(Some(93.0), 90.0), &DefaultBrain).outcome;
        assert!(matches!(at, ResourceAdmissionOutcome::Defer { .. }));
    }

    // ── Kill-switch: skip reasoning, keep the rail ─────────────────────────

    #[test]
    fn kill_switch_off_skips_reasoning_keeps_rail() {
        // Disabled + disk 95% ⇒ Defer via the rail, WITHOUT calling the brain.
        let deferred =
            evaluate_resource_admission(&ctx(Some(95.0), 90.0), &NeverCalledBrain, false).outcome;
        assert!(matches!(deferred, ResourceAdmissionOutcome::Defer { .. }));
        // Disabled + disk 50% ⇒ Admit (rail inert), still no brain call.
        let admitted =
            evaluate_resource_admission(&ctx(Some(50.0), 90.0), &NeverCalledBrain, false).outcome;
        assert_eq!(admitted, ResourceAdmissionOutcome::Admit);
    }

    #[test]
    fn kill_switch_classifier() {
        assert!(!resource_admission_enabled_from(Some("off")));
        assert!(!resource_admission_enabled_from(Some("  OFF  ")));
        assert!(resource_admission_enabled_from(Some("on")));
        assert!(resource_admission_enabled_from(Some("")));
        assert!(resource_admission_enabled_from(Some("garbage")));
        assert!(resource_admission_enabled_from(None));
    }

    // ── Gather (fake provider) ─────────────────────────────────────────────

    #[test]
    fn gather_computes_disk_pct_from_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let c = gather_resource_admission_ctx(
            tmp.path(),
            "g1",
            &FakeDisk {
                free: 20,
                total: 100,
            },
            90.0,
        );
        assert_eq!(c.goal_id, "g1");
        assert_eq!(c.disk_used_pct, Some(80.0));
        assert_eq!(c.admission_ceiling_pct, 90.0);
        assert!(c.disk_total_gb.is_some());
        // Empty temp state root ⇒ no live engineers.
        assert_eq!(c.in_flight_engineers, 0);
    }

    #[test]
    fn gather_unknown_disk_on_provider_error() {
        let tmp = tempfile::tempdir().unwrap();
        let c = gather_resource_admission_ctx(tmp.path(), "g1", &ErrDisk, 90.0);
        assert_eq!(c.disk_used_pct, None, "provider error ⇒ unknown disk");
    }

    // ── Full gate wiring (fake provider), end-to-end hermetic ──────────────

    #[test]
    fn gate_hard_rail_defers_admit_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        // 5% free of 100 ⇒ 95% used ⇒ over the 90 ceiling.
        let out = with_brain_judgment_scope(|| {
            let out = run_resource_admission_gate_with(
                tmp.path(),
                "cand",
                &stub(admit("go")),
                &FakeDisk {
                    free: 5,
                    total: 100,
                },
                true,
                90.0,
            );
            // The gate records a judgment on the decision path.
            let records = take_brain_judgments();
            let rec = records
                .iter()
                .find(|r| r.phase == BrainPhase::ResourceAdmission)
                .expect("a ResourceAdmission judgment must be pushed");
            assert_eq!(rec.decision, "defer");
            assert!(
                rec.context_summary.contains("ceiling=90%"),
                "{}",
                rec.context_summary
            );
            out
        });
        assert!(matches!(out, ResourceAdmissionOutcome::Defer { .. }));
    }

    #[test]
    fn gate_admits_below_ceiling_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        // 40% free of 100 ⇒ 60% used ⇒ below the ceiling.
        let out = run_resource_admission_gate_with(
            tmp.path(),
            "cand",
            &stub(admit("plenty")),
            &FakeDisk {
                free: 40,
                total: 100,
            },
            true,
            90.0,
        );
        assert_eq!(out, ResourceAdmissionOutcome::Admit);
    }

    // ── Observability ──────────────────────────────────────────────────────

    #[test]
    fn pushes_resource_admission_judgment() {
        let d = defer("disk 88% approaching ceiling");
        let records = with_brain_judgment_scope(|| {
            record_resource_admission("cand", &d, Some(88.0), 90.0, Some(12), 7, false);
            take_brain_judgments()
        });
        let rec = records
            .iter()
            .find(|r| r.phase == BrainPhase::ResourceAdmission)
            .expect("a ResourceAdmission judgment must be pushed");
        assert_eq!(rec.decision, "defer");
        assert!(
            rec.context_summary.contains("disk=88%"),
            "{}",
            rec.context_summary
        );
        assert!(
            rec.context_summary.contains("ceiling=90%"),
            "{}",
            rec.context_summary
        );
        assert!(
            rec.rationale.contains("approaching ceiling"),
            "{}",
            rec.rationale
        );
    }

    #[test]
    fn metric_context_carries_resource_reasoning() {
        let d = reclaim("freeing 12 GiB of stale build caches");
        let s = resource_admission_metric_context("cand", &d, Some(92.0), 90.0, Some(22), 11);
        assert!(s.contains("decision=reclaim_first"), "s: {s}");
        assert!(s.contains("disk=92%/ceiling=90%"), "s: {s}");
        assert!(s.contains("worktrees=22"), "s: {s}");
        assert!(s.contains("in_flight=11"), "s: {s}");
        assert!(s.contains("freeing 12 GiB"), "s: {s}");
    }

    #[test]
    fn metric_context_renders_unknown_disk() {
        let s = resource_admission_metric_context("cand", &admit("ok"), None, 90.0, None, 0);
        assert!(s.contains("disk=unknown%/ceiling=90%"), "s: {s}");
        assert!(s.contains("worktrees=unknown"), "s: {s}");
    }
}
