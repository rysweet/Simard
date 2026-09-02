//! TDD contract (Step 7) for Problem 1's apply artifact — the idempotent
//! `scripts/enable-merge-queue.sh` script that enables GitHub's native merge
//! queue and relaxes the strict up-to-date-before-merge requirement (issue
//! #1050).
//!
//! `main` is managed by external branch protection (not settings-as-code in
//! this repo), so CI cannot self-prove queue enablement. The accepted in-repo
//! artifact is this documented, idempotent apply script. Its operator contract
//! is fixed in docs/howto/merge-queue.md:
//!
//!   Flags:   --repo <owner/name>  --branch <name>  --dry-run  -h|--help
//!   Exit:    0 success/dry-run/help · 1 generic · 2 bad args · 3 no admin (403)
//!   Safety:  never echoes/logs a token; validates --repo/--branch against
//!            ^[A-Za-z0-9._/-]+$; pins X-GitHub-Api-Version: 2022-11-28;
//!            --dry-run prints only the HTTP method + path and writes nothing.
//!
//! These tests exercise that contract as no-network, no-auth assertions (the
//! `--help` and `--dry-run` paths must not require GitHub credentials). They
//! FAIL until scripts/enable-merge-queue.sh exists and honors the contract.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn script_path() -> PathBuf {
    repo_root().join("scripts").join("enable-merge-queue.sh")
}

fn read_script() -> String {
    let p = script_path();
    std::fs::read_to_string(&p).unwrap_or_else(|e| {
        panic!(
            "scripts/enable-merge-queue.sh must exist ({}): {e}. It is the \
             documented apply artifact for the merge-queue remediation (issue \
             #1050).",
            p.display()
        )
    })
}

/// Run the script with the given args, returning its captured output. Panics
/// with a TDD-oriented message if the script cannot be spawned at all.
fn run(args: &[&str]) -> Output {
    Command::new(script_path())
        .args(args)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "failed to execute scripts/enable-merge-queue.sh {args:?}: {e}. \
                 The script must exist and be executable (issue #1050)."
            )
        })
}

#[test]
fn script_exists_and_is_executable() {
    let p = script_path();
    assert!(
        p.is_file(),
        "scripts/enable-merge-queue.sh must exist — the documented apply \
         artifact that enables the merge queue and relaxes strict \
         up-to-date-before-merge (issue #1050)."
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&p)
            .expect("stat enable-merge-queue.sh")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "scripts/enable-merge-queue.sh must be executable (mode {mode:o}); \
             the docs invoke it directly as `scripts/enable-merge-queue.sh`."
        );
    }
}

#[test]
fn script_uses_strict_bash_mode() {
    let body = read_script();
    assert!(
        body.contains("set -euo pipefail"),
        "scripts/enable-merge-queue.sh must use `set -euo pipefail` so a failed \
         gh call or unset variable aborts loudly instead of silently no-op'ing."
    );
}

#[test]
fn help_flag_prints_usage_and_exits_zero() {
    let out = run(&["--help"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "`--help` must exit 0 (documented exit-code contract). stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let lc = combined.to_lowercase();
    assert!(
        lc.contains("usage") || lc.contains("--dry-run"),
        "`--help` must print usage mentioning the flags. Got:\n{combined}"
    );
}

#[test]
fn unknown_flag_exits_two() {
    // Documented exit code 2 = invalid arguments / input validation failure.
    let out = run(&["--definitely-not-a-flag"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an unknown flag must exit 2 (invalid arguments), got {:?}. stderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn invalid_repo_value_is_rejected_with_exit_two() {
    // --repo is validated against ^[A-Za-z0-9._/-]+$; a value with a shell
    // metacharacter must be rejected (exit 2), never interpolated into a call.
    let out = run(&["--repo", "bad;rm -rf /", "--dry-run"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an invalid --repo value must be rejected with exit 2 (input \
         validation), got {:?}. stderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn dry_run_writes_nothing_and_prints_method_and_path() {
    // --dry-run must not require auth or the network: it prints the HTTP method
    // + path that WOULD be called and exits 0 without writing.
    let out = run(&["--dry-run", "--repo", "rysweet/Simard", "--branch", "main"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "`--dry-run` must exit 0 without writing or needing auth. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("rulesets"),
        "`--dry-run` must print the API path it would call (…/rulesets for the \
         required_merge_queue ruleset). Got:\n{combined}"
    );
    let has_method = ["POST", "PUT", "PATCH", "GET"]
        .iter()
        .any(|m| combined.contains(m));
    assert!(
        has_method,
        "`--dry-run` must print the HTTP method it would use. Got:\n{combined}"
    );
}

#[test]
fn script_targets_merge_queue_ruleset_and_relaxes_strict() {
    let body = read_script();
    assert!(
        body.contains("required_merge_queue"),
        "the script must configure the native merge queue via the \
         `required_merge_queue` ruleset rule."
    );
    assert!(
        body.contains("rulesets"),
        "the script must call the GitHub rulesets API to create/update the \
         merge-queue ruleset."
    );
    assert!(
        body.contains("strict"),
        "the script must relax the strict up-to-date-before-merge requirement \
         (set `strict: false`) so the queue's freshness guarantee replaces it."
    );
}

#[test]
fn script_pins_github_api_version() {
    let body = read_script();
    assert!(
        body.contains("X-GitHub-Api-Version: 2022-11-28"),
        "the script must pin `X-GitHub-Api-Version: 2022-11-28` so a future API \
         default can't silently change behavior."
    );
}

#[test]
fn script_validates_repo_and_branch_inputs() {
    let body = read_script();
    assert!(
        body.contains("A-Za-z0-9._/-"),
        "the script must regex-validate --repo/--branch against \
         `^[A-Za-z0-9._/-]+$` before using them in any API call."
    );
}

#[test]
fn script_never_echoes_or_logs_a_token() {
    // Least-astonishment safety guard: the token must never be echoed, printed,
    // or logged. Catch the obvious footguns as a source-shaped assertion.
    let body = read_script();
    for bad in [
        "echo $GITHUB_TOKEN",
        "echo ${GITHUB_TOKEN}",
        "echo $GH_TOKEN",
    ] {
        assert!(
            !body.contains(bad),
            "the script must never echo/log a token (found `{bad}`)."
        );
    }
}

#[test]
fn script_passes_shellcheck() {
    // Skip gracefully where shellcheck isn't installed (e.g. minimal CI images);
    // where present it must be clean.
    let probe = Command::new("shellcheck").arg("--version").output();
    if probe.is_err() {
        eprintln!("shellcheck not installed; skipping enable-merge-queue.sh lint");
        return;
    }
    let out = Command::new("shellcheck")
        .arg(script_path())
        .output()
        .expect("run shellcheck");
    assert!(
        out.status.success(),
        "shellcheck must pass on scripts/enable-merge-queue.sh.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
