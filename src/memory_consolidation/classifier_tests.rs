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
    EpisodeMetadata, EventKind, IntakeContext, IntakeDecision, classify,
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
