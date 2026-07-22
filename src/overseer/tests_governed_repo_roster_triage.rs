//! Tests for the **governed-repo-roster escalation-triage** decision
//! (goal `move-the-governed-repo-roster-out-of-framework-a8f57a50`).
//!
//! Context. The Overseer's escalation-triage brain
//! (`prompt_assets/simard/overseer/escalation_triage.md`) was handed a
//! genuinely-blocked goal: move the governed-repo roster out of framework code
//! and into Simard's identity as agentically-curated, mutable, deploy-durable
//! state. Simard could not automatically tell when this goal was finished — its
//! finish line was a multi-part prose wish with nothing a done-gate could
//! certify — and the worker assigned to it wedged on a stale worktree that still
//! held a cognitive-store lock, so the safeguard marked the goal blocked.
//!
//! The chosen course-correction is **`rewrite-done-gate`**, landed as an
//! additive, CI-green charter (`Specs/GOVERNED_REPO_ROSTER.md`): the charter's
//! `State` is `RATIFIED`, and the goal's finish criteria are re-pointed at the
//! charter's single machine-checkable acceptance test (§2) instead of the vague
//! multi-part wish. No Rust escalation-seam change, no CI hard gate.
//!
//! Each assertion is the executable contract the course-correction must keep
//! satisfying. To avoid coupling the build to incidental doc wording, the
//! assertions are anchored on stable structural markers — section headings, the
//! `**State**` line, literal commands, the goal id, and the decision enum value
//! — rather than free prose, so cosmetic rewording does not break the tests
//! while genuine contract removals still fail loud.
//!
//! Everything here is hermetic: it reads only checked-in repository artifacts
//! relative to `CARGO_MANIFEST_DIR`. No network, no `~/.simard`, no goal store.

use std::path::PathBuf;

/// The blocked goal this triage course-corrects.
const GOAL_ID: &str = "move-the-governed-repo-roster-out-of-framework-a8f57a50";

/// The machine-checkable rewrite artifact the `rewrite-done-gate` decision
/// points the goal at.
const CHARTER: &str = "Specs/GOVERNED_REPO_ROSTER.md";

/// The plain-English how-to that records what the triage decided and did.
const HOWTO: &str = "docs/howto/triage-the-governed-repo-roster-goal.md";

/// The single machine-checkable acceptance gate the rewritten done-gate names.
const ACCEPTANCE_CMD: &str = "cargo test -p simard governed_repo_roster";

/// Raw machine markers the OPERATOR must never see. The triage inputs
/// (`internal_why`, `reason_marker`) carry these; every operator-visible string
/// the triage emits must translate them away.
const OPERATOR_JARGON_TOKENS: &[&str] = &[
    "OODA-SAFEGUARD",
    "UNCLEAR-CRITERIA",
    "GENUINELY-STUCK",
    "evidence=[",
    "why=",
    "health-review:blocked-goal-unclear-criteria",
    "\u{1F512}", // the 🔒 lock marker
];

fn read_repo_file(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("expected repo file {rel} to exist and be readable: {e}"))
}

/// Extract the body of the first `## ` (H2) section whose heading contains
/// `needle` (case-insensitive), spanning from that heading up to the next `## `
/// heading (or end of file). Returns an empty string when no such section
/// exists, so callers assert the section's presence explicitly and fail loud on
/// a missing structural anchor.
fn section(doc: &str, needle: &str) -> String {
    let needle_lower = needle.to_lowercase();
    let mut out = String::new();
    let mut in_section = false;
    for line in doc.lines() {
        if line.starts_with("## ") {
            if in_section {
                break; // reached the next H2 section — stop.
            }
            if line.to_lowercase().contains(&needle_lower) {
                in_section = true;
            }
        }
        if in_section {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Extract the concatenation of every fenced ```json block in the how-to — the
/// strings the operator actually sees.
fn howto_operator_json_blocks(howto: &str) -> String {
    let mut out = String::new();
    let mut in_json = false;
    for line in howto.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```json") {
            in_json = true;
            continue;
        }
        if in_json && trimmed.starts_with("```") {
            in_json = false;
            continue;
        }
        if in_json {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn assert_free_of_operator_jargon(text: &str, context: &str) {
    for token in OPERATOR_JARGON_TOKENS {
        assert!(
            !text.contains(token),
            "{context} is operator-visible and must be plain English, but contains \
             the raw marker {token:?}:\n{text}"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Section A — the `rewrite-done-gate` course-correction landed
// ════════════════════════════════════════════════════════════════════════════

/// The performed rewrite ratifies the charter: its `State` is `RATIFIED`, not
/// `PROPOSED`. This is the single concrete, machine-observable artifact the
/// `rewrite-done-gate` decision produces.
#[test]
fn charter_is_ratified_not_proposed() {
    let charter = read_repo_file(CHARTER);

    let state_line = charter
        .lines()
        .find(|l| l.contains("**State**"))
        .unwrap_or_else(|| {
            panic!("the charter must carry a single `- **State**:` line in its Status section")
        });

    assert!(
        state_line.contains("RATIFIED"),
        "the rewrite-done-gate course-correction must ratify the charter, but the \
         State line is still: {state_line:?}"
    );
    assert!(
        !state_line.contains("PROPOSED"),
        "the ratified charter must no longer read PROPOSED: {state_line:?}"
    );

    let state_lines = charter.lines().filter(|l| l.contains("**State**")).count();
    assert_eq!(
        state_lines, 1,
        "the ratification must edit a single State line in place, leaving exactly \
         one (found {state_lines})"
    );
}

/// The charter absorbs THIS goal's slug so a future resurfacing resolves to the
/// charter's machine-checkable gate rather than re-opening an open-ended cycle.
#[test]
fn charter_governs_this_goal_slug() {
    let charter = read_repo_file(CHARTER);
    assert!(
        charter.contains(GOAL_ID),
        "the charter must name the goal slug it re-points ({GOAL_ID}) so a future \
         resurfacing resolves here"
    );
    let lower = charter.to_lowercase();
    assert!(
        lower.contains("canonical written charter") || lower.contains("governs goal slug"),
        "the charter must declare itself the canonical consolidation point for the goal"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section B — the rewritten done-gate is machine-checkable (not a prose wish)
// ════════════════════════════════════════════════════════════════════════════

/// The rewrite replaces an unmeasurable finish line with one a done-gate can
/// certify from command output and files: a fresh identity seeds the roster, a
/// runtime mutation persists, an install survives without reset, and a single
/// named guard command is green. This is what makes the decision
/// `rewrite-done-gate` rather than a fresh vague target.
#[test]
fn charter_done_gate_is_machine_checkable() {
    let charter = read_repo_file(CHARTER);
    let done = section(&charter, "Measurable done-criteria");
    assert!(
        !done.is_empty(),
        "the charter must carry a `## 2. Measurable done-criteria` section — the \
         structural home of the machine-checkable done-gate"
    );
    let lower = done.to_lowercase();

    assert!(
        done.contains(ACCEPTANCE_CMD),
        "§2's machine-checkable done-gate must name the single acceptance command \
         ({ACCEPTANCE_CMD:?})"
    );
    // The three roster properties from the goal, each machine-observable.
    assert!(
        lower.contains("identity") && (lower.contains("target_repos") || lower.contains("seed")),
        "§2 must require the roster to be SEEDED FROM THE IDENTITY"
    );
    assert!(
        lower.contains("runtime") && (lower.contains("mutab") || lower.contains("add or remove")),
        "§2 must require the roster to be MUTABLE AT RUNTIME"
    );
    assert!(
        (lower.contains("self-deploy") || lower.contains("install")) && lower.contains("survive"),
        "§2 must require the roster to SURVIVE A SELF-DEPLOY without reset"
    );
    assert!(
        lower.contains("merged"),
        "§2's finish line must be observable — a specific PR the done-gate sees MERGED"
    );
}

/// The §2 acceptance bullets are recorded as un-checked list items describing
/// still-open conditions, justifying `rewrite-done-gate` over
/// `complete-delivered-goal`: no merged PR delivers the roster move yet.
#[test]
fn charter_done_criteria_justify_rewrite_over_completion() {
    let charter = read_repo_file(CHARTER);
    let done = section(&charter, "Measurable done-criteria");
    assert!(
        !done.is_empty(),
        "the charter must carry a `## 2. Measurable done-criteria` section"
    );
    // No §2 acceptance bullet may be pre-checked: the roster move is not shipped,
    // which is exactly why complete-delivered-goal was rejected.
    assert!(
        !done.contains("- [x]") && !done.contains("- [X]"),
        "no §2 acceptance bullet may be pre-checked: the roster move is not certified \
         delivered, which is why complete-delivered-goal was rejected"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section C — root cause: unmeasurable finish line + a stale-worktree wedge
// ════════════════════════════════════════════════════════════════════════════

/// The true root cause is an unmeasurable multi-part done-gate combined with a
/// worker wedged on a stale worktree holding a cognitive-store lock. The charter
/// must state this root cause and disambiguate scope (in / out) so a future
/// cycle is not pointed at an open-ended wish.
#[test]
fn charter_states_root_cause_and_scope() {
    let charter = read_repo_file(CHARTER);
    let why = section(&charter, "Why this charter exists");
    assert!(
        !why.is_empty(),
        "the charter must carry a `## Why this charter exists now` section stating the root cause"
    );
    let why_lower = why.to_lowercase();
    assert!(
        why_lower.contains("unmeasurable")
            || why_lower.contains("nothing a done-gate could certify"),
        "the root cause must name the unmeasurable finish line"
    );
    assert!(
        why_lower.contains("worktree") && why_lower.contains("lock"),
        "the root cause must name the stale-worktree / held-lock wedge"
    );

    let scope = section(&charter, "What the goal means");
    assert!(
        !scope.is_empty(),
        "the charter must carry a `## 1. What the goal means` section"
    );
    let scope_lower = scope.to_lowercase();
    assert!(
        scope_lower.contains("ecosystem_repos.toml") && scope_lower.contains("identity"),
        "§1 must contrast today's framework file with the identity-scoped target"
    );
    assert!(
        scope_lower.contains("in scope") && scope_lower.contains("out of scope"),
        "§1 must carry an explicit in-scope / out-of-scope disambiguation"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section D — the plain-English how-to records the decision, jargon-free
// ════════════════════════════════════════════════════════════════════════════

/// The how-to documents this exact triage: the decision is `rewrite-done-gate`,
/// `complete-delivered-goal` was considered and rejected, no human was
/// escalated to, and the stale-worktree wedge was cleared so a fresh worker can
/// act.
#[test]
fn howto_records_the_rewrite_done_gate_decision_for_this_goal() {
    let howto = read_repo_file(HOWTO);
    let lower = howto.to_lowercase();

    assert!(
        lower.contains(GOAL_ID),
        "the how-to must name the goal being triaged"
    );
    assert!(
        lower.contains("rewrite-done-gate"),
        "the how-to must record the chosen decision: rewrite-done-gate"
    );
    assert!(
        lower.contains("complete-delivered-goal"),
        "the how-to must show complete-delivered-goal was considered (and rejected)"
    );
    assert!(
        lower.contains("ratif"),
        "the how-to must document ratifying the charter as an action taken"
    );
    assert!(
        lower.contains("re-point") || lower.contains("repoint") || lower.contains("point the goal"),
        "the how-to must document re-pointing the goal's done-criteria at the charter"
    );
    assert!(
        lower.contains("no escalation")
            || lower.contains("nothing needed from you")
            || lower.contains("without paging")
            || lower.contains("no human"),
        "the how-to must record that no human escalation was required (escalate = null)"
    );
    // Criterion: the stale-worktree / lock wedge was accounted for.
    assert!(
        lower.contains("worktree") && lower.contains("lock"),
        "the how-to must record clearing the stale-worktree / held-lock wedge so a \
         fresh worker can act"
    );
}

/// The operator-visible OUTPUT example in the how-to (the fenced ```json blocks)
/// must be pure plain English — the whole point of the triage is to translate
/// the raw markers away before a person sees them.
#[test]
fn howto_operator_output_is_free_of_raw_markers() {
    let howto = read_repo_file(HOWTO);
    let operator_json = howto_operator_json_blocks(&howto);
    assert!(
        !operator_json.trim().is_empty(),
        "the how-to must include an operator-visible OUTPUT json example to check"
    );
    assert_free_of_operator_jargon(&operator_json, "how-to operator OUTPUT json example");
    assert!(
        operator_json.contains("rewrite-done-gate"),
        "the operator OUTPUT example must carry the chosen decision value"
    );
    assert!(
        operator_json.to_lowercase().contains("roster"),
        "the operator OUTPUT example must be about the roster goal being triaged"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section E — the recipe's OUTPUT contract the triage answered (6 keys)
// ════════════════════════════════════════════════════════════════════════════

/// The triage's deliverable is the recipe's six-key OUTPUT object. The recipe
/// asset must define exactly those keys and offer `rewrite-done-gate` as one of
/// the three decision enum values — the contract this triage's output satisfies.
#[test]
fn triage_recipe_defines_the_six_key_output_and_rewrite_decision() {
    let recipe = read_repo_file("prompt_assets/simard/overseer/escalation_triage.md");
    let lower = recipe.to_lowercase();

    for key in [
        "\"problem\"",
        "\"next_step\"",
        "\"root_cause\"",
        "\"decision\"",
        "\"action_taken\"",
        "\"escalate\"",
    ] {
        assert!(
            recipe.contains(key),
            "the recipe OUTPUT contract must define the {key} field the triage fills in"
        );
    }

    for decision in [
        "rewrite-done-gate",
        "complete-delivered-goal",
        "ask-operator-one-question",
    ] {
        assert!(
            lower.contains(decision),
            "the recipe must offer {decision:?} as a decision enum value"
        );
    }

    assert!(
        lower.contains("null"),
        "the recipe OUTPUT must allow escalate = null when no human is required"
    );
}
