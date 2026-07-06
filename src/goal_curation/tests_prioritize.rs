//! TDD contract for the goal **prioritization pass** and the `priority_explicit`
//! provenance flag (issue #2695 follow-up: flat-priority remediation).
//!
//! The operator sees almost every goal at the same priority (e.g. many at p3),
//! which is effectively no prioritization. These tests pin the SUBSTANCE half of
//! the fix: a pure, deterministic pass that re-scores the *undifferentiated*
//! goals into a spread band while leaving the operator's *explicitly-set*
//! priorities untouched.
//!
//! Design decisions these tests encode (see the design spec for #2695):
//!   * `ActiveGoal.priority_explicit: bool` is additive provenance
//!     (`#[serde(default)]`, skipped when `false`) — only the operator
//!     `goal set-priority` path sets it `true`. The pass touches only goals
//!     where `priority_explicit == false`.
//!   * `prioritize(goals, signals, now)` is a PURE function: same inputs (with an
//!     injected `now`) always yield the same output, it never reorders the
//!     goals (it only rewrites `priority`), and every re-scored priority is
//!     banded into `1..=5` (never `< 1`).
//!   * Differentiation is driven by structured goal-graph signals
//!     (`depends_on` — a goal many others depend on is a bottleneck) plus each
//!     goal's own lifecycle fields (Blocked / InProgress → more urgent;
//!     standing/perpetual → less urgent). No brittle string parsing (G3).
//!
//! These tests are RED until the model field, the builder, and the
//! `goal_curation::prioritize` module exist.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::goal_curation::{ActiveGoal, GoalProgress};
// The pass lives in the (new) `prioritize` submodule of `goal_curation`.
use super::prioritize::{PrioritizationSignals, prioritize};

/// A fixed, injected clock so the pass is deterministic under test.
fn fixed_now() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid fixed timestamp")
}

/// A plain, undifferentiated goal: priority p3, NOT operator-set
/// (`priority_explicit == false`), status `NotStarted`.
fn flat_goal(id: &str) -> ActiveGoal {
    ActiveGoal::new(id, format!("goal {id}"), 3)
}

/// Look up a goal by id in a slice, panicking with context if absent.
fn by_id<'a>(goals: &'a [ActiveGoal], id: &str) -> &'a ActiveGoal {
    goals
        .iter()
        .find(|g| g.id == id)
        .unwrap_or_else(|| panic!("goal {id:?} missing from prioritize() output"))
}

/// A five-goal board whose goals all start at the SAME flat priority (p3) and
/// are all non-explicit, but carry five DISTINCT signal profiles so a correct
/// pass must spread them apart. Returns `(goals, signals)`.
fn flat_board_with_distinct_signals() -> (Vec<ActiveGoal>, PrioritizationSignals) {
    // A bottleneck others depend on, actively in progress.
    let mut foundation = flat_goal("foundation");
    foundation.status = GoalProgress::InProgress { percent: 40 };

    // Also a bottleneck, and blocked (needs attention to unblock downstream).
    let mut midtier = flat_goal("midtier");
    midtier.status = GoalProgress::Blocked("waiting on foundation".to_string());

    // Leaf goals that depend on the bottlenecks — nothing depends on them.
    let leaf_a = flat_goal("leaf-a");
    let leaf_b = flat_goal("leaf-b");

    // A standing/perpetual goal — durable, no terminal state, lower urgency.
    let standing = flat_goal("standing").mark_standing();

    // An isolated idle goal with no signals at all — the weakest.
    let loner = flat_goal("loner");

    let goals = vec![foundation, midtier, leaf_a, leaf_b, standing, loner];

    // Structured `depends_on` signal: dependent -> [its blockers].
    // foundation is depended on by midtier + leaf-a (2 dependents);
    // midtier is depended on by leaf-a + leaf-b (2 dependents).
    let mut depends_on: HashMap<String, Vec<String>> = HashMap::new();
    depends_on.insert("midtier".to_string(), vec!["foundation".to_string()]);
    depends_on.insert(
        "leaf-a".to_string(),
        vec!["foundation".to_string(), "midtier".to_string()],
    );
    depends_on.insert("leaf-b".to_string(), vec!["midtier".to_string()]);

    let signals = PrioritizationSignals { depends_on };
    (goals, signals)
}

// ---------------------------------------------------------------------------
// Provenance flag: additive, default-false, builder-set, serde round-trip.
// ---------------------------------------------------------------------------

#[test]
fn active_goal_priority_explicit_defaults_false() {
    let g = ActiveGoal::new("g", "desc", 3);
    assert!(
        !g.priority_explicit,
        "a freshly-created goal must be non-explicit so the pass may differentiate it"
    );
}

#[test]
fn with_priority_explicit_builder_sets_the_flag() {
    let g = ActiveGoal::new("g", "desc", 2).with_priority_explicit(true);
    assert!(
        g.priority_explicit,
        "with_priority_explicit(true) must mark the goal's priority as operator-set"
    );
    let cleared = g.with_priority_explicit(false);
    assert!(
        !cleared.priority_explicit,
        "with_priority_explicit(false) must clear the flag"
    );
}

#[test]
fn priority_explicit_is_skipped_when_false_but_present_when_true() {
    // Additive-serialization invariant: a non-explicit goal serializes WITHOUT
    // the key so pre-existing goal-board snapshots stay byte-identical.
    let implicit = ActiveGoal::new("g", "desc", 3);
    let v = serde_json::to_value(&implicit).expect("serialize implicit goal");
    assert!(
        v.get("priority_explicit").is_none(),
        "priority_explicit==false must be omitted from JSON (skip_serializing_if) so legacy \
         snapshots remain byte-identical; got {v}"
    );

    let explicit = ActiveGoal::new("g", "desc", 3).with_priority_explicit(true);
    let v = serde_json::to_value(&explicit).expect("serialize explicit goal");
    assert_eq!(
        v.get("priority_explicit"),
        Some(&serde_json::json!(true)),
        "priority_explicit==true must be serialized so operator intent survives a reload"
    );
}

#[test]
fn legacy_goal_json_without_priority_explicit_deserializes_to_false() {
    // A pre-#2695 goal-board snapshot has no `priority_explicit` key.
    let legacy = serde_json::json!({
        "id": "old-goal",
        "description": "predates the provenance flag",
        "priority": 3,
        "status": "NotStarted",
        "assigned_to": null
    });
    let g: ActiveGoal = serde_json::from_value(legacy).expect("legacy goal must still deserialize");
    assert!(
        !g.priority_explicit,
        "a legacy goal with no priority_explicit key must default to false (eligible for the pass)"
    );
}

// ---------------------------------------------------------------------------
// The prioritization pass.
// ---------------------------------------------------------------------------

#[test]
fn prioritize_differentiates_a_flat_undifferentiated_set() {
    let (goals, signals) = flat_board_with_distinct_signals();
    // Precondition: everything is the same flat priority — the operator's exact
    // complaint ("almost all goals are the same priority ⇒ no prioritization").
    assert!(
        goals
            .iter()
            .all(|g| g.priority == 3 && !g.priority_explicit),
        "test setup: all goals must start flat and non-explicit"
    );

    let out = prioritize(&goals, &signals, fixed_now());

    let priorities: Vec<u32> = out.iter().map(|g| g.priority).collect();
    let distinct: std::collections::BTreeSet<u32> = priorities.iter().copied().collect();

    // The pass must actually SPREAD the priorities — not leave them collapsed.
    assert!(
        distinct.len() >= 3,
        "prioritize must differentiate a flat set into meaningfully spread priorities; \
         got {priorities:?} ({} distinct)",
        distinct.len()
    );
    assert!(
        !priorities.iter().all(|&p| p == 3),
        "prioritize must not leave every goal at the default p3 (that is no prioritization)"
    );

    // Signal-strength ordering: a bottleneck that others depend on and is
    // actively in progress must out-rank (lower number = higher priority) an
    // isolated idle goal with no signals.
    assert!(
        by_id(&out, "foundation").priority < by_id(&out, "loner").priority,
        "a depended-on, in-progress bottleneck must rank ABOVE an isolated idle goal"
    );
    assert!(
        by_id(&out, "midtier").priority < by_id(&out, "loner").priority,
        "a depended-on, blocked goal must rank ABOVE an isolated idle goal"
    );
}

#[test]
fn prioritize_bands_every_rescored_priority_into_1_through_5() {
    let (goals, signals) = flat_board_with_distinct_signals();
    let out = prioritize(&goals, &signals, fixed_now());
    for g in &out {
        assert!(
            (1..=5).contains(&g.priority),
            "re-scored priority for {:?} must be banded into 1..=5 (never 0, never runaway); \
             got p{}",
            g.id,
            g.priority
        );
    }
}

#[test]
fn prioritize_leaves_explicitly_set_priorities_intact() {
    // A goal the operator explicitly pinned at p3 — even though it carries the
    // strongest possible differentiating signals (many dependents) — must keep
    // its EXACT priority and its explicit flag. "Differentiate the undifferentiated,
    // never reshuffle the operator's explicit choices."
    let pinned =
        ActiveGoal::new("pinned", "operator pinned this at p3", 3).with_priority_explicit(true);
    let mut ordinary = flat_goal("ordinary");
    ordinary.status = GoalProgress::InProgress { percent: 10 };

    let goals = vec![pinned, ordinary];

    let mut depends_on: HashMap<String, Vec<String>> = HashMap::new();
    // Three goals depend on "pinned" — a very strong signal that would otherwise
    // push it toward p1. It must NOT move because it is operator-set.
    depends_on.insert("a".to_string(), vec!["pinned".to_string()]);
    depends_on.insert("b".to_string(), vec!["pinned".to_string()]);
    depends_on.insert("c".to_string(), vec!["pinned".to_string()]);
    let signals = PrioritizationSignals { depends_on };

    let out = prioritize(&goals, &signals, fixed_now());

    let pinned_out = by_id(&out, "pinned");
    assert_eq!(
        pinned_out.priority, 3,
        "an explicitly-set (priority_explicit) goal must keep its exact priority"
    );
    assert!(
        pinned_out.priority_explicit,
        "the pass must preserve the priority_explicit provenance flag"
    );
}

#[test]
fn prioritize_is_deterministic_under_a_fixed_clock() {
    let (goals, signals) = flat_board_with_distinct_signals();
    let now = fixed_now();
    let first = prioritize(&goals, &signals, now);
    let second = prioritize(&goals, &signals, now);
    assert_eq!(
        first, second,
        "prioritize must be a pure function: identical inputs + clock ⇒ identical output"
    );
}

#[test]
fn prioritize_preserves_goal_identity_and_order() {
    // The pass rewrites `priority` only — it must never drop, add, or reorder
    // goals (display ordering is the renderer's job, not the pass's).
    let (goals, signals) = flat_board_with_distinct_signals();
    let in_ids: Vec<String> = goals.iter().map(|g| g.id.clone()).collect();

    let out = prioritize(&goals, &signals, fixed_now());
    let out_ids: Vec<String> = out.iter().map(|g| g.id.clone()).collect();

    assert_eq!(
        out_ids, in_ids,
        "prioritize must preserve goal identity and input order (it only re-scores priority)"
    );
}

#[test]
fn prioritize_ranks_a_blocker_above_a_non_blocker() {
    // Minimal, focused contract: of two otherwise-identical flat goals, the one
    // that OTHERS depend on (a bottleneck) must rank strictly higher.
    let blocker = flat_goal("blocker");
    let leaf = flat_goal("leaf");
    let goals = vec![blocker, leaf];

    let mut depends_on: HashMap<String, Vec<String>> = HashMap::new();
    depends_on.insert("leaf".to_string(), vec!["blocker".to_string()]);
    let signals = PrioritizationSignals { depends_on };

    let out = prioritize(&goals, &signals, fixed_now());
    assert!(
        by_id(&out, "blocker").priority < by_id(&out, "leaf").priority,
        "a goal that others depend on must be prioritized above a goal nothing depends on"
    );
}
