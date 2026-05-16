//! Golden / snapshot tests for the `simard` CLI surface.
//!
//! These tests run the real binary and assert on key invariants of its output
//! so that accidental regressions in help text, version strings, or error
//! messages are caught early.

use assert_cmd::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── helpers ──────────────────────────────────────────────────────────────

fn simard() -> Command {
    Command::cargo_bin("simard").expect("simard binary must be buildable")
}

/// Subcommands that MUST appear in the top-level help text.
const EXPECTED_SUBCOMMANDS: &[&str] = &[
    "engineer",
    "meeting",
    "goal-curation",
    "improvement-curation",
    "gym",
    "ooda",
    "spawn",
    "handover",
    "update",
    "self-test",
    "install",
    "review",
    "bootstrap",
    "act-on-decisions",
];

// ── tests ────────────────────────────────────────────────────────────────

#[test]
fn help_flag_succeeds_and_contains_subcommands() {
    let assert = simard().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    for sub in EXPECTED_SUBCOMMANDS {
        assert!(
            stdout.contains(sub),
            "help text missing subcommand '{sub}':\n{stdout}"
        );
    }
}

#[test]
fn help_text_mentions_simard() {
    let assert = simard().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("Simard") || stdout.contains("simard"),
        "help text should mention 'simard':\n{stdout}"
    );
}

#[test]
fn no_args_prints_help() {
    let with_help = simard().arg("--help").output().expect("--help");
    let no_args = simard().output().expect("no args");

    assert_eq!(
        with_help.stdout, no_args.stdout,
        "running with no args should produce the same output as --help"
    );
}

#[test]
fn short_help_flag_works() {
    let long_help = simard().arg("--help").output().expect("--help");
    let short_help = simard().arg("-h").output().expect("-h");

    assert_eq!(
        long_help.stdout, short_help.stdout,
        "-h and --help should produce identical output"
    );
}

#[test]
fn invalid_subcommand_fails_with_message() {
    let assert = simard().arg("nonsense-cmd").assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);

    assert!(
        stderr.contains("unsupported command") && stderr.contains("nonsense-cmd"),
        "expected 'unsupported command' error for invalid subcommand, got:\n{stderr}"
    );
}

#[test]
fn version_string_is_semver() {
    // The binary has no --version flag; verify the Cargo-embedded version
    // matches the expected semver pattern so the constant stays in sync.
    let parts: Vec<&str> = VERSION.split('.').collect();
    assert_eq!(parts.len(), 3, "version should be semver: {VERSION}");
    for part in &parts {
        part.parse::<u32>()
            .unwrap_or_else(|_| panic!("non-numeric version component '{part}' in {VERSION}"));
    }
    assert_eq!(
        VERSION, "0.17.0",
        "bump this assertion when version changes"
    );
}

#[test]
fn help_text_is_stable_across_calls() {
    let first = simard().arg("--help").output().expect("first --help");
    let second = simard().arg("--help").output().expect("second --help");

    assert_eq!(
        first.stdout, second.stdout,
        "help text should be deterministic across invocations"
    );
}

#[test]
fn help_documents_compatibility_binaries() {
    let assert = simard().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("simard_operator_probe") && stdout.contains("simard-gym"),
        "help text should mention compatibility binaries:\n{stdout}"
    );
}

// ── issue #1746: `simard meeting --help` regression suite ────────────────
//
// Before the fix, `simard meeting --help` (and `-h`) was silently treated as
// the meeting topic name `--help` and entered an interactive REPL that
// blocked on stdin forever — making the binary appear hung. These tests
// invoke the real `simard` binary with closed stdin and a hard timeout so
// any regression that re-introduces the hang fails fast and visibly.

fn meeting_help_should_not_hang(flag: &str) {
    let assert = simard()
        .arg("meeting")
        .arg(flag)
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    for keyword in &["meeting", "run", "read", "repl", "Usage"] {
        assert!(
            stdout.contains(keyword),
            "`simard meeting {flag}` stdout should mention '{keyword}':\n{stdout}"
        );
    }
}

#[test]
fn meeting_double_dash_help_succeeds_quickly() {
    meeting_help_should_not_hang("--help");
}

#[test]
fn meeting_short_dash_help_succeeds_quickly() {
    meeting_help_should_not_hang("-h");
}

#[test]
fn meeting_help_word_succeeds_quickly() {
    meeting_help_should_not_hang("help");
}

#[test]
fn meeting_unknown_flag_errors_quickly() {
    let assert = simard()
        .arg("meeting")
        .arg("--definitely-not-a-real-flag")
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);

    assert!(
        stderr.contains("unknown flag") && stderr.contains("--definitely-not-a-real-flag"),
        "unknown meeting flag must produce a visible stderr error, got: {stderr}"
    );
}
