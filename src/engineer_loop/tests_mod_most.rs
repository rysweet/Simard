use super::execution::parse_status_paths;
use crate::PhaseOutcome;

#[test]
fn parse_status_paths_multiple_mixed_statuses() {
    let paths =
        parse_status_paths(" M modified.rs\nA  added.rs\n?? untracked.txt\n D deleted.rs\n");
    assert_eq!(paths.len(), 4);
    assert!(paths.contains(&"modified.rs".to_string()));
    assert!(paths.contains(&"added.rs".to_string()));
    assert!(paths.contains(&"untracked.txt".to_string()));
    assert!(paths.contains(&"deleted.rs".to_string()));
}

// ---- parse_status_paths: renamed files ----

#[test]
fn parse_status_paths_renamed_file() {
    let paths = parse_status_paths("R  old.rs -> new.rs\n");
    // Should produce at least one path
    assert!(!paths.is_empty());
}

// ---- architecture_gap_summary: probe with both keywords ----

#[test]
fn architecture_gap_summary_with_architecture_and_probe_and_contracts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Specs")).unwrap();
    std::fs::write(dir.path().join("Specs/ProductArchitecture.md"), "# Arch").unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("simard_operator_probe.rs"),
        r#"fn main() { "terminal-run" }"#,
    )
    .unwrap();
    let docs_dir = dir.path().join("docs/reference");
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::write(docs_dir.join("runtime-contracts.md"), "# Contracts").unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("terminal-run"));
    assert!(result.contains("runtime contracts docs mention"));
}

// ---- is_meeting_decision_record: near-miss cases ----

#[test]
fn is_meeting_decision_record_missing_decisions() {
    let value = "agenda=a updates=b risks=c next_steps=d open_questions=e goals=f";
    assert!(!super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_missing_risks() {
    let value = "agenda=a updates=b decisions=c next_steps=d open_questions=e goals=f";
    assert!(!super::is_meeting_decision_record(value));
}

// ---- PhaseTrace/PhaseOutcome coverage ----

#[test]
fn phase_outcome_success_debug() {
    let outcome = PhaseOutcome::Success;
    let debug = format!("{:?}", outcome);
    assert!(debug.contains("Success"));
}

#[test]
fn phase_outcome_failed_debug() {
    let outcome = PhaseOutcome::Failed("test error".to_string());
    let debug = format!("{:?}", outcome);
    assert!(debug.contains("test error"));
}

// ---- constants: additional validation ----

#[test]
fn cleared_git_env_vars_all_start_with_git() {
    for var in super::CLEARED_GIT_ENV_VARS {
        assert!(
            var.starts_with("GIT_"),
            "cleared env var should start with GIT_: {var}"
        );
    }
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn max_carried_meeting_decisions_is_positive() {
    const { assert!(super::MAX_CARRIED_MEETING_DECISIONS > 0) };
    const { assert!(super::MAX_CARRIED_MEETING_DECISIONS <= 10) };
}

// ---- is_meeting_decision_record ----

#[test]
fn is_meeting_decision_record_full_match() {
    let value = "agenda=sprint review updates=done decisions=ship risks=none next_steps=deploy open_questions=none goals=release";
    assert!(super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_missing_field_v2() {
    let value = "agenda=sprint updates=done decisions=ship risks=none";
    assert!(!super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_empty_v2() {
    assert!(!super::is_meeting_decision_record(""));
}

#[test]
fn is_meeting_decision_record_partial_fragments() {
    let value = "agenda= updates= decisions=";
    assert!(!super::is_meeting_decision_record(value));
}

// ---- constants: identity and base type ----

#[test]
fn engineer_identity_is_nonempty() {
    assert!(!super::ENGINEER_IDENTITY.is_empty());
}

#[test]
fn engineer_base_type_is_nonempty() {
    assert!(!super::ENGINEER_BASE_TYPE.is_empty());
}

#[test]
fn execution_scope_is_local_only() {
    assert_eq!(super::EXECUTION_SCOPE, "local-only");
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn cargo_timeout_exceeds_git_timeout() {
    const { assert!(super::CARGO_COMMAND_TIMEOUT_SECS >= super::GIT_COMMAND_TIMEOUT_SECS) };
}
