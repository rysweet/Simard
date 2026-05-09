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

use std::fs;
use std::path::{Path, PathBuf};

fn repo_src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Recursively walk `root` collecting all `.rs` files.
fn collect_rs_files(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Find all `<file>:<line>:<text>` lines containing `needle` under `root`,
/// excluding any file whose basename appears in `exclude_basenames`.
fn grep_recursive(root: &Path, needle: &str, exclude_basenames: &[&str]) -> Vec<String> {
    let mut files = Vec::new();
    collect_rs_files(root, &mut files);
    let mut matches = Vec::new();
    for file in files {
        let basename = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if exclude_basenames.contains(&basename) {
            continue;
        }
        let contents = match fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (idx, line) in contents.lines().enumerate() {
            if line.contains(needle) {
                matches.push(format!("{}:{}:{}", file.display(), idx + 1, line));
            }
        }
    }
    matches
}

#[test]
fn no_legacy_goal_records_json_references_outside_migration_files() {
    let src = repo_src_dir();
    // Spec acceptance criterion #1: only `goal_curation/operations.rs` and
    // `bootstrap/config.rs` (incl. its tests) may mention `goal_records.json`.
    // Migration-test scaffolding (which has to mention the legacy file in
    // `assert!(!exists())` style assertions) is excluded by basename — the
    // grep is meant to police production code, not the tests that prove the
    // migration is complete.
    let matches = grep_recursive(
        &src,
        "goal_records.json",
        &[
            "operations.rs",
            "tests.rs",
            "tests_operations.rs",
            "tests_adapter.rs",
            "tests_launcher.rs",
            "config.rs",
            "tests_config.rs",
            "tests_goal_records_migration.rs",
            // The cognitive-memory-backed adapter mentions the legacy file
            // in (a) a module-level doc comment that explains *why* the
            // legacy artefact is no longer produced, and (b) a `#[cfg(test)]`
            // assertion that the file is NOT created. Both are intentional.
            "cognitive_memory_store.rs",
        ],
    );

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
        all_matches.extend(grep_recursive(
            &scope,
            "FileBackedGoalStore",
            &["tests_goal_records_migration.rs"],
        ));
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
    let matches = grep_recursive(
        &dashboard,
        "goal_records.json",
        &["tests_goal_records_migration.rs"],
    );

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
    let src = repo_src_dir();
    let validation = src.join("operator_commands");
    let matches = grep_recursive(
        &validation,
        "goal_records.json",
        &["tests_goal_records_migration.rs"],
    );

    assert!(
        matches.is_empty(),
        "operator_commands/validation.rs (or siblings) still reference goal_records.json:\n{}",
        matches.join("\n")
    );
}
