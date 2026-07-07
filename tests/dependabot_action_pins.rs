//! Regression guard: `taiki-e/install-action` pins must stay
//! Dependabot-resolvable.
//!
//! Background — a real default-branch CI failure this test prevents from
//! recurring: the repository SHA-pins GitHub Actions for supply-chain
//! hardening. For `taiki-e/install-action` the pins originally targeted the
//! action's **per-tool tags** (`# cargo-audit`, `# cargo-deny`, ...). Those tag
//! commits live on a lineage *diverged* from the action's default branch, so
//! GitHub's Dependabot `github_actions` updater could not resolve them when it
//! shallow-clones the action to look for newer versions:
//!
//! ```text
//! Error processing taiki-e/install-action (HelperSubprocessFailed)
//! error: no such commit 754bf4dbae00ad1b16b244717154b96ba27d2416
//! ```
//!
//! That aborted the whole `dependabot/dependabot-updates` run, turning the
//! default branch's Actions health red even though every functional workflow
//! was green. The fix pins the reachable `v2` release SHA and selects the tool
//! via the `with: tool:` input, so the pin is both SHA-hardened *and*
//! Dependabot-updatable.
//!
//! This test encodes that invariant as a file-shaped, no-network assertion (an
//! operator running the equivalent `grep` gets the same answer CI does): every
//! `uses: taiki-e/install-action@<sha> # <comment>` in every workflow must
//!   1. SHA-pin (40 hex chars), matching the repo's supply-chain policy, and
//!   2. carry a **version-tag** comment (`# v2`, `# v2.82.10`), i.e. a ref
//!      reachable from the action's default branch — never a per-tool tag name.

use std::fs;
use std::path::{Path, PathBuf};

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows")
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

/// A single `uses: taiki-e/install-action@<sha> # <comment>` reference.
struct Pin {
    file: String,
    sha: String,
    comment: String,
}

/// Parse `taiki-e/install-action` `uses:` lines out of a workflow file.
fn parse_install_action_pins(path: &Path) -> Vec<Pin> {
    let file = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<unknown>")
        .to_string();
    let contents =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let mut pins = Vec::new();
    for raw in contents.lines() {
        let line = raw.trim();
        // Only actual `uses:` step lines, never comments/prose.
        if line.starts_with('#') || !line.contains("uses:") {
            continue;
        }
        let Some(after) = line.split("taiki-e/install-action@").nth(1) else {
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
    for f in workflow_files() {
        pins.extend(parse_install_action_pins(&f));
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
