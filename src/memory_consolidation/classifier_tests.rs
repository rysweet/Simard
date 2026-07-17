//! TDD (RED) tests for the episode-ingestion classifier (issue #2327).
//!
//! These tests are written **before** the production code and are expected
//! to FAIL TO COMPILE until the classifier module lands. That unresolved-path
//! error IS the intended red signal — it is concrete and deterministic, and
//! `cargo build --release` stays green because this module is `#[cfg(test)]`.
//!
//! ## What the classifier introduces
//!
//! A new module `crate::memory_consolidation::classifier` with a pure
//! function used at every `store_episode` site to decide whether an episode
//! is operational noise (DROP), low-value bookkeeping (DOWN-SCOPE), or a
//! meaningful episodic event worth full-importance storage (STORE):
//!
//! ```ignore
//! pub enum EventKind {
//!     ActionFailure, ActionCompleted, Handoff, GoalArchival,
//!     GoalPromotion, UserDecision, RecipeFailure, Operational,
//! }
//! pub struct EpisodeMetadata {
//!     pub importance: f64,
//!     pub event_kind: EventKind,
//!     pub goal_id: Option<String>,
//!     pub cycle: Option<u32>,
//!     pub is_operational: bool,
//! }
//! pub struct IntakeContext { pub goal_id: Option<String>, pub cycle: Option<u32> }
//! pub enum IntakeDecision { Drop, DownScope(EpisodeMetadata), Store(EpisodeMetadata) }
//! pub fn classify(content: &str, source_label: &str, ctx: &IntakeContext) -> IntakeDecision;
//! ```
//!
//! Contract (issue #2327, EPISODE INGESTION POLICY):
//! - DROP: session start/complete/persist markers, "flushing working memory",
//!   and "brain: continue_skipping (… no decision keyword …)" details.
//! - DOWN-SCOPE: operational bookkeeping (e.g. consolidation-intake hydration
//!   summaries) — persisted but at low importance with `is_operational = true`.
//! - STORE (full importance, `is_operational = false`): action failures,
//!   completed actions with durable outcomes, handoffs, goal archival /
//!   promotions, user decisions, recipe failures.
//! - OVERRIDE: any episode carrying a failure/error summary is STORED even if
//!   it also matches a drop marker (A7).

use crate::memory_consolidation::classifier::{
    EpisodeMetadata, EventKind, IntakeContext, classify,
};

// ───────────────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────────────

fn ctx(goal_id: Option<&str>, cycle: Option<u32>) -> IntakeContext {
    IntakeContext {
        goal_id: goal_id.map(str::to_string),
        cycle,
    }
}

fn empty_ctx() -> IntakeContext {
    ctx(None, None)
}

// ───────────────────────────────────────────────────────────────────────────
// DROP: operational-noise markers
// ───────────────────────────────────────────────────────────────────────────

/// `intake_memory_operations` writes
/// `"Session {id} started with objective: {obj}"` under `session-intake`.
/// That is pure session-lifecycle noise and MUST be dropped.
#[test]
fn noise_session_intake_marker_is_dropped() {
    let decision = classify(
        "Session sess-abc started with objective: ship the feature",
        "session-intake",
        &empty_ctx(),
    );
    assert!(
        decision.is_dropped(),
        "session-intake start marker must be dropped, got {decision:?}"
    );
    assert!(
        decision.metadata().is_none(),
        "a dropped episode has no metadata (it is never stored)"
    );
}

/// `persistence_memory_operations` writes
/// `"Session {id} completed and persisted"` under `session-persistence`.
#[test]
fn noise_session_persistence_marker_is_dropped() {
    let decision = classify(
        "Session sess-abc completed and persisted",
        "session-persistence",
        &empty_ctx(),
    );
    assert!(
        decision.is_dropped(),
        "session-persistence completion marker must be dropped, got {decision:?}"
    );
}

/// `consolidation_persistence` writes
/// `"Session {id} flushing working memory to episodes"`.
#[test]
fn noise_flushing_working_memory_is_dropped() {
    let decision = classify(
        "Session sess-abc flushing working memory to episodes",
        "consolidation-persistence",
        &empty_ctx(),
    );
    assert!(
        decision.is_dropped(),
        "'flushing working memory' marker must be dropped, got {decision:?}"
    );
}

/// `apply_decision_to_state` produces
/// `"brain: continue_skipping ({rationale})"` outcome details which flow into
/// the reflection transcript. A continue-skipping line with no decision keyword
/// is pure noise and MUST be dropped.
#[test]
fn noise_continue_skipping_no_decision_keyword_is_dropped() {
    let decision = classify(
        "brain: continue_skipping (no decision keyword found in latest cycle output)",
        "act-outcome",
        &empty_ctx(),
    );
    assert!(
        decision.is_dropped(),
        "continue_skipping/no-decision-keyword noise must be dropped, got {decision:?}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// STORE: meaningful episodic events (with full metadata)
// ───────────────────────────────────────────────────────────────────────────

/// Action failures are the highest-value episodics. They MUST be stored at
/// full importance with `is_operational = false` and ALL FIVE metadata keys
/// populated from the call context (goal_id, cycle threaded through).
#[test]
fn meaningful_action_failure_is_stored_with_full_metadata() {
    let decision = classify(
        "act: cargo build failed with error E0432: unresolved import `foo::bar`",
        "act-outcome",
        &ctx(Some("goal-xyz"), Some(7)),
    );
    assert!(
        decision.is_store(),
        "action failure must be stored at full importance, got {decision:?}"
    );

    let meta = decision
        .metadata()
        .expect("a stored episode carries metadata");
    assert!(
        matches!(meta.event_kind, EventKind::ActionFailure),
        "event_kind must be ActionFailure, got {:?}",
        meta.event_kind
    );
    assert!(
        !meta.is_operational,
        "a meaningful episodic must not be flagged operational"
    );
    assert!(
        meta.importance >= 0.8,
        "failures are the highest-importance episodics; got {}",
        meta.importance
    );
    assert_eq!(
        meta.goal_id.as_deref(),
        Some("goal-xyz"),
        "goal_id must be threaded from the intake context"
    );
    assert_eq!(
        meta.cycle,
        Some(7),
        "cycle must be threaded from the intake context"
    );

    // The metadata serializes to a JSON object with EXACTLY the five
    // documented keys, ready to hand to `store_episode(.., Some(&json))`.
    let json = meta.to_json();
    for key in [
        "importance",
        "event_kind",
        "goal_id",
        "cycle",
        "is_operational",
    ] {
        assert!(
            json.get(key).is_some(),
            "metadata JSON must contain key `{key}`; got {json}"
        );
    }
    assert_eq!(
        json["event_kind"].as_str(),
        Some("action_failure"),
        "event_kind serializes to its snake_case label"
    );
    assert_eq!(json["goal_id"].as_str(), Some("goal-xyz"));
    assert_eq!(json["is_operational"].as_bool(), Some(false));
}

/// Completed actions with a durable outcome (opened/merged PR, etc.) are
/// stored as `ActionCompleted`.
#[test]
fn meaningful_completed_action_is_stored() {
    let decision = classify(
        "act: opened PR #42 for goal ship-feature and merged it successfully",
        "act-outcome",
        &ctx(Some("ship-feature"), Some(3)),
    );
    assert!(decision.is_store(), "durable completion must be stored");
    let meta = decision.metadata().expect("stored carries metadata");
    assert!(
        matches!(meta.event_kind, EventKind::ActionCompleted),
        "durable completion → ActionCompleted, got {:?}",
        meta.event_kind
    );
    assert!(!meta.is_operational);
    assert!(
        meta.importance >= 0.7,
        "meaningful episodics live in the 0.7–0.9 band; got {}",
        meta.importance
    );
}

/// Regression (whole-word durable-completion matching): git status vocabulary
/// naming an *outstanding* merge — `unmerged` / `submerged` — must NOT be
/// promoted to a durable [`EventKind::ActionCompleted`] episode. A bare-substring
/// `contains("merged")` scan fired inside those tokens, injecting a phantom
/// "action completed" episodic (0.7-band importance) that distillation would
/// later mine into a phantom completion fact, dragging recall precision down.
/// With no failure or other meaningful signal present, such content falls
/// through to the operational down-scope tier instead.
#[test]
fn unmerged_paths_are_not_a_durable_completion() {
    for content in [
        "act: 3 unmerged paths remain after rebase; conflicts still outstanding",
        "act: the changelog section is submerged under the release notes header",
    ] {
        let decision = classify(content, "act-outcome", &ctx(Some("ship-feature"), Some(5)));
        assert!(
            !decision.is_store(),
            "'{content}' embeds 'merged' inside 'unmerged'/'submerged' and must NOT \
             be stored as a durable completion, got {decision:?}"
        );
        let meta = decision
            .metadata()
            .expect("down-scoped episodes carry metadata");
        assert!(
            !matches!(meta.event_kind, EventKind::ActionCompleted),
            "'{content}' must not classify as ActionCompleted, got {:?}",
            meta.event_kind
        );
        assert!(
            decision.is_downscoped() && meta.is_operational,
            "unmatched non-failure content down-scopes to the operational tier, got {decision:?}"
        );
    }
}

/// Complement of [`unmerged_paths_are_not_a_durable_completion`]: a genuine
/// `merged` used as a whole word still classifies as a durable completion, so the
/// word-boundary tightening does not regress the real signal it protects.
#[test]
fn whole_word_merged_is_still_a_durable_completion() {
    let decision = classify(
        "act: merged PR #7 for goal ship-feature",
        "act-outcome",
        &ctx(Some("ship-feature"), Some(6)),
    );
    assert!(decision.is_store(), "a real merged PR must be stored");
    let meta = decision.metadata().expect("stored carries metadata");
    assert!(
        matches!(meta.event_kind, EventKind::ActionCompleted),
        "whole-word 'merged' → ActionCompleted, got {:?}",
        meta.event_kind
    );
    assert!(meta.importance >= 0.7);
}

/// Handoffs between sessions/worktrees are durable coordination events.
#[test]
fn meaningful_handoff_is_stored() {
    let decision = classify(
        "handoff: transferred goal ship-feature to engineer worktree wt-7",
        "handoff",
        &ctx(Some("ship-feature"), Some(4)),
    );
    assert!(decision.is_store(), "handoff must be stored");
    let meta = decision.metadata().unwrap();
    assert!(
        matches!(meta.event_kind, EventKind::Handoff),
        "handoff → Handoff, got {:?}",
        meta.event_kind
    );
    assert!(!meta.is_operational);
}

/// Goal promotions (backlog → active) are durable goal-board transitions.
#[test]
fn meaningful_goal_promotion_is_stored() {
    let decision = classify(
        "promoted goal ship-feature from backlog to active",
        "goal-curator",
        &ctx(Some("ship-feature"), Some(9)),
    );
    assert!(decision.is_store(), "goal promotion must be stored");
    let meta = decision.metadata().unwrap();
    assert!(
        matches!(meta.event_kind, EventKind::GoalPromotion),
        "promotion → GoalPromotion, got {:?}",
        meta.event_kind
    );
    assert!(!meta.is_operational);
}

/// User decisions are first-class episodic memory regardless of source.
#[test]
fn meaningful_user_decision_is_stored() {
    let decision = classify(
        "user decided to prioritize security fixes over new feature work",
        "user-decision",
        &empty_ctx(),
    );
    assert!(decision.is_store(), "user decision must be stored");
    let meta = decision.metadata().unwrap();
    assert!(
        matches!(meta.event_kind, EventKind::UserDecision),
        "user decision → UserDecision, got {:?}",
        meta.event_kind
    );
    assert!(!meta.is_operational);
}

/// Recipe failures are stored (a distinct failure sub-kind).
#[test]
fn meaningful_recipe_failure_is_stored() {
    let decision = classify(
        "recipe distill-episodes.yaml failed: recipe exited with code 1",
        "recipe-outcome",
        &empty_ctx(),
    );
    assert!(decision.is_store(), "recipe failure must be stored");
    let meta = decision.metadata().unwrap();
    assert!(
        matches!(meta.event_kind, EventKind::RecipeFailure),
        "recipe failure → RecipeFailure, got {:?}",
        meta.event_kind
    );
    assert!(
        meta.importance >= 0.8,
        "failures are highest importance; got {}",
        meta.importance
    );
}

// ───────────────────────────────────────────────────────────────────────────
// OVERRIDE: failure/error summary beats the drop rule (A7)
// ───────────────────────────────────────────────────────────────────────────

/// A session-lifecycle marker that ALSO carries a failure/error summary MUST
/// be retained (stored), because the failure signal overrides the drop rule.
#[test]
fn noise_marker_with_failure_summary_is_retained() {
    let decision = classify(
        "Session sess-abc completed and persisted; cargo test failed with error",
        "session-persistence",
        &empty_ctx(),
    );
    assert!(
        !decision.is_dropped(),
        "a failure summary must override the drop rule, got {decision:?}"
    );
    assert!(
        decision.is_store(),
        "failure-carrying episodes are stored at full importance"
    );
    let meta = decision.metadata().expect("retained carries metadata");
    assert!(
        matches!(
            meta.event_kind,
            EventKind::ActionFailure | EventKind::RecipeFailure
        ),
        "the override classifies as a failure kind, got {:?}",
        meta.event_kind
    );
}

// ───────────────────────────────────────────────────────────────────────────
// DOWN-SCOPE: operational bookkeeping kept at low importance
// ───────────────────────────────────────────────────────────────────────────

/// Cross-session hydration bookkeeping (`consolidation-intake`) is operational:
/// not worth full-importance storage, but kept as a low-importance trace rather
/// than dropped. It MUST be flagged `is_operational` with low importance.
#[test]
fn operational_bookkeeping_is_downscoped() {
    let decision = classify(
        "Hydrated 5 prior-session facts for cross-session recall",
        "consolidation-intake",
        &empty_ctx(),
    );
    assert!(
        decision.is_downscoped(),
        "operational bookkeeping must be down-scoped, got {decision:?}"
    );
    let meta = decision.metadata().expect("down-scoped carries metadata");
    assert!(
        meta.is_operational,
        "down-scoped operational episodes must set is_operational = true"
    );
    assert!(
        meta.importance <= 0.2,
        "down-scoped operational importance must be low; got {}",
        meta.importance
    );
    assert!(
        matches!(meta.event_kind, EventKind::Operational),
        "down-scoped bookkeeping → Operational, got {:?}",
        meta.event_kind
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Metadata JSON shape
// ───────────────────────────────────────────────────────────────────────────

/// `EpisodeMetadata::to_json` always emits all five keys, with `goal_id` and
/// `cycle` rendered as JSON null when absent (so the object shape is stable).
#[test]
fn metadata_to_json_emits_all_five_keys_with_nulls() {
    let meta = EpisodeMetadata {
        importance: 0.9,
        event_kind: EventKind::ActionFailure,
        goal_id: None,
        cycle: None,
        is_operational: false,
    };
    let json = meta.to_json();
    assert!(json.is_object(), "metadata must serialize to a JSON object");
    let obj = json.as_object().unwrap();
    assert_eq!(
        obj.len(),
        5,
        "exactly five keys expected, got {:?}",
        obj.keys().collect::<Vec<_>>()
    );
    assert!(json["goal_id"].is_null(), "absent goal_id renders as null");
    assert!(json["cycle"].is_null(), "absent cycle renders as null");
    assert_eq!(json["importance"].as_f64(), Some(0.9));
}

// ───────────────────────────────────────────────────────────────────────────
// FAILURE-SIGNAL FIDELITY: word-boundary + inflection matching
//
// The failure override must fire on genuine failure words and their inflections
// but NOT on unrelated words that merely embed a failure stem as a substring.
// The pre-fix naive substring scan mis-classified "exceptional" (⊃ "exception"),
// "hispanic" (⊃ "panic"), and "terror" / "mirror" (⊃ "error") as full-importance
// ActionFailures, polluting distillation with phantom failure facts.
// ───────────────────────────────────────────────────────────────────────────

/// `true` when a decision stored the episode as a failure kind at full
/// importance — the disposition the override produces.
fn is_failure_store(decision: &crate::memory_consolidation::classifier::IntakeDecision) -> bool {
    decision.is_store()
        && decision.metadata().is_some_and(|m| {
            matches!(
                m.event_kind,
                EventKind::ActionFailure | EventKind::RecipeFailure
            )
        })
}

/// Look-alike words that only *contain* a failure stem as a substring must NOT
/// trigger the failure override. Each phrase carries no other meaningful marker,
/// so absent a (spurious) failure signal it down-scopes as operational noise.
#[test]
fn failure_look_alikes_do_not_trigger_override() {
    let benign = [
        // "exceptional" ⊃ "exception" — derivational -al, not an inflection.
        "delivered exceptional results this planning window",
        // "hispanic" ⊃ "panic" — a wholly unrelated word.
        "filed the hispanic community outreach note",
        // "terror" ⊃ "error" — coincidental substring.
        "a night of terror over the tricky refactor has passed",
        // "mirror" ⊃ "error" — coincidental substring.
        "mirror the staging cluster into the preview slot",
        // "terrorism" ⊃ "error" plus a derivational -ism.
        "read a briefing on counter-terrorism policy",
    ];
    for phrase in benign {
        let decision = classify(phrase, "planning-notes", &empty_ctx());
        assert!(
            !is_failure_store(&decision),
            "look-alike must NOT be a failure store: {phrase:?} → {decision:?}"
        );
        assert!(
            decision.is_downscoped(),
            "benign unmatched content down-scopes as operational: {phrase:?} → {decision:?}"
        );
    }
}

/// Genuine failure words — including plural / past / gerund inflections and the
/// `c → ck` doubling in the panic family — MUST trigger the override at full
/// importance. This guards against the fix over-tightening into false negatives.
#[test]
fn failure_inflections_trigger_override() {
    let failing = [
        "caught an exception in the request handler",
        "unhandled exceptions in the parser stage",
        "the deploy fails intermittently on cold start",
        "unit test failing after the dependency bump",
        "three errors surfaced in the worker log",
        "repeated failures during the rollout",
        "the worker panicked mid-batch",
        "the process kept panicking under load",
        "the service panics on empty input",
    ];
    for phrase in failing {
        let decision = classify(phrase, "act", &empty_ctx());
        assert!(
            is_failure_store(&decision),
            "genuine failure word must trigger the override: {phrase:?} → {decision:?}"
        );
        let meta = decision.metadata().expect("failure store carries metadata");
        assert_eq!(
            meta.importance, 0.9,
            "failures are the highest-importance episodics: {phrase:?}"
        );
    }
}

/// The override still beats a drop marker when the failure word is an inflection
/// (regression guard for A7 under the new word-boundary matcher).
#[test]
fn inflected_failure_overrides_drop_marker() {
    let decision = classify(
        "Session sess-xyz completed and persisted; two integration tests failing",
        "session-persistence",
        &empty_ctx(),
    );
    assert!(
        is_failure_store(&decision),
        "an inflected failure word must override the drop marker, got {decision:?}"
    );
}

/// Compound PascalCase error/exception TYPE names — where the failure stem sits
/// at the END of a delimiter-less compound token — MUST still trigger the
/// override. These are a genuine failure-signal class (idiomatic Rust error
/// types end in `Error`) that a naive whole-word prefix rule would miss.
#[test]
fn compound_error_type_names_trigger_override() {
    let failing = [
        "Encountered IoError: connection reset by peer",
        "hit a ParseError while decoding the frame",
        "threw NullPointerException at line 42",
        "an IllegalStateException surfaced during shutdown",
        "SendError propagated up the channel",
        "the RuntimeException bubbled up",
        "collected several ValidationErrors this pass",
    ];
    for phrase in failing {
        let decision = classify(phrase, "act", &empty_ctx());
        assert!(
            is_failure_store(&decision),
            "compound error/exception type name must trigger the override: {phrase:?} → {decision:?}"
        );
        let meta = decision.metadata().expect("failure store carries metadata");
        assert_eq!(
            meta.importance, 0.9,
            "compound failure type names store at full importance: {phrase:?}"
        );
    }
}

/// The compound-type-name pass is CASE-SENSITIVE on the PascalCase segment, so
/// it must NOT resurrect the lowercase look-alikes it is designed to skip:
/// `terror` ends in the letters `error` but not in the capitalised `Error`.
#[test]
fn compound_pass_does_not_resurrect_lowercase_look_alikes() {
    for phrase in [
        "a night of terror over the tricky refactor has passed",
        "read a briefing on counter-terrorism policy",
    ] {
        let decision = classify(phrase, "planning-notes", &empty_ctx());
        assert!(
            !is_failure_store(&decision),
            "lowercase look-alike must stay excluded by the compound pass: {phrase:?} → {decision:?}"
        );
    }
}
