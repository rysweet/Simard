//! Regression guard: the flaky-apt hardening step must stay wired into every CI
//! job that runs `apt-get update`, and must run *before* it.
//!
//! Background — a real default-branch CI failure this test prevents from
//! recurring (issue #2975): GitHub-hosted runner images ship
//! `packages.microsoft.com` apt sources that intermittently serve an invalid
//! `InRelease` file:
//!
//! ```text
//! E: Failed to fetch https://packages.microsoft.com/.../InRelease
//!    Clearsigned file isn't valid, got 'NOSPLIT' ...
//! E: The repository '... noble InRelease' is no longer signed.
//! ```
//!
//! When that happens every `apt-get update` exits 100, turning the default
//! branch's Actions health red even though nothing in our code changed. The fix
//! (`scripts/ci-harden-apt.sh`) strips those Microsoft sources before any
//! `apt-get update`. This test encodes the operator-visible contract as a
//! file-shaped, no-network assertion (an operator running the equivalent `grep`
//! gets the same answer CI does): each apt-consuming job wires the hardening
//! step, and the hardening step precedes the apt consumer in the file.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Byte offset of the first occurrence of `needle` in `haystack`, or panics with
/// a descriptive message tying the miss back to the CI-health invariant.
fn require_index(haystack: &str, needle: &str, file: &str, why: &str) -> usize {
    haystack.find(needle).unwrap_or_else(|| {
        panic!("{file}: expected to find `{needle}` ({why}). The flaky-apt hardening (issue #2975) must stay wired here.")
    })
}

#[test]
fn harden_script_exists_and_is_marked_executable() {
    let script = repo_root().join("scripts").join("ci-harden-apt.sh");
    assert!(
        script.is_file(),
        "scripts/ci-harden-apt.sh must exist — it strips the flaky \
         packages.microsoft.com apt sources before every CI `apt-get update` (issue #2975)."
    );

    // The workflows invoke it as `bash <script>`, so a missing +x bit would not
    // break CI, but we keep the executable bit as documented intent and so an
    // operator can run it directly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&script)
            .expect("stat ci-harden-apt.sh")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "scripts/ci-harden-apt.sh should be executable (mode {:o}).",
            mode
        );
    }

    let body = read(&script);
    assert!(
        body.contains("packages\\.microsoft\\.com") || body.contains("packages.microsoft.com"),
        "ci-harden-apt.sh must target packages.microsoft.com sources."
    );
}

#[test]
fn rust_runner_prep_hardens_apt_before_mold_install() {
    let path = repo_root()
        .join(".github")
        .join("actions")
        .join("rust-runner-prep")
        .join("action.yml");
    let yml = read(&path);

    let harden = require_index(
        &yml,
        "scripts/ci-harden-apt.sh",
        "rust-runner-prep/action.yml",
        "the apt-hardening step invocation",
    );
    // The mold install step is the `apt-get update` consumer in this action.
    // Match the real command (`sudo apt-get update`), not the `apt-get update`
    // mention inside the hardening step's own comment.
    let apt_update = require_index(
        &yml,
        "sudo apt-get update",
        "rust-runner-prep/action.yml",
        "the mold install `sudo apt-get update`",
    );
    assert!(
        harden < apt_update,
        "rust-runner-prep/action.yml: the ci-harden-apt.sh step must run BEFORE \
         `apt-get update` (else the flaky Microsoft source still breaks the update)."
    );
}

#[test]
fn verify_e2e_dashboard_hardens_apt_before_playwright_deps() {
    let path = repo_root()
        .join(".github")
        .join("workflows")
        .join("verify.yml");
    let yml = read(&path);

    let harden = require_index(
        &yml,
        "scripts/ci-harden-apt.sh",
        "verify.yml",
        "the apt-hardening step invocation",
    );
    // `playwright install --with-deps` is the `apt-get update` consumer here.
    // Match the real run command (`npx playwright install --with-deps`), not the
    // mention inside the hardening step's own comment.
    let playwright = require_index(
        &yml,
        "npx playwright install --with-deps",
        "verify.yml",
        "the Playwright with-deps install (runs apt-get update)",
    );
    assert!(
        harden < playwright,
        "verify.yml: the ci-harden-apt.sh step must run BEFORE \
         `playwright install --with-deps` (which runs apt-get update)."
    );
}
