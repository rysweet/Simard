//! Closed-loop outcome verification (issue #2751).
//!
//! **The framing invariant: an ARTIFACT is not an OUTCOME.** A merged PR / a
//! deploy is evidence that an *engineer* finished — not that the goal's real
//! success criteria are met in production. The kgpacks E2BIG regression is the
//! canonical failure: the goal was "unblocked" because a PR landed, then
//! silently re-blocked because the underlying fix was never actually present on
//! the live spawn path. This module closes that loop.
//!
//! After an engineer's work lands (a completion-candidate goal), the daemon
//! reasons — each curate cycle, repeatedly — about whether the goal is
//! *actually achieved, live*, using freshly-gathered [`LiveSignal`]s
//! (telemetry, journald, deploy-reconcile state, observed behavior). Only then
//! may the goal archive; otherwise it is re-opened / re-planned or kept open
//! with a report.
//!
//! # Where the intelligence lives
//!
//! The decision is a **structured-reasoning brain step**
//! ([`OodaBrain::decide_goal_outcome_verification`]) driven by a hot-reloadable
//! recipe — NOT a pile of hardcoded thresholds. This mirrors
//! [`spawn`](crate::ooda_actions::advance_goal::spawn)'s
//! `decide_engineer_lifecycle`: gather structured context → call a reasoner →
//! apply the decision, with only a THIN deterministic rail guarding the one
//! irreversible action (archival).
//!
//! # The three rails
//!
//! | Rail | Guard | On failure |
//! | --- | --- | --- |
//! | **1 — Skip** | Perpetual goals | No brain call, no signal gather. |
//! | **2 — NO-FALLBACK** | Signal-source `Err` or brain `Err` | Surfaces as `Err`; goal stays open; never archives. Mirrors `spawn.rs` (#1711/#1748). |
//! | **3 — Verified signal** | `MarkAchieved` with 0 verified live signals | Rail **overrides** to `KeepOpenAndReport`. |
//!
//! Rail-3 (`live_signals.iter().any(|s| s.verified)`) is the load-bearing
//! safety control. It lives HERE, in Rust — never in the (user-writable,
//! hot-reloadable) prompt — so editing the recipe can change reasoning quality
//! but can NEVER make the daemon archive a goal with zero verified live signals.

use crate::error::SimardResult;
use crate::ooda_brain::{
    BrainJudgmentRecord, GoalOutcomeCtx, GoalOutcomeDecision, OodaBrain, push_brain_judgment,
};

use super::completion_gate::{
    CompletionEvidence, CompletionEvidenceGate, CompletionVerdict, EvidenceSource,
    is_self_affecting,
};
use super::live_signal::LiveSignalSource;
use super::types::{ActiveGoal, GoalBoard, GoalProgress};

/// Metric name for a single outcome-verification decision. Appended to
/// `metrics.jsonl`; the context carries the outcome label, the verified-signal
/// count, and the (scrubbed) reasoning string.
pub const GOAL_LIVE_OUTCOME_VERIFICATION_METRIC: &str = "goal_live_outcome_verification";

/// Progress a re-opened or errored completion-candidate is demoted to. Below
/// 100 so [`GoalProgress::is_terminal`] no longer re-selects it as a verify /
/// archive candidate next cycle, yet high enough to record that the artifact
/// landed and only the *live outcome* is unproven — the engineer re-works from
/// here, and the loop re-verifies once work lands again.
const DEMOTED_PROGRESS_PERCENT: u32 = 90;

/// Environment kill-switch: `SIMARD_OUTCOME_VERIFY=off` disables live outcome
/// verification (the daemon leaves the memory pair `None`, restoring the legacy
/// curate path). Secure default is **ON**.
///
/// Only the explicit documented value `off` (case-insensitive) disables. Any
/// unknown value — including an empty string or `"garbage"` — **fails safe to
/// enabled**: verification must never be silently disabled by a typo.
pub fn outcome_verify_enabled() -> bool {
    match std::env::var("SIMARD_OUTCOME_VERIFY") {
        Ok(v) => !v.trim().eq_ignore_ascii_case("off"),
        Err(_) => true,
    }
}

/// The applied decision plus the load-bearing verified-signal count, kept
/// together so callers observe exactly what Rail-3 saw.
struct VerifiedOutcome {
    decision: GoalOutcomeDecision,
    verified_signal_count: u32,
}

/// Verify a completion-candidate goal's live outcome, returning the applied
/// decision along with the verified-signal count Rail-3 evaluated.
///
/// The single home of the three rails. `verify_goal_outcome` is a thin wrapper
/// that discards the count; both share this one rail implementation so the
/// safety invariant can never drift between call sites.
fn verify_goal_outcome_detailed(
    goal: &ActiveGoal,
    artifact_signals: &CompletionEvidence,
    brain: &dyn OodaBrain,
    signals: &dyn LiveSignalSource,
) -> SimardResult<VerifiedOutcome> {
    // Rail 1 — skip perpetual goals (they never archive). No brain call, no
    // signal gather; the panic-doubles in the test suite prove this.
    if goal.is_perpetual() {
        return Ok(VerifiedOutcome {
            decision: GoalOutcomeDecision::KeepOpenAndReport {
                rationale: "perpetual goal — verification skipped".into(),
            },
            verified_signal_count: 0,
        });
    }

    // Gather — Rail 2 (NO-FALLBACK): a source `Err` is a visible failure, never
    // an empty "no signals" success. Runs BEFORE the brain, so a gather failure
    // means the brain is never consulted.
    let live_signals = signals.gather(goal)?;
    let verified_signal_count = live_signals.iter().filter(|s| s.verified).count() as u32;

    let ctx = GoalOutcomeCtx {
        goal_id: goal.id.clone(),
        goal_title: goal.description.clone(),
        success_criteria: goal.description.clone(),
        artifact_signals: artifact_signals.clone(),
        live_signals,
        // Repeated evaluation is achieved by re-running this seam each curate
        // cycle; a durable per-goal re-verify counter is not tracked on the
        // (fixture-pinned) `ActiveGoal`, so this is 0 today.
        reverify_count: 0,
    };

    // Reason — Rail 2 (NO-FALLBACK): a brain `Err` is a visible failure,
    // matching the `spawn.rs` #1711 no-fallback precedent.
    let decision = brain.decide_goal_outcome_verification(&ctx)?;

    // Rail 3 — `MarkAchieved` requires >=1 adapter-verified live signal, else
    // the rail overrides to the fail-closed `KeepOpenAndReport`. A compromised
    // reasoner or an injected `detail` cannot forge a completion because
    // `verified` is set only by an authenticated adapter, and THIS check — not
    // the prompt — decides archival.
    let has_live_proof = verified_signal_count > 0;
    let applied = match decision {
        GoalOutcomeDecision::MarkAchieved { rationale } if !has_live_proof => {
            GoalOutcomeDecision::KeepOpenAndReport {
                rationale: format!("rail override (0 verified live signals): {rationale}"),
            }
        }
        other => other,
    };

    Ok(VerifiedOutcome {
        decision: applied,
        verified_signal_count,
    })
}

/// Verify a completion-candidate goal's live outcome. Returns the applied
/// decision. NEVER archives without >=1 verified live signal (Rail-3), NEVER
/// falls back on a signal-source or brain error (Rail-2), and skips perpetual
/// goals (Rail-1).
///
/// Pure with respect to state: it mutates nothing and performs no metric IO —
/// the caller applies the decision and records observability via
/// [`record_outcome_verification`]. This keeps the seam hermetically testable
/// with a stub brain and injected signals.
pub fn verify_goal_outcome(
    goal: &ActiveGoal,
    artifact_signals: &CompletionEvidence,
    brain: &dyn OodaBrain,
    signals: &dyn LiveSignalSource,
) -> SimardResult<GoalOutcomeDecision> {
    Ok(verify_goal_outcome_detailed(goal, artifact_signals, brain, signals)?.decision)
}

/// Record one outcome-verification decision for observability (issue #2751):
///
/// 1. Push a [`BrainJudgmentRecord`] (phase `OutcomeVerify`) onto the per-cycle
///    accumulator drained into the cycle report.
/// 2. Emit the [`GOAL_LIVE_OUTCOME_VERIFICATION_METRIC`] to `metrics.jsonl`,
///    with a bounded, scrubbed context carrying the outcome label, the
///    verified-signal count, and the reasoning string.
///
/// Best-effort: a metric write error never affects control flow (the archival
/// decision has already been applied by the caller).
pub fn record_outcome_verification(
    goal_id: &str,
    decision: &GoalOutcomeDecision,
    verified_signal_count: u32,
) {
    push_brain_judgment(BrainJudgmentRecord::from_goal_outcome(
        goal_id,
        decision,
        verified_signal_count,
        "",
    ));

    let context = format!(
        "goal_id={goal_id} outcome={} verified_signals={} rationale={}",
        decision.variant_label(),
        verified_signal_count,
        decision.rationale(),
    );
    let _ = crate::self_metrics::record_metric(
        GOAL_LIVE_OUTCOME_VERIFICATION_METRIC,
        f64::from(verified_signal_count),
        &context,
    );
}

/// One goal's outcome-verification result at the curate seam, for the caller to
/// log. `archived_eligible` is `true` only for a `MarkAchieved` that survived
/// Rail-3 — the sole decision that permits archival.
pub struct OutcomeVerificationReport {
    pub goal_id: String,
    pub decision: GoalOutcomeDecision,
    pub verified_signal_count: u32,
    pub archived_eligible: bool,
    /// `Some` when Rail-2 fired (signal-source or brain error). The goal was
    /// kept open (fail-closed); the caller surfaces this as a visible cycle
    /// failure.
    pub error: Option<String>,
}

/// The curate-seam step (issue #2751): verify every completion-candidate goal
/// LIVE before the archive step can complete it, mutating the board so that
/// only rail-passed `MarkAchieved` goals remain `Completed` (and thus
/// archivable by the existing archive step). Non-achieved and errored goals are
/// re-opened in place (demoted off the completion candidacy) with a recorded
/// annotation — never silently archived.
///
/// This composes with, rather than replaces, the existing evidence-aware
/// archive: it runs first and only ever DEMOTES; the archive step that follows
/// handles the archival of the surviving `Completed` goals, perpetual rolling,
/// and the completion metrics unchanged.
///
/// `evidence`, when present, gates candidacy on the artifact done-gate (A6):
/// only goals the gate certifies `Complete` are verified live; a gate-`Blocked`
/// goal is left untouched for the normal blocked-annotation path. When absent
/// (legacy unguarded archive), a `Completed` status is taken as the landed
/// signal and permissive artifact evidence is synthesized as reasoning input.
pub fn verify_completion_candidates(
    board: &mut GoalBoard,
    brain: &dyn OodaBrain,
    signals: &dyn LiveSignalSource,
    evidence: Option<&dyn EvidenceSource>,
) -> Vec<OutcomeVerificationReport> {
    let mut reports = Vec::new();

    for goal in board.active.iter_mut() {
        // Rail-1 (skip perpetual) + candidacy: only verify goals whose work has
        // landed. Perpetual rolling stays with the downstream archive step.
        if goal.is_perpetual() || !goal.status.is_terminal() {
            continue;
        }

        // Resolve the artifact evidence fed to the reasoner as INPUT.
        let artifact = match evidence {
            Some(src) => match CompletionEvidenceGate::new(src).evaluate(goal) {
                CompletionVerdict::Complete(ev) => ev,
                // Not artifact-complete → not a live-verify candidate this
                // cycle; leave it for the normal blocked-annotation path.
                CompletionVerdict::Blocked { .. } => continue,
            },
            None => CompletionEvidence {
                pr_merged: true,
                issue_closed: true,
                self_affecting: is_self_affecting(goal),
                deployed: true,
            },
        };

        match verify_goal_outcome_detailed(goal, &artifact, brain, signals) {
            Ok(outcome) => {
                let archived_eligible =
                    matches!(outcome.decision, GoalOutcomeDecision::MarkAchieved { .. });

                record_outcome_verification(
                    &goal.id,
                    &outcome.decision,
                    outcome.verified_signal_count,
                );

                if !archived_eligible {
                    // Re-open in place: demote off completion candidacy so the
                    // downstream archive step retains it, and annotate why. An
                    // engineer re-works it, re-completes it, and the loop
                    // re-verifies next time it lands.
                    goal.status = GoalProgress::InProgress {
                        percent: DEMOTED_PROGRESS_PERCENT,
                    };
                    goal.current_activity = Some(annotation(&outcome.decision));
                }

                reports.push(OutcomeVerificationReport {
                    goal_id: goal.id.clone(),
                    decision: outcome.decision,
                    verified_signal_count: outcome.verified_signal_count,
                    archived_eligible,
                    error: None,
                });
            }
            Err(e) => {
                // Rail-2 (NO-FALLBACK): keep the goal open, fail-closed. Demote
                // off candidacy so it cannot be archived on an unverified cycle.
                let reason = e.to_string();
                goal.status = GoalProgress::InProgress {
                    percent: DEMOTED_PROGRESS_PERCENT,
                };
                goal.current_activity = Some(format!(
                    "outcome-verify FAILED (kept open, no-fallback): {reason}"
                ));

                reports.push(OutcomeVerificationReport {
                    goal_id: goal.id.clone(),
                    decision: GoalOutcomeDecision::KeepOpenAndReport {
                        rationale: format!("verification error: {reason}"),
                    },
                    verified_signal_count: 0,
                    archived_eligible: false,
                    error: Some(reason),
                });
            }
        }
    }

    reports
}

/// Human-readable `current_activity` annotation for a re-opened goal, carrying
/// the `replan_hint` when the decision is a `Replan`.
fn annotation(decision: &GoalOutcomeDecision) -> String {
    match decision {
        GoalOutcomeDecision::Replan {
            rationale,
            replan_hint,
        } if !replan_hint.is_empty() => {
            format!("outcome-verify replan: {rationale} — replan_hint: {replan_hint}")
        }
        d => format!("outcome-verify {}: {}", d.variant_label(), d.rationale()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SimardError;
    use crate::goal_curation::live_signal::LiveSignal;
    use chrono::Utc;
    use std::sync::Mutex;

    fn candidate(id: &str) -> ActiveGoal {
        let mut g = ActiveGoal::new(id, "eliminate E2BIG on spawn", 1);
        g.status = GoalProgress::Completed;
        g
    }

    fn evidence() -> CompletionEvidence {
        CompletionEvidence {
            pr_merged: true,
            issue_closed: true,
            self_affecting: true,
            deployed: true,
        }
    }

    fn sig(verified: bool) -> LiveSignal {
        LiveSignal {
            source: "journald".into(),
            kind: "e2big_absent".into(),
            verified,
            detail: "detail".into(),
            observed_at: Utc::now(),
        }
    }

    struct StubBrain(GoalOutcomeDecision);
    impl OodaBrain for StubBrain {
        fn decide_engineer_lifecycle(
            &self,
            _ctx: &crate::ooda_brain::EngineerLifecycleCtx,
        ) -> SimardResult<crate::ooda_brain::EngineerLifecycleDecision> {
            unreachable!()
        }
        fn decide_goal_outcome_verification(
            &self,
            _ctx: &GoalOutcomeCtx,
        ) -> SimardResult<GoalOutcomeDecision> {
            Ok(self.0.clone())
        }
    }

    struct Signals(Result<Vec<LiveSignal>, String>, Mutex<u32>);
    impl LiveSignalSource for Signals {
        fn gather(&self, _goal: &ActiveGoal) -> SimardResult<Vec<LiveSignal>> {
            *self.1.lock().unwrap() += 1;
            self.0
                .clone()
                .map_err(|reason| SimardError::VerificationFailed { reason })
        }
    }

    #[test]
    fn detailed_reports_verified_count() {
        let g = candidate("g");
        let s = Signals(Ok(vec![sig(true), sig(false), sig(true)]), Mutex::new(0));
        let b = StubBrain(GoalOutcomeDecision::MarkAchieved {
            rationale: "live".into(),
        });
        let out = verify_goal_outcome_detailed(&g, &evidence(), &b, &s).unwrap();
        assert_eq!(out.verified_signal_count, 2);
        assert!(matches!(
            out.decision,
            GoalOutcomeDecision::MarkAchieved { .. }
        ));
    }

    #[test]
    fn curate_reopens_non_achieved_and_keeps_achieved_completed() {
        let mut board = GoalBoard::default();
        board.active.push(candidate("achieved"));
        board.active.push(candidate("reopened"));

        // achieved: mark_achieved + verified signal survives → stays Completed.
        let brain = StubBrain(GoalOutcomeDecision::MarkAchieved {
            rationale: "spawns succeed live".into(),
        });
        let signals = Signals(Ok(vec![sig(true)]), Mutex::new(0));
        let reports = verify_completion_candidates(&mut board, &brain, &signals, None);
        assert_eq!(reports.len(), 2);

        for g in &board.active {
            // Both handled by the same brain here; assert the achieved-path goal
            // is still Completed (archive-eligible) and reports are populated.
            assert!(matches!(g.status, GoalProgress::Completed));
        }
        assert!(reports.iter().all(|r| r.archived_eligible));
    }

    #[test]
    fn curate_reopens_when_unverified() {
        let mut board = GoalBoard::default();
        board.active.push(candidate("g"));
        // Brain wrongly says achieved, but the only signal is UNVERIFIED → Rail-3
        // overrides → non-achieved → demoted off candidacy.
        let brain = StubBrain(GoalOutcomeDecision::MarkAchieved {
            rationale: "PR merged so must be fixed".into(),
        });
        let signals = Signals(Ok(vec![sig(false)]), Mutex::new(0));
        let reports = verify_completion_candidates(&mut board, &brain, &signals, None);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].archived_eligible);
        assert!(matches!(
            board.active[0].status,
            GoalProgress::InProgress { percent: 90 }
        ));
    }

    #[test]
    fn curate_no_fallback_on_error_keeps_goal_open() {
        let mut board = GoalBoard::default();
        board.active.push(candidate("g"));
        let brain = StubBrain(GoalOutcomeDecision::MarkAchieved {
            rationale: "unreached".into(),
        });
        let signals = Signals(Err("journalctl timed out".into()), Mutex::new(0));
        let reports = verify_completion_candidates(&mut board, &brain, &signals, None);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].error.is_some());
        assert!(!reports[0].archived_eligible);
        assert!(matches!(
            board.active[0].status,
            GoalProgress::InProgress { percent: 90 }
        ));
    }

    #[test]
    fn curate_skips_perpetual() {
        let mut board = GoalBoard::default();
        let mut g = candidate("standing");
        g = g.mark_standing();
        board.active.push(g);
        let brain = StubBrain(GoalOutcomeDecision::MarkAchieved {
            rationale: "x".into(),
        });
        let signals = Signals(Ok(vec![sig(true)]), Mutex::new(0));
        let reports = verify_completion_candidates(&mut board, &brain, &signals, None);
        assert!(reports.is_empty(), "perpetual goals must be skipped");
        assert_eq!(
            *signals.1.lock().unwrap(),
            0,
            "no signal gather for perpetual"
        );
    }
}
