//! TEST-FIRST (Step 7 TDD) for the *agentic root-cause* no-progress breaker
//! (issue #16). These specify the pure policy layer of the upgraded safeguard
//! described in `docs/concepts/no-progress-root-cause-resolution.md`.
//!
//! # What is being specified
//!
//! Today the breaker, at the threshold, runs the done-gate once and either marks
//! the goal done, drops it, or **escalates to a bare "needs human review" block
//! that states no reason**. The production incident: seven `kgpacks-rs` goals
//! were parked as "no progress" when the work was *already done* (issues
//! CLOSED, workstream PRs MERGED) — the brain kept returning no-action because
//! there was nothing left to do, but nothing marked the goals complete, so the
//! safeguard misread "done" as "stuck".
//!
//! The upgrade: BEFORE authoring any block, run a root-cause investigation that
//! classifies **WHY** the goal made no shippable progress into one of the
//! stable tokens below, gathers structured evidence, and routes the goal down a
//! self-resolving ladder — only escalating to a human as a last resort, and then
//! **with the concrete WHY + evidence attached to the block reason** (never a
//! bare "needs human review").
//!
//! These are the *pure* tests: classification tokens, the WHY/evidence value
//! types, the WHY-aware block-reason renderer (which must preserve the existing
//! sentinel-recognition + count-parse invariants), and the pure
//! classification -> resolution map. Side-effecting wiring is specified in
//! `crate::ooda_loop::tests_no_progress_investigation`.
//!
//! They are RED until the implementation (`no_progress_why` module + the
//! extended `no_progress_breaker` policy) exists.

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BLOCKED_PREFIX, NO_PROGRESS_BREAKER_THRESHOLD, NoProgressResolution,
    is_no_progress_marker, no_progress_blocked_reason, no_progress_blocked_reason_with_why,
    resolution_for_why,
};
use crate::goal_curation::no_progress_why::{
    Evidence, NoProgressClass, NoProgressWhy, NoProgressWhyReasoner,
};
use crate::goal_curation::types::ActiveGoal;

// --- fixtures ---------------------------------------------------------------

fn goal(id: &str) -> ActiveGoal {
    ActiveGoal::new(id, "harden the supply chain", 1)
}

/// Evidence for a merged PR / closed issue pair (the `kgpacks-rs` "already done"
/// shape).
fn already_done_evidence() -> Vec<Evidence> {
    vec![
        Evidence::new("issue", "#16", "CLOSED"),
        Evidence::new("pr", "#33", "MERGED"),
    ]
}

// --- classification tokens are a stable, durable contract -------------------

#[test]
fn classification_tokens_are_the_documented_stable_set() {
    // These tokens travel verbatim into block reasons, tracking issues, logs and
    // metrics, so — like the goal-edge type strings — they are a durable
    // cross-system contract and must never drift.
    assert_eq!(NoProgressClass::AlreadyComplete.token(), "ALREADY-COMPLETE");
    assert_eq!(
        NoProgressClass::UpstreamDependency.token(),
        "UPSTREAM-DEPENDENCY"
    );
    assert_eq!(
        NoProgressClass::MissingPrecondition.token(),
        "MISSING-PRECONDITION"
    );
    assert_eq!(NoProgressClass::UnclearCriteria.token(), "UNCLEAR-CRITERIA");
    assert_eq!(NoProgressClass::GenuinelyStuck.token(), "GENUINELY-STUCK");
}

#[test]
fn every_class_has_a_nonempty_uppercase_hyphenated_token() {
    for class in NoProgressClass::ALL {
        let t = class.token();
        assert!(!t.is_empty(), "token must be non-empty for {class:?}");
        assert!(
            t.chars().all(|c| c.is_ascii_uppercase() || c == '-'),
            "token {t:?} must be UPPERCASE-HYPHENATED (durable contract)"
        );
    }
}

// --- evidence value type ----------------------------------------------------

#[test]
fn evidence_renders_a_human_readable_reference() {
    let e = Evidence::new("issue", "#16", "CLOSED");
    let rendered = e.render();
    assert!(rendered.contains("issue"), "render was {rendered:?}");
    assert!(rendered.contains("#16"), "render was {rendered:?}");
    assert!(rendered.contains("CLOSED"), "render was {rendered:?}");
}

#[test]
fn why_renders_bracketed_evidence_list_or_none() {
    let why = NoProgressWhy::new(NoProgressClass::AlreadyComplete, already_done_evidence());
    let rendered = why.render_evidence();
    assert!(rendered.contains("#16"), "evidence list was {rendered:?}");
    assert!(rendered.contains("#33"), "evidence list was {rendered:?}");

    // No evidence must render an explicit sentinel, never an empty string, so an
    // escalation reason always reads coherently.
    let empty = NoProgressWhy::new(NoProgressClass::GenuinelyStuck, vec![]);
    let none = empty.render_evidence();
    assert!(
        !none.trim().is_empty(),
        "empty-evidence render must be an explicit token, got {none:?}"
    );
}

// --- the WHY-aware block-reason renderer (the headline requirement) ----------

#[test]
fn why_aware_block_reason_carries_classification_and_evidence_and_is_not_bare() {
    // When a block is truly unavoidable, the reason MUST carry the classified
    // WHY + evidence — never the old bare "needs human review" sentinel with no
    // diagnosis.
    let n = 3;
    let why = NoProgressWhy::new(
        NoProgressClass::GenuinelyStuck,
        vec![Evidence::new("pr", "#7", "OPEN")],
    );
    let reason = no_progress_blocked_reason_with_why(n, &why);

    // Carries the classification token…
    assert!(
        reason.contains("GENUINELY-STUCK"),
        "block reason must name the classification: {reason:?}"
    );
    // …and the concrete evidence link…
    assert!(
        reason.contains("#7"),
        "block reason must attach the evidence: {reason:?}"
    );
    // …and is strictly richer than the bare generic sentinel.
    assert_ne!(
        reason,
        no_progress_blocked_reason(n),
        "WHY-bearing reason must not equal the bare generic sentinel"
    );
    assert!(
        reason.len() > no_progress_blocked_reason(n).len(),
        "WHY-bearing reason must be strictly richer than the bare sentinel"
    );
}

#[test]
fn why_aware_block_reason_preserves_sentinel_recognition_and_count_parse() {
    // Backward-compat invariant: the WHY is appended to (never replaces) the
    // existing sentinel, so the self-heal predicate (`is_no_progress_marker`),
    // `simard goal unblock-all`, and the overseer's `{prefix}{count}` parse all
    // keep working unchanged on the richer string.
    let n = 4;
    let why = NoProgressWhy::new(
        NoProgressClass::UnclearCriteria,
        vec![Evidence::new("issue", "#21", "OPEN")],
    );
    let reason = no_progress_blocked_reason_with_why(n, &why);

    assert!(
        reason.starts_with(NO_PROGRESS_BLOCKED_PREFIX),
        "must keep the [OODA-SAFEGUARD] prefix: {reason:?}"
    );
    assert!(
        is_no_progress_marker(&reason),
        "self-heal / unblock-all must still recognise the marker: {reason:?}"
    );

    // The overseer parses the count by stripping the prefix and reading leading
    // digits; the appended WHY must not disturb that.
    let rest = reason
        .strip_prefix(NO_PROGRESS_BLOCKED_PREFIX)
        .expect("prefix present");
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    assert_eq!(
        digits.parse::<u32>().ok(),
        Some(n),
        "count must still parse as {n} from {reason:?}"
    );
}

// --- the pure classification -> resolution map ------------------------------

#[test]
fn already_complete_maps_to_mark_done() {
    let why = NoProgressWhy::new(NoProgressClass::AlreadyComplete, already_done_evidence());
    assert_eq!(
        resolution_for_why(NO_PROGRESS_BREAKER_THRESHOLD, why, false),
        NoProgressResolution::MarkDone,
        "a goal proven done by live artifacts must auto-complete, not block"
    );
}

#[test]
fn missing_precondition_maps_to_heal() {
    let why = NoProgressWhy::new(
        NoProgressClass::MissingPrecondition,
        vec![Evidence::new("repo", "kgpacks-rs", "absent")],
    );
    match resolution_for_why(NO_PROGRESS_BREAKER_THRESHOLD, why, false) {
        NoProgressResolution::Heal { why } => {
            assert_eq!(why.class, NoProgressClass::MissingPrecondition);
        }
        other => panic!("expected Heal for a missing precondition, got {other:?}"),
    }
}

#[test]
fn upstream_dependency_maps_to_defer_with_the_blocking_ref() {
    let why = NoProgressWhy::new(
        NoProgressClass::UpstreamDependency,
        vec![Evidence::new("dependency-goal", "upstream-goal", "OPEN")],
    );
    match resolution_for_why(NO_PROGRESS_BREAKER_THRESHOLD, why, false) {
        NoProgressResolution::Defer { blocking_ref, .. } => {
            assert!(
                blocking_ref.contains("upstream-goal"),
                "defer must record the specific blocking upstream: {blocking_ref:?}"
            );
        }
        other => panic!("expected Defer for an upstream dependency, got {other:?}"),
    }
}

#[test]
fn unclear_or_stuck_maps_to_spawn_engineer_on_the_first_attempt() {
    // First qualifying threshold with no prior guided retry: spawn an engineer
    // to investigate+fix the WHY — do NOT go straight to a human.
    for class in [
        NoProgressClass::UnclearCriteria,
        NoProgressClass::GenuinelyStuck,
    ] {
        let why = NoProgressWhy::new(class, vec![Evidence::new("pr", "#7", "OPEN")]);
        match resolution_for_why(
            NO_PROGRESS_BREAKER_THRESHOLD,
            why,
            /* guided_retry_used */ false,
        ) {
            NoProgressResolution::SpawnEngineer { task, .. } => {
                assert!(
                    task.to_ascii_lowercase().contains("#7")
                        || task.to_ascii_lowercase().contains("stuck")
                        || task.to_ascii_lowercase().contains("why"),
                    "the engineer task must embed the WHY as guidance, got {task:?}"
                );
            }
            other => panic!("expected SpawnEngineer for {class:?} first attempt, got {other:?}"),
        }
    }
}

#[test]
fn stuck_escalates_with_why_only_after_the_guided_retry_is_exhausted() {
    // The bounded ladder: once a guided engineer retry has already been spent on
    // this goal and it is STILL stuck, escalate to a human — but with the
    // concrete WHY + evidence attached, never a bare "needs human review".
    let why = NoProgressWhy::new(
        NoProgressClass::GenuinelyStuck,
        vec![Evidence::new("pr", "#7", "OPEN")],
    );
    match resolution_for_why(
        NO_PROGRESS_BREAKER_THRESHOLD,
        why,
        /* guided_retry_used */ true,
    ) {
        NoProgressResolution::Escalate {
            blocked_reason,
            issue_title,
            issue_body,
        } => {
            assert!(
                is_no_progress_marker(&blocked_reason),
                "escalation must still carry the sentinel: {blocked_reason:?}"
            );
            assert!(
                blocked_reason.contains("GENUINELY-STUCK"),
                "escalation reason must name the WHY: {blocked_reason:?}"
            );
            assert!(
                blocked_reason.contains("#7"),
                "escalation reason must attach evidence: {blocked_reason:?}"
            );
            assert_ne!(
                blocked_reason,
                no_progress_blocked_reason(NO_PROGRESS_BREAKER_THRESHOLD),
                "escalation reason must never be the bare generic sentinel"
            );
            assert!(
                issue_title.to_ascii_lowercase().contains("stuck")
                    || issue_title.contains("GENUINELY-STUCK"),
                "issue title should reflect the diagnosis: {issue_title:?}"
            );
            assert!(
                issue_body.contains("GENUINELY-STUCK") && issue_body.contains("#7"),
                "issue body must carry the WHY + evidence: {issue_body:?}"
            );
        }
        other => panic!("expected Escalate once the guided retry is spent, got {other:?}"),
    }
}

#[test]
fn terminal_stuck_with_no_evidence_surfaces_a_failure_never_a_none_block() {
    // THE live-daemon defect (2026-07-15): a goal that never produced a tracked
    // issue/PR (the six `simard-identity-*`, the coverage/coin/parity goals) has
    // empty evidence, so the terminal rung used to author
    //   `🔒 [OODA-SAFEGUARD] … why=GENUINELY-STUCK evidence=[(none)]`.
    // The pure policy must NEVER emit an Escalate here — an evidence-less terminal
    // outcome is a SURFACED investigation failure (fail visible + retriable), not
    // a bare generic block.
    for class in [
        NoProgressClass::GenuinelyStuck,
        NoProgressClass::UnclearCriteria,
    ] {
        let why = NoProgressWhy::new(class, vec![]);
        match resolution_for_why(
            NO_PROGRESS_BREAKER_THRESHOLD,
            why,
            /* guided_retry_used */ true,
        ) {
            NoProgressResolution::SurfaceInvestigationFailure {
                class: surfaced_class,
                reason,
            } => {
                assert_eq!(
                    surfaced_class, class,
                    "the surfaced failure must carry the classified WHY so a bounded \
                     escalation can name the accurate root cause"
                );
                assert!(
                    reason.contains(class.token()),
                    "the surfaced failure should name the classified WHY: {reason:?}"
                );
                assert!(
                    !reason.contains("(none)"),
                    "even the surfaced-failure reason must not read as an \
                     evidence=[(none)] block: {reason:?}"
                );
            }
            other => panic!(
                "an evidence-less terminal outcome for {class:?} must surface a failure, \
                 never Escalate/park with (none), got {other:?}"
            ),
        }
    }
}

// --- the reasoner seam (agentic investigation is injected) ------------------

/// A hermetic fake reasoner: returns a canned finding (or a canned error) so the
/// investigation is exercised without a recipe run / any I/O.
struct FakeReasoner {
    result: Result<NoProgressWhy, String>,
}

impl NoProgressWhyReasoner for FakeReasoner {
    fn investigate(&self, _goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
        self.result
            .clone()
            .map_err(|reason| SimardError::VerificationFailed { reason })
    }
}

#[test]
fn reasoner_trait_returns_a_classified_why() {
    let reasoner = FakeReasoner {
        result: Ok(NoProgressWhy::new(
            NoProgressClass::AlreadyComplete,
            already_done_evidence(),
        )),
    };
    let why = reasoner
        .investigate(&goal("kgpacks-issue-16"))
        .expect("fake reasoner returns Ok");
    assert_eq!(why.class, NoProgressClass::AlreadyComplete);
    assert_eq!(why.evidence.len(), 2);
}

#[test]
fn reasoner_error_is_propagated_for_the_caller_to_fail_closed() {
    // A reasoner failure must surface as an Err so the adapter can fail CLOSED
    // (take no terminal action, neither block nor complete) — never silently
    // swallow it into a spurious block or completion.
    let reasoner = FakeReasoner {
        result: Err("recipe transport failed".to_string()),
    };
    let err = reasoner
        .investigate(&goal("g"))
        .expect_err("reasoner failure must be an Err");
    assert!(
        err.to_string().contains("recipe transport failed"),
        "the concrete cause must be preserved: {err}"
    );
}
