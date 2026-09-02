//! Regression guard: SHA-pinned GitHub Actions that Dependabot must be able to
//! resolve must stay Dependabot-resolvable — specifically
//! `taiki-e/install-action` and `dtolnay/rust-toolchain`.
//!
//! Background — a real default-branch CI failure this test prevents from
//! recurring: the repository SHA-pins GitHub Actions for supply-chain
//! hardening. GitHub's Dependabot `github_actions` updater shallow-clones each
//! action's **default branch** to look for newer versions, so a pin whose
//! commit is *not reachable from that default branch* aborts the whole
//! `dependabot/dependabot-updates` run with `error: no such commit ...`.
//!
//! Two actions in this repo hit that trap:
//!
//!   * `taiki-e/install-action` was pinned to the action's **per-tool tags**
//!     (`# cargo-audit`, `# cargo-deny`, ...), whose commits live on a lineage
//!     diverged from the default branch:
//!
//!     ```text
//!     Error processing taiki-e/install-action (HelperSubprocessFailed)
//!     error: no such commit 754bf4dbae00ad1b16b244717154b96ba27d2416
//!     ```
//!
//!   * `dtolnay/rust-toolchain` was pinned to the HEAD of dtolnay's per-channel
//!     **branches** (`# stable`, `# nightly`), which are likewise diverged from
//!     the action's default branch — the same `no such commit` failure.
//!
//! Both aborted the `dependabot-updates` run, turning the default branch's
//! Actions health red even though every functional workflow was green. The fix
//! pins each action's reachable **release SHA** (`taiki-e/install-action`'s `v2`
//! release, `dtolnay/rust-toolchain`'s `v1` release) and selects the tool /
//! toolchain channel via the `with:` input, so each pin is both SHA-hardened
//! *and* Dependabot-updatable.
//!
//! This test encodes that invariant as a file-shaped, no-network assertion (an
//! operator running the equivalent `grep` gets the same answer CI does): every
//! `uses: <action>@<sha> # <comment>` in every workflow **and** in-repo
//! composite `action.yml` must
//!   1. SHA-pin (40 hex chars), matching the repo's supply-chain policy, and
//!   2. carry a **version-tag** comment (`# v2`, `# v1`), i.e. a ref reachable
//!      from the action's default branch — never a per-tool/per-channel name.

use std::fs;
use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workflows_dir() -> PathBuf {
    manifest_dir().join(".github").join("workflows")
}

/// Every `.yml`/`.yaml` file under `.github/workflows`.
fn workflow_files() -> Vec<PathBuf> {
    let dir = workflows_dir();
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
    {
        let path = entry.expect("dir entry").path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "yml" || e == "yaml")
        {
            out.push(path);
        }
    }
    out.sort();
    assert!(
        !out.is_empty(),
        "expected at least one workflow under {}",
        dir.display()
    );
    out
}

/// Recursively collect in-repo composite `action.yml`/`action.yaml` definitions
/// under `.github/actions`. Dependabot's `github_actions` updater reads these
/// too, so a diverged pin here fails the update run just like a workflow does.
fn composite_action_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_action_yaml(&manifest_dir().join(".github").join("actions"), &mut out);
    out.sort();
    out
}

fn collect_action_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_action_yaml(&path, out);
        } else if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n == "action.yml" || n == "action.yaml")
        {
            out.push(path);
        }
    }
}

/// Every file Dependabot's `github_actions` updater reads for `uses:` pins:
/// workflow files plus in-repo composite `action.yml` definitions.
fn pin_source_files() -> Vec<PathBuf> {
    let mut files = workflow_files();
    files.extend(composite_action_files());
    files
}

/// A single `uses: <action>@<sha> # <comment>` reference.
struct Pin {
    file: String,
    sha: String,
    comment: String,
}

/// Parse `uses: <action>@<sha> # <comment>` lines for `action` out of a file.
fn parse_pins(path: &Path, action: &str) -> Vec<Pin> {
    let file = path
        .strip_prefix(manifest_dir())
        .unwrap_or(path)
        .display()
        .to_string();
    let contents =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let needle = format!("{action}@");
    let mut pins = Vec::new();
    for raw in contents.lines() {
        let line = raw.trim();
        // Only actual `uses:` step lines, never comments/prose.
        if line.starts_with('#') || !line.contains("uses:") {
            continue;
        }
        let Some(after) = line.split(&needle).nth(1) else {
            continue;
        };
        // `after` == "<sha> # <comment>" (or "<sha>" with no comment).
        let (rev, comment) = match after.split_once('#') {
            Some((rev, comment)) => (rev.trim().to_string(), comment.trim().to_string()),
            None => (after.trim().to_string(), String::new()),
        };
        pins.push(Pin {
            file: file.clone(),
            sha: rev,
            comment,
        });
    }
    pins
}

fn is_hex_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// A version tag is `v` followed by a digit (`v2`, `v2.82.10`) — the shape of a
/// ref reachable from the action's default branch. A per-tool tag comment
/// (`cargo-audit`, `cargo-deny`, ...) is exactly what breaks Dependabot.
fn is_version_tag_comment(comment: &str) -> bool {
    let mut chars = comment.chars();
    chars.next() == Some('v') && chars.next().is_some_and(|c| c.is_ascii_digit())
}

#[test]
fn taiki_install_action_pins_are_dependabot_resolvable() {
    let mut pins = Vec::new();
    for f in pin_source_files() {
        pins.extend(parse_pins(&f, "taiki-e/install-action"));
    }

    assert!(
        !pins.is_empty(),
        "expected at least one taiki-e/install-action pin under .github/workflows \
         (the cargo-audit/cargo-deny/cargo-vet/cargo-llvm-cov/cargo-cyclonedx jobs); \
         found none — did a job get renamed or removed?"
    );

    for pin in &pins {
        assert!(
            is_hex_sha(&pin.sha),
            "{}: taiki-e/install-action must be SHA-pinned (40 hex chars) for \
             supply-chain hardening, got `{}`.",
            pin.file,
            pin.sha
        );
        assert!(
            is_version_tag_comment(&pin.comment),
            "{}: taiki-e/install-action@{} must carry a VERSION-tag comment \
             (e.g. `# v2` / `# v2.82.10`), not `# {}`. Per-tool tags \
             (`cargo-audit`, `cargo-deny`, ...) resolve to commits diverged from \
             the action's default branch, which breaks Dependabot's \
             github_actions updater (`no such commit ...`) and fails the \
             default-branch dependabot-updates run. Pin the `v2` release SHA and \
             pass the tool via the `with: tool:` input instead.",
            pin.file,
            pin.sha,
            if pin.comment.is_empty() {
                "<no comment>"
            } else {
                &pin.comment
            }
        );
    }
}

#[test]
fn dtolnay_rust_toolchain_pins_are_dependabot_resolvable() {
    let mut pins = Vec::new();
    for f in pin_source_files() {
        pins.extend(parse_pins(&f, "dtolnay/rust-toolchain"));
    }

    assert!(
        !pins.is_empty(),
        "expected at least one dtolnay/rust-toolchain pin under .github/workflows \
         or .github/actions (the coverage/release jobs and the rust-runner-prep \
         composite action); found none — did a job get renamed or removed?"
    );

    for pin in &pins {
        assert!(
            is_hex_sha(&pin.sha),
            "{}: dtolnay/rust-toolchain must be SHA-pinned (40 hex chars) for \
             supply-chain hardening, got `{}`.",
            pin.file,
            pin.sha
        );
        assert!(
            is_version_tag_comment(&pin.comment),
            "{}: dtolnay/rust-toolchain@{} must carry a VERSION-tag comment \
             (e.g. `# v1`), not `# {}`. dtolnay maintains per-channel *branches* \
             (`stable`, `nightly`, `beta`, ...) whose HEAD commits are diverged \
             from the action's default branch, so pinning them breaks \
             Dependabot's github_actions updater (`no such commit ...`) and fails \
             the default-branch dependabot-updates run. Pin the `v1` release SHA \
             (reachable from the default branch) and select the channel via the \
             `with: toolchain:` input instead.",
            pin.file,
            pin.sha,
            if pin.comment.is_empty() {
                "<no comment>"
            } else {
                &pin.comment
            }
        );
    }
}
