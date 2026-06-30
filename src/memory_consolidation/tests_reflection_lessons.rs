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
    LESSON_RECURRENCE_THRESHOLD, LessonLearningReport, Verdict, VerifiedFailureObservation,
    count_recurring_failures, has_lesson_for, learn_from_verified_failures, lesson_name,
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

// ── live wiring: learn_from_verified_failures (#2458 production trigger) ──────
//
// These pin the end-to-end loop the OODA curate phase drives from the FU1
// (#2456) `Refuted` signal: verified failure → reflection → recurring lesson →
// recall. They exercise the same `LibraryCognitiveMemory` backend the daemon
// uses.

const REFUTED_OBJECTIVE: &str = "Ship the websocket reconnect backoff for the dashboard";
const REFUTED_CLASS: &str = "pr_not_merged";

fn obs() -> VerifiedFailureObservation {
    VerifiedFailureObservation::new(REFUTED_OBJECTIVE, REFUTED_CLASS)
}

/// A single verified failure writes a reflection but — below the recurrence
/// threshold — distils no lesson (AC-4 + AC-6 through the live entry point).
#[test]
fn learn_from_verified_failures_one_off_reflects_no_lesson() {
    let m = mem();
    let report = learn_from_verified_failures(
        &m,
        std::slice::from_ref(&obs()),
        LESSON_RECURRENCE_THRESHOLD,
    );
    assert_eq!(
        report,
        LessonLearningReport {
            reflections_recorded: 1,
            lessons_distilled: 0,
            repeat_failures: 0,
        }
    );
    assert!(
        !has_lesson_for(&m, REFUTED_OBJECTIVE, REFUTED_CLASS).expect("ok"),
        "a single refuted completion must not become a lesson"
    );
}

/// `threshold` recurring verified failures distil exactly one recallable
/// `lesson:` procedure (AC-5 + AC-7 through the live entry point).
#[test]
fn learn_from_verified_failures_recurrence_distills_recallable_lesson() {
    let m = mem();
    let batch = vec![obs(); LESSON_RECURRENCE_THRESHOLD as usize];

    let report = learn_from_verified_failures(&m, &batch, LESSON_RECURRENCE_THRESHOLD);
    assert_eq!(report.reflections_recorded, LESSON_RECURRENCE_THRESHOLD);
    assert_eq!(
        report.lessons_distilled, 1,
        "recurrence crosses the threshold once"
    );
    assert_eq!(
        report.repeat_failures, 0,
        "no pre-existing lesson to regress against"
    );

    assert!(
        has_lesson_for(&m, REFUTED_OBJECTIVE, REFUTED_CLASS).expect("ok"),
        "the lesson must exist after the recurrence threshold is reached"
    );
    // A later objective sharing a goal-type token recalls the lesson.
    let expected = lesson_name(REFUTED_OBJECTIVE, REFUTED_CLASS);
    let recalled = m.recall_procedure("websocket", 10).expect("recall");
    assert!(
        recalled.iter().any(|p| p.name == expected),
        "lesson {expected:?} must surface for a related objective; got {:?}",
        recalled.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
}

/// A failure that recurs **after** a lesson already exists is counted as a
/// self-regression (`repeat_failures`), not as a fresh lesson — the headline
/// #2458 measurement that the repeat-failure rate can trend down against.
#[test]
fn learn_from_verified_failures_counts_repeat_after_lesson() {
    let m = mem();
    // First pass establishes the lesson.
    let batch = vec![obs(); LESSON_RECURRENCE_THRESHOLD as usize];
    let first = learn_from_verified_failures(&m, &batch, LESSON_RECURRENCE_THRESHOLD);
    assert_eq!(first.lessons_distilled, 1);

    // A subsequent failure on the same key recurs despite the lesson.
    let second = learn_from_verified_failures(
        &m,
        std::slice::from_ref(&obs()),
        LESSON_RECURRENCE_THRESHOLD,
    );
    assert_eq!(
        second,
        LessonLearningReport {
            reflections_recorded: 1,
            lessons_distilled: 0,
            repeat_failures: 1,
        },
        "a recurrence past an existing lesson is a repeat-failure, not a new lesson"
    );
}

/// One call carrying `threshold + 1` recurrences distils the lesson and flags
/// the trailing recurrence as a repeat in the same pass.
#[test]
fn learn_from_verified_failures_distill_and_repeat_in_one_pass() {
    let m = mem();
    let batch = vec![obs(); LESSON_RECURRENCE_THRESHOLD as usize + 1];
    let report = learn_from_verified_failures(&m, &batch, LESSON_RECURRENCE_THRESHOLD);
    assert_eq!(report.reflections_recorded, LESSON_RECURRENCE_THRESHOLD + 1);
    assert_eq!(report.lessons_distilled, 1);
    assert_eq!(report.repeat_failures, 1);
}

/// An empty batch is a pure no-op (the common case: most cycles refute nothing).
#[test]
fn learn_from_verified_failures_empty_batch_is_noop() {
    let m = mem();
    let report = learn_from_verified_failures(&m, &[], LESSON_RECURRENCE_THRESHOLD);
    assert!(report.is_empty());
}

/// A `threshold` of 0 falls back to the default so a misconfiguration cannot
/// turn every one-off failure into a lesson.
#[test]
fn learn_from_verified_failures_threshold_zero_falls_back_to_default() {
    let m = mem();
    let report = learn_from_verified_failures(&m, std::slice::from_ref(&obs()), 0);
    assert_eq!(
        report.lessons_distilled, 0,
        "threshold 0 must not distil a one-off"
    );
    assert!(!has_lesson_for(&m, REFUTED_OBJECTIVE, REFUTED_CLASS).expect("ok"));
}

/// Raw, un-normalized objective/error-class text is normalized into the same
/// `(goal_type, error_class)` key the recall path uses.
#[test]
fn learn_from_verified_failures_normalizes_keys() {
    let m = mem();
    let raw = VerifiedFailureObservation::new("Ship The  WebSocket Reconnect!", "PR Not Merged");
    let batch = vec![raw; LESSON_RECURRENCE_THRESHOLD as usize];
    let report = learn_from_verified_failures(&m, &batch, LESSON_RECURRENCE_THRESHOLD);
    assert_eq!(report.lessons_distilled, 1);
    assert!(
        has_lesson_for(&m, "Ship The  WebSocket Reconnect!", "PR Not Merged").expect("ok"),
        "normalization must be consistent between store and lookup"
    );
}
