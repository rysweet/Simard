//! Contract tests for issue #3181 — Simard is a PURE-RUST daemon: zero Python,
//! zero kuzu. These are enforced by the same `cargo test` gate that runs in CI,
//! so the "no Python / no kuzu" invariants cannot silently regress.
//!
//! TDD note: several of these tests FAIL until the migration lands. They encode
//! the acceptance contract, not the current state.
//!
//! Self-match guard: this file must scan `tests/**/*.rs` (which includes itself)
//! for forbidden tokens. To avoid matching its own source, every forbidden
//! literal is assembled at runtime via `concat_token(..)` (so the contiguous
//! literal never appears in this file) AND this file is excluded from the scan
//! via the `:!tests/pure_rust_daemon.rs` pathspec.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Repository root = the crate manifest dir (Cargo.toml lives at the repo root).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Build a forbidden token from fragments so this test file never contains the
/// contiguous literal it is searching for.
fn concat_token(parts: &[&str]) -> String {
    parts.concat()
}

/// Return tracked files matching a `git ls-files` pathspec.
fn git_ls_files(pathspecs: &[&str]) -> Vec<String> {
    let mut args = vec!["ls-files"];
    args.extend_from_slice(pathspecs);
    let out = Command::new("git")
        .args(&args)
        .current_dir(repo_root())
        .output()
        .expect("[simard] git ls-files should run");
    assert!(
        out.status.success(),
        "[simard] git ls-files failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// Run `git grep -F` for a fixed-string `needle` over `pathspecs` and return the
/// matching `path:line:content` records. Empty when there are no matches.
fn git_grep_fixed(needle: &str, case_insensitive: bool, pathspecs: &[&str]) -> Vec<String> {
    let mut args = vec!["grep", "-I", "--no-color", "-n", "-F"];
    if case_insensitive {
        args.push("-i");
    }
    args.push("-e");
    args.push(needle);
    args.push("--");
    args.extend_from_slice(pathspecs);

    let out = Command::new("git")
        .args(&args)
        .current_dir(repo_root())
        .output()
        .expect("[simard] git grep should run");

    // git grep: exit 0 = matches found, 1 = no matches, >1 = real error.
    match out.status.code() {
        Some(0) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.to_string())
            .collect(),
        Some(1) => Vec::new(),
        other => panic!(
            "[simard] git grep errored (code {other:?}): {}",
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

/// Exclude this test file from `tests/` scans so it never matches its own
/// (fragment-assembled) needles.
const SELF_EXCLUDE: &str = ":!tests/pure_rust_daemon.rs";

// ---------------------------------------------------------------------------
// 1. Zero tracked Python — the headline acceptance criterion.
// ---------------------------------------------------------------------------

#[test]
fn repo_has_no_tracked_python_files() {
    let py = git_ls_files(&["*.py"]);
    assert!(
        py.is_empty(),
        "[simard] Simard must be Python-free, but these .py files are still tracked:\n  {}",
        py.join("\n  ")
    );
}

#[test]
fn repo_has_no_python_packaging_files() {
    let mut offenders = Vec::new();
    for spec in [
        "pyproject.toml",
        "**/pyproject.toml",
        "setup.py",
        "**/setup.py",
        "setup.cfg",
        "**/setup.cfg",
        "requirements*.txt",
        "**/requirements*.txt",
        "Pipfile",
        "**/Pipfile",
        "conftest.py",
        "**/conftest.py",
    ] {
        offenders.extend(git_ls_files(&[spec]));
    }
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "[simard] Python packaging/config files must not exist:\n  {}",
        offenders.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// 2. No Python runtime dependency in Rust source.
// ---------------------------------------------------------------------------

#[test]
fn src_does_not_spawn_python3() {
    // literal: Command::new("python3")
    let needle = concat_token(&["Command::new(\"python", "3\")"]);
    let hits = git_grep_fixed(&needle, false, &["src"]);
    assert!(
        hits.is_empty(),
        "[simard] no Rust source may spawn a python3 subprocess:\n  {}",
        hits.join("\n  ")
    );
}

#[test]
fn src_does_not_spawn_pre_commit_binary() {
    // literal: Command::new("pre-commit")
    let needle = concat_token(&["Command::new(\"pre-", "commit\")"]);
    let hits = git_grep_fixed(&needle, false, &["src"]);
    assert!(
        hits.is_empty(),
        "[simard] engineer worktrees must gate via committed git hooks, not the \
         Python `pre-commit` binary:\n  {}",
        hits.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// 3. No kuzu in the Rust code path or Rust tests (docs/history excluded).
// ---------------------------------------------------------------------------

#[test]
fn rust_sources_and_tests_are_kuzu_free() {
    // literal: kuzu  (case-insensitive: Kuzu, KuzuGraphStore, ...)
    let needle = concat_token(&["ku", "zu"]);
    let hits = git_grep_fixed(&needle, true, &["src", "tests", SELF_EXCLUDE]);
    assert!(
        hits.is_empty(),
        "[simard] the graph store is the embedded `lbug` crate; no kuzu references \
         may remain in Rust code or tests:\n  {}",
        hits.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// 4. Shell/YAML test harnesses must not shell out to Python.
// ---------------------------------------------------------------------------

#[test]
fn gadugi_and_qa_scenarios_are_python_free() {
    // literals: python3 , PYTHONPATH
    let py3 = concat_token(&["python", "3"]);
    let pythonpath = concat_token(&["PYTHON", "PATH"]);
    let mut hits = Vec::new();
    for needle in [py3, pythonpath] {
        hits.extend(git_grep_fixed(
            &needle,
            false,
            &["tests/gadugi", "tests/qa-scenarios"],
        ));
    }
    assert!(
        hits.is_empty(),
        "[simard] gadugi/qa harnesses must use jq/native tooling and the native Rust \
         memory bridge, not python3:\n  {}",
        hits.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// 5. CI: the verify workflow is Python-free yet preserves every cargo gate.
// ---------------------------------------------------------------------------

fn verify_yml() -> String {
    let path = repo_root().join(".github/workflows/verify.yml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("[simard] cannot read {}: {e}", path.display()))
}

#[test]
fn verify_workflow_has_no_python_tooling() {
    let yml = verify_yml();
    // Fragment-assembled so this Rust test file never contains the literals.
    let forbidden = [
        concat_token(&["setup-", "python"]),
        concat_token(&["pip ", "install"]),
        concat_token(&["pre_", "commit"]),
        concat_token(&["py", "test"]),
        concat_token(&["python -m ", ""]),
    ];
    let mut found = Vec::new();
    for token in &forbidden {
        if yml.contains(token.as_str()) {
            found.push(token.clone());
        }
    }
    assert!(
        found.is_empty(),
        "[simard] .github/workflows/verify.yml must not use Python tooling, but found: {}",
        found.join(", ")
    );
}

#[test]
fn verify_workflow_runs_cargo_gates_directly() {
    let yml = verify_yml();

    // fmt gate — must run directly, not via the Python pre-commit framework.
    assert!(
        yml.contains("cargo fmt") && (yml.contains("--check") || yml.contains("-- --check")),
        "[simard] verify.yml must run `cargo fmt --check` directly"
    );

    // clippy gate at full strictness.
    assert!(
        yml.contains("cargo clippy") && yml.contains("-D warnings"),
        "[simard] verify.yml must run `cargo clippy ... -D warnings` directly"
    );

    // test gate.
    assert!(
        yml.contains("cargo test"),
        "[simard] verify.yml must run `cargo test`"
    );

    // supply-chain gate.
    assert!(
        yml.contains("cargo deny") || yml.contains("cargo-deny"),
        "[simard] verify.yml must keep the cargo-deny gate"
    );

    // Rust-only gate.
    assert!(
        yml.contains("check-rust-only-gate.sh"),
        "[simard] verify.yml must keep the Rust-only gate"
    );
}

// ---------------------------------------------------------------------------
// 6. Committed native git hooks replace the Python pre-commit framework.
// ---------------------------------------------------------------------------

#[test]
fn committed_pre_commit_hook_exists_and_runs_cargo_not_python() {
    let hook = repo_root().join("hooks/pre-commit");
    assert!(
        hook.is_file(),
        "[simard] a committed native git hook must exist at hooks/pre-commit \
         (wired via core.hooksPath) to preserve local commit gating without Python"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&hook).unwrap().permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "[simard] hooks/pre-commit must be executable (mode {mode:o})"
        );
    }

    let body = std::fs::read_to_string(&hook).expect("[simard] read hooks/pre-commit");
    assert!(
        body.contains("cargo"),
        "[simard] hooks/pre-commit must invoke cargo directly"
    );

    // The native hook must not delegate back to Python tooling.
    let py3 = concat_token(&["python", "3"]);
    let pip = concat_token(&["pip ", "install"]);
    let pc = concat_token(&["pre-", "commit run"]);
    for bad in [&py3, &pip, &pc] {
        assert!(
            !body.contains(bad.as_str()),
            "[simard] hooks/pre-commit must be Python-free, but references `{bad}`"
        );
    }
}

// ---------------------------------------------------------------------------
// 7. The Rust-only gate must block a NEW .py file ANYWHERE (not just src/,
//    python/). Verified against a throwaway git repo using the repo's own
//    gate script so we exercise the real logic.
// ---------------------------------------------------------------------------

fn run_git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("[simard] git should run")
        .success();
    assert!(ok, "[simard] git {args:?} failed in {}", dir.display());
}

#[test]
fn rust_only_gate_blocks_new_python_anywhere() {
    let gate = repo_root().join("scripts/check-rust-only-gate.sh");
    assert!(gate.is_file(), "[simard] gate script must exist");

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    run_git(root, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::create_dir_all(root.join("tests/gadugi")).unwrap();

    // Copy the real gate script into the throwaway repo.
    std::fs::copy(&gate, root.join("scripts/check-rust-only-gate.sh")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("scripts/check-rust-only-gate.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    // Stage a new Python file OUTSIDE src/ and python/ (the previously
    // grandfathered blind spot the tightened gate must now catch).
    std::fs::write(root.join("tests/gadugi/audit.py"), "print('nope')\n").unwrap();
    run_git(root, &["add", "-A"]);

    let out = Command::new("bash")
        .arg("scripts/check-rust-only-gate.sh")
        .arg("--staged")
        .current_dir(root)
        .output()
        .expect("[simard] gate script should run");

    assert!(
        !out.status.success(),
        "[simard] the Rust-only gate must FAIL on a new .py file anywhere in the repo \
         (tests/gadugi/audit.py). stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn rust_only_gate_allows_rust_files() {
    let gate = repo_root().join("scripts/check-rust-only-gate.sh");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    run_git(root, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::copy(&gate, root.join("check-rust-only-gate.sh")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    run_git(root, &["add", "src/main.rs"]);

    let out = Command::new("bash")
        .arg("check-rust-only-gate.sh")
        .arg("--staged")
        .current_dir(root)
        .output()
        .expect("[simard] gate script should run");

    assert!(
        out.status.success(),
        "[simard] the Rust-only gate must PASS on pure-Rust changes. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
