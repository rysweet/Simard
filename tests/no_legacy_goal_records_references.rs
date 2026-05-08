//! Failing TDD acceptance test (issue #1590, Step 7).
//!
//! Encodes the canonical acceptance criterion (#1) from the spec verbatim:
//!
//! > `rg -n 'goal_records\.json' src/` returns matches **only** in
//! > `src/goal_curation/operations.rs` and `src/bootstrap/config.rs`
//! > (incl. its tests). Zero matches elsewhere — including in tracing
//! > strings and comments.
//!
//! Plus the secondary criterion (#2):
//!
//! > `rg -n 'FileBackedGoalStore' src/operator_commands_meeting/
//! > src/engineer_loop/` returns no matches.
//!
//! These tests will fail until the migration described in spec sections 1-11
//! is complete. They are intentionally `rg`-shaped so that operators running
//! the same shell command get the same answer.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Run ripgrep with arbitrary args, return stdout lines (one per match).
/// Treats exit code 1 (no matches) as success; panics on other failures.
fn rg(args: &[&str]) -> Vec<String> {
    let output = Command::new("rg")
        .args(args)
        .output()
        .expect("ripgrep (rg) must be installed for migration acceptance tests");
    if !output.status.success() && output.status.code() != Some(1) {
        panic!(
            "rg {:?} failed: stderr={}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.trim().is_empty() {
        return vec![];
    }
    stdout.lines().map(|l| l.to_string()).collect()
}

#[test]
fn no_legacy_goal_records_json_references_outside_migration_files() {
    let src = repo_src_dir();
    // Spec acceptance criterion #1: only `goal_curation/operations.rs` and
    // `bootstrap/config.rs` (incl. its tests) may mention `goal_records.json`.
    // Migration-test scaffolding (which has to mention the legacy file in
    // `assert!(!exists())` style assertions) is excluded by glob — the
    // grep is meant to police production code, not the tests that prove the
    // migration is complete.
    let matches = rg(&[
        "-n",
        "-F",
        "goal_records.json",
        src.to_str().unwrap(),
        "-g",
        "!**/goal_curation/operations.rs",
        "-g",
        "!**/goal_curation/tests.rs",
        "-g",
        "!**/goal_curation/tests_operations.rs",
        "-g",
        "!**/goal_curation/tests_adapter.rs",
        "-g",
        "!**/memory_ipc/tests_launcher.rs",
        "-g",
        "!**/bootstrap/config.rs",
        "-g",
        "!**/bootstrap/tests_config.rs",
        "-g",
        "!**/tests_goal_records_migration.rs",
    ]);

    assert!(
        matches.is_empty(),
        "Migration incomplete: {} reference(s) to `goal_records.json` remain in production \
         code. Allowed files: goal_curation/operations.rs, bootstrap/config.rs (+tests).\n\
         Stragglers:\n{}",
        matches.len(),
        matches.join("\n")
    );
}

#[test]
fn no_file_backed_goal_store_references_in_meeting_or_engineer_paths() {
    let src = repo_src_dir();
    // Spec acceptance criterion #2 — meeting + engineer paths must use
    // `active_goals_as_records(&load_goal_board(...))` instead of the
    // `FileBackedGoalStore`. Migration test scaffolding is excluded.
    let mut all_matches: Vec<String> = Vec::new();
    for scope in [
        src.join("operator_commands_meeting"),
        src.join("engineer_loop"),
    ] {
        if !scope.exists() {
            continue;
        }
        all_matches.extend(rg(&[
            "-n",
            "-F",
            "FileBackedGoalStore",
            scope.to_str().unwrap(),
            "-g",
            "!**/tests_goal_records_migration.rs",
        ]));
    }

    assert!(
        all_matches.is_empty(),
        "Migration incomplete: {} `FileBackedGoalStore` reference(s) remain in meeting / \
         engineer code paths.\nMatches:\n{}",
        all_matches.len(),
        all_matches.join("\n")
    );
}

#[test]
fn dashboard_handlers_do_not_reference_goal_records_json() {
    // Spec acceptance criterion #6 — every dashboard handler must flow
    // through the cognitive-memory writer/reader bridge. The dashboard
    // module must contain no references to the legacy file in production
    // code (test scaffolding excluded).
    let src = repo_src_dir();
    let dashboard = src.join("operator_commands_dashboard");
    let matches = rg(&[
        "-n",
        "-F",
        "goal_records.json",
        dashboard.to_str().unwrap(),
        "-g",
        "!**/tests_goal_records_migration.rs",
    ]);

    assert!(
        matches.is_empty(),
        "Dashboard handlers still reference goal_records.json directly:\n{}",
        matches.join("\n")
    );
}

#[test]
fn validation_module_does_not_reference_goal_records_json() {
    // Spec ambiguity A1 + acceptance criterion #1 (which overrides the
    // "out of scope" hint in the task description): operator_commands/
    // validation.rs:45 currently refers to the legacy file too and must
    // be migrated. This guards against regression.
    let _ = Path::new(""); // silence unused-import warnings when nothing else uses Path
    let src = repo_src_dir();
    let validation = src.join("operator_commands");
    let matches = rg(&[
        "-n",
        "-F",
        "goal_records.json",
        validation.to_str().unwrap(),
        "-g",
        "!**/tests_goal_records_migration.rs",
    ]);

    assert!(
        matches.is_empty(),
        "operator_commands/validation.rs (or siblings) still reference goal_records.json:\n{}",
        matches.join("\n")
    );
}
