//! Executable acceptance guard for the operator's absolute rule (issue #2951):
//! *nothing may be named "bridge"*. "Bridge" conveys no meaning; every use must
//! be renamed to an intent-revealing term from the RPC / client / reader /
//! source / transport / handoff vocabulary.
//!
//! The guard enforces the rule at TWO levels, both shaped like the operator's
//! own `git grep` so a human running the same command gets the same answer:
//!
//!  1. `no_camelcase_bridge_naming_in_src` — case-sensitive CamelCase `Bridge`
//!     substring: every type / trait / variant identifier (`BridgeRequest`,
//!     `CognitiveMemoryBridge`, `OodaBridges`, ...).
//!
//!  2. `no_lowercase_bridge_word_in_src` — the *strengthened* check this issue
//!     adds: the lowercase stem `bridge` as a standalone COMPONENT — in string
//!     literals (operator log lines, telemetry identities), comments/docs,
//!     snake_case identifiers (`launch_enrichment_bridges`, a local `bridge`
//!     binding) and module-internal names. This is what makes an operator log
//!     line like `bridge 'memory-ipc' transport error: ...` impossible to
//!     reintroduce.
//!
//!  3. `misnamed_bridge_modules_are_renamed_on_disk` — structural: every
//!     misnamed module is gone and its accurate replacement exists.
//!
//! ## Component-boundary matching (why not a naive `-i` substring)
//!
//! A match is the case-insensitive stem `bridge` whose immediately-preceding
//! character is NOT an ASCII letter. This flags real components — `bridge`,
//! `bridges`, `_bridge`, `bridge_name`, `"bridge '{}'..."` — while never
//! flagging the stem when it is buried inside an unrelated English / proper
//! word: `abridged`, `Cambridge`, `Bainbridge` (a real citation in
//! `src/overseer/deploy.rs`). The right side is unrestricted, so plurals and
//! suffixes still match. The classifier is a pure function so its boundary
//! logic is unit-tested (`guard_classifier_*`) independent of the tree.
//!
//! ## The allowlist is essentially empty (by design)
//!
//! Exactly one runtime survivor: the JSON-RPC method NAME `bridge.health`. It
//! is the wire-protocol method the EXTERNAL memory / knowledge server
//! (`amplihack-memory-lib`'s `simard_memory_bridge.py`) answers to; renaming it
//! would break interop and is out of scope. It is exempted per-occurrence (not
//! per-line) and ONLY as the method *name* — a quoted literal `"bridge.health"`
//! or a doc reference to it — so a local variable named `bridge` that merely
//! calls `.health()`, and a renameable "a bridge server" comment sharing the
//! same line, are BOTH still flagged.
//!
//! The only excluded files are the no-`Bridge` linter and its own fixtures —
//! `src/overseer/pr_verify.rs`, `src/overseer/merge_ops.rs`,
//! `src/operator_commands_dashboard/index_html/tests_tab_meta.rs` — which must
//! keep the literal `"Bridge"` / `"bridge"` as their detection substrings, plus
//! this test file itself. Everything else — log strings, telemetry identities,
//! config keys, comments, identifiers, module-internal names — is in-repo and
//! renameable, so the guard flags it until the mechanical rename lands.
//!
//! ## TDD status
//!
//! `no_lowercase_bridge_word_in_src` FAILS until the mechanical rename is
//! complete (RED). The CamelCase and module tests already pass. These tests are
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

/// Byte offsets of every lowercase `bridge`-stem COMPONENT on `line`.
///
/// A component is the case-insensitive stem `bridge` whose immediately-preceding
/// byte is not an ASCII letter, so embedded stems (`abridged`, `Cambridge`,
/// `Bainbridge`) are ignored while real components (`bridge`, `bridges`,
/// `_bridge`, `bridge_name`, `"bridge.health"`) are reported. The right side is
/// unrestricted. Pure function → the boundary logic is unit-testable without the
/// source tree.
fn lowercase_bridge_components(line: &str) -> Vec<usize> {
    const STEM: &[u8] = b"bridge";
    let raw = line.as_bytes();
    let lower = line.to_ascii_lowercase();
    let low = lower.as_bytes();
    let mut hits = Vec::new();
    let mut i = 0usize;
    while i + STEM.len() <= low.len() {
        if &low[i..i + STEM.len()] == STEM {
            let prev_is_letter = i > 0 && raw[i - 1].is_ascii_alphabetic();
            if !prev_is_letter {
                hits.push(i);
            }
            i += STEM.len();
        } else {
            i += 1;
        }
    }
    hits
}

/// The single documented allowlist entry: the EXTERNAL JSON-RPC method NAME
/// `bridge.health` (answered by `amplihack-memory-lib`'s memory / knowledge
/// server). Renaming it would break wire interop, so it is exempted
/// per-occurrence — but ONLY as the method name (a quoted literal or a doc
/// reference), never as a local variable named `bridge` that invokes
/// `.health()`, which must still be renamed.
fn is_external_health_method(lower_line: &str, at: usize) -> bool {
    const NAME: &str = "bridge.health";
    let tail = &lower_line[at..];
    tail.starts_with(NAME) && !tail[NAME.len()..].starts_with('(')
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

#[test]
fn no_lowercase_bridge_word_in_src() {
    let src = repo_src_dir();
    // The no-`Bridge` linter and its own fixtures must retain the detection
    // substring; this test file necessarily discusses "bridge" throughout.
    let exclude = [
        "no_bridge_naming.rs",
        "pr_verify.rs",
        "merge_ops.rs",
        "tests_tab_meta.rs",
    ];

    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    files.sort();

    let mut stragglers: Vec<String> = Vec::new();
    for file in files {
        let basename = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if exclude.contains(&basename) {
            continue;
        }
        let contents = match fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (idx, line) in contents.lines().enumerate() {
            let low = line.to_ascii_lowercase();
            let has_straggler = lowercase_bridge_components(line)
                .into_iter()
                .any(|at| !is_external_health_method(&low, at));
            if has_straggler {
                stragglers.push(format!("{}:{}:{}", file.display(), idx + 1, line.trim()));
            }
        }
    }

    const SAMPLE: usize = 40;
    let sample = stragglers
        .iter()
        .take(SAMPLE)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        stragglers.is_empty(),
        "Rename incomplete: {} line(s) in `src/` still contain the meaningless \
         lowercase `bridge` component.\n\
         The operator rule is absolute — nothing may be named `bridge`, including \
         runtime log strings, telemetry identities, comments, snake_case \
         identifiers and module-internal names. Rename each to an intent-revealing \
         term (memory-ipc client/transport, memory recall reader/source, \
         knowledge-pack reader/source, rpc client/transport, engineer handoff, \
         ...). The only permitted survivor is the external JSON-RPC method name \
         `bridge.health`.\n\
         Showing first {} of {} straggler line(s):\n{}",
        stragglers.len(),
        sample.lines().count(),
        stragglers.len(),
        sample
    );
}

#[test]
fn guard_classifier_flags_real_components() {
    // Every shape the strengthened rule must eliminate: log string, snake_case
    // identifier, comment, on-disk/telemetry identity, module name, quoted word.
    for line in [
        "let bridge = connect();",
        "fn launch_enrichment_bridges() {}",
        "// Both bridges are optional",
        "message: format!(\"bridge '{name}' transport error: {reason}\")",
        "const STORE: &str = \"cognitive-bridge-memory\";",
        "mod memory_bridge;",
        "tab.title == \"bridge\"",
    ] {
        assert!(
            !lowercase_bridge_components(line).is_empty(),
            "classifier failed to flag a real `bridge` component in: {line}"
        );
    }
}

#[test]
fn guard_classifier_ignores_embedded_stems() {
    // Stem buried inside an unrelated English / proper word → never flagged.
    for line in [
        "the log was abridged for brevity",
        "shipped from Cambridge, MA",
        "deploying into an unstable process (Bainbridge's irony)",
    ] {
        assert!(
            lowercase_bridge_components(line).is_empty(),
            "classifier wrongly flagged an embedded stem in: {line}"
        );
    }
}

#[test]
fn guard_planted_lowercase_bridge_is_detected() {
    // A planted straggler of each shape the rename must eliminate — proving the
    // guard's RED signal fires on lowercase `bridge`, not only CamelCase.
    let planted = "\
        eprintln!(\"[simard] bridge 'memory-ipc' transport error\");\n\
        pub fn launch_enrichment_bridges() {}\n\
        /// degrade the knowledge bridge to None\n";
    let flagged = planted
        .lines()
        .filter(|l| !lowercase_bridge_components(l).is_empty())
        .count();
    assert_eq!(
        flagged, 3,
        "guard must flag every planted lowercase straggler"
    );
}

#[test]
fn guard_allowlists_only_external_health_method_name() {
    // The quoted external method name is the one exempt survivor.
    let literal = "method: \"bridge.health\".to_string(),";
    let low = literal.to_ascii_lowercase();
    let hits = lowercase_bridge_components(literal);
    assert_eq!(hits.len(), 1, "the health method is a bridge component");
    assert!(
        hits.iter().all(|&at| is_external_health_method(&low, at)),
        "the quoted `bridge.health` method name must be allowlisted"
    );

    // A LOCAL VARIABLE named `bridge` invoking `.health()` is NOT the external
    // method name — it must still be flagged for rename.
    let call = "let h = bridge.health().unwrap();";
    let low_call = call.to_ascii_lowercase();
    assert!(
        lowercase_bridge_components(call)
            .into_iter()
            .any(|at| !is_external_health_method(&low_call, at)),
        "a `bridge`-named variable calling .health() must still be flagged"
    );

    // A line MIXING the allowlisted method name with a renameable "a bridge
    // server" comment is still a straggler (per-occurrence, not per-line).
    let mixed = "/// Health reported by a bridge server via `bridge.health`.";
    let low_mixed = mixed.to_ascii_lowercase();
    let non_allowlisted = lowercase_bridge_components(mixed)
        .into_iter()
        .filter(|&at| !is_external_health_method(&low_mixed, at))
        .count();
    assert_eq!(
        non_allowlisted, 1,
        "the `a bridge server` component on a bridge.health line must still flag"
    );
}
