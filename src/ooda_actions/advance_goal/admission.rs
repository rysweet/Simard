//! Dependency/overlap-aware engineer **admission** gate (issue #2690).
//!
//! # The problem
//!
//! Per-goal single-flight (`find_live_engineer_for_goal` in
//! [`super::spawn`]) stops two engineers on the SAME goal. But two DIFFERENT
//! goals routinely touch the SAME files (`goals_status.rs`; the duplicate
//! multi-line-chat PRs #2698/#2696; the broken-main Adapter-rename incident), so
//! parallel engineers collide at merge — rebase churn, or broken main. There was
//! no cross-goal file-footprint awareness. This module adds it.
//!
//! # Where the intelligence lives
//!
//! The decision is a **structured-reasoning brain step**
//! ([`OodaBrain::decide_engineer_admission`]) driven by a hot-reloadable recipe
//! (`ooda-engineer-admission.yaml`) — NOT a pile of hardcoded thresholds. It
//! mirrors [`super::spawn`]'s `decide_engineer_lifecycle` and the outcome
//! verifier's `decide_goal_outcome_verification`: gather structured context →
//! call a reasoner → apply the decision, with only a THIN deterministic rail
//! guarding the one certain-collision case.
//!
//! # The two rails
//!
//! | Rail | Guard | On fire |
//! | --- | --- | --- |
//! | **1 — Exact-path (hard)** | Candidate scope non-empty AND `⊆` one live engineer's `changed_files` | Deterministic `Defer` (skip cycle, benign, no worktree, no failure) — overrides the brain. Inert when scope is empty. |
//! | **2 — Fail-open (soft)** | `decide_engineer_admission` returns `Err` | `Admit` via [`engineer_admission_fallback`] + loud `tracing::warn`. Never stalls. |
//!
//! Rail 1 is the load-bearing control. It lives HERE, in Rust — never in the
//! (user-writable, hot-reloadable) recipe — so editing the prompt can change
//! *scheduling quality* but can NEVER make the daemon start a second engineer on
//! top of one that already holds the exact target paths.
//!
//! Polarity note: unlike the outcome verifier (fail-**closed**), this gate is
//! fail-**open** — wrongly stalling a spawn is cheaper to recover from than
//! wrongly blocking the fleet.

use std::path::Path;

use super::overlap;
use crate::goal_curation::ActiveGoal;
use crate::ooda_brain::{
    BrainJudgmentRecord, CandidateGoal, EngineerAdmissionCtx, EngineerAdmissionDecision,
    LiveEngineerSignal, OodaBrain, prompt_store, push_brain_judgment,
};

/// Prompt-asset name for the admission reasoning prompt. Registered in
/// [`crate::ooda_brain::prompt_store`] (embedded fallback + hot-reload) so the
/// judgment record can stamp the exact prompt version that produced a decision.
pub const ADMISSION_PROMPT_NAME: &str = "ooda_engineer_admission.md";

/// Metric name for one admission decision, appended to `metrics.jsonl`. The
/// context carries the decision label, the blocking goal ids, and the (scrubbed)
/// overlap reasoning.
pub const ADMISSION_DECISION_METRIC: &str = "engineer_admission_decision";

/// The applied admission result the spawn seam acts on: either proceed to
/// worktree allocation with a (possibly hint-augmented) `task`, or skip this
/// cycle with a benign detail string (no worktree, no failure counted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdmissionOutcome {
    /// Spawn now. `task` is the original task, possibly augmented with a
    /// rebase-after scheduling hint (the `SerializeAfter` channel).
    Admit { task: String },
    /// Do NOT spawn this cycle. `detail` is the benign skip outcome the seam
    /// returns as `make_outcome(action, true, detail)`.
    Defer { detail: String },
}

/// Environment kill-switch: `SIMARD_ENGINEER_ADMISSION=off` disables the gate
/// (restoring today's collision-blind spawn). Secure default is **ON**.
///
/// Only the explicit documented value `off` (case-insensitive) disables. Any
/// unknown value — including an empty string or `"garbage"` — keeps scheduling
/// **enabled**: the gate must never be silently disabled by a typo. Because the
/// gate is already fail-open, this is an incident lever, not a safety necessity.
pub fn engineer_admission_enabled() -> bool {
    match std::env::var("SIMARD_ENGINEER_ADMISSION") {
        Ok(v) => admission_enabled_from(Some(&v)),
        Err(_) => true,
    }
}

/// Pure kill-switch classifier (testable without touching the process env).
fn admission_enabled_from(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => !v.trim().eq_ignore_ascii_case("off"),
        None => true,
    }
}

/// The fail-open Rail-2 fallback: admit, loudly. Returned when the brain errors
/// so a broken reasoner can never stall the fleet.
pub fn engineer_admission_fallback(_ctx: &EngineerAdmissionCtx) -> EngineerAdmissionDecision {
    EngineerAdmissionDecision::Admit {
        rationale: "fail-open (issue #2690): brain error — admitting; scheduling is an \
                    optimization, not a stall gate"
            .into(),
    }
}

/// Rail 1 — the deterministic exact-path collision predicate. Returns the
/// blocking engineer's `goal_id` when the candidate's non-empty predicted scope
/// is a **subset** of some single live engineer's `changed_files` (a CERTAIN
/// collision). `None` when the scope is empty (unknown ⇒ rail inert) or no
/// single engineer fully holds it.
fn exact_path_collision(ctx: &EngineerAdmissionCtx) -> Option<String> {
    use std::collections::BTreeSet;
    let scope: BTreeSet<&str> = ctx
        .candidate
        .predicted_scope
        .iter()
        .map(String::as_str)
        .collect();
    if scope.is_empty() {
        return None;
    }
    ctx.live_engineers.iter().find_map(|e| {
        let held: BTreeSet<&str> = e.changed_files.iter().map(String::as_str).collect();
        if scope.is_subset(&held) {
            Some(e.goal_id.clone())
        } else {
            None
        }
    })
}

/// The result of evaluating the admission rails over a ctx: the seam
/// [`AdmissionOutcome`] the caller acts on, plus the raw decision + `fallback`
/// flag the caller records for observability. Keeping observability OUT of the
/// pure evaluator (mirroring `outcome_verify::verify_goal_outcome`) means the
/// hermetic rail tests do not write metric files — only the dedicated
/// observability path (`run_admission_gate` / `record_engineer_admission`) does.
#[derive(Debug, Clone)]
pub(crate) struct AdmissionEvaluation {
    pub outcome: AdmissionOutcome,
    pub decision: EngineerAdmissionDecision,
    pub fallback: bool,
}

/// The seam core: run the two rails over an already-assembled
/// [`EngineerAdmissionCtx`] and apply the decision. **Pure** — mutates no board,
/// allocates no worktree, writes no metric, pushes no judgment. Returns the
/// outcome plus the decision/fallback for the caller to record. This keeps the
/// gate hermetically testable with a stub brain and injected live-engineer
/// signals, with zero filesystem side effects.
pub(crate) fn evaluate_admission(
    ctx: &EngineerAdmissionCtx,
    task: &str,
    brain: &dyn OodaBrain,
) -> AdmissionEvaluation {
    // Rail 1 — exact-path (hard, deterministic). A CERTAIN collision: the
    // candidate's non-empty scope is fully held by ONE live engineer. Defer
    // regardless of what the brain would say — this is the one control that
    // survives a broken or compromised (prompt-injected) reasoner.
    if let Some(blocker) = exact_path_collision(ctx) {
        let decision = EngineerAdmissionDecision::Defer {
            blocked_by: vec![blocker.clone()],
            rationale: format!(
                "exact-path rail: candidate scope fully held by live engineer '{blocker}' \
                 (certain merge collision)"
            ),
            retry_after_secs: None,
        };
        tracing::warn!(
            target: "simard::ooda_brain",
            goal = %ctx.candidate.id,
            blocked_by = %blocker,
            "engineer-admission: exact-path rail — deferring certain collision (overrides brain)",
        );
        let outcome = AdmissionOutcome::Defer {
            detail: defer_detail(&decision),
        };
        return AdmissionEvaluation {
            outcome,
            decision,
            // Deterministic rail block ⇒ marked as a fallback (lower confidence)
            // for the judgment record, consistent with the fail-open path.
            fallback: true,
        };
    }

    // Rail 2 — reason (fail-OPEN on error). A brain Err admits, but LOUDLY.
    let (decision, fallback) = match brain.decide_engineer_admission(ctx) {
        Ok(d) => (d, false),
        Err(e) => {
            tracing::warn!(
                target: "simard::ooda_brain",
                goal = %ctx.candidate.id,
                error = %e,
                "decide_engineer_admission FAILED — failing OPEN to Admit (issue #2690)",
            );
            (engineer_admission_fallback(ctx), true)
        }
    };

    let outcome = apply_admission(&decision, task);
    AdmissionEvaluation {
        outcome,
        decision,
        fallback,
    }
}

/// Translate a brain/rail decision into the seam's [`AdmissionOutcome`].
fn apply_admission(decision: &EngineerAdmissionDecision, task: &str) -> AdmissionOutcome {
    match decision {
        EngineerAdmissionDecision::Admit { .. } => AdmissionOutcome::Admit {
            task: task.to_string(),
        },
        EngineerAdmissionDecision::Defer { .. } => AdmissionOutcome::Defer {
            detail: defer_detail(decision),
        },
        EngineerAdmissionDecision::SerializeAfter {
            after_goal_id,
            overlap_files,
            ..
        } => AdmissionOutcome::Admit {
            task: append_rebase_hint(task, after_goal_id, overlap_files),
        },
    }
}

/// Human-readable benign skip detail for a `Defer`.
fn defer_detail(decision: &EngineerAdmissionDecision) -> String {
    format!(
        "spawn deferred (issue #2690): overlaps live engineer(s) {:?} — {}",
        decision.blocking_goals(),
        decision.rationale()
    )
}

/// Append an advisory rebase-after scheduling hint to the engineer `task`
/// string (the `SerializeAfter` channel — no new machinery). The engineer is
/// told to rebase onto the named goal's landed work before touching the
/// overlapping files.
pub(crate) fn append_rebase_hint(
    task: &str,
    after_goal_id: &str,
    overlap_files: &[String],
) -> String {
    let files = if overlap_files.is_empty() {
        "the overlapping files".to_string()
    } else {
        overlap_files.join(", ")
    };
    format!(
        "{task}\n\n[scheduling hint (issue #2690): a live engineer for goal '{after_goal_id}' is \
         editing {files}. Rebase onto its merged work before editing those files, and do NOT \
         re-open the same edits in parallel — serialize to avoid a merge collision.]"
    )
}

/// Record one admission decision for observability (issue #2690):
///
/// 1. Push a [`BrainJudgmentRecord`] (phase `EngineerAdmission`) onto the
///    per-cycle accumulator drained into the cycle report. The deterministic
///    Rail-1 block and the fail-open Rail-2 path both set `fallback = true`.
/// 2. Emit the [`ADMISSION_DECISION_METRIC`] to `metrics.jsonl` with a bounded
///    context carrying the decision label, the blocking goal ids, and the
///    reasoning string.
///
/// Best-effort: a metric write error never affects control flow.
pub(crate) fn record_engineer_admission(
    goal_id: &str,
    decision: &EngineerAdmissionDecision,
    fallback: bool,
) {
    push_brain_judgment(BrainJudgmentRecord::from_engineer_admission(
        goal_id,
        decision,
        fallback,
        prompt_store::current_version(ADMISSION_PROMPT_NAME),
    ));
    let _ = crate::self_metrics::record_metric(
        ADMISSION_DECISION_METRIC,
        decision.blocking_goals().len() as f64,
        &admission_metric_context(goal_id, decision),
    );
}

/// Build the bounded metric context string for an admission decision — carries
/// the overlap reasoning so `simard status` / metric queries can audit *why* a
/// spawn was deferred or serialized.
fn admission_metric_context(goal_id: &str, decision: &EngineerAdmissionDecision) -> String {
    format!(
        "goal_id={goal_id} decision={} blocked_by=[{}] rationale={}",
        decision.variant_label(),
        decision.blocking_goals().join(","),
        decision.rationale(),
    )
}

/// Assemble the [`EngineerAdmissionCtx`] for a candidate goal. **Pure &
/// best-effort**: every `gh` / `git` call is absent-tolerant and degrades to an
/// empty default. Never panics, never blocks, never shells out under the OODA
/// state lock (the seam calls this off-lock with a cloned goal snapshot).
///
/// - The candidate's predicted scope is derived best-effort from its prior PRs.
/// - Live engineers are the on-disk live-claimed set MINUS the candidate's own
///   goal (the same-goal case is already handled upstream by the lifecycle
///   branch), each annotated with its changed files and the overlap.
pub(crate) fn gather_engineer_admission_ctx(
    state_root: &Path,
    goal: &ActiveGoal,
    repo_root: &Path,
) -> EngineerAdmissionCtx {
    let predicted_scope = predict_candidate_scope(goal, repo_root);
    let candidate = CandidateGoal {
        id: goal.id.clone(),
        title: goal.description.clone(),
        predicted_scope: predicted_scope.clone(),
    };

    let mut live_engineers = Vec::new();
    for e in crate::engineer_worktree::live_claimed_engineers(state_root) {
        // The candidate's own goal is handled upstream (same-goal single-flight).
        if e.goal_id == goal.id {
            continue;
        }
        let changed = overlap::changed_files(&e.worktree_path, overlap::DEFAULT_BASE_BRANCH);
        let overlap_with_candidate = overlap::overlap(&predicted_scope, &changed);
        let depended_on = goal
            .wip_refs
            .iter()
            .any(|w| w.ref_id == e.goal_id || w.label.contains(&e.goal_id));
        live_engineers.push(LiveEngineerSignal {
            goal_id: e.goal_id,
            pid: e.pid,
            worktree_path: e.worktree_path.display().to_string(),
            changed_files: changed,
            overlap_with_candidate,
            depended_on,
        });
    }

    EngineerAdmissionCtx {
        candidate,
        live_engineers,
        repo_root: repo_root.display().to_string(),
    }
}

/// Best-effort prediction of a candidate goal's file footprint from its prior
/// PRs. For each `pr`-kind wip-ref, `gh pr view <n> --json files` in `repo_root`
/// yields the touched paths. Absent-tolerant: `gh` missing / any error ⇒ the
/// contribution is empty, and an empty overall scope makes the exact-path rail
/// inert (fail-open).
fn predict_candidate_scope(goal: &ActiveGoal, repo_root: &Path) -> Vec<String> {
    let mut scope: Vec<String> = Vec::new();
    for w in &goal.wip_refs {
        if !w.kind.eq_ignore_ascii_case("pr") {
            continue;
        }
        scope.extend(pr_files(&w.ref_id, repo_root));
    }
    scope.sort();
    scope.dedup();
    scope
}

/// Fetch the file list of a PR via `gh`, in `repo_root`. Empty on any error.
fn pr_files(pr: &str, repo_root: &Path) -> Vec<String> {
    let out = std::process::Command::new("gh")
        .current_dir(repo_root)
        .args(["pr", "view", pr, "--json", "files", "-q", ".files[].path"])
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().replace('\\', "/"))
        .filter(|l| !l.is_empty())
        .collect()
}

/// The full admission gate as invoked by the spawn seam: kill-switch → gather →
/// decide. When the gate is disabled (`SIMARD_ENGINEER_ADMISSION=off`) it skips
/// gather/reason/rails entirely and admits — no judgment, no metric.
pub(crate) fn run_admission_gate(
    state_root: &Path,
    goal: &ActiveGoal,
    repo_root: &Path,
    task: &str,
    brain: &dyn OodaBrain,
) -> AdmissionOutcome {
    if !engineer_admission_enabled() {
        return AdmissionOutcome::Admit {
            task: task.to_string(),
        };
    }
    let ctx = gather_engineer_admission_ctx(state_root, goal, repo_root);
    let eval = evaluate_admission(&ctx, task, brain);
    // Observability lives HERE (not in the pure evaluator): push the judgment and
    // emit the metric once, on the real decision path.
    record_engineer_admission(&ctx.candidate.id, &eval.decision, eval.fallback);
    eval.outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{SimardError, SimardResult};
    use crate::ooda_brain::{
        BrainPhase, EngineerLifecycleCtx, EngineerLifecycleDecision, take_brain_judgments,
        with_brain_judgment_scope,
    };

    /// A hermetic admission brain: returns the injected decision (or error) and
    /// records the ctx it was handed so tests can assert the gather→ctx wiring.
    struct StubAdmissionBrain(SimardResult<EngineerAdmissionDecision>);

    impl OodaBrain for StubAdmissionBrain {
        fn decide_engineer_lifecycle(
            &self,
            _ctx: &EngineerLifecycleCtx,
        ) -> SimardResult<EngineerLifecycleDecision> {
            unreachable!("admission tests never take the lifecycle path")
        }

        fn decide_engineer_admission(
            &self,
            _ctx: &EngineerAdmissionCtx,
        ) -> SimardResult<EngineerAdmissionDecision> {
            match &self.0 {
                Ok(d) => Ok(d.clone()),
                Err(_) => Err(SimardError::AdapterInvocationFailed {
                    base_type: "recipe-engineer-admission-brain".into(),
                    reason: "stub injected error".into(),
                }),
            }
        }
    }

    fn engineer(goal_id: &str, changed: &[&str]) -> LiveEngineerSignal {
        LiveEngineerSignal {
            goal_id: goal_id.into(),
            pid: 4242,
            worktree_path: format!("/tmp/{goal_id}"),
            changed_files: changed.iter().map(|s| s.to_string()).collect(),
            overlap_with_candidate: Vec::new(),
            depended_on: false,
        }
    }

    fn ctx(
        candidate_id: &str,
        scope: &[&str],
        live: Vec<LiveEngineerSignal>,
    ) -> EngineerAdmissionCtx {
        EngineerAdmissionCtx {
            candidate: CandidateGoal {
                id: candidate_id.into(),
                title: "some goal".into(),
                predicted_scope: scope.iter().map(|s| s.to_string()).collect(),
            },
            live_engineers: live,
            repo_root: "/tmp/repo".into(),
        }
    }

    fn stub(d: EngineerAdmissionDecision) -> StubAdmissionBrain {
        StubAdmissionBrain(Ok(d))
    }

    // T1 — stub Defer ⇒ benign skip outcome, no worktree (the seam returns
    // Defer; dispatch turns this into make_outcome(action, true, ...)).
    #[test]
    fn t1_stub_defer_yields_skip_outcome() {
        let c = ctx("cand", &[], vec![]);
        let brain = stub(EngineerAdmissionDecision::Defer {
            blocked_by: vec!["other".into()],
            rationale: "collides".into(),
            retry_after_secs: None,
        });
        let out = evaluate_admission(&c, "task", &brain).outcome;
        match out {
            AdmissionOutcome::Defer { detail } => {
                assert!(detail.contains("deferred"), "detail: {detail}");
                assert!(detail.contains("other"), "names blocker: {detail}");
            }
            other => panic!("expected Defer, got {other:?}"),
        }
    }

    // T2 — stub Admit ⇒ proceed with the unchanged task.
    #[test]
    fn t2_stub_admit_proceeds_with_task() {
        let c = ctx("cand", &[], vec![]);
        let brain = stub(EngineerAdmissionDecision::Admit {
            rationale: "independent".into(),
        });
        let out = evaluate_admission(&c, "original task", &brain).outcome;
        assert_eq!(
            out,
            AdmissionOutcome::Admit {
                task: "original task".into()
            }
        );
    }

    // T3 — stub SerializeAfter ⇒ proceed, task carries the rebase-after hint.
    #[test]
    fn t3_serialize_after_augments_task() {
        let c = ctx("cand", &[], vec![]);
        let brain = stub(EngineerAdmissionDecision::SerializeAfter {
            after_goal_id: "goals-status-fix".into(),
            overlap_files: vec!["src/operator_commands_ooda/goals_status.rs".into()],
            rationale: "shared file".into(),
        });
        let out = evaluate_admission(&c, "original task", &brain).outcome;
        match out {
            AdmissionOutcome::Admit { task } => {
                assert!(task.starts_with("original task"), "keeps base task: {task}");
                assert!(
                    task.contains("goals-status-fix"),
                    "names after goal: {task}"
                );
                assert!(
                    task.contains("goals_status.rs"),
                    "names overlap file: {task}"
                );
                assert!(task.contains("scheduling hint"), "carries hint: {task}");
            }
            other => panic!("expected Admit with hint, got {other:?}"),
        }
    }

    // T5 — exact-path rail: a different-goal live engineer whose changed_files
    // COVER the candidate's scope ⇒ deterministic Defer even when the stub says
    // Admit. The rail overrides the brain (also T-sec2).
    #[test]
    fn t5_exact_path_rail_overrides_stub_admit() {
        let c = ctx(
            "cand",
            &["src/goals_status.rs"],
            vec![engineer("other", &["src/goals_status.rs", "src/extra.rs"])],
        );
        // Stub would ADMIT — the rail must still block.
        let brain = stub(EngineerAdmissionDecision::Admit {
            rationale: "prompt says parallelize".into(),
        });
        let out = evaluate_admission(&c, "task", &brain).outcome;
        match out {
            AdmissionOutcome::Defer { detail } => {
                assert!(detail.contains("other"), "names holding engineer: {detail}");
            }
            other => panic!("exact-path rail must Defer, got {other:?}"),
        }
    }

    // Issue #2935: raising the per-cycle count ceiling to 24 must NOT bypass the
    // exact-path overlap rail. Even when the daemon may spawn up to 24 engineers,
    // a candidate whose predicted scope is fully held by a live engineer (a
    // duplicate/overlapping goal) is deterministically deferred — a brain that
    // would Admit is overridden. The count cap and the overlap gate are
    // independent axes; the higher cap only raises the number of *independent*
    // goals that may run.
    #[test]
    fn count_cap_24_does_not_bypass_exact_path_rail() {
        let c = ctx(
            "cand",
            &["src/shared.rs"],
            vec![engineer("other", &["src/shared.rs", "src/other.rs"])],
        );
        let brain = stub(EngineerAdmissionDecision::Admit {
            rationale: "count cap is 24 — plenty of room".into(),
        });
        let out = evaluate_admission(&c, "task", &brain).outcome;
        match out {
            AdmissionOutcome::Defer { detail } => {
                assert!(
                    detail.contains("other"),
                    "names the blocking engineer: {detail}"
                );
            }
            other => panic!(
                "exact-path rail must still Defer a duplicate/overlapping goal at count cap 24, got {other:?}"
            ),
        }
    }

    // T6 — brain Err ⇒ fail OPEN to Admit (never a silent stall).
    #[test]
    fn t6_brain_error_fails_open_to_admit() {
        let c = ctx("cand", &[], vec![]);
        let brain = StubAdmissionBrain(Err(SimardError::AdapterInvocationFailed {
            base_type: "x".into(),
            reason: "y".into(),
        }));
        let out = evaluate_admission(&c, "task", &brain).outcome;
        assert_eq!(
            out,
            AdmissionOutcome::Admit {
                task: "task".into()
            },
            "brain error must fail OPEN"
        );
    }

    // T7 — empty candidate scope ⇒ exact-path rail inert ⇒ brain honored.
    #[test]
    fn t7_empty_scope_rail_inert_brain_honored() {
        // A live engineer touches files, but the candidate scope is UNKNOWN
        // (empty) so the rail cannot fire; the stub's Defer must be honored.
        let c = ctx("cand", &[], vec![engineer("other", &["src/a.rs"])]);
        let brain = stub(EngineerAdmissionDecision::Defer {
            blocked_by: vec!["other".into()],
            rationale: "brain-judged overlap".into(),
            retry_after_secs: None,
        });
        let out = evaluate_admission(&c, "task", &brain).outcome;
        assert!(matches!(out, AdmissionOutcome::Defer { .. }));

        // And with the SAME empty scope + a live engineer, a stub Admit is also
        // honored (the rail does not force a block on unknown scope).
        let brain_admit = stub(EngineerAdmissionDecision::Admit {
            rationale: "independent".into(),
        });
        let out2 = evaluate_admission(&c, "task", &brain_admit).outcome;
        assert_eq!(
            out2,
            AdmissionOutcome::Admit {
                task: "task".into()
            }
        );
    }

    // T8 — observability: a judgment record (phase EngineerAdmission) is pushed
    // on the decision path, and the metric context carries the overlap reasoning.
    // This is the one test that exercises `record_engineer_admission` (judgment +
    // metric); the rail tests above stay pure (no filesystem side effects).
    #[test]
    fn t8_pushes_engineer_admission_judgment() {
        let c = ctx("cand", &[], vec![]);
        let brain = stub(EngineerAdmissionDecision::Defer {
            blocked_by: vec!["other".into()],
            rationale: "collides on goals_status.rs".into(),
            retry_after_secs: None,
        });
        let records = with_brain_judgment_scope(|| {
            let eval = evaluate_admission(&c, "task", &brain);
            record_engineer_admission(&c.candidate.id, &eval.decision, eval.fallback);
            take_brain_judgments()
        });
        let rec = records
            .iter()
            .find(|r| r.phase == BrainPhase::EngineerAdmission)
            .expect("an EngineerAdmission judgment must be pushed");
        assert_eq!(rec.decision, "defer");
        assert!(
            rec.rationale.contains("goals_status.rs"),
            "rationale: {}",
            rec.rationale
        );
    }

    #[test]
    fn t8_metric_context_carries_overlap_reasoning() {
        let d = EngineerAdmissionDecision::Defer {
            blocked_by: vec!["other-goal".into()],
            rationale: "already rewriting src/x.rs".into(),
            retry_after_secs: None,
        };
        let ctx_str = admission_metric_context("cand", &d);
        assert!(ctx_str.contains("decision=defer"));
        assert!(ctx_str.contains("blocked_by=[other-goal]"));
        assert!(ctx_str.contains("already rewriting src/x.rs"));
    }

    // T10 — kill-switch classifier: `off` disables, everything else stays ON.
    #[test]
    fn t10_kill_switch_classifier() {
        assert!(!admission_enabled_from(Some("off")));
        assert!(!admission_enabled_from(Some("  OFF  ")));
        assert!(admission_enabled_from(Some("on")));
        assert!(admission_enabled_from(Some("")));
        assert!(admission_enabled_from(Some("garbage")));
        assert!(admission_enabled_from(None));
    }

    // T10 — gate disabled ⇒ Admit with the untouched task, no gather (a bogus
    // state_root/repo would otherwise be walked). Hermetic: env set + restored,
    // serialized via the `cognitive_memory` key (process-global env mutation).
    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn t10_gate_disabled_admits_without_gather() {
        let goal = ActiveGoal::new("cand", "desc", 1);
        let brain = stub(EngineerAdmissionDecision::Defer {
            blocked_by: vec!["x".into()],
            rationale: "would block".into(),
            retry_after_secs: None,
        });
        let tmp = tempfile::tempdir().unwrap();

        // SAFETY: serialized via the `cognitive_memory` serial key above; set and
        // restored within this single-threaded window.
        unsafe { std::env::set_var("SIMARD_ENGINEER_ADMISSION", "off") };
        let out = run_admission_gate(tmp.path(), &goal, tmp.path(), "task", &brain);
        unsafe { std::env::remove_var("SIMARD_ENGINEER_ADMISSION") };

        assert_eq!(
            out,
            AdmissionOutcome::Admit {
                task: "task".into()
            }
        );
    }

    // T-sec2 — a prompt that says `admit` cannot override the hard exact-path
    // rail (covered by t5); here assert the rail fires only on a FULL subset,
    // not a partial overlap (a partial overlap is left to the brain).
    #[test]
    fn tsec2_rail_requires_full_subset_not_partial() {
        // Candidate needs TWO files; the live engineer holds only ONE of them.
        let c = ctx(
            "cand",
            &["src/a.rs", "src/b.rs"],
            vec![engineer("other", &["src/a.rs"])],
        );
        // Partial overlap ⇒ rail inert ⇒ brain (Admit) honored.
        let brain = stub(EngineerAdmissionDecision::Admit {
            rationale: "only partial overlap, safe to parallelize".into(),
        });
        let out = evaluate_admission(&c, "task", &brain).outcome;
        assert_eq!(
            out,
            AdmissionOutcome::Admit {
                task: "task".into()
            }
        );
    }

    // gather: no live engineers ⇒ empty live set, candidate id/title populated.
    #[test]
    fn gather_populates_candidate_and_excludes_nothing_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let goal = ActiveGoal::new("cand", "the description", 1);
        let out = gather_engineer_admission_ctx(tmp.path(), &goal, tmp.path());
        assert_eq!(out.candidate.id, "cand");
        assert_eq!(out.candidate.title, "the description");
        assert!(out.live_engineers.is_empty());
        assert!(out.repo_root.contains(tmp.path().to_str().unwrap()));
    }
}
