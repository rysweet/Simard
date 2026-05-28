//! Contract tests for prompt-driven TDD discipline (issue #1927).
//!
//! TDD commit-ordering enforcement belongs in the engineer system prompt —
//! not in CI gate scripts that parse `git log` output. These tests verify:
//!
//! 1. The engineer system prompt contains the TDD commit-ordering instruction.
//! 2. No bash CI scripts exist that attempt to enforce TDD via git history parsing.
//! 3. No CI workflow files reference TDD ordering checks.
//! 4. The TDD instruction lives in the Quality Standards section (not bolted
//!    on at the end or buried in an unrelated section).

use std::path::Path;

const ENGINEER_SYSTEM_PROMPT: &str = include_str!("../prompt_assets/simard/engineer_system.md");

// ---------------------------------------------------------------------------
// Core contract: the TDD instruction exists in the prompt
// ---------------------------------------------------------------------------

#[test]
fn engineer_prompt_contains_tdd_commit_ordering_instruction() {
    assert!(
        ENGINEER_SYSTEM_PROMPT.contains("Test-Driven Development"),
        "engineer_system.md must contain a Test-Driven Development instruction — \
         TDD discipline is enforced through the prompt, not CI scripts"
    );
}

#[test]
fn tdd_instruction_requires_tests_before_implementation() {
    assert!(
        ENGINEER_SYSTEM_PROMPT.contains("write tests before implementation code")
            || ENGINEER_SYSTEM_PROMPT.contains("Always write tests before implementation"),
        "TDD instruction must explicitly require writing tests before implementation code"
    );
}

#[test]
fn tdd_instruction_requires_test_commit_before_implementation_commit() {
    assert!(
        ENGINEER_SYSTEM_PROMPT.contains("test commit must come before the implementation commit"),
        "TDD instruction must require test commits to precede implementation commits"
    );
}

#[test]
fn tdd_instruction_rejects_ci_script_enforcement() {
    assert!(
        ENGINEER_SYSTEM_PROMPT.contains("not through CI scripts")
            || ENGINEER_SYSTEM_PROMPT.contains("not through bash")
            || ENGINEER_SYSTEM_PROMPT.contains("not through CI scripts or git history parsing"),
        "TDD instruction must explicitly state that enforcement is prompt-based, not script-based"
    );
}

// ---------------------------------------------------------------------------
// Quality Standards placement: TDD instruction is in the right section
// ---------------------------------------------------------------------------

#[test]
fn tdd_instruction_is_in_quality_standards_section() {
    let quality_standards_start = ENGINEER_SYSTEM_PROMPT
        .find("## Quality Standards")
        .expect("engineer_system.md must have a '## Quality Standards' section");

    let tdd_start = ENGINEER_SYSTEM_PROMPT
        .find("Test-Driven Development")
        .expect("engineer_system.md must contain 'Test-Driven Development'");

    // Find the next H2 section after Quality Standards
    let after_quality = &ENGINEER_SYSTEM_PROMPT[quality_standards_start + 1..];
    let next_section = after_quality
        .find("\n## ")
        .map(|offset| quality_standards_start + 1 + offset)
        .unwrap_or(ENGINEER_SYSTEM_PROMPT.len());

    assert!(
        tdd_start > quality_standards_start && tdd_start < next_section,
        "TDD instruction must live inside the '## Quality Standards' section, \
         not bolted on at the end or in an unrelated section. \
         Quality Standards at byte {quality_standards_start}, \
         TDD at byte {tdd_start}, \
         next section at byte {next_section}"
    );
}

// ---------------------------------------------------------------------------
// Negative contracts: no bash CI gate scripts for TDD
// ---------------------------------------------------------------------------

#[test]
fn no_tdd_ordering_bash_script_exists() {
    let script_path = Path::new("scripts/check-tdd-ordering.sh");
    assert!(
        !script_path.exists(),
        "scripts/check-tdd-ordering.sh must not exist — \
         TDD enforcement is prompt-driven, not script-driven"
    );
}

#[test]
fn no_tdd_ci_workflow_exists() {
    let workflows_dir = Path::new(".github/workflows");
    if workflows_dir.exists() {
        for entry in std::fs::read_dir(workflows_dir).expect("should read workflows dir") {
            let entry = entry.expect("should read entry");
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_lowercase();
            assert!(
                !name_str.contains("tdd-ordering") && !name_str.contains("tdd_ordering"),
                "CI workflow '{}' must not exist — \
                 TDD enforcement is prompt-driven, not CI-gate-driven",
                name.to_string_lossy()
            );
        }
    }
}

#[test]
fn no_ci_workflow_references_tdd_ordering_script() {
    let workflows_dir = Path::new(".github/workflows");
    if !workflows_dir.exists() {
        return;
    }
    for entry in std::fs::read_dir(workflows_dir).expect("should read workflows dir") {
        let entry = entry.expect("should read entry");
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext == "yml" || ext == "yaml")
        {
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("should read {}", path.display()));
            assert!(
                !contents.contains("check-tdd-ordering"),
                "CI workflow '{}' must not reference check-tdd-ordering — \
                 TDD enforcement is prompt-driven",
                path.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt content quality: TDD instruction is actionable
// ---------------------------------------------------------------------------

#[test]
fn tdd_instruction_describes_the_workflow_steps() {
    // The instruction should describe the concrete steps, not just say "do TDD"
    assert!(
        ENGINEER_SYSTEM_PROMPT.contains("write a failing test")
            || ENGINEER_SYSTEM_PROMPT.contains("failing test that defines"),
        "TDD instruction must describe the concrete workflow: write a failing test first"
    );
    assert!(
        ENGINEER_SYSTEM_PROMPT.contains("commit the test"),
        "TDD instruction must mention committing the test separately"
    );
    assert!(
        ENGINEER_SYSTEM_PROMPT.contains("write the implementation")
            || ENGINEER_SYSTEM_PROMPT.contains("implementation that makes the test pass"),
        "TDD instruction must describe writing implementation after the test"
    );
}
