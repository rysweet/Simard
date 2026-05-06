use super::execution::parse_status_paths;

#[test]
fn is_meeting_decision_record_positive() {
    let value = "agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win";
    assert!(super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_missing_field() {
    // Missing "goals="
    let value =
        "agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none";
    assert!(!super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_empty_string() {
    assert!(!super::is_meeting_decision_record(""));
}

#[test]
fn is_meeting_decision_record_partial_match() {
    let value = "agenda=stuff decisions=yes";
    assert!(!super::is_meeting_decision_record(value));
}

// ---- constants tests ----

#[test]
fn engineer_identity_constant() {
    assert_eq!(super::ENGINEER_IDENTITY, "simard-engineer");
}

#[test]
fn engineer_base_type_constant() {
    assert_eq!(super::ENGINEER_BASE_TYPE, "terminal-shell");
}

#[test]
fn execution_scope_constant() {
    assert_eq!(super::EXECUTION_SCOPE, "local-only");
}

#[test]
fn max_carried_meeting_decisions_is_reasonable() {
    let m = super::MAX_CARRIED_MEETING_DECISIONS;
    assert!(m > 0, "must be positive, got {m}");
    assert!(m <= 10, "must be <= 10, got {m}");
}

#[test]
fn cleared_git_env_vars_is_nonempty() {
    assert!(!super::CLEARED_GIT_ENV_VARS.is_empty());
    assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_DIR"));
    assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_WORK_TREE"));
    assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_INDEX_FILE"));
}

#[test]
fn git_command_timeout_is_reasonable() {
    let t = super::GIT_COMMAND_TIMEOUT_SECS;
    assert!(t >= 10, "git timeout too low: {t}");
    assert!(t <= 300, "git timeout too high: {t}");
}

#[test]
fn cargo_command_timeout_is_reasonable() {
    let t = super::CARGO_COMMAND_TIMEOUT_SECS;
    assert!(t >= 30, "cargo timeout too low: {t}");
    assert!(t <= 600, "cargo timeout too high: {t}");
}

// ---- architecture_gap_summary tests ----

#[test]
fn architecture_gap_summary_no_architecture_file() {
    let dir = tempfile::tempdir().unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("missing Specs/ProductArchitecture.md"));
}

#[test]
fn architecture_gap_summary_with_architecture_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Specs")).unwrap();
    std::fs::write(
        dir.path().join("Specs/ProductArchitecture.md"),
        "# Architecture",
    )
    .unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("Specs/ProductArchitecture.md"));
    assert!(result.contains("engineer mode"));
}

#[test]
fn architecture_gap_summary_with_probe_engineer_loop_run() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("simard_operator_probe.rs"),
        r#"fn main() { "engineer-loop-run" }"#,
    )
    .unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("engineer-loop-run"));
}

#[test]
fn architecture_gap_summary_with_probe_terminal_run() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("simard_operator_probe.rs"),
        r#"fn main() { "terminal-run" }"#,
    )
    .unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("terminal-run"));
}

#[test]
fn architecture_gap_summary_with_probe_no_match() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(bin_dir.join("simard_operator_probe.rs"), "fn main() {}").unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("does not yet expose"));
}

#[test]
fn architecture_gap_summary_with_runtime_contracts_docs() {
    let dir = tempfile::tempdir().unwrap();
    let docs_dir = dir.path().join("docs/reference");
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::write(docs_dir.join("runtime-contracts.md"), "# Contracts").unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("runtime contracts docs mention"));
}

#[test]
fn architecture_gap_summary_without_runtime_contracts_docs() {
    let dir = tempfile::tempdir().unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("runtime contracts docs are absent"));
}

#[test]
fn architecture_gap_summary_all_files_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Specs")).unwrap();
    std::fs::write(dir.path().join("Specs/ProductArchitecture.md"), "# Arch").unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("simard_operator_probe.rs"),
        r#"fn main() { "engineer-loop-run" }"#,
    )
    .unwrap();
    let docs_dir = dir.path().join("docs/reference");
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::write(docs_dir.join("runtime-contracts.md"), "# Contracts").unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("Specs/ProductArchitecture.md"));
    assert!(result.contains("engineer-loop-run"));
    assert!(result.contains("runtime contracts docs mention"));
}

// ---- is_meeting_decision_record additional tests ----

#[test]
fn is_meeting_decision_record_fields_in_different_order() {
    let value = "goals=win open_questions=none next_steps=go risks=low decisions=yes updates=things agenda=stuff";
    assert!(super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_with_extra_content() {
    let value = "prefix agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win suffix";
    assert!(super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_missing_agenda() {
    let value =
        "updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win";
    assert!(!super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_only_agenda() {
    assert!(!super::is_meeting_decision_record("agenda=stuff"));
}

// ---- parse_status_paths additional tests ----

#[test]
fn parse_status_paths_empty_input() {
    let paths = parse_status_paths("");
    assert!(paths.is_empty());
}

#[test]
fn parse_status_paths_whitespace_only() {
    let paths = parse_status_paths("   \n  \n");
    assert!(paths.is_empty());
}

#[test]
fn parse_status_paths_single_modification() {
    let paths = parse_status_paths(" M src/main.rs\n");
    assert_eq!(paths, vec!["src/main.rs"]);
}

#[test]
fn parse_status_paths_untracked_files() {
    let paths = parse_status_paths("?? new_file.txt\n");
    assert_eq!(paths, vec!["new_file.txt"]);
}

#[test]
fn parse_status_paths_added_file() {
    let paths = parse_status_paths("A  added.rs\n");
    assert_eq!(paths, vec!["added.rs"]);
}

#[test]
fn parse_status_paths_deleted_file() {
    let paths = parse_status_paths(" D removed.rs\n");
    assert_eq!(paths, vec!["removed.rs"]);
}
