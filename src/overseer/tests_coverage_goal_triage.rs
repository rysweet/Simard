//! TDD (RED) tests for the **coverage-goal escalation-triage** decision
//! (goal `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`).
//!
//! Context. The Overseer's escalation-triage brain
//! (`prompt_assets/simard/overseer/escalation_triage.md`) was handed a
//! genuinely-blocked goal: Simard had tried and failed five times in a row to
//! raise her own test coverage to 70%, restarting the same work without ever
//! finishing or making measurable progress. The triage had to pick exactly one
//! course-correction:
//!
//!   * `rewrite-done-gate`         — the finish line is unmeasurable; re-scope it
//!                                   so completion is machine-checkable,
//!   * `complete-delivered-goal`   — a single merged PR already delivered it, or
//!   * `ask-operator-one-question` — a human decision is genuinely required.
//!
//! The chosen course-correction is **`rewrite-done-gate`**, landed as an
//! additive, CI-green edit to the Coverage-Audit Charter
//! (`Specs/COVERAGE_AUDIT.md`): the charter's `State` flips
//! `PROPOSED → RATIFIED`, and the goal's finish criteria are re-pointed at the
//! charter's machine-checkable milestones (§2/§3) instead of the vague
//! "70% everywhere". No Rust escalation-seam change, no CI hard gate.
//!
//! These tests are written BEFORE that edit lands. Today `Specs/COVERAGE_AUDIT.md`
//! still reads `State: PROPOSED`, so `charter_is_ratified_not_proposed` FAILS —
//! that failure is the RED of red→green→refactor. Each assertion below is the
//! executable contract the course-correction must satisfy.
//!
//! Everything here is hermetic: it reads only checked-in repository artifacts
//! relative to `CARGO_MANIFEST_DIR`. No network, no `~/.simard`, no goal store.

use std::path::PathBuf;

/// The blocked goal this triage course-corrects.
const GOAL_ID: &str = "audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a";

/// The machine-checkable rewrite artifact the `rewrite-done-gate` decision
/// points the goal at.
const CHARTER: &str = "Specs/COVERAGE_AUDIT.md";

/// The plain-English how-to that records what the triage decided and did.
const HOWTO: &str = "docs/howto/triage-a-stuck-coverage-goal.md";

/// The observable evidence surface the rewritten done-gate checks.
const LEDGER: &str = "docs/testing/COVERAGE_BASELINE.md";

/// Raw machine markers the OPERATOR must never see. The triage inputs
/// (`internal_why`, `reason_marker`) carry these; every operator-visible string
/// the triage emits must translate them away. NOTE: the charter itself is an
/// ENGINEER-facing artifact and may quote these tokens (it does, in
/// "Why this charter exists"), so this set is asserted only against
/// operator-visible surfaces, never against the whole charter.
const OPERATOR_JARGON_TOKENS: &[&str] = &[
    "OODA-SAFEGUARD",
    "UNCLEAR-CRITERIA",
    "GENUINELY-STUCK",
    "evidence=[",
    "why=",
    "health-review:stuck-goal",
    "\u{1F512}", // the 🔒 lock marker
];

fn read_repo_file(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("expected repo file {rel} to exist and be readable: {e}"))
}

/// Extract the fenced ```json OUTPUT example block from the how-to — the set of
/// strings the operator actually sees. Returns the concatenation of every JSON
/// fence so jargon assertions cover all operator-visible example values.
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
// Section A — the `rewrite-done-gate` course-correction landed (the RED test)
// ════════════════════════════════════════════════════════════════════════════

/// The performed rewrite ratifies the charter: its `State` is `RATIFIED`, not
/// the pre-triage `PROPOSED`. This is the single concrete, machine-observable
/// artifact the `rewrite-done-gate` decision produces, and it is the assertion
/// that is RED today (the charter still reads `PROPOSED`).
#[test]
fn charter_is_ratified_not_proposed() {
    let charter = read_repo_file(CHARTER);

    // Locate the single `State:` bullet in the Status section.
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

    // Fail-loud, additive edit: exactly one State line — not a duplicate appended
    // alongside the old PROPOSED one.
    let state_lines = charter.lines().filter(|l| l.contains("**State**")).count();
    assert_eq!(
        state_lines, 1,
        "the ratification must edit the existing State line in place, leaving exactly \
         one (found {state_lines}); a fail-loud rewrite never appends a duplicate"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section B — the rewritten done-gate is machine-checkable (not "70% everywhere")
// ════════════════════════════════════════════════════════════════════════════

/// The rewrite replaces an unmeasurable finish line with one a done-gate can
/// certify from command output and files: per-group aggregate ≥ 70% measured by
/// `cargo llvm-cov`, an empty backlog in the ledger, and a deterministic scan,
/// tombstoned via a concrete command. This is what makes the decision
/// `rewrite-done-gate` rather than a fresh vague target.
#[test]
fn charter_done_gate_is_machine_checkable() {
    let charter = read_repo_file(CHARTER);
    let lower = charter.to_lowercase();

    assert!(
        lower.contains("cargo llvm-cov"),
        "the machine-checkable done-gate must name the measuring command (cargo llvm-cov)"
    );
    assert!(
        charter.contains("70%") || charter.contains("\u{2265} 70") || lower.contains("70% "),
        "the done-gate must state the ≥70% aggregate bar"
    );
    assert!(
        lower.contains("aggregate line coverage"),
        "the unit of measurement must be the per-group aggregate line coverage, not a \
         single workspace-wide percentage"
    );
    assert!(
        lower.contains("simard goal remove"),
        "the done-gate must specify the concrete tombstone command a cycle runs when DONE"
    );
    assert!(
        charter.contains(LEDGER) || lower.contains("companion ledger"),
        "the done-gate must reference the observable ledger evidence surface"
    );
}

/// The three whole-audit DONE conditions are recorded as UNCHECKED checkboxes
/// (`- [ ]`). An unchecked gate is precisely why `complete-delivered-goal` was
/// REJECTED: the audit is not yet certified complete (ledger backlog + §3 scan
/// conditions remain open), and no single merged PR delivers the whole audit —
/// so re-scoping the gate, not claiming completion, is the correct decision.
#[test]
fn charter_done_criteria_are_unchecked_justifying_rewrite_over_completion() {
    let charter = read_repo_file(CHARTER);
    let unchecked = charter.matches("- [ ]").count();
    assert!(
        unchecked >= 3,
        "the whole-audit DONE gate must list its (still-open) machine-checkable \
         conditions as unchecked checkboxes, justifying rewrite-done-gate over \
         complete-delivered-goal; found {unchecked}"
    );
    assert!(
        !charter.contains("- [x]") && !charter.contains("- [X]"),
        "no DONE condition may be pre-checked: the audit is not certified complete, \
         which is exactly why complete-delivered-goal was rejected"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section C — root cause: mis-scoped / wrong-repository done-gate
// ════════════════════════════════════════════════════════════════════════════

/// The true root cause is a mis-scoped, unmeasurable done-gate: "raise to 70%"
/// was read as one workspace-wide figure aimed at a DIFFERENT repository
/// (`amplihack-rs`), uncertifiable from this checkout. The charter must fix the
/// scope explicitly so a future cycle is not pointed at the wrong workspace.
#[test]
fn charter_disambiguates_repository_scope_as_root_cause() {
    let charter = read_repo_file(CHARTER);
    let lower = charter.to_lowercase();

    assert!(
        lower.contains("amplihack-rs"),
        "the charter must name the wrong repository the goal was mis-scoped against"
    );
    assert!(
        lower.contains("rysweet/simard")
            || lower.contains("this repository")
            || lower.contains("this `simard` repo")
            || lower.contains("simard crate"),
        "the charter must scope the goal to THIS Simard repository"
    );
    assert!(
        lower.contains("in scope") && lower.contains("out of scope"),
        "the charter must carry an explicit in-scope / out-of-scope disambiguation (§1)"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section D — the goal's done-criteria are re-pointed at the charter
// ════════════════════════════════════════════════════════════════════════════

/// The `action_taken` re-points the goal family at the charter's machine-checkable
/// milestones. The charter must consolidate the recurring coverage-70 goal family
/// so a future resurfacing of THIS goal resolves to the charter rather than
/// re-opening another planning cycle.
#[test]
fn charter_consolidates_the_coverage_goal_family() {
    let charter = read_repo_file(CHARTER);
    let lower = charter.to_lowercase();

    assert!(
        lower.contains("consolidates goal slugs") || lower.contains("canonical written charter"),
        "the charter must declare itself the canonical consolidation point for the \
         recurring coverage goal family"
    );
    // The goal's own subject — audit coverage / raise to 70% — is named so a
    // resurfacing links here.
    assert!(
        lower.contains("70%") && lower.contains("coverage") && lower.contains("audit"),
        "the charter must name the coverage-audit-to-70% goal subject it re-points"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section E — scope guardrail: no workspace-wide CI hard gate (escalate = null)
// ════════════════════════════════════════════════════════════════════════════

/// The course-correction stays inside the charter's self-authorized scope
/// (§1–§3 are "actionable immediately … do not change any code or CI"), so
/// `escalate` is null. The rewrite must NOT introduce a workspace-wide hard
/// coverage gate in CI — the owner rejected that (PRs #2150/#2151), and §4
/// records it as explicitly out of scope.
#[test]
fn charter_forbids_workspace_wide_ci_hard_gate() {
    let charter = read_repo_file(CHARTER);
    let lower = charter.to_lowercase();

    assert!(
        lower.contains("not in this charter") || lower.contains("explicitly not"),
        "the charter must carry an explicit out-of-scope section (§4)"
    );
    assert!(
        lower.contains("workspace-wide") && lower.contains("ci"),
        "§4 must explicitly exclude a workspace-wide CI coverage gate"
    );
    assert!(
        charter.contains("#2150") && charter.contains("#2151"),
        "§4 must cite the owner's rejection of the bash CI gate (PRs #2150 / #2151)"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section F — the plain-English how-to records the decision, jargon-free
// ════════════════════════════════════════════════════════════════════════════

/// The how-to documents this exact triage: the decision is `rewrite-done-gate`,
/// `complete-delivered-goal` was considered and rejected, and no human was
/// escalated to (escalate = null / "nothing needed from you"). It also shows the
/// ratification and the re-point as the concrete actions taken.
#[test]
fn howto_records_the_rewrite_done_gate_decision_for_this_goal() {
    let howto = read_repo_file(HOWTO);
    let lower = howto.to_lowercase();

    assert!(
        lower.contains("rewrite-done-gate"),
        "the how-to must record the chosen decision: rewrite-done-gate"
    );
    assert!(
        lower.contains("complete-delivered-goal"),
        "the how-to must show complete-delivered-goal was considered (and rejected)"
    );
    // The concrete actions: ratify the charter and re-point the goal's criteria.
    assert!(
        lower.contains("ratif"),
        "the how-to must document ratifying the charter as the action taken"
    );
    assert!(
        lower.contains("re-point") || lower.contains("repoint") || lower.contains("point the goal"),
        "the how-to must document re-pointing the goal's done-criteria at the charter"
    );
    // No human escalation was required for this decision.
    assert!(
        lower.contains("no escalation")
            || lower.contains("nothing needed from you")
            || lower.contains("without escalat")
            || lower.contains("no human"),
        "the how-to must record that no human escalation was required (escalate = null)"
    );
}

/// The operator-visible OUTPUT example in the how-to (the fenced ```json blocks)
/// must be pure plain English — the whole point of the triage is to translate
/// the raw markers away before a person sees them. Marker tokens are allowed
/// elsewhere in the how-to only inside its "must not leak" guard checklist, so
/// this assertion is scoped to the operator-facing JSON example.
#[test]
fn howto_operator_output_is_free_of_raw_markers() {
    let howto = read_repo_file(HOWTO);
    let operator_json = howto_operator_json_blocks(&howto);
    assert!(
        !operator_json.trim().is_empty(),
        "the how-to must include an operator-visible OUTPUT json example to check"
    );
    assert_free_of_operator_jargon(&operator_json, "how-to operator OUTPUT json example");
    // The example must actually reference this goal and the chosen decision so
    // it documents THIS triage, not a generic one.
    assert!(
        operator_json.contains(GOAL_ID) || operator_json.contains("coverage"),
        "the operator OUTPUT example must be about the coverage goal being triaged"
    );
    assert!(
        operator_json.contains("rewrite-done-gate"),
        "the operator OUTPUT example must carry the chosen decision value"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section G — the recipe's OUTPUT contract the triage answered (6 keys)
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

    // `escalate` is nullable — the recipe must permit null for the case (this one)
    // where no human decision is genuinely required.
    assert!(
        lower.contains("null"),
        "the recipe OUTPUT must allow escalate = null when no human is required"
    );
}
