use super::execution::*;
use super::types::{EngineerActionKind, SelectedEngineerAction};
use super::{CARGO_COMMAND_TIMEOUT_SECS, GIT_COMMAND_TIMEOUT_SECS};
use crate::error::SimardError;
use std::path::Path;
use std::time::Duration;

#[test]
fn timeout_for_cargo_command() {
    let timeout = timeout_for_command(&["cargo", "test"]);
    assert_eq!(timeout, Duration::from_secs(CARGO_COMMAND_TIMEOUT_SECS));
}

#[test]
fn timeout_for_git_command() {
    let timeout = timeout_for_command(&["git", "status"]);
    assert_eq!(timeout, Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS));
}

#[test]
fn timeout_for_other_command() {
    let timeout = timeout_for_command(&["ls", "-la"]);
    assert_eq!(timeout, Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS));
}

#[test]
fn parse_status_paths_typical_output() {
    let stdout = " M src/main.rs\n M src/lib.rs\n";
    let paths = parse_status_paths(stdout);
    assert_eq!(paths, vec!["src/main.rs", "src/lib.rs"]);
}

#[test]
fn parse_status_paths_empty_input() {
    let paths = parse_status_paths("");
    assert!(paths.is_empty());
}

#[test]
fn parse_status_paths_short_line() {
    let paths = parse_status_paths("AB\n");
    assert_eq!(paths, vec!["AB"]);
}

#[test]
fn trimmed_stdout_non_empty() {
    let output = CommandOutput {
        status: std::process::Command::new("true").status().unwrap(),
        stdout: "  hello world  ".to_string(),
        stderr: String::new(),
    };
    assert_eq!(trimmed_stdout(&output).unwrap(), "hello world");
}

#[test]
fn trimmed_stdout_empty_errors() {
    let output = CommandOutput {
        status: std::process::Command::new("true").status().unwrap(),
        stdout: "   ".to_string(),
        stderr: String::new(),
    };
    assert!(trimmed_stdout(&output).is_err());
}

#[test]
fn trimmed_stdout_allow_empty_trims() {
    let output = CommandOutput {
        status: std::process::Command::new("true").status().unwrap(),
        stdout: "  text  ".to_string(),
        stderr: String::new(),
    };
    assert_eq!(trimmed_stdout_allow_empty(&output), "text");
}

#[test]
fn run_command_empty_argv_errors() {
    let result = run_command(Path::new("."), &[]);
    assert!(result.is_err());
}

#[test]
fn run_command_rejects_newlines_in_args() {
    let result = run_command(Path::new("."), &["echo", "hello\nworld"]);
    assert!(result.is_err());
}

#[test]
fn run_command_rejects_empty_segments() {
    let result = run_command(Path::new("."), &["echo", ""]);
    assert!(result.is_err());
}

fn assert_has_flag_with_value(argv: &[String], flag: &str) {
    let pos = argv.iter().position(|a| a == flag);
    assert!(pos.is_some(), "expected flag {flag} in argv: {argv:?}");
    let idx = pos.unwrap();
    assert!(
        idx + 1 < argv.len(),
        "flag {flag} at end of argv with no value: {argv:?}"
    );
    assert!(
        !argv[idx + 1].is_empty(),
        "value for {flag} is empty in argv: {argv:?}"
    );
}

#[test]
fn sanitize_issue_create_args_always_includes_title_and_body_empty_body() {
    let argv = sanitize_issue_create_args(
        "fix the thing",
        "",
        &[],
        Some("goal-42"),
        Some("engineer-1"),
    );
    assert_eq!(argv[0], "gh");
    assert_eq!(argv[1], "issue");
    assert_eq!(argv[2], "create");
    assert_has_flag_with_value(&argv, "--title");
    assert_has_flag_with_value(&argv, "--body");
    let body_idx = argv.iter().position(|a| a == "--body").unwrap();
    assert!(argv[body_idx + 1].contains("goal-42"));
    assert!(argv[body_idx + 1].contains("engineer-1"));
    assert!(argv[body_idx + 1].contains("~/.simard/agent_logs/"));
}

#[test]
fn sanitize_issue_create_args_whitespace_body_uses_placeholder() {
    let argv = sanitize_issue_create_args("title", "   \n\t  ", &[], None, None);
    assert_has_flag_with_value(&argv, "--body");
    let body_idx = argv.iter().position(|a| a == "--body").unwrap();
    assert!(argv[body_idx + 1].contains("unknown"));
    assert!(argv[body_idx + 1].starts_with("_(spawned by OODA daemon"));
}

#[test]
fn sanitize_issue_create_args_multiline_title_collapsed() {
    let argv = sanitize_issue_create_args(
        "line one\nline two\rline three",
        "body content",
        &[],
        None,
        None,
    );
    assert_has_flag_with_value(&argv, "--title");
    let title_idx = argv.iter().position(|a| a == "--title").unwrap();
    let title_val = &argv[title_idx + 1];
    assert!(!title_val.contains('\n'));
    assert!(!title_val.contains('\r'));
    assert!(title_val.contains("line one"));
    assert!(title_val.contains("line two"));
    assert!(title_val.contains("line three"));
    assert_has_flag_with_value(&argv, "--body");
}

#[test]
fn sanitize_issue_create_args_special_chars_preserved() {
    let title = "fix: $weird `chars` & \"quotes\" <html> 中文";
    let body = "body with !@#$%^&*()_+-={}[]|\\:;'<>,.?/ symbols";
    let argv = sanitize_issue_create_args(title, body, &[], None, None);
    let title_idx = argv.iter().position(|a| a == "--title").unwrap();
    let body_idx = argv.iter().position(|a| a == "--body").unwrap();
    assert_eq!(&argv[title_idx + 1], title);
    assert_eq!(&argv[body_idx + 1], body);
    assert_has_flag_with_value(&argv, "--title");
    assert_has_flag_with_value(&argv, "--body");
}

#[test]
fn sanitize_issue_create_args_with_labels_preserves_order() {
    let argv =
        sanitize_issue_create_args("t", "b", &["bug".to_string(), "p1".to_string()], None, None);
    assert_has_flag_with_value(&argv, "--title");
    assert_has_flag_with_value(&argv, "--body");
    let label_positions: Vec<usize> = argv
        .iter()
        .enumerate()
        .filter_map(|(i, a)| if a == "--label" { Some(i) } else { None })
        .collect();
    assert_eq!(label_positions.len(), 2);
    assert_eq!(argv[label_positions[0] + 1], "bug");
    assert_eq!(argv[label_positions[1] + 1], "p1");
}

#[test]
fn sanitize_issue_create_args_empty_title_substituted() {
    let argv = sanitize_issue_create_args("   ", "body", &[], None, None);
    assert_has_flag_with_value(&argv, "--title");
    let title_idx = argv.iter().position(|a| a == "--title").unwrap();
    assert!(!argv[title_idx + 1].is_empty());
}

#[test]
fn sanitize_issue_create_args_parses_under_gh_help() {
    // Integration test: confirm gh CLI accepts the sanitized argv
    // shape. We append --help so gh prints help and exits 0 without
    // actually creating an issue. Skipped gracefully if gh is missing.
    use std::process::Command;
    let gh_available = Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !gh_available {
        eprintln!("skipping: gh CLI not available in test environment");
        return;
    }
    let argv = sanitize_issue_create_args(
        "test title",
        "",
        &["bug".to_string()],
        Some("goal-1"),
        Some("agent-1"),
    );
    // argv[0] is "gh"; pass the rest plus --help
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.arg("--help");
    let output = cmd.output().expect("failed to invoke gh");
    assert!(
        output.status.success(),
        "gh failed to parse sanitized argv {:?}: stderr={}",
        argv,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sanitize_issue_create_args_passes_run_command_validator() {
    // Validator rejects empty or multi-line argv segments. Sanitized
    // output must satisfy it for any input.
    let cases: Vec<(&str, &str)> = vec![
        ("", ""),
        ("multi\nline\ntitle", ""),
        ("t", "multi\nline\nbody"),
        ("\r\n", "\n\r"),
        ("normal title", "normal body"),
    ];
    for (title, body) in cases {
        let argv = sanitize_issue_create_args(title, body, &[], Some("g"), Some("a"));
        for seg in &argv {
            assert!(
                !seg.is_empty(),
                "empty argv segment for inputs ({title:?}, {body:?}): {argv:?}"
            );
            assert!(
                !seg.contains('\n') && !seg.contains('\r'),
                "multi-line argv segment for inputs ({title:?}, {body:?}): {argv:?}"
            );
        }
        assert!(argv.iter().any(|a| a == "--title"));
        assert!(argv.iter().any(|a| a == "--body"));
    }
}
