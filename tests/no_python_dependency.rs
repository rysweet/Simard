//! Executable acceptance guard for the operator's absolute rule (issue #3181):
//! *Simard is a PURE-RUST daemon — it must not depend on a Python runtime for
//! its code, scripts, tests, or CI gates, and it must not reference the Python
//! `kuzu` package.* Simard's graph store is the embedded `lbug` (LadybugDB)
//! Rust crate, never a Python `kuzu` package.
//!
//! The repo already declares this intent (`.pre-commit-config.yaml` /
//! `scripts/check-rust-only-gate.sh` — "Rust-only gate — no new .py/.js/.ts").
//! This guard finishes the job: it fails (RED) while any grandfathered Python
//! or `kuzu` reference survives and passes (GREEN) only once the migration in
//! issue #3181 is complete. Every check is deliberately shaped like the
//! `git grep` / `git ls-files` an operator would run by hand, so a human
//! running the same command gets the same answer.
//!
//! ## What each check enforces (and its TDD colour at the time of writing)
//!
//!  1. `no_tracked_python_source_files` (RED) — zero tracked `*.py` files
//!     anywhere in the repo. This is the unambiguous core of "Python-free" and
//!     cannot be gamed. RED until all eight survivors are deleted
//!     (`scripts/dashboard_audit/contradiction_evidence.py`,
//!     `tests/fixtures/echo_rpc.py`, the four `tests/test_*_migration.py` /
//!     `tests/test_audit_dashboard.py` standalone pytest files, and the two
//!     `tests/e2e-dashboard/smoke_python/*.py` files).
//!
//!  2. `no_tracked_python_packaging_files` (RED) — zero Python packaging /
//!     tooling manifests (`requirements*.txt`, `pyproject.toml`, `setup.py`,
//!     `setup.cfg`, `Pipfile*`, `tox.ini`, `pytest.ini`, `.python-version`,
//!     `conftest.py`). RED until `tests/e2e-dashboard/smoke_python/
//!     requirements.txt` (and its `conftest.py`) are removed.
//!
//!  3. `ci_workflows_do_not_invoke_python` (RED) — no `.github/workflows/*.yml`
//!     job may set up or invoke Python (`actions/setup-python`, `pip install`,
//!     `python -m pip`, `python -m pre_commit`, `pytest`, `playwright install`).
//!     RED until `verify.yml` (fmt/clippy/test gates + smoke_python) and
//!     `docs.yml` (mkdocs) run their gates via cargo / a non-Python mechanism.
//!
//!  4. `gate_and_setup_scripts_run_cargo_not_python` (RED) — no committed
//!     git hook (`hooks/*`) or setup/gate script (`scripts/*.sh`) may shell out
//!     to Python or the Python `pre-commit` framework. RED until
//!     `scripts/install-precommit.sh` (installs the Python `pre-commit` tool)
//!     and `scripts/verify-docs.sh` (`pip install mkdocs`) are replaced by a
//!     native mechanism.
//!
//!  5. `rust_only_gate_rejects_new_python_anywhere` (RED) — a BEHAVIOURAL test:
//!     `scripts/check-rust-only-gate.sh` must reject a new `.py` file **no
//!     matter where it lives**, not only under `src/` or `python/`. The current
//!     gate only inspects those two directories, so a `tests/leak.py` slips
//!     through — exactly how the grandfathered Python was never caught. RED
//!     until the gate is hardened to scan `.py` anywhere (allow-list empty).
//!
//!  6. `no_kuzu_in_rust_code` (RED) — zero case-insensitive `kuzu` in any
//!     `*.rs`. RED until `src/cmd_ensure_deps.rs`'s `check_python_package("kuzu")`
//!     is removed and the two historical comments in
//!     `tests/cognitive_memory_procedure_recall_unified.rs` and
//!     `tests/install_real.rs` are reworded to LadybugDB / `lbug`.
//!
//!  7. `no_kuzu_in_ci_and_gate_scripts` (GREEN regression guard) — the CI and
//!     gate mechanism must never reference `kuzu`. Already green; this locks it.
//!
//!  8. `no_python_process_spawn_in_rust` (RED) — no Rust code may spawn a
//!     Python interpreter or check for one as a runtime dependency
//!     (`Command::new("python3")`, `check_binary("python3", ...)`,
//!     `check_python_package(...)`). RED until `src/rpc_transport/subprocess.rs`
//!     (the dead `SubprocessRpcTransport` that spawns `python3`) and the two
//!     `src/cmd_ensure_deps.rs` checks are removed.
//!
//!  9. `native_git_hooks_present_and_python_free` (RED) — the Python
//!     `pre-commit` framework must be replaced by committed git hooks
//!     (`hooks/pre-commit`, `hooks/pre-push`, wired via `core.hooksPath`) that
//!     run `cargo` directly and invoke no Python. RED until those hooks exist.
//!
//! 10. `python_precommit_framework_config_removed` (RED) — the Python
//!     `pre-commit` framework's own config (`.pre-commit-config.yaml`) must be
//!     gone; keeping it implies keeping the Python tool that reads it. RED
//!     until removed.
//!
//! The two `classifier_*` unit tests pin the pure-function Python-invocation
//! detector so its boundary logic is verified independently of the tree.
//!
//! ## Deliberate scope boundaries (documented so the guard is honest, not lax)
//!
//!  * **Docs prose is out of scope for the `kuzu` word-ban.** Checks 6 and 7
//!    scrub `kuzu` from Simard's *code and gate mechanism*, where "kuzu-free"
//!    is unambiguous (no Python `kuzu` package, no dependency check, none in the
//!    gates). Operator docs legitimately name the real on-disk artifact
//!    `cognitive_memory.ladybug.kuzu-backup` (emitted by the legacy-directory
//!    migration) and the upstream KuzuDB C++ lineage of the `lbug` fork;
//!    scrubbing the word there would MISDOCUMENT a real artifact and a real
//!    build fact — a zero-BS violation. Docs wording is left to human review.
//!
//!  * **`.github/hooks/` is out of scope.** Those are amplihack agent-runtime
//!    hooks (they best-effort call a `*.py` that is not even tracked in this
//!    repo), not Simard's build/CI gate mechanism. This guard polices Simard's
//!    own gates (`.github/workflows/`, `scripts/`, committed `hooks/`).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// This guard's own basename — it necessarily contains every forbidden token
/// (`python`, `kuzu`, `pip install`, `Command::new("python3")`, ...) as needles
/// and fixtures, so every `*.rs` content scan must exclude it.
const SELF: &str = "no_python_dependency.rs";

/// Tracked files, exactly as `git ls-files` reports them (repo-relative paths).
/// Using git (not a filesystem walk) keeps the check tracked-only and fast,
/// skips `target/`, and mirrors what `scripts/check-rust-only-gate.sh` itself
/// consults — so an operator running `git ls-files` sees the same universe.
fn git_tracked_files() -> Vec<String> {
    let out = Command::new("git")
        .arg("ls-files")
        .current_dir(repo_root())
        .output()
        .expect("[simard] guard: failed to run `git ls-files` (git must be on PATH)");
    assert!(
        out.status.success(),
        "[simard] guard: `git ls-files` exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Read a tracked file's contents (repo-relative path). Non-UTF-8 / unreadable
/// files are treated as empty — the checks only reason about text.
fn read_tracked(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).unwrap_or_default()
}

/// High-signal substrings that mark an actual Python-runtime invocation or
/// setup — as opposed to the mere English word "Python" (which appears
/// legitimately in prose, historical comments, and the Rust-only gate's own
/// diagnostics). A line matches iff it contains one of these tokens. The set is
/// deliberately about *verbs* (`install`, `run`, `setup-python`, `-m ...`) so
/// that `# prohibited Python files`, `New .py files under src/`, and
/// `"$file" == python/*.py` in `check-rust-only-gate.sh` are NOT flagged.
///
/// NOTE (Node/TypeScript is out of scope, issue #3181): these tokens target the
/// *Python* runtime specifically. The e2e-dashboard job legitimately runs
/// `npx playwright install` — that is Node's Playwright browser provisioning
/// (JS/TS, grandfathered like the rest of `tests/e2e-dashboard/`), NOT Python.
/// Python-driven Playwright (`python -m playwright install ...`) is still caught
/// by the `python -m ` token, and pip-installed Playwright by `pip install`, so
/// dropping a bare `playwright install` token loses no Python coverage while
/// no longer false-flagging the Node command.
const PYTHON_INVOCATION_TOKENS: &[&str] = &[
    "setup-python",
    "pip install",
    "pip3 install",
    "pipx install",
    "python -m ",
    "python3 -m ",
    "-m pip",
    "-m pre_commit",
    "pre_commit run",
    "pre-commit run",
    "pre-commit install",
    "pytest ",
];

/// Pure function: does `line` invoke or provision a Python runtime? Pure so the
/// boundary logic is unit-tested (`classifier_*`) without the source tree.
fn line_invokes_python(line: &str) -> bool {
    PYTHON_INVOCATION_TOKENS
        .iter()
        .any(|tok| line.contains(tok))
}

#[test]
fn no_tracked_python_source_files() {
    let py: Vec<String> = git_tracked_files()
        .into_iter()
        .filter(|p| p.ends_with(".py"))
        .collect();

    assert!(
        py.is_empty(),
        "[simard] Not yet Python-free: {} tracked `*.py` file(s) remain. Simard is a \
         pure-Rust daemon (issue #3181) — every one of these must be deleted (dead \
         Python fixtures/scripts) or ported to Rust:\n{}",
        py.len(),
        py.join("\n")
    );
}

#[test]
fn no_tracked_python_packaging_files() {
    // Python packaging / test-config manifests. Their presence means something
    // still expects a Python toolchain, even if no `.py` remained.
    let banned_basenames = [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "Pipfile",
        "Pipfile.lock",
        "tox.ini",
        "pytest.ini",
        ".python-version",
        "conftest.py",
    ];
    let offenders: Vec<String> = git_tracked_files()
        .into_iter()
        .filter(|p| {
            let b = basename(p);
            banned_basenames.contains(&b) || (b.starts_with("requirements") && b.ends_with(".txt"))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "[simard] Python packaging/tooling manifest(s) still tracked ({}). Remove them so no \
         Python toolchain is implied:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

#[test]
fn ci_workflows_do_not_invoke_python() {
    // Every CI gate must run without provisioning or invoking Python. The
    // fmt/clippy/test gates must run as direct `cargo` steps; docs must build
    // via a non-Python mechanism.
    let mut stragglers: Vec<String> = Vec::new();
    for path in git_tracked_files() {
        if !path.starts_with(".github/workflows/") {
            continue;
        }
        if !(path.ends_with(".yml") || path.ends_with(".yaml")) {
            continue;
        }
        for (idx, line) in read_tracked(&path).lines().enumerate() {
            if line_invokes_python(line) {
                stragglers.push(format!("{}:{}:{}", path, idx + 1, line.trim()));
            }
        }
    }

    assert!(
        stragglers.is_empty(),
        "[simard] CI still provisions/invokes Python in {} workflow line(s). Replace \
         `actions/setup-python` + `pip install pre-commit` + `python -m pre_commit run ...` \
         with direct `cargo fmt/clippy/test/deny` steps, and the pytest/playwright \
         smoke_python steps with Rust-native coverage (issue #3181):\n{}",
        stragglers.len(),
        stragglers.join("\n")
    );
}

#[test]
fn gate_and_setup_scripts_run_cargo_not_python() {
    // Committed git hooks and setup/gate scripts must shell out to `cargo`,
    // never to Python or the Python `pre-commit` framework.
    let mut stragglers: Vec<String> = Vec::new();
    for path in git_tracked_files() {
        let in_scope =
            (path.starts_with("scripts/") && path.ends_with(".sh")) || path.starts_with("hooks/");
        if !in_scope {
            continue;
        }
        for (idx, line) in read_tracked(&path).lines().enumerate() {
            if line_invokes_python(line) {
                stragglers.push(format!("{}:{}:{}", path, idx + 1, line.trim()));
            }
        }
    }

    assert!(
        stragglers.is_empty(),
        "[simard] {} setup/gate/hook line(s) still invoke Python. The pre-commit framework \
         is Python; replace it with committed git hooks that run `cargo` directly, and remove \
         the `pip install` docs step (issue #3181):\n{}",
        stragglers.len(),
        stragglers.join("\n")
    );
}

#[test]
fn rust_only_gate_rejects_new_python_anywhere() {
    // BEHAVIOURAL: the Rust-only gate must catch a new `.py` file no matter
    // where it lives — not only under `src/` or `python/`. That directory
    // blind-spot is exactly how the grandfathered Python (`tests/*.py`,
    // `scripts/dashboard_audit/*.py`) was never flagged. We copy the real gate
    // script into a throwaway git repo, plant `.py` files OUTSIDE src/python,
    // and require a non-zero exit.
    let gate_src = repo_root().join("scripts/check-rust-only-gate.sh");
    assert!(
        gate_src.exists(),
        "[simard] scripts/check-rust-only-gate.sh is missing — the Rust-only gate must exist"
    );

    let tmp = std::env::temp_dir().join(format!(
        "simard-rustonly-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("scripts")).expect("[simard] guard: mkdir scripts");
    fs::create_dir_all(tmp.join("tests")).expect("[simard] guard: mkdir tests");
    fs::create_dir_all(tmp.join("docs")).expect("[simard] guard: mkdir docs");
    fs::create_dir_all(tmp.join("src")).expect("[simard] guard: mkdir src");

    fs::copy(&gate_src, tmp.join("scripts/check-rust-only-gate.sh"))
        .expect("[simard] guard: copy gate script");
    // Plant Python OUTSIDE src/ and python/ — the current gate's blind spot.
    fs::write(tmp.join("tests/leak.py"), "print('leak')\n").expect("[simard] guard: write leak.py");
    fs::write(tmp.join("docs/leak.py"), "print('leak')\n").expect("[simard] guard: write doc py");
    // A legitimate Rust file so a hardened gate that only bans .py still passes
    // the control case below.
    fs::write(tmp.join("src/main.rs"), "fn main() {}\n").expect("[simard] guard: write main.rs");

    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&tmp)
            .status()
            .expect("[simard] guard: git invocation failed")
            .success();
        assert!(ok, "[simard] guard: `git {}` failed", args.join(" "));
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "guard@example.invalid"]);
    git(&["config", "user.name", "guard"]);
    git(&["add", "-A"]);

    let status = Command::new("bash")
        .arg("scripts/check-rust-only-gate.sh")
        .current_dir(&tmp)
        .status()
        .expect("[simard] guard: failed to run the gate script");

    let rejected = !status.success();

    // Positive control: with the .py files removed, an all-Rust tree must PASS
    // (exit 0) — proving the hardened gate rejects Python specifically, not
    // everything.
    fs::remove_file(tmp.join("tests/leak.py")).ok();
    fs::remove_file(tmp.join("docs/leak.py")).ok();
    git(&["add", "-A"]);
    let control_ok = Command::new("bash")
        .arg("scripts/check-rust-only-gate.sh")
        .current_dir(&tmp)
        .status()
        .expect("[simard] guard: failed to run the gate script (control)")
        .success();

    let _ = fs::remove_dir_all(&tmp);

    assert!(
        rejected,
        "[simard] Rust-only gate did NOT reject `tests/leak.py` / `docs/leak.py`. The gate only \
         inspects src/ and python/, so Python anywhere else slips through — the exact blind spot \
         that let the grandfathered Python survive. Harden it to reject `.py` anywhere \
         (allow-list empty) so none can return (issue #3181)."
    );
    assert!(
        control_ok,
        "[simard] Rust-only gate rejected an all-Rust tree — the hardening must ban Python \
         specifically, not fail unconditionally."
    );
}

#[test]
fn no_kuzu_in_rust_code() {
    let mut stragglers: Vec<String> = Vec::new();
    for path in git_tracked_files() {
        if !path.ends_with(".rs") || basename(&path) == SELF {
            continue;
        }
        for (idx, line) in read_tracked(&path).lines().enumerate() {
            if line.to_ascii_lowercase().contains("kuzu") {
                stragglers.push(format!("{}:{}:{}", path, idx + 1, line.trim()));
            }
        }
    }

    assert!(
        stragglers.is_empty(),
        "[simard] {} Rust line(s) still reference `kuzu`. Simard's graph store is the embedded \
         `lbug` (LadybugDB) crate, never a Python `kuzu` package — remove the dependency check \
         and reword historical comments to LadybugDB / `lbug` (issue #3181):\n{}",
        stragglers.len(),
        stragglers.join("\n")
    );
}

#[test]
fn no_kuzu_in_ci_and_gate_scripts() {
    // Regression guard (currently green): the CI + gate mechanism must never
    // name `kuzu`, so a Python-`kuzu` dependency can never creep back into a
    // gate.
    let mut stragglers: Vec<String> = Vec::new();
    for path in git_tracked_files() {
        let in_scope = path.starts_with(".github/") || path.starts_with("scripts/");
        if !in_scope {
            continue;
        }
        for (idx, line) in read_tracked(&path).lines().enumerate() {
            if line.to_ascii_lowercase().contains("kuzu") {
                stragglers.push(format!("{}:{}:{}", path, idx + 1, line.trim()));
            }
        }
    }

    assert!(
        stragglers.is_empty(),
        "[simard] {} CI/gate line(s) reference `kuzu`. The gate mechanism must be kuzu-free:\n{}",
        stragglers.len(),
        stragglers.join("\n")
    );
}

#[test]
fn no_python_process_spawn_in_rust() {
    // No Rust code may spawn a Python interpreter or treat one as a runtime
    // dependency. `SubprocessRpcTransport` (dead — only its own #[cfg(test)]
    // tests construct it) spawns `python3`; `cmd_ensure_deps` checks for a
    // `python3` binary and a `kuzu` Python package. All must go.
    let needles = [
        "Command::new(\"python",
        "check_binary(\"python3\"",
        "check_python_package",
    ];
    let mut stragglers: Vec<String> = Vec::new();
    for path in git_tracked_files() {
        if !path.ends_with(".rs") || basename(&path) == SELF {
            continue;
        }
        for (idx, line) in read_tracked(&path).lines().enumerate() {
            if needles.iter().any(|n| line.contains(n)) {
                stragglers.push(format!("{}:{}:{}", path, idx + 1, line.trim()));
            }
        }
    }

    assert!(
        stragglers.is_empty(),
        "[simard] {} Rust line(s) spawn or require a Python runtime. Remove the dead \
         `SubprocessRpcTransport` python3 spawn and the `cmd_ensure_deps` python3 / kuzu checks \
         (production uses NativeRpcTransport; the store is the embedded `lbug` crate) — \
         issue #3181:\n{}",
        stragglers.len(),
        stragglers.join("\n")
    );
}

#[test]
fn native_git_hooks_present_and_python_free() {
    // The Python `pre-commit` framework must be replaced by committed git hooks
    // that run `cargo` directly. `core.hooksPath` is wired to `hooks/`; the
    // standard local gates live in `hooks/pre-commit` and `hooks/pre-push`.
    let mut problems: Vec<String> = Vec::new();
    for hook in ["hooks/pre-commit", "hooks/pre-push"] {
        let path = repo_root().join(hook);
        if !path.exists() {
            problems.push(format!(
                "missing: {hook} (commit a native git hook that runs cargo directly)"
            ));
            continue;
        }
        let body = fs::read_to_string(&path).unwrap_or_default();
        if body.trim().is_empty() {
            problems.push(format!("empty: {hook}"));
            continue;
        }
        if !body.contains("cargo") {
            problems.push(format!(
                "{hook} does not run `cargo` — it must gate via cargo directly"
            ));
        }
        for (idx, line) in body.lines().enumerate() {
            if line_invokes_python(line) {
                problems.push(format!(
                    "{hook}:{}: invokes Python: {}",
                    idx + 1,
                    line.trim()
                ));
            }
        }
        // The hook must not delegate back to the Python pre-commit framework.
        if body.contains("pre_commit") || body.contains("pre-commit run") {
            problems.push(format!(
                "{hook} delegates to the Python `pre-commit` framework"
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "[simard] Native git hooks are not in place (issue #3181). Local commit/push gating must \
         be preserved WITHOUT the Python pre-commit framework — committed `hooks/pre-commit` and \
         `hooks/pre-push` running cargo directly, wired via `core.hooksPath`:\n{}",
        problems.join("\n")
    );
}

#[test]
fn python_precommit_framework_config_removed() {
    // `.pre-commit-config.yaml` is the config of the Python `pre-commit` tool;
    // keeping it implies keeping the Python framework that reads it.
    let cfg = repo_root().join(".pre-commit-config.yaml");
    assert!(
        !cfg.exists(),
        "[simard] `.pre-commit-config.yaml` still present — it drives the Python `pre-commit` \
         framework. Remove it and gate via committed native git hooks + direct cargo CI steps \
         (issue #3181)."
    );
}

#[test]
fn classifier_flags_python_invocations() {
    for line in [
        "      - uses: actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065 # v5",
        "        run: python -m pip install --upgrade pip pre-commit",
        "          python -m pre_commit run --all-files --hook-stage pre-commit",
        "          pip install -r tests/e2e-dashboard/smoke_python/requirements.txt",
        "          python -m playwright install chromium",
        "          pytest tests/e2e-dashboard/smoke_python/ -v",
        "  python3 -m pip install --user --upgrade \"pre-commit>=3.7\"",
        "  pipx install \"pre-commit>=3.7\"",
        "  info \"install with: pip install mkdocs-material pymdown-extensions\"",
    ] {
        assert!(
            line_invokes_python(line),
            "classifier failed to flag a real Python invocation: {line}"
        );
    }
}

#[test]
fn classifier_ignores_prose_and_rust_only_gate_lines() {
    // Legitimate prose / history / gate-internals that mention "Python" or
    // ".py" but do NOT invoke a Python runtime must never be flagged — else the
    // guard would forbid the Rust-only gate from describing what it bans, or
    // forbid honest historical comments.
    for line in [
        "# ── Check for prohibited Python files ──",
        "  echo \"New .py files under src/ or python/\"",
        "  if [[ \"$file\" == src/*.py || \"$file\" == python/*.py ]]; then",
        "    /// backends (legacy Python memory, IPC client, test mocks) keep working",
        "                fact(\"python asyncio\", \"event loop\"),",
        "//! never a Python `kuzu` package.",
    ] {
        assert!(
            !line_invokes_python(line),
            "classifier wrongly flagged a non-invocation line: {line}"
        );
    }
}
