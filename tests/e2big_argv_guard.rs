//! Anti-regression guard for the E2BIG ("Argument list too long") class
//! (issue #2640) — a grep-shaped, CI-visible test (shaped like the existing
//! `tests/no_bridge_naming.rs`) that FAILS the build if the recurring pattern is
//! reintroduced or the single-chokepoint facade is removed.
//!
//! The E2BIG failure kept recurring because it was fixed one launch site at a
//! time. This guard makes the *class* impossible to reintroduce silently by
//! asserting three durable invariants over `src/**/*.rs` (excluding this crate's
//! own tests and doc-comments):
//!
//!   1. **No `$(cat` argv-expansion.** The root pattern — `sh -c "… -p \"$(cat
//!      FILE)\""` — expands a file's CONTENTS into argv and overflows `ARG_MAX`.
//!      Piping (`cat 'PATH' | cmd`) is allowed; only contents-expansion is
//!      forbidden. (Protects #2660.)
//!   2. **No inline of an unbounded recipe key.** The Tier-A keys that carry
//!      unbounded agent context — `day_context`, `draft`, `episodes`, `pr_body`
//!      — must appear only as their file-channel `<key>_path` form, never inlined
//!      as `<key>=<value>`. (Protects #2692/#2700.)
//!   3. **The single spawn facade exists.** `src/spawn_payload/mod.rs` is present
//!      and registered as `pub mod spawn_payload;` in `src/lib.rs`, so every
//!      launch site has one policy-enforcing chokepoint to route through.
//!
//! Scope note: assertions 1–2 are pure regression guards (currently green, red on
//! any regression). Assertion 3 encodes this task's net-new requirement and is
//! RED until the facade lands. The Tier-B inline sites (`task_description`,
//! `proposal`, `objective`, …) are driven to the facade by the per-path transport
//! tests, not this coarse grep, because those keys are also used by bounded,
//! already-safe callers (e.g. `ooda_brain::recipe_brain`) and a key-name grep
//! would false-positive on them.
//!
//! Intentionally shell-grep-shaped so operators running the same checks get the
//! same answer. No `simard` imports, so this crate compiles and runs even while
//! the facade-dependent per-path test crates are RED.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn src_dir() -> PathBuf {
    repo_root().join("src")
}

/// Recursively collect all `.rs` files under `root`.
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

/// A file is a test module (excluded from the production scan) if its basename
/// starts with `tests_` or ends with `_tests.rs`. Such files legitimately embed
/// the antipatterns as fixtures / assertion strings.
fn is_test_module(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    name.starts_with("tests_") || name.ends_with("_tests.rs")
}

/// A source line is a comment (skipped) if its trimmed form starts with `//`
/// (covers `//`, `///`, `//!`). Production antipatterns live in real code /
/// command strings, not comments.
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// Return `<file>:<line>:<text>` for every non-comment production line under
/// `src/` containing `needle`.
fn grep_production(needle: &str) -> Vec<String> {
    let mut files = Vec::new();
    collect_rs_files(&src_dir(), &mut files);
    let mut hits = Vec::new();
    for file in files {
        if is_test_module(&file) {
            continue;
        }
        let contents = match fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (idx, line) in contents.lines().enumerate() {
            if is_comment_line(line) {
                continue;
            }
            if line.contains(needle) {
                hits.push(format!("{}:{}:{}", file.display(), idx + 1, line.trim()));
            }
        }
    }
    hits
}

/// (1) No `$(cat …)` contents-expansion into argv anywhere in production `src/`.
#[test]
fn no_cat_command_substitution_in_argv() {
    let hits = grep_production("$(cat");
    assert!(
        hits.is_empty(),
        "issue #2640 regression: `$(cat …)` argv contents-expansion reintroduced.\n\
         This expands a file's CONTENTS into argv and overflows ARG_MAX (E2BIG).\n\
         Deliver the payload on STDIN (`cat 'PATH' | cmd`, or the spawn_payload\n\
         facade) instead. Offending line(s):\n{}",
        hits.join("\n")
    );
}

/// (2) The unbounded Tier-A recipe keys are never inlined as `<key>=<value>` —
/// they must ride on argv only as their file-channel `<key>_path` form.
#[test]
fn unbounded_recipe_keys_are_never_inlined() {
    let unbounded_keys = ["day_context", "draft", "episodes", "pr_body"];
    let mut hits = Vec::new();
    for key in unbounded_keys {
        // The inline value form is `"<key>={` (a format! string literal opening
        // the interpolation); the safe file form `<key>_path=` never matches this.
        hits.extend(grep_production(&format!("\"{key}={{")));
    }
    assert!(
        hits.is_empty(),
        "issue #2640/#2692 regression: an unbounded recipe context key is inlined\n\
         into argv (`<key>={{…}}`). These carry a full day's / PR's context and\n\
         overflow ARG_MAX. Route them through the file channel\n\
         (`spawn_payload::recipe_context` / `recipe_context_file::ContextFile`) so\n\
         only `<key>_path=<abs>` rides on argv. Offending line(s):\n{}",
        hits.join("\n")
    );
}

/// (3) The single spawn facade exists and is registered — the one chokepoint
/// every agent/recipe launch routes through. RED until the fix adds it.
#[test]
fn spawn_payload_facade_module_exists_and_is_registered() {
    let facade = src_dir().join("spawn_payload").join("mod.rs");
    assert!(
        facade.exists(),
        "issue #2640: the single spawn facade `src/spawn_payload/mod.rs` must exist\n\
         so every large-payload agent/recipe launch has ONE policy-enforcing\n\
         chokepoint (payload >= ARGV_PAYLOAD_MAX_BYTES never touches argv/envp)."
    );

    let lib_rs = fs::read_to_string(src_dir().join("lib.rs")).expect("read src/lib.rs");
    assert!(
        lib_rs.contains("pub mod spawn_payload;"),
        "issue #2640: `src/lib.rs` must register `pub mod spawn_payload;` so the\n\
         facade is a public, routable chokepoint."
    );
}
