//! Memory-backed acceptance tests for the failure→lesson half of the
//! procedural-learning loop (issue #2458), exercised through the Simard
//! [`CognitiveMemoryOps`] surface against `LibraryCognitiveMemory::in_memory()`
//! (the sole real backend).
//!
//! These pin the acceptance contract from
//! `docs/reference/procedural-learning-loop.md`:
//!
//! | AC | guard |
//! |----|-------|
//! | AC-4 | a `VerifiedFailure` writes a `reflection:failure` episode |
//! | AC-5 | a recurring `(goal_type, error_class)` (>= threshold) becomes a `lesson:` procedure |
//! | AC-6 | a one-off failure (count = 1) does **not** become a lesson |
//! | AC-7 | a lesson is recallable for a later objective mentioning the goal-type |
//! | AC-8 | `Verdict::Unverified` (and `VerifiedSuccess`) distil/reflect nothing |
//!
//! The pure gate/normalization/metric contracts (AC-10, AC-11) live in the
//! module's inline `#[cfg(test)] mod tests`.

use super::reflection_lessons::{
    LESSON_RECURRENCE_THRESHOLD, Verdict, count_recurring_failures, has_lesson_for, lesson_name,
    maybe_distill_lesson, record_failure_reflection,
};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

fn mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

fn cargo_failure() -> Verdict {
    Verdict::VerifiedFailure {
        error_class: "cargo_test_failed".to_string(),
    }
}

const OBJECTIVE: &str = "Fix CI linker OOM in the release build";

/// AC-4: a verified failure stores a `reflection:failure` episode that
/// `count_recurring_failures` can find by its (goal_type, error_class) key.
#[test]
fn verified_failure_writes_reflection() {
    let m = mem();
    let verdict = cargo_failure();

    let id = record_failure_reflection(&m, OBJECTIVE, &verdict, "pin the toolchain")
        .expect("reflection store ok");
    assert!(id.is_some(), "a verified failure must write a reflection");

    let count = count_recurring_failures(
        &m,
        "fix-ci-linker-oom-in-the-release-build",
        "cargo_test_failed",
    )
    .expect("count ok");
    assert_eq!(count, 1, "exactly the one reflection just stored");
}

/// AC-8: neither an unverified outcome nor a verified *success* writes a
/// reflection — the fail-safe gate (R10) learns nothing without a real failure
/// signal.
#[test]
fn unverified_and_success_write_no_reflection() {
    let m = mem();

    assert!(
        record_failure_reflection(&m, OBJECTIVE, &Verdict::Unverified, "h")
            .expect("ok")
            .is_none(),
        "unverified must not reflect"
    );
    assert!(
        record_failure_reflection(&m, OBJECTIVE, &Verdict::VerifiedSuccess, "h")
            .expect("ok")
            .is_none(),
        "a verified success must not reflect"
    );
    let count = count_recurring_failures(
        &m,
        "fix-ci-linker-oom-in-the-release-build",
        "cargo_test_failed",
    )
    .expect("count ok");
    assert_eq!(count, 0);
}

/// AC-6: a one-off failure (count = 1, below the default threshold of 2) does
/// NOT become a lesson — recurrence gating keeps procedural memory clean.
#[test]
fn one_off_failure_is_not_a_lesson() {
    let m = mem();
    let verdict = cargo_failure();

    let ep = record_failure_reflection(&m, OBJECTIVE, &verdict, "pin toolchain")
        .expect("ok")
        .expect("some id");

    let lesson = maybe_distill_lesson(&m, &verdict, OBJECTIVE, LESSON_RECURRENCE_THRESHOLD, &[ep])
        .expect("distill ok");
    assert!(
        lesson.is_none(),
        "a single failure must not become a lesson"
    );
    assert!(
        !has_lesson_for(
            &m,
            "fix-ci-linker-oom-in-the-release-build",
            "cargo_test_failed"
        )
        .expect("ok"),
        "no lesson should exist after one failure"
    );
}

/// AC-5 + AC-7: a recurring failure (>= threshold reflections) becomes a
/// retrievable `lesson:` procedure that a later matching objective recalls.
#[test]
fn recurring_failure_becomes_recallable_lesson() {
    let m = mem();
    let verdict = cargo_failure();

    // Two verified failures on the same (goal_type, error_class).
    let mut sources = Vec::new();
    for _ in 0..LESSON_RECURRENCE_THRESHOLD {
        let ep = record_failure_reflection(&m, OBJECTIVE, &verdict, "pin the toolchain")
            .expect("ok")
            .expect("some id");
        sources.push(ep);
    }

    let lesson_id = maybe_distill_lesson(
        &m,
        &verdict,
        OBJECTIVE,
        LESSON_RECURRENCE_THRESHOLD,
        &sources,
    )
    .expect("distill ok")
    .expect("recurring failure must become a lesson");
    assert!(!lesson_id.is_empty());

    let goal_type = "fix-ci-linker-oom-in-the-release-build";
    assert!(
        has_lesson_for(&m, goal_type, "cargo_test_failed").expect("ok"),
        "the lesson must exist after the recurrence threshold is reached"
    );

    // AC-7: a later objective sharing a goal-type token recalls the lesson.
    let expected = lesson_name(goal_type, "cargo_test_failed");
    let recalled = m.recall_procedure("linker", 10).expect("recall");
    assert!(
        recalled.iter().any(|p| p.name == expected),
        "the lesson {expected:?} must surface for a related objective; got {:?}",
        recalled.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
}

/// AC-8 (lesson path): `maybe_distill_lesson` is a no-op on `Unverified` even if
/// reflections happen to exist — the gate is load-bearing, not advisory.
#[test]
fn unverified_distills_no_lesson_even_with_reflections() {
    let m = mem();
    let verdict = cargo_failure();
    for _ in 0..LESSON_RECURRENCE_THRESHOLD {
        record_failure_reflection(&m, OBJECTIVE, &verdict, "h").expect("ok");
    }

    let lesson = maybe_distill_lesson(
        &m,
        &Verdict::Unverified,
        OBJECTIVE,
        LESSON_RECURRENCE_THRESHOLD,
        &[],
    )
    .expect("ok");
    assert!(lesson.is_none(), "Unverified must distil nothing (R10)");
}

/// Lesson stores are idempotent by name (#2298): distilling the same recurring
/// failure twice reinforces the single lesson rather than duplicating it.
#[test]
fn lesson_store_is_idempotent() {
    let m = mem();
    let verdict = cargo_failure();
    for _ in 0..LESSON_RECURRENCE_THRESHOLD {
        record_failure_reflection(&m, OBJECTIVE, &verdict, "h").expect("ok");
    }

    let first = maybe_distill_lesson(&m, &verdict, OBJECTIVE, LESSON_RECURRENCE_THRESHOLD, &[])
        .expect("ok")
        .expect("lesson");
    let second = maybe_distill_lesson(&m, &verdict, OBJECTIVE, LESSON_RECURRENCE_THRESHOLD, &[])
        .expect("ok")
        .expect("lesson again");
    assert_eq!(first, second, "idempotent store returns the same node id");

    let goal_type = "fix-ci-linker-oom-in-the-release-build";
    let expected = lesson_name(goal_type, "cargo_test_failed");
    let matches = m
        .recall_procedure("cargo", 50)
        .expect("recall")
        .into_iter()
        .filter(|p| p.name == expected)
        .count();
    assert_eq!(matches, 1, "the lesson must not be duplicated");
}

/// The recurrence marker is key-scoped: reflections for one
/// (goal_type, error_class) do not inflate another's count (collision-safe even
/// across prefix-overlapping goal types).
#[test]
fn recurrence_count_is_key_scoped() {
    let m = mem();
    let v = cargo_failure();
    record_failure_reflection(&m, "Fix CI", &v, "h").expect("ok");
    record_failure_reflection(&m, "Fix CI runner flake", &v, "h").expect("ok");

    assert_eq!(
        count_recurring_failures(&m, "fix-ci", "cargo_test_failed").expect("ok"),
        1,
        "only the exact-key reflection counts, not the prefix-overlapping one"
    );
    assert_eq!(
        count_recurring_failures(&m, "fix-ci-runner-flake", "cargo_test_failed").expect("ok"),
        1
    );
}
