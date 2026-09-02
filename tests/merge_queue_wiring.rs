//! TDD contract (Step 7) for Problem 1 — the merge-queue live-lock remediation
//! (issue #1050).
//!
//! Background — `main` branch protection used strict "up-to-date-before-merge"
//! (`required_status_checks.strict = true`) across its required contexts. The
//! required CI matrix takes ~35 min while `main` advances roughly every 30 min,
//! so a PR can never satisfy the strict freshness gate before a new commit
//! lands — starving the backlog into a merge **live-lock**. The remediation is
//! GitHub's native merge queue: the required CI must also run on the
//! `merge_group` event so the same required contexts execute against the
//! queued, freshly-merged result instead of relying on `strict`.
//!
//! These are file-shaped, no-network assertions (an operator running the
//! equivalent `grep` gets the same answer CI does). They encode the
//! ADDITIVE / NON-BREAKING contract:
//!
//! * `verify.yml` (the required-context workflow) must add a `merge_group:`
//!   trigger **without** removing `push:`/`pull_request:` or renaming/removing
//!   any of the eight required jobs (renaming a job renames its required
//!   status-check context and would silently break required-context
//!   enforcement).
//! * `coverage.yml` must also gain a `merge_group:` trigger so it runs in the
//!   queue context.
//! * No job may gate on `github.event.pull_request.*`, which is `null` under
//!   the `merge_group` payload and would let unverified code merge.
//! * The how-to doc must exist, link issue #1050, and reference the apply
//!   script.
//!
//! They FAIL until the `merge_group` triggers, the doc, and the script are in
//! place, then PASS once the implementation lands.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The `on:` block of a workflow file: everything from the top-level `on:` line
/// up to (but not including) the next top-level key (`permissions:`, `env:`,
/// `jobs:`, …). Used so trigger assertions can't be satisfied by an unrelated
/// occurrence of the word deeper in the file.
fn on_block(yaml: &str) -> String {
    let mut out = String::new();
    let mut in_on = false;
    for line in yaml.lines() {
        if line.starts_with("on:") {
            in_on = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_on {
            // A new top-level key (no leading whitespace, ends the on: block).
            let is_top_level_key =
                !line.is_empty() && !line.starts_with([' ', '\t']) && line.contains(':');
            if is_top_level_key {
                break;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The eight required jobs whose status-check contexts gate `main`. Renaming
/// any of these renames its required context and silently breaks enforcement,
/// so the merge-queue change must keep every one of them.
const REQUIRED_VERIFY_JOBS: [&str; 8] = [
    "pre-commit",
    "cargo-audit",
    "cargo-deny",
    "cargo-vet",
    "npm-audit",
    "scripts-tests",
    "install-real",
    "e2e-dashboard",
];

#[test]
fn verify_workflow_has_merge_group_trigger() {
    let yaml = read(&repo_root().join(".github/workflows/verify.yml"));
    let on = on_block(&yaml);
    assert!(
        on.contains("merge_group:"),
        "verify.yml `on:` block must add a `merge_group:` trigger so the eight \
         required contexts run in the GitHub merge queue and resolve the strict \
         up-to-date live-lock (issue #1050). `on:` block was:\n{on}"
    );
}

#[test]
fn verify_workflow_keeps_pull_request_and_push_triggers() {
    // ADDITIVE / NON-BREAKING: adding merge_group must not remove the existing
    // triggers, or ordinary PR / branch CI would stop running.
    let yaml = read(&repo_root().join(".github/workflows/verify.yml"));
    let on = on_block(&yaml);
    assert!(
        on.contains("pull_request:"),
        "verify.yml must keep its `pull_request:` trigger (additive change). on:\n{on}"
    );
    assert!(
        on.contains("push:"),
        "verify.yml must keep its `push:` trigger (additive change). on:\n{on}"
    );
}

#[test]
fn verify_workflow_keeps_all_required_job_contexts() {
    // Check-name stability: the required status-check context IS the job id.
    // Renaming/removing a job breaks required-context enforcement even though
    // the workflow still "runs". Every required job must remain, spelled
    // exactly, as a top-level `  <job>:` key under `jobs:`.
    let yaml = read(&repo_root().join(".github/workflows/verify.yml"));
    for job in REQUIRED_VERIFY_JOBS {
        let needle = format!("\n  {job}:");
        assert!(
            yaml.contains(&needle),
            "verify.yml must keep required job `{job}` (its status-check context \
             gates `main`). The merge-queue change is additive and must not \
             rename or remove it."
        );
    }
}

#[test]
fn verify_workflow_does_not_gate_on_pull_request_payload() {
    // Under the merge_group event, `github.event.pull_request` is null. A job
    // that gates on it (e.g. `if: github.event.pull_request.draft == false`)
    // would evaluate unexpectedly in the queue and could let unverified code
    // through. Keep the required workflow free of pull_request-payload gating.
    let yaml = read(&repo_root().join(".github/workflows/verify.yml"));
    assert!(
        !yaml.contains("github.event.pull_request"),
        "verify.yml must not gate on `github.event.pull_request.*` — it is null \
         under the merge_group payload and would break gating in the queue. \
         Guard on `github.event_name` instead if event-specific logic is needed."
    );
}

#[test]
fn coverage_workflow_has_merge_group_trigger() {
    let yaml = read(&repo_root().join(".github/workflows/coverage.yml"));
    let on = on_block(&yaml);
    assert!(
        on.contains("merge_group:"),
        "coverage.yml `on:` block must add a `merge_group:` trigger so coverage \
         runs consistently in the queue context (issue #1050). on:\n{on}"
    );
}

#[test]
fn merge_queue_howto_doc_exists_and_links_issue_and_script() {
    let doc = repo_root().join("docs/howto/merge-queue.md");
    assert!(
        doc.is_file(),
        "docs/howto/merge-queue.md must exist — it documents queue enablement, \
         the apply script, and the external branch-protection management for \
         issue #1050."
    );
    let body = read(&doc);
    assert!(
        body.contains("#1050") || body.contains("issues/1050"),
        "docs/howto/merge-queue.md must link issue #1050."
    );
    assert!(
        body.contains("scripts/enable-merge-queue.sh"),
        "docs/howto/merge-queue.md must reference the apply script \
         scripts/enable-merge-queue.sh."
    );
}

#[test]
fn merge_queue_howto_is_wired_into_mkdocs_nav() {
    // A how-to that isn't in the nav is undiscoverable (and the docs-integrity
    // gate treats it as an orphan).
    let nav = read(&repo_root().join("mkdocs.yml"));
    assert!(
        nav.contains("howto/merge-queue.md"),
        "mkdocs.yml nav must list howto/merge-queue.md so the merge-queue how-to \
         is discoverable."
    );
}
