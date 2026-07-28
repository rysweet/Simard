//! TEST-FIRST (Step 7 TDD) — reblock-issue **signature stabilization** that ends
//! the Overseer's `recurring_goal_reblock in simard::overseer` stewardship-issue
//! churn (process_health, HIGH).
//!
//! # The churn these tests kill
//!
//! When the Overseer re-observes a goal being re-blocked it files a deduplicated
//! stewardship issue keyed on `failure_signature(failure_kind, error_text)`. The
//! `dedup_key` (which flows into BOTH `failure_kind` and the error text) embeds a
//! **volatile goal identifier** — e.g. `simard-identity-<slug>` or a positional
//! `goal-<n>`. So every re-observation of the *same* underlying re-block cause
//! produces a different signature and files a fresh issue: the observed storm of
//! `recurring_goal_reblock in simard::overseer` issues (8 open in 24h).
//!
//! # The contract (what the fix must make true)
//!
//! 1. A new pure, total helper `fold_volatile_goal_ids(dedup_key)` folds the
//!    known volatile identifier shapes to stable placeholders and returns
//!    everything else byte-for-byte (conservative — distinct causes keep distinct
//!    keys).
//! 2. Applied upstream of the existing `failure_signature`, two re-block
//!    observations that differ ONLY by a volatile goal id must collapse to ONE
//!    `failure_signature` (so the stewardship dedup files ONE issue), while two
//!    genuinely different causes keep distinct signatures.
//!
//! The end-to-end property is asserted through the public `decide_read_only`
//! seam (which builds the brief the stewardship filer dedups on). RED until
//! `fold_volatile_goal_ids` exists and is applied on the reblock path.

use super::observer::fold_volatile_goal_ids;
use super::{Intervention, decide_read_only};
use crate::overseer::signal::{Priority, Problem, ProblemKind};
use crate::stewardship::failure_signature;

// --- fixtures ---------------------------------------------------------------

/// Build a process-health problem (the reblock family routes here → FileIssue)
/// with the given dedup key.
fn reblock_problem(dedup_key: &str) -> Problem {
    Problem {
        kind: ProblemKind::ProcessHealth,
        priority: Priority::Normal,
        dedup_key: dedup_key.to_string(),
        summary: "recurring_goal_reblock in simard::overseer".to_string(),
        evidence: vec![],
        why: None,
    }
}

/// The stewardship dedup signature the filer would compute for a problem —
/// extracted through the public Decide seam so the test tracks the real path.
fn dedup_signature(problem: &Problem) -> String {
    match decide_read_only(problem) {
        Intervention::FileIssue { run } => failure_signature(&run.failure_kind, &run.error_text),
        other => panic!("a ProcessHealth problem must route to FileIssue, got {other:?}"),
    }
}

// === fold_volatile_goal_ids: the folding table ==============================

#[test]
fn folds_simard_identity_slugs_to_a_stable_placeholder() {
    assert_eq!(
        fold_volatile_goal_ids("recurring_goal_reblock simard-identity-atelier-furniture-de"),
        "recurring_goal_reblock simard-identity-*",
        "a simard-identity-<slug> id must fold to the stable simard-identity-* placeholder"
    );
    // Two DIFFERENT identity slugs fold to the SAME string.
    assert_eq!(
        fold_volatile_goal_ids("reblock simard-identity-luxe-coastal-lighting"),
        fold_volatile_goal_ids("reblock simard-identity-artisan-heritage-textiles"),
        "distinct identity slugs of the same cause must fold identically"
    );
}

#[test]
fn folds_positional_goal_slugs_to_a_stable_placeholder() {
    assert_eq!(
        fold_volatile_goal_ids("recurring_goal_reblock goal-12"),
        "recurring_goal_reblock goal-*",
        "a positional goal-<n> id must fold to the stable goal-* placeholder"
    );
    assert_eq!(
        fold_volatile_goal_ids("reblock goal-3"),
        fold_volatile_goal_ids("reblock goal-9871"),
        "distinct positional goal ids of the same cause must fold identically"
    );
}

#[test]
fn leaves_unrelated_text_byte_for_byte() {
    // Conservative: only the known volatile shapes are rewritten; everything else
    // passes through untouched so genuinely different causes keep distinct keys.
    for s in [
        "PanicInStep",
        "process:distill_fail",
        "recurring_goal_reblock in simard::overseer",
        "coverage-goal-parity", // not the `goal-<n>` shape
        "identity",             // not the `simard-identity-<slug>` shape
    ] {
        assert_eq!(
            fold_volatile_goal_ids(s),
            s,
            "unrelated text must be returned byte-for-byte: {s:?}"
        );
    }
}

// === end-to-end dedup property (the churn stopper) ==========================

#[test]
fn reblocks_differing_only_by_identity_slug_dedup_to_one_signature() {
    let a = reblock_problem("recurring_goal_reblock simard-identity-atelier-furniture-de");
    let b = reblock_problem("recurring_goal_reblock simard-identity-luxe-coastal-lighting");

    assert_ne!(
        a.dedup_key, b.dedup_key,
        "the two dedup keys genuinely differ (only by the volatile identity slug)"
    );
    assert_eq!(
        dedup_signature(&a),
        dedup_signature(&b),
        "two re-block observations of the SAME cause that differ only by a volatile \
         simard-identity slug must collapse to ONE stewardship signature — otherwise a fresh \
         `recurring_goal_reblock` issue is filed every cycle (the storm)"
    );
}

#[test]
fn reblocks_differing_only_by_positional_goal_id_dedup_to_one_signature() {
    let a = reblock_problem("recurring_goal_reblock goal-12 in simard::overseer");
    let b = reblock_problem("recurring_goal_reblock goal-4087 in simard::overseer");

    assert_ne!(a.dedup_key, b.dedup_key);
    assert_eq!(
        dedup_signature(&a),
        dedup_signature(&b),
        "re-blocks differing only by a positional goal id must dedup to ONE signature"
    );
}

#[test]
fn genuinely_different_reblock_causes_keep_distinct_signatures() {
    // The fix must NOT over-collapse: two DIFFERENT underlying causes keep
    // distinct signatures so each still gets its own tracked issue.
    let admission = reblock_problem("recurring_goal_reblock goal-12 admission-gate-rejected");
    let unclear = reblock_problem("recurring_goal_reblock goal-12 unclear-criteria");
    assert_ne!(
        dedup_signature(&admission),
        dedup_signature(&unclear),
        "distinct re-block causes must keep distinct signatures (no over-collapse)"
    );
}
