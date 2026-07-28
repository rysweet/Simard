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

/// Edge / negative cases (Step 18b review, finding #4): pin the exact fold
/// boundaries so a future refactor of the single-scan folder cannot silently
/// over-collapse a distinct cause OR under-fold a real volatile id. Each case
/// documents WHY the folder does (or does not) rewrite it.
#[test]
fn fold_boundaries_are_exact_and_utf8_safe() {
    // `goal-` NOT followed by an ASCII digit is NOT the positional shape — it
    // must survive untouched (a real goal-id run needs at least one digit).
    for untouched in [
        "goal-",              // trailing prefix, no slug at all
        "goal-abc",           // letters, not digits
        "goal-x1",            // starts with a letter, not a digit
        "goal- 12",           // a space breaks the digit run immediately
        "reblock goal-",      // trailing prefix mid-string
        "simard-identity-",   // trailing identity prefix, EMPTY slug
        "simard-identity- x", // space breaks the slug run immediately
    ] {
        assert_eq!(
            fold_volatile_goal_ids(untouched),
            untouched,
            "a non-matching volatile-prefix shape must be returned byte-for-byte: {untouched:?}"
        );
    }

    // Partial fold: the DIGIT run folds, trailing non-digits are preserved
    // verbatim — the fold consumes exactly `goal-<digits>` and no more.
    assert_eq!(
        fold_volatile_goal_ids("goal-12abc"),
        "goal-*abc",
        "only the leading digit run of a positional id folds; the rest is preserved"
    );

    // Every occurrence in a key folds independently.
    assert_eq!(
        fold_volatile_goal_ids("goal-1 blocks goal-4087"),
        "goal-* blocks goal-*",
        "multiple positional ids in one key each fold to the stable placeholder"
    );
    assert_eq!(
        fold_volatile_goal_ids("simard-identity-nordic-hearth and goal-9"),
        "simard-identity-* and goal-*",
        "identity and positional shapes fold together in the same key"
    );

    // UTF-8 safety: multibyte scalars adjacent to a fold must be copied whole
    // (never split on a byte boundary), and the fold itself is unaffected.
    assert_eq!(
        fold_volatile_goal_ids("café goal-7 ☕ simard-identity-x1 ✓"),
        "café goal-* ☕ simard-identity-* ✓",
        "multibyte characters around a fold must survive intact (char-boundary safe)"
    );
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

// === MEDIUM (Step 17b review): verify the fold against the REAL, production ===
// === dedup_key shapes `classify_signal` actually emits ======================
//
// The prior fold tests used illustrative `"recurring_goal_reblock <id>"` keys.
// The reviewer asked to pin the fold against a CAPTURED real reblock dedup_key.
// The production keys carrying a volatile goal id are emitted verbatim by
// `crate::overseer::mod::classify_signal`:
//   * GoalBlocked  => `format!("goal:blocked:{goal_id}")`
//   * StaleGoal    => `format!("goal:stale:{goal_id}")`
//   * LoopDetected => `format!("loop:{goal_id}")`
//   * DriftCorrection => `format!("drift:{goal_id}")`
// and the goal_id itself is a volatile `simard-identity-<slug>` or `goal-<n>`.
// These tests assert the fold folds exactly those real shapes (and only the
// volatile suffix), so recurrences of the same cause collapse to ONE signature.

#[test]
fn folds_real_goal_blocked_dedup_key_shape() {
    // Real production key: `goal:blocked:<goal_id>`.
    assert_eq!(
        fold_volatile_goal_ids("goal:blocked:simard-identity-luxe-coastal-lighting"),
        "goal:blocked:simard-identity-*",
        "the real GoalBlocked key must fold its volatile identity slug"
    );
    assert_eq!(
        fold_volatile_goal_ids("goal:blocked:goal-4087"),
        "goal:blocked:goal-*",
        "the real GoalBlocked key must fold its volatile positional id"
    );
    // Two re-block recurrences of the SAME cause (differing only by the volatile
    // goal id) collapse to ONE folded key across BOTH identity shapes.
    assert_eq!(
        fold_volatile_goal_ids("goal:blocked:simard-identity-atelier-furniture-de"),
        fold_volatile_goal_ids("goal:blocked:simard-identity-luxe-coastal-lighting"),
    );
    assert_eq!(
        fold_volatile_goal_ids("goal:blocked:goal-12"),
        fold_volatile_goal_ids("goal:blocked:goal-4087"),
    );
}

#[test]
fn folds_real_loop_stale_and_drift_dedup_key_shapes() {
    // `loop:<goal_id>`, `goal:stale:<goal_id>`, `drift:<goal_id>` — the other
    // real classify_signal shapes that embed a volatile goal id.
    assert_eq!(
        fold_volatile_goal_ids("loop:simard-identity-nordic-hearth-ceramics"),
        "loop:simard-identity-*",
    );
    assert_eq!(
        fold_volatile_goal_ids("goal:stale:goal-9871"),
        "goal:stale:goal-*",
    );
    assert_eq!(fold_volatile_goal_ids("drift:goal-3"), "drift:goal-*");
    // The stable `goal:blocked:` / `goal:stale:` prefixes contain the substring
    // "goal" but NOT the `goal-<digit>` shape, so they are preserved untouched —
    // only the trailing volatile id is folded (no over-collapse of the prefix).
    assert_eq!(
        fold_volatile_goal_ids("goal:stale:simard-identity-x1"),
        "goal:stale:simard-identity-*",
    );
}

#[test]
fn real_reblock_keys_of_different_goals_dedup_to_one_signature_end_to_end() {
    // End-to-end through the Decide seam using the REAL `goal:blocked:<id>` key
    // shape: two blocked-goal recurrences differing only by the volatile goal id
    // must file ONE stewardship signature.
    let a = reblock_problem("goal:blocked:simard-identity-atelier-furniture-de");
    let b = reblock_problem("goal:blocked:simard-identity-luxe-coastal-lighting");
    assert_ne!(a.dedup_key, b.dedup_key, "the raw keys genuinely differ");
    assert_eq!(
        dedup_signature(&a),
        dedup_signature(&b),
        "real GoalBlocked re-block keys differing only by the volatile identity slug must \
         collapse to ONE stewardship signature"
    );
}
