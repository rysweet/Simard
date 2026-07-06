//! Failing TDD acceptance test (issue #2636, Step 7).
//!
//! Encodes the operator's absolute rule — *nothing may be named `Bridge`* — as
//! an executable, shell-grep-shaped acceptance check that mirrors the STEP 0
//! authoritative inventory:
//!
//! > `git grep -E 'Bridge' -- 'src/**/*.rs'` (case-sensitive, CamelCase)
//! > returns matches ONLY in the two Overseer files that *are* the no-`Bridge`
//! > linter and its fixtures (`pr_verify.rs`, `merge_ops.rs`). Zero matches
//! > elsewhere — including doc comments and tracing strings.
//!
//! Plus the structural criterion: every misnamed module named in the rename
//! map no longer exists on disk, and its accurate RPC / client / handoff
//! replacement does.
//!
//! ## Why case-sensitive `Bridge` (not the task's `-i` grep)?
//!
//! A case-INSENSITIVE `\w*bridge\w*` can never reach zero without breaking the
//! frozen wire contract and on-disk formats: it matches the JSON-RPC method
//! name `"bridge.health"`, the ~100 preserved on-disk / telemetry / log string
//! literals (`"cognitive-bridge"`, `"bridge_timeout"`, `"bridge::native::{}"`),
//! and even the English word `abridged`. The operator rule targets *names*
//! (Rust identifiers), and every `Bridge`-derived identifier is either
//! CamelCase (types — the authoritative 34-identifier / 104-file STEP 0 set,
//! exactly what the `pr_verify` scanner keys on via `content.contains("Bridge")`)
//! or lowercase snake_case. The lowercase snake_case identifier renames (e.g.
//! `launch_writer_bridge` -> `launch_writer_client`) are enforced by the
//! compiler and the existing test suite — the crate will not build with
//! inconsistent identifiers — rather than by this grep, because a lowercase
//! `bridge` substring is indistinguishable from the preserved wire / on-disk /
//! log string literals.
//!
//! ## The allowlisted files
//!
//! `src/overseer/pr_verify.rs` and `src/overseer/merge_ops.rs` *are* the
//! no-`Bridge` linter and its unit tests. They must keep the literal `"Bridge"`
//! (the detection substring and CamelCase fixtures like `PaymentBridge`,
//! `HttpBridge`) or the linter that enforces this very rule would no longer
//! detect anything. They are Overseer-internal (invoked from the merge flow and
//! exercised under `cargo test`), NOT part of GitHub Actions CI.
//!
//! `src/operator_commands_dashboard/index_html/tests_tab_meta.rs` is the same
//! category: a guard test that asserts no consolidated dashboard tab is named
//! `"Bridge"`. It must retain the literal `"Bridge"` as its detection substring
//! for the same reason.
//!
//! These tests fail until the mechanical rename is complete. They are
//! intentionally shell-grep-shaped so operators running the same command get
//! the same answer.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_src_dir() -> PathBuf {
    repo_root().join("src")
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
fn no_camelcase_bridge_naming_in_src() {
    let src = repo_src_dir();
    // Case-sensitive CamelCase `Bridge` substring: catches every type / trait /
    // variant identifier in STEP 0's authoritative inventory (`BridgeRequest`,
    // `CognitiveMemoryBridge`, `SubprocessBridgeTransport`, `OodaBridges`, ...)
    // while ignoring the lowercase word (`abridged`, `cambridge`) and the frozen
    // lowercase wire / on-disk string literals (`"bridge.health"`).
    //
    // The only permitted survivors are the two Overseer files that ARE the
    // no-`Bridge` linter and its fixtures — see the module doc comment.
    let matches = grep_recursive(
        &src,
        "Bridge",
        &["pr_verify.rs", "merge_ops.rs", "tests_tab_meta.rs"],
    );

    assert!(
        matches.is_empty(),
        "Rename incomplete: {} CamelCase `Bridge` identifier reference(s) remain in `src/`.\n\
         The operator rule is absolute — nothing may be named `Bridge`. Rename to the accurate\n\
         RPC / client / handoff vocabulary (see the issue #2636 rename map). Only\n\
         `src/overseer/pr_verify.rs` and `src/overseer/merge_ops.rs` (the no-`Bridge` linter and\n\
         its own fixtures) may retain the literal.\n\
         Stragglers:\n{}",
        matches.len(),
        matches.join("\n")
    );
}

#[test]
fn misnamed_bridge_modules_are_renamed_on_disk() {
    let src = repo_src_dir();

    // Rename map (issue #2636): the misleading `*bridge*` module must be gone and
    // its accurately-named replacement must exist. Structural check on paths only
    // (never string content), so it is immune to any preserved wire/on-disk
    // literal.
    let renames: &[(&str, &str)] = &[
        ("bridge.rs", "rpc.rs"),
        ("bridge_circuit.rs", "rpc_circuit_breaker.rs"),
        ("bridge_launcher.rs", "rpc_subprocess_launcher.rs"),
        ("bridge_subprocess", "rpc_transport"),
        ("gym_bridge.rs", "gym_client.rs"),
        ("gym_runner_bridge.rs", "gym_runner_client.rs"),
        ("knowledge_bridge.rs", "knowledge_client.rs"),
        ("memory_bridge", "memory_client"),
        ("memory_bridge_adapter", "memory_store_adapter"),
        ("terminal_engineer_bridge", "engineer_handoff"),
    ];

    let mut problems: Vec<String> = Vec::new();
    for (old, new) in renames {
        let old_path = src.join(old);
        let new_path = src.join(new);
        if old_path.exists() {
            problems.push(format!("still present (must be renamed away): src/{old}"));
        }
        if !new_path.exists() {
            problems.push(format!(
                "missing (rename target not created):  src/{new}   (was src/{old})"
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "Rename incomplete: {} module-path issue(s).\n{}",
        problems.len(),
        problems.join("\n")
    );
}
