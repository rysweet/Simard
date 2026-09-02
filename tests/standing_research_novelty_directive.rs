//! Durable prompt-asset contract for novelty-first steering of Simard's
//! STANDING cognition-research goal (issue #4347).
//!
//! Operator directive: the standing goal
//! `continuously-research-and-improve-your-own-cogn-70ab8541`
//! ("Continuously research and improve your own cognition … STANDING PERPETUAL
//! goal") keeps producing narrow INCREMENTAL parse-site / dedup fixes. It must
//! instead, each cycle, FIRST survey GENUINELY NEW cognition-research
//! directions and PREFER pursuing a novel one (prototype + benchmark against
//! the recall-precision / fact-yield baseline; a durable PR OR a memory-recorded
//! reasoned negative result) over another incremental refinement — falling back
//! to incremental maintenance only when no novel direction is viable, and
//! SAYING SO.
//!
//! The fix is durable in the daemon's prompt assets (self-scoped to standing
//! cognition/research goals), so it survives goal-board re-persist and is NOT a
//! runtime CLI priority tweak. These tests pin that the directive text is
//! present in the objective handed to the goal session and reinforced at the
//! orient/decide reasoning points. They assert on stable keyword invariants,
//! not full-sentence snapshots, so ordinary rewording does not break them.

use std::fs;
use std::path::PathBuf;

fn prompt(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read prompt asset {}: {e}", path.display()))
}

fn prompt_lc(name: &str) -> String {
    prompt(name).to_lowercase()
}

fn assert_contains(haystack_lc: &str, needle: &str, file: &str, what: &str) {
    assert!(
        haystack_lc.contains(&needle.to_lowercase()),
        "{file} must express {what} (expected to contain {needle:?})"
    );
}

fn assert_contains_any(haystack_lc: &str, needles: &[&str], file: &str, what: &str) {
    assert!(
        needles
            .iter()
            .any(|n| haystack_lc.contains(&n.to_lowercase())),
        "{file} must express {what} (expected one of {needles:?})"
    );
}

#[test]
fn objective_directs_standing_research_goal_to_seek_novel_directions_first() {
    let c = prompt_lc("goal_session_objective.md");

    // Self-scopes to a standing cognition/research goal (not pinned to the slug).
    assert_contains(
        &c,
        "standing",
        "goal_session_objective.md",
        "self-scoping to a standing goal",
    );
    assert_contains_any(
        &c,
        &["cognition", "research"],
        "goal_session_objective.md",
        "self-scoping to a cognition/research goal",
    );

    // (1) FIRST survey genuinely NEW / unexplored directions.
    assert_contains(
        &c,
        "novel",
        "goal_session_objective.md",
        "seeking genuinely novel research directions",
    );
    assert_contains_any(
        &c,
        &["survey", "unexplored", "not yet tried", "not yet explored"],
        "goal_session_objective.md",
        "surveying unexplored directions each cycle",
    );

    // (2) PREFER a novel direction OVER incremental refinement, with
    //     benchmark / negative-result discipline.
    assert_contains(
        &c,
        "incremental",
        "goal_session_objective.md",
        "contrasting novel work against incremental refinement",
    );
    assert_contains(
        &c,
        "benchmark",
        "goal_session_objective.md",
        "benchmarking a novel direction against the baseline",
    );
    assert_contains_any(
        &c,
        &["negative result", "negative-result"],
        "goal_session_objective.md",
        "recording a reasoned negative result when a novel direction fails",
    );

    // (3) Fall back to incremental only when no novel direction is viable — and say so.
    assert_contains_any(
        &c,
        &["fall back", "fallback", "falls back"],
        "goal_session_objective.md",
        "falling back to incremental only when no novel direction is viable",
    );
}

#[test]
fn orient_reinforces_novelty_first_for_standing_research_goals() {
    let c = prompt_lc("ooda_orient.md");
    assert_contains(
        &c,
        "novel",
        "ooda_orient.md",
        "reinforcing novelty-first steering at the orient step",
    );
    assert_contains(
        &c,
        "incremental",
        "ooda_orient.md",
        "preferring novel over incremental at the orient step",
    );
    assert_contains_any(
        &c,
        &["standing", "research", "cognition"],
        "ooda_orient.md",
        "scoping the novelty-first reinforcement to standing research/cognition work",
    );
}

#[test]
fn decide_reinforces_novelty_first_for_standing_research_goals() {
    let c = prompt_lc("ooda_decide.md");
    assert_contains(
        &c,
        "novel",
        "ooda_decide.md",
        "reinforcing novelty-first steering at the decide step",
    );
    assert_contains(
        &c,
        "incremental",
        "ooda_decide.md",
        "preferring novel over incremental at the decide step",
    );
    assert_contains_any(
        &c,
        &["standing", "research", "cognition"],
        "ooda_decide.md",
        "scoping the novelty-first reinforcement to standing research/cognition work",
    );
}
