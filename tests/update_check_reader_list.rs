//! TDD contract (Step 7) for Problem 2 — keep the `SIMARD_NO_UPDATE_CHECK`
//! reader list consistent between docs and code (issue #1055).
//!
//! PR #1055 completed the "reader list" for the update-check opt-out env var,
//! and its `Test` check regressed. The durable fix is a regression guard that
//! makes docs↔code drift a *test* failure instead of a silent doc lie: the
//! how-to/reference doc claims exactly two wired entry points read the flag
//! through a single shared guard, and those must match the actual source.
//!
//! Design invariant ("central guard"): `SIMARD_NO_UPDATE_CHECK` is read in
//! exactly one module, `src/update_check.rs`, and the two launch paths honor it
//! only by calling `run_update_check()` / `run_update_check_background()`. The
//! entry-point binaries (`src/main.rs`, `src/bin/simard_tui/main.rs`) must NOT
//! read the env var independently — a scattered read would diverge from the
//! documented reader list and could regress the opt-out contract.
//!
//! These are file-shaped, no-network assertions. They FAIL if the doc's reader
//! list drifts from the code or the env-var guard leaks out of the central
//! module.

use std::fs;
use std::path::{Path, PathBuf};

const ENV_VAR: &str = "SIMARD_NO_UPDATE_CHECK";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

#[test]
fn cli_entry_point_calls_run_update_check() {
    let src = read(&repo_root().join("src/main.rs"));
    assert!(
        src.contains("run_update_check()"),
        "src/main.rs (the `simard` CLI entry point) must call \
         `run_update_check()` — this is one of the two documented reader-list \
         rows (issue #1055)."
    );
}

#[test]
fn tui_entry_point_calls_run_update_check_background() {
    let src = read(&repo_root().join("src/bin/simard_tui/main.rs"));
    assert!(
        src.contains("run_update_check_background()"),
        "src/bin/simard_tui/main.rs (the `simard-tui` entry point) must call \
         `run_update_check_background()` — the second documented reader-list \
         row (issue #1055)."
    );
}

#[test]
fn env_guard_is_read_only_in_the_central_module() {
    // The opt-out flag must be consulted in exactly one place. Any additional
    // reader (in an entry point or elsewhere) diverges from the documented
    // reader list and risks a regressed opt-out contract.
    let src_dir = repo_root().join("src");
    let mut readers: Vec<String> = Vec::new();
    collect_env_readers(&src_dir, &src_dir, &mut readers);
    readers.sort();
    assert_eq!(
        readers,
        vec!["update_check.rs".to_string()],
        "`{ENV_VAR}` must be read only in src/update_check.rs (the central \
         guard). Files reading it: {readers:?}. Route launch paths through \
         run_update_check[_background]() instead of adding new readers (issue \
         #1055)."
    );
}

/// Walk `dir` and record every `.rs` file (path relative to `root`) that reads
/// `SIMARD_NO_UPDATE_CHECK` via `std::env::var*` / `env::var*`. Test files and
/// the reference doc are excluded — this checks production readers only.
fn collect_env_readers(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_env_readers(root, &path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let body = read(&path);
        let reads_env = body.lines().any(|line| {
            line.contains(ENV_VAR) && (line.contains("env::var") || line.contains("std::env::var"))
        });
        if reads_env {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
}

#[test]
fn reference_doc_lists_both_wired_readers() {
    // The reference doc's reader-list table must name both entry-point files
    // and their functions, keeping the documented list aligned with the code.
    let doc = read(&repo_root().join("docs/reference/update-check.md"));
    for needle in [
        "src/main.rs",
        "src/bin/simard_tui/main.rs",
        "run_update_check()",
        "run_update_check_background()",
        ENV_VAR,
    ] {
        assert!(
            doc.contains(needle),
            "docs/reference/update-check.md must mention `{needle}` so its \
             reader list stays consistent with the code (issue #1055)."
        );
    }
}

#[test]
fn reference_doc_describes_the_central_shared_guard() {
    // The doc must state that both paths honor the flag through the single
    // shared guard in src/update_check.rs (not a per-binary main() check).
    let doc = read(&repo_root().join("docs/reference/update-check.md"));
    assert!(
        doc.contains("src/update_check.rs"),
        "docs/reference/update-check.md must attribute the shared \
         `{ENV_VAR}` guard to src/update_check.rs (central-guard framing, \
         issue #1055)."
    );
}
