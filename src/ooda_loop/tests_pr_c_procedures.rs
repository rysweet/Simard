//! TDD (RED) tests for PR-C: OODA cycle procedure naming.
//!
//! Covers the contract documented in
//! `docs/reference/cognitive-memory-bootstrap-procedures.md` §"OODA
//! cycle storage changes (runtime — distinct from bootstrap seeding)"
//! for PR-C (issue #2281, problem 3).
//!
//! The current production code at `src/ooda_loop/cycle.rs:343` writes:
//!
//! ```ignore
//! let proc_name = format!("ooda:{}", outcome.action.kind);
//! adapters.memory.store_procedure(&proc_name, &steps, &[])?;
//! ```
//!
//! After PR-C the writer must construct a goal-scoped, trigger-bearing
//! name via two helpers that live next to the writer in `cycle.rs`:
//!
//! * `pattern_for(ActionKind) -> &'static str`
//! * `base_triggers_for(ActionKind) -> &'static [&'static str]`
//! * `derive_triggers_from_objective(objective, action_desc) -> Vec<String>`
//! * `compose_procedure_name(action_kind, goal_id_opt, objective, desc) -> String`
//!
//! Tests exercise the helpers directly (compose_procedure_name and
//! derive_triggers_from_objective) so the contract is pinned without
//! booting a full OODA cycle.
//!
//! ## Expected red signal
//!
//! Pre-PR-C those helpers do not exist; the file will fail to compile,
//! which IS the TDD red. PR-C adds the empty helper signatures first
//! (making compilation succeed), then fleshes out the bodies (making
//! the assertions pass).

#![cfg(test)]

use crate::ooda_loop::cycle::{
    compose_procedure_name, derive_triggers_from_objective, pattern_for,
};
use crate::ooda_loop::types::ActionKind;

/// `pattern_for(AdvanceGoal)` returns `"pr-merge"`. Catches anyone
/// renaming the pattern set without updating the docs.
#[test]
fn pattern_for_advance_goal_is_pr_merge() {
    assert_eq!(pattern_for(ActionKind::AdvanceGoal), "pr-merge");
}

/// `pattern_for(RunImprovement)` returns `"ci-fix"`.
#[test]
fn pattern_for_run_improvement_is_ci_fix() {
    assert_eq!(pattern_for(ActionKind::RunImprovement), "ci-fix");
}

/// `pattern_for(RunGymEval)` returns `"run-tests"`.
#[test]
fn pattern_for_run_gym_eval_is_run_tests() {
    assert_eq!(pattern_for(ActionKind::RunGymEval), "run-tests");
}

/// `compose_procedure_name` for a successful AdvanceGoal action
/// produces a name with the `pr-merge:` prefix, the goal id as
/// scope, the `| triggers:` suffix, and ALL the base triggers.
#[test]
fn compose_name_for_advance_goal_includes_pr_merge_prefix_and_base_triggers() {
    let name = compose_procedure_name(
        ActionKind::AdvanceGoal,
        Some("fix-auth-bug"),
        "open and review PR for the auth change",
        "spawn engineer to land the fix",
    );

    assert!(
        name.starts_with("pr-merge:fix-auth-bug"),
        "name must start with 'pr-merge:fix-auth-bug'; got: {name}"
    );
    assert!(
        name.contains("| triggers:"),
        "name must contain '| triggers:' separator; got: {name}"
    );
    // Base triggers from the doc table for AdvanceGoal.
    for kw in ["merge", "pr", "review", "ci"] {
        assert!(
            name.contains(kw),
            "name must contain base trigger '{kw}'; got: {name}"
        );
    }
}

/// `compose_procedure_name` for `RunImprovement` uses the `ci-fix:`
/// pattern + the goal id as scope. When `goal_id` is None it falls
/// back to `ad-hoc` per the doc.
#[test]
fn compose_name_for_run_improvement_uses_ci_fix_pattern() {
    let with_goal = compose_procedure_name(
        ActionKind::RunImprovement,
        Some("improve-coverage"),
        "fix the CI lint failure",
        "patch clippy warning",
    );
    assert!(
        with_goal.starts_with("ci-fix:improve-coverage"),
        "name must start with 'ci-fix:improve-coverage'; got: {with_goal}"
    );

    let without_goal = compose_procedure_name(ActionKind::RunImprovement, None, "fix lint", "");
    assert!(
        without_goal.starts_with("ci-fix:ad-hoc"),
        "name with None goal_id must use 'ad-hoc' scope; got: {without_goal}"
    );
}

/// `derive_triggers_from_objective` extracts `#NNNN` PR numbers
/// and file extensions like `.toml`. Both must end up in the returned
/// list (lowercased, deduplicated), and `compose_procedure_name`
/// folds them into the rendered name.
///
/// File extensions must be at least 3 characters (aligned with the
/// read-side `tokenize_objective` floor in `memory_consolidation` —
/// shorter tokens can never be matched by tokenized recall and only
/// add visual noise that resembles trailing-token truncation).
#[test]
fn procedure_name_contains_objective_derived_triggers() {
    let derived =
        derive_triggers_from_objective("merge PR #2281 fixing config.toml", "engineer review");
    let derived_set: std::collections::HashSet<&str> = derived.iter().map(|s| s.as_str()).collect();
    assert!(
        derived_set.contains("2281"),
        "derived triggers must include the PR number '2281'; got: {derived_set:?}"
    );
    assert!(
        derived_set.contains("toml"),
        "derived triggers must include the file extension 'toml'; got: {derived_set:?}"
    );

    // End-to-end: compose_procedure_name must surface those captures
    // in the rendered name.
    let name = compose_procedure_name(
        ActionKind::AdvanceGoal,
        Some("fix-cog-mem"),
        "merge PR #2281 fixing config.toml",
        "engineer review",
    );
    assert!(
        name.contains("2281"),
        "rendered name must contain '2281' from objective; got: {name}"
    );
    assert!(
        name.contains("toml"),
        "rendered name must contain 'toml' file-extension capture; got: {name}"
    );

    // Base triggers must still appear FIRST in the trigger list per
    // the doc's merge order rule.
    let pos_merge = name.find("merge").expect("base trigger 'merge' missing");
    let pos_2281 = name.find("2281").expect("derived '2281' missing");
    assert!(
        pos_merge < pos_2281,
        "base triggers must precede derived triggers in the rendered name; \
         got: {name}",
    );
}

/// Edge case: objective with no `#N` and no `.ext` — the derived
/// list is empty, the rendered name is well-formed without crashes,
/// and only base triggers appear.
#[test]
fn derive_triggers_handles_no_pr_or_ext_match() {
    let derived =
        derive_triggers_from_objective("investigate the recent slowness", "ad-hoc inquiry");
    assert!(
        derived.is_empty(),
        "no PR number, no file extension → derived list must be empty; \
         got: {derived:?}"
    );

    let name = compose_procedure_name(
        ActionKind::AdvanceGoal,
        Some("perf-audit"),
        "investigate the recent slowness",
        "ad-hoc inquiry",
    );
    assert!(
        name.contains("| triggers:"),
        "name must still be well-formed: {name}"
    );
    for kw in ["merge", "pr", "review", "ci"] {
        assert!(
            name.contains(kw),
            "base trigger '{kw}' must still appear; got: {name}"
        );
    }
}

/// Derived triggers must dedupe against base triggers and against
/// each other. An objective like "merge PR #2281 #2281 .toml .toml" must
/// not produce duplicate keywords in the rendered name.
#[test]
fn derived_triggers_dedupe_against_base_and_self() {
    let derived = derive_triggers_from_objective(
        "merge merge PR #2281 #2281 config.toml manifest.toml",
        "duplicate test",
    );

    // "2281" should appear at most once (self-dedup).
    let count_2281 = derived.iter().filter(|s| *s == "2281").count();
    assert_eq!(
        count_2281, 1,
        "derived triggers must self-dedupe; '2281' appeared {count_2281} times"
    );
    let count_toml = derived.iter().filter(|s| *s == "toml").count();
    assert_eq!(
        count_toml, 1,
        "derived triggers must self-dedupe; 'toml' appeared {count_toml} times"
    );

    // "merge" is a base trigger; it must NOT appear in derived (which
    // would cause a double when rendered).
    assert!(
        !derived.iter().any(|s| s == "merge"),
        "derived triggers must not duplicate base triggers; got: {derived:?}"
    );
}

/// ws2 #2295: short file-extension matches (1- or 2-char) must NOT
/// produce derived triggers. The read-side `tokenize_objective` floors
/// at 3 chars, so any sub-3-char trigger in a procedure name is dead
/// weight that can never be matched — and when it lands as the
/// trailing trigger it looks like the mid-word truncation symptom
/// users have flagged (a name ending in `…,distill,g`).
#[test]
fn derive_triggers_rejects_short_file_extensions() {
    // `.g` (1 char), `.rs` (2 chars), `.go` (2 chars), `.py` (2 chars)
    // are all valid file-extension shapes that the read-side
    // tokenizer cannot match. They must be dropped.
    let derived = derive_triggers_from_objective(
        "touch .g read cycle.rs build main.go ship script.py",
        "short-ext probe",
    );
    for shorty in ["g", "rs", "go", "py"] {
        assert!(
            !derived.iter().any(|t| t == shorty),
            "1- and 2-char file extensions must not be emitted as derived triggers; \
             got '{shorty}' in {derived:?}"
        );
    }

    // 3-char extensions are still accepted — they ARE matchable by
    // tokenize_objective.
    let kept = derive_triggers_from_objective("update vars.tfvars and config.toml", "");
    assert!(
        kept.iter().any(|t| t == "toml"),
        "3+ char extensions must still be extracted; got: {kept:?}"
    );
}
