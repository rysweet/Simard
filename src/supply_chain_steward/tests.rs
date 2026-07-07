//! TDD tests for the supply-chain advisory remediation reasoner (issue #2741).
//!
//! These tests are written **first** and pin the contract of the pure
//! [`decide`] function — the deterministic rail described in
//! `docs/reference/supply-chain-advisory-stewardship.md`.
//!
//! Coverage mirrors the design's decision table and the task's mandated cases:
//! - patched-version-available → `Bump`
//! - no fix + not yet ignored → `JustifiedIgnore` (with a tracker-bound reason)
//! - **never silent-suppress**: a *fixable* advisory never becomes an ignore
//! - fix exists but unresolvable / behind a git dep → `Escalate`
//! - no fix + already ignored → `NoAction`
//! - stale ignore (fix has since shipped) → `Bump` / `Escalate`, never `NoAction`
//! - determinism / purity

use super::decide::decide;
use super::types::{Advisory, Decision, PatchStatus, RemediationContext};

// ───────────────────────────── builders ─────────────────────────────

/// An advisory WITH a fixed upstream release (`versions.patched` present).
fn advisory_with_fix(id: &str, krate: &str, installed: &str, req: &str) -> Advisory {
    Advisory {
        id: id.to_string(),
        crate_name: krate.to_string(),
        installed: installed.to_string(),
        patched: PatchStatus::Fixed {
            requirement: req.to_string(),
        },
        title: format!("{krate}: vulnerability {id}"),
        url: format!("https://rustsec.org/advisories/{id}"),
    }
}

/// An advisory with NO fixed upstream release.
fn advisory_no_fix(id: &str, krate: &str, installed: &str) -> Advisory {
    Advisory {
        id: id.to_string(),
        crate_name: krate.to_string(),
        installed: installed.to_string(),
        patched: PatchStatus::None,
        title: format!("{krate}: vulnerability {id}"),
        url: format!("https://rustsec.org/advisories/{id}"),
    }
}

/// Context where a patched version resolves cleanly against the lockfile.
fn ctx_resolvable(to: &str) -> RemediationContext {
    RemediationContext {
        resolvable_patch: Some(to.to_string()),
        behind_git_dep: false,
        already_ignored: false,
    }
}

// ───────────────────────── mandated case 1 ──────────────────────────
// patched-version-available → Bump

#[test]
fn patched_version_available_yields_minimal_bump() {
    // RUSTSEC-2026-0204 says "upgrade to >= 0.9.20"; 0.9.20 resolves cleanly.
    let adv = advisory_with_fix(
        "RUSTSEC-2026-0204",
        "crossbeam-epoch",
        "0.9.18",
        ">= 0.9.20",
    );
    let ctx = ctx_resolvable("0.9.20");

    let decision = decide(&adv, &ctx);

    assert_eq!(
        decision,
        Decision::Bump {
            crate_name: "crossbeam-epoch".to_string(),
            from: "0.9.18".to_string(),
            to: "0.9.20".to_string(),
        },
        "a resolvable patched version must yield the minimal bump"
    );
}

#[test]
fn bump_carries_installed_as_from_and_resolvable_as_to() {
    let adv = advisory_with_fix("RUSTSEC-2025-0001", "widget", "1.2.3", ">= 1.2.5");
    let ctx = ctx_resolvable("1.2.5");

    match decide(&adv, &ctx) {
        Decision::Bump {
            crate_name,
            from,
            to,
        } => {
            assert_eq!(crate_name, "widget");
            assert_eq!(
                from, "1.2.3",
                "`from` must be the installed lockfile version"
            );
            assert_eq!(to, "1.2.5", "`to` must be the resolvable patched version");
        }
        other => panic!("expected Bump, got {other:?}"),
    }
}

// ───────────────────────── mandated case 2 ──────────────────────────
// no fix + not yet ignored → JustifiedIgnore (with a tracker-bound reason)

#[test]
fn no_fix_and_not_yet_ignored_yields_justified_ignore() {
    let adv = advisory_no_fix("RUSTSEC-2023-0071", "rsa", "0.9.6");
    let ctx = RemediationContext {
        resolvable_patch: None,
        behind_git_dep: false,
        already_ignored: false,
    };

    match decide(&adv, &ctx) {
        Decision::JustifiedIgnore {
            advisory_id,
            crate_name,
            reason,
        } => {
            assert_eq!(advisory_id, "RUSTSEC-2023-0071");
            assert_eq!(crate_name, "rsa");
            // The reason must name the advisory + crate and state the deterministic
            // fact that makes an ignore legitimate here: no upstream fix. The
            // execution layer appends the tracking-issue URL before writing it.
            assert!(
                reason.contains("RUSTSEC-2023-0071"),
                "reason must name the advisory: {reason}"
            );
            assert!(
                reason.contains("rsa"),
                "reason must name the crate: {reason}"
            );
            assert!(
                reason.to_lowercase().contains("no fixed upstream")
                    || reason.to_lowercase().contains("no upstream fix"),
                "reason must state that no upstream fix exists: {reason}"
            );
            assert!(
                reason.to_lowercase().contains("track"),
                "reason must reference tracking for remediation: {reason}"
            );
        }
        other => panic!("expected JustifiedIgnore, got {other:?}"),
    }
}

// ───────────────────────── mandated case 3 ──────────────────────────
// HARD RAIL: never silent-suppress a *fixable* advisory.

#[test]
fn fixable_advisory_is_never_justified_ignored_across_all_contexts() {
    let adv = advisory_with_fix(
        "RUSTSEC-2026-0204",
        "crossbeam-epoch",
        "0.9.18",
        ">= 0.9.20",
    );

    // Exhaustively vary the context flags. For EVERY combination, a fixable
    // advisory must map to Bump or Escalate — never JustifiedIgnore, never
    // NoAction (which would be silent suppression / silent skipping of a fix).
    for resolvable in [None, Some("0.9.20".to_string())] {
        for behind_git_dep in [false, true] {
            for already_ignored in [false, true] {
                let ctx = RemediationContext {
                    resolvable_patch: resolvable.clone(),
                    behind_git_dep,
                    already_ignored,
                };
                let decision = decide(&adv, &ctx);
                match decision {
                    Decision::Bump { .. } | Decision::Escalate { .. } => {}
                    Decision::JustifiedIgnore { .. } => panic!(
                        "HARD RAIL VIOLATION: fixable advisory routed to JustifiedIgnore \
                         (resolvable={resolvable:?}, behind_git_dep={behind_git_dep}, \
                         already_ignored={already_ignored})"
                    ),
                    Decision::NoAction => panic!(
                        "HARD RAIL VIOLATION: fixable advisory silently skipped as NoAction \
                         (resolvable={resolvable:?}, behind_git_dep={behind_git_dep}, \
                         already_ignored={already_ignored})"
                    ),
                }
            }
        }
    }
}

#[test]
fn justified_ignore_is_only_ever_produced_from_the_no_fix_branch() {
    // The structural guarantee, stated directly: the sole path to an ignore is
    // (patched == None && !already_ignored).
    let no_fix = advisory_no_fix("RUSTSEC-2099-0001", "orphan", "1.0.0");
    let with_fix = advisory_with_fix("RUSTSEC-2099-0002", "fixable", "1.0.0", ">= 1.1.0");

    // Only the no-fix + not-ignored combo is a JustifiedIgnore.
    assert!(matches!(
        decide(
            &no_fix,
            &RemediationContext {
                resolvable_patch: None,
                behind_git_dep: false,
                already_ignored: false,
            }
        ),
        Decision::JustifiedIgnore { .. }
    ));

    // Any fixable advisory, under any context, is not a JustifiedIgnore.
    for behind_git_dep in [false, true] {
        for already_ignored in [false, true] {
            for resolvable in [None, Some("1.1.0".to_string())] {
                assert!(
                    !matches!(
                        decide(
                            &with_fix,
                            &RemediationContext {
                                resolvable_patch: resolvable.clone(),
                                behind_git_dep,
                                already_ignored,
                            }
                        ),
                        Decision::JustifiedIgnore { .. }
                    ),
                    "fixable advisory must never be a JustifiedIgnore"
                );
            }
        }
    }
}

// ───────────────────────── mandated case 4 (A4) ─────────────────────
// fix exists but cannot be applied here → Escalate (issue only, no PR, no ignore).

#[test]
fn fix_exists_but_not_resolvable_yields_escalate() {
    let adv = advisory_with_fix("RUSTSEC-2025-0009", "tangled", "2.0.0", ">= 3.0.0");
    let ctx = RemediationContext {
        // A fix exists upstream, but no satisfying version resolves against the
        // lockfile's constraints (e.g. a major bump another dep pins away from).
        resolvable_patch: None,
        behind_git_dep: false,
        already_ignored: false,
    };

    match decide(&adv, &ctx) {
        Decision::Escalate {
            advisory_id,
            reason,
        } => {
            assert_eq!(advisory_id, "RUSTSEC-2025-0009");
            assert!(
                reason.to_lowercase().contains("resolve"),
                "escalation reason should explain the unresolvable fix: {reason}"
            );
        }
        other => panic!("expected Escalate, got {other:?}"),
    }
}

#[test]
fn fix_behind_first_party_git_dep_yields_escalate_even_when_resolvable() {
    let adv = advisory_with_fix("RUSTSEC-2025-0010", "moka", "0.12.0", ">= 0.12.8");
    let ctx = RemediationContext {
        // Even with a resolvable patch, a crate reached only behind a first-party
        // git dependency must be bumped upstream, not in Simard's lockfile.
        resolvable_patch: Some("0.12.8".to_string()),
        behind_git_dep: true,
        already_ignored: false,
    };

    match decide(&adv, &ctx) {
        Decision::Escalate {
            advisory_id,
            reason,
        } => {
            assert_eq!(advisory_id, "RUSTSEC-2025-0010");
            assert!(
                reason.to_lowercase().contains("git"),
                "escalation reason should mention the git-dep boundary: {reason}"
            );
        }
        other => panic!("expected Escalate (git-dep), got {other:?}"),
    }
}

// ───────────────────────── mandated case 5 ──────────────────────────
// no fix + already ignored → NoAction (existing justified ignore still valid).

#[test]
fn no_fix_and_already_ignored_yields_no_action() {
    let adv = advisory_no_fix("RUSTSEC-2023-0071", "rsa", "0.9.6");
    let ctx = RemediationContext {
        resolvable_patch: None,
        behind_git_dep: false,
        already_ignored: true,
    };

    assert_eq!(
        decide(&adv, &ctx),
        Decision::NoAction,
        "an already-justified ignore with no upstream fix needs no further action"
    );
}

// ─────────────────── stale-ignore revalidation (A4/design) ──────────
// An advisory previously ignored as "no fix" whose fix has since shipped is
// NOT honoured — it is corrected to a Bump (or Escalate), never NoAction.

#[test]
fn stale_ignore_with_now_resolvable_fix_is_corrected_to_bump() {
    // Exactly the RUSTSEC-2026-0204 shape: once ignored as "no fix", now a fix
    // ships ("upgrade to >= 0.9.20"). The steward must correct, not honour.
    let adv = advisory_with_fix(
        "RUSTSEC-2026-0204",
        "crossbeam-epoch",
        "0.9.18",
        ">= 0.9.20",
    );
    let ctx = RemediationContext {
        resolvable_patch: Some("0.9.20".to_string()),
        behind_git_dep: false,
        already_ignored: true, // stale
    };

    assert_eq!(
        decide(&adv, &ctx),
        Decision::Bump {
            crate_name: "crossbeam-epoch".to_string(),
            from: "0.9.18".to_string(),
            to: "0.9.20".to_string(),
        },
        "a stale ignore whose fix has shipped must be corrected to a Bump, not NoAction"
    );
}

#[test]
fn stale_ignore_with_unresolvable_fix_is_corrected_to_escalate() {
    let adv = advisory_with_fix("RUSTSEC-2026-0300", "stale-crate", "1.0.0", ">= 2.0.0");
    let ctx = RemediationContext {
        resolvable_patch: None,
        behind_git_dep: false,
        already_ignored: true, // stale
    };

    match decide(&adv, &ctx) {
        Decision::Escalate { advisory_id, .. } => assert_eq!(advisory_id, "RUSTSEC-2026-0300"),
        other => {
            panic!("expected Escalate for a stale ignore with an unresolvable fix, got {other:?}")
        }
    }
}

// ───────────────────────── purity / determinism ─────────────────────

#[test]
fn decide_is_deterministic_for_identical_inputs() {
    let adv = advisory_with_fix("RUSTSEC-2025-0100", "det", "1.0.0", ">= 1.0.1");
    let ctx = ctx_resolvable("1.0.1");

    assert_eq!(
        decide(&adv, &ctx),
        decide(&adv, &ctx),
        "decide() must be pure: identical inputs → identical output"
    );
}

#[test]
fn default_context_with_no_fix_yields_justified_ignore() {
    // Sanity-check the RemediationContext::Default (all false / None): with no
    // upstream fix and no prior ignore, that is the JustifiedIgnore path.
    let adv = advisory_no_fix("RUSTSEC-2024-0055", "lonely", "0.1.0");
    assert!(matches!(
        decide(&adv, &RemediationContext::default()),
        Decision::JustifiedIgnore { .. }
    ));
}

// ═══════════════════════════ parse.rs ═══════════════════════════

use super::parse::parse_audit_json;

const SAMPLE_AUDIT_JSON: &str = r#"{
  "vulnerabilities": {
    "found": true,
    "count": 2,
    "list": [
      {
        "advisory": {
          "id": "RUSTSEC-2026-0204",
          "title": "crossbeam-epoch null-pointer deref in Display",
          "url": "https://rustsec.org/advisories/RUSTSEC-2026-0204"
        },
        "versions": { "patched": [">= 0.9.20"], "unaffected": [] },
        "package": { "name": "crossbeam-epoch", "version": "0.9.18" }
      },
      {
        "advisory": { "id": "RUSTSEC-2023-0071", "title": "rsa Marvin attack", "url": null },
        "versions": { "patched": [], "unaffected": [] },
        "package": { "name": "rsa", "version": "0.9.6" }
      }
    ]
  }
}"#;

#[test]
fn parse_audit_json_reads_fixed_and_unfixed_advisories() {
    let advisories = parse_audit_json(SAMPLE_AUDIT_JSON).expect("parse");
    assert_eq!(advisories.len(), 2);

    let fixed = &advisories[0];
    assert_eq!(fixed.id, "RUSTSEC-2026-0204");
    assert_eq!(fixed.crate_name, "crossbeam-epoch");
    assert_eq!(fixed.installed, "0.9.18");
    assert_eq!(
        fixed.patched,
        PatchStatus::Fixed {
            requirement: ">= 0.9.20".to_string()
        }
    );
    assert_eq!(
        fixed.url,
        "https://rustsec.org/advisories/RUSTSEC-2026-0204"
    );

    let unfixed = &advisories[1];
    assert_eq!(unfixed.id, "RUSTSEC-2023-0071");
    assert_eq!(unfixed.patched, PatchStatus::None);
    // Null upstream url falls back to the canonical rustsec URL.
    assert_eq!(
        unfixed.url,
        "https://rustsec.org/advisories/RUSTSEC-2023-0071"
    );
}

#[test]
fn parse_audit_json_empty_when_no_vulnerabilities() {
    let json = r#"{ "vulnerabilities": { "found": false, "count": 0, "list": [] } }"#;
    assert!(parse_audit_json(json).expect("parse").is_empty());
    // Missing `vulnerabilities` key entirely is also fine (defaults to empty).
    assert!(parse_audit_json("{}").expect("parse").is_empty());
}

#[test]
fn parse_audit_json_rejects_malformed() {
    let err = parse_audit_json("not json at all").unwrap_err();
    assert!(matches!(
        err,
        crate::error::SimardError::SupplyChainAuditParseFailed { .. }
    ));
}

// ═══════════════════════════ config.rs ═══════════════════════════

use super::config::{
    IgnoreFiles, audit_ignored_ids, deny_ignored_ids, insert_audit_ignore, insert_deny_ignore,
    remove_ignore_entry,
};

const DENY_MIN: &str = "[advisories]\n\
db-urls = [\"https://github.com/rustsec/advisory-db\"]\n\
ignore = [\n\
    { id = \"RUSTSEC-2023-0071\", reason = \"rsa Marvin; no fix. Tracked: https://x/19\" },\n\
]\n";

const AUDIT_MIN: &str = "[advisories]\n\
ignore = [\n\
    \"RUSTSEC-2023-0071\",\n\
]\n";

#[test]
fn parses_existing_ignored_ids_from_both_shapes() {
    let deny = deny_ignored_ids(DENY_MIN);
    assert!(deny.contains("RUSTSEC-2023-0071"));
    assert_eq!(deny.len(), 1);

    let audit = audit_ignored_ids(AUDIT_MIN);
    assert!(audit.contains("RUSTSEC-2023-0071"));
    assert_eq!(audit.len(), 1);
}

#[test]
fn insert_deny_ignore_adds_inline_entry_and_is_idempotent() {
    let updated = insert_deny_ignore(
        DENY_MIN,
        "RUSTSEC-2099-0001",
        "no fix. Tracked: https://x/1",
    )
    .unwrap();
    let ids = deny_ignored_ids(&updated);
    assert!(ids.contains("RUSTSEC-2099-0001"));
    assert!(ids.contains("RUSTSEC-2023-0071"));
    // Result still parses as valid TOML.
    toml::from_str::<toml::Value>(&updated).expect("valid toml");
    // Idempotent: inserting again is a no-op.
    let again = insert_deny_ignore(&updated, "RUSTSEC-2099-0001", "x").unwrap();
    assert_eq!(again, updated);
}

#[test]
fn insert_audit_ignore_adds_bare_id_and_is_idempotent() {
    let updated = insert_audit_ignore(
        AUDIT_MIN,
        "RUSTSEC-2099-0001",
        "no fix. Tracked: https://x/1",
    )
    .unwrap();
    let ids = audit_ignored_ids(&updated);
    assert!(ids.contains("RUSTSEC-2099-0001"));
    toml::from_str::<toml::Value>(&updated).expect("valid toml");
    let again = insert_audit_ignore(&updated, "RUSTSEC-2099-0001", "x").unwrap();
    assert_eq!(again, updated);
}

#[test]
fn remove_ignore_entry_drops_entry_and_its_comment_block() {
    let with =
        insert_deny_ignore(DENY_MIN, "RUSTSEC-2099-0001", "x. Tracked: https://x/1").unwrap();
    assert!(deny_ignored_ids(&with).contains("RUSTSEC-2099-0001"));
    let without = remove_ignore_entry(&with, "RUSTSEC-2099-0001");
    assert!(!deny_ignored_ids(&without).contains("RUSTSEC-2099-0001"));
    // The original entry survives; only the targeted one is removed.
    assert!(deny_ignored_ids(&without).contains("RUSTSEC-2023-0071"));
    toml::from_str::<toml::Value>(&without).expect("valid toml");
}

/// Write a minimal repo (deny.toml + .cargo/audit.toml) into `dir`.
fn write_min_repo(dir: &std::path::Path) {
    std::fs::write(dir.join("deny.toml"), DENY_MIN).unwrap();
    std::fs::create_dir_all(dir.join(".cargo")).unwrap();
    std::fs::write(dir.join(".cargo").join("audit.toml"), AUDIT_MIN).unwrap();
}

#[test]
fn ignore_files_add_writes_both_files_in_sync() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    let files = IgnoreFiles::at_root(dir.path());

    assert!(files.ignored_ids_in_sync().unwrap());
    assert!(!files.is_ignored("RUSTSEC-2099-0002").unwrap());

    files
        .add_justified_ignore(
            "RUSTSEC-2099-0002",
            "no fixed upstream release; unreachable in Simard",
            "https://github.com/rysweet/Simard/issues/42",
        )
        .unwrap();

    assert!(files.is_ignored("RUSTSEC-2099-0002").unwrap());
    assert!(
        files.ignored_ids_in_sync().unwrap(),
        "both files must list the identical ignored-ID set"
    );
    // The embedded tracking URL is present in deny.toml's reason.
    let deny = std::fs::read_to_string(dir.path().join("deny.toml")).unwrap();
    assert!(deny.contains("https://github.com/rysweet/Simard/issues/42"));
}

#[test]
fn ignore_files_refuse_write_without_tracker_url() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    let files = IgnoreFiles::at_root(dir.path());

    let err = files
        .add_justified_ignore("RUSTSEC-2099-0003", "no fix", "   ")
        .unwrap_err();
    assert!(matches!(
        err,
        crate::error::SimardError::SupplyChainSuppressionWithoutTracker { .. }
    ));
    // Nothing was written.
    assert!(!files.is_ignored("RUSTSEC-2099-0003").unwrap());
}

#[test]
fn ignore_files_remove_clears_from_both() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    let files = IgnoreFiles::at_root(dir.path());
    files.remove_ignore("RUSTSEC-2023-0071").unwrap();
    assert!(!files.is_ignored("RUSTSEC-2023-0071").unwrap());
    assert!(files.ignored_ids_in_sync().unwrap());
}

// ═══════════════════════════ execute.rs ═══════════════════════════

use super::execute::{RemediationOutcome, execute};
use super::gh::FakeSupplyChainGh;
use crate::stewardship::{GhIssue, MergeOutcome, failure_signature};

fn fixed_advisory() -> Advisory {
    Advisory {
        id: "RUSTSEC-2026-0204".to_string(),
        crate_name: "crossbeam-epoch".to_string(),
        installed: "0.9.18".to_string(),
        patched: PatchStatus::Fixed {
            requirement: ">= 0.9.20".to_string(),
        },
        title: "crossbeam-epoch null deref".to_string(),
        url: "https://rustsec.org/advisories/RUSTSEC-2026-0204".to_string(),
    }
}

fn nofix_advisory() -> Advisory {
    Advisory {
        id: "RUSTSEC-2099-0009".to_string(),
        crate_name: "orphan".to_string(),
        installed: "1.0.0".to_string(),
        patched: PatchStatus::None,
        title: "orphan unfixable".to_string(),
        url: "https://rustsec.org/advisories/RUSTSEC-2099-0009".to_string(),
    }
}

#[test]
fn execute_justified_ignore_files_issue_before_writing_ignore() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    let files = IgnoreFiles::at_root(dir.path());
    let gh = FakeSupplyChainGh::default();
    let adv = nofix_advisory();

    let decision = Decision::JustifiedIgnore {
        advisory_id: adv.id.clone(),
        crate_name: adv.crate_name.clone(),
        reason: "no fixed upstream release; unreachable in Simard's usage".to_string(),
    };

    let outcome = execute(decision, &adv, &files, &gh).unwrap();

    match outcome {
        RemediationOutcome::FiledJustifiedIgnore {
            advisory_id,
            issue_url,
            pr_url,
            ..
        } => {
            assert_eq!(advisory_id, "RUSTSEC-2099-0009");
            assert!(issue_url.contains("/issues/"));
            assert!(
                pr_url.contains("/pull/"),
                "ignore must be committed via a PR"
            );
        }
        other => panic!("expected FiledJustifiedIgnore, got {other:?}"),
    }

    // The issue was created BEFORE the ignore write, the ignore is present in
    // both files and in sync, and a PR was opened to COMMIT those edits (a
    // justified ignore that never lands in the repo is useless) — but it is
    // never self-merged (security suppressions stay under human review).
    let log = gh.log.borrow();
    let create_idx = log
        .iter()
        .position(|l| l.starts_with("create_issue"))
        .unwrap();
    let search_idx = log.iter().position(|l| l.starts_with("search")).unwrap();
    assert!(search_idx < create_idx, "search then create: {log:?}");
    assert!(
        log.iter()
            .any(|l| l.starts_with("open_pr:chore/advisory-rustsec-2099-0009")),
        "ignore edits must be committed via a PR: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l.starts_with("self_merge:")),
        "a security suppression must never self-merge: {log:?}"
    );
    assert!(files.is_ignored("RUSTSEC-2099-0009").unwrap());
    assert!(files.ignored_ids_in_sync().unwrap());
}

#[test]
fn execute_bump_opens_pr_removes_stale_ignore_and_self_merges_with_token() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    // Pre-seed a STALE ignore for the now-fixable advisory in both files.
    let files = IgnoreFiles::at_root(dir.path());
    files
        .add_justified_ignore(
            "RUSTSEC-2026-0204",
            "stale: was no-fix",
            "https://github.com/rysweet/Simard/issues/1",
        )
        .unwrap();
    assert!(files.is_ignored("RUSTSEC-2026-0204").unwrap());

    let gh = FakeSupplyChainGh {
        has_token: true,
        merge_outcome: MergeOutcome::Merged {
            pr_number: 101,
            repo: "rysweet/Simard".to_string(),
        },
        ..Default::default()
    };
    let adv = fixed_advisory();
    let decision = Decision::Bump {
        crate_name: adv.crate_name.clone(),
        from: adv.installed.clone(),
        to: "0.9.20".to_string(),
    };

    let outcome = execute(decision, &adv, &files, &gh).unwrap();
    match outcome {
        RemediationOutcome::OpenedBumpPr { merged, .. } => {
            assert!(merged, "should self-merge green")
        }
        other => panic!("expected OpenedBumpPr, got {other:?}"),
    }

    let log = gh.log.borrow();
    // Issue 3: the worktree is reset to the scan base before the mutating bump,
    // so each advisory's PR commit carries only its own change.
    let reset_idx = log
        .iter()
        .position(|l| l == "reset_to_scan_base")
        .expect("bump must reset to scan base");
    let update_idx = log
        .iter()
        .position(|l| l.starts_with("cargo_update:crossbeam-epoch@0.9.20"))
        .expect("bump must run cargo update");
    assert!(reset_idx < update_idx, "reset before mutation: {log:?}");
    assert!(
        log.iter()
            .any(|l| l.starts_with("open_pr:chore/advisory-rustsec-2026-0204"))
    );
    assert!(log.iter().any(|l| l.starts_with("self_merge:")));
    // Stale ignore removed from both files.
    assert!(!files.is_ignored("RUSTSEC-2026-0204").unwrap());
}

#[test]
fn execute_bump_without_token_labels_needs_ci_and_never_self_merges() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    let files = IgnoreFiles::at_root(dir.path());
    let gh = FakeSupplyChainGh {
        has_token: false,
        ..Default::default()
    };
    let adv = fixed_advisory();
    let decision = Decision::Bump {
        crate_name: adv.crate_name.clone(),
        from: adv.installed.clone(),
        to: "0.9.20".to_string(),
    };

    let outcome = execute(decision, &adv, &files, &gh).unwrap();
    match outcome {
        RemediationOutcome::OpenedBumpPr { merged, .. } => {
            assert!(!merged, "must NOT self-merge without a CI-triggering token")
        }
        other => panic!("expected OpenedBumpPr, got {other:?}"),
    }
    let log = gh.log.borrow();
    assert!(
        log.iter().any(|l| l.contains("needs-CI-trigger")),
        "PR must be labelled needs-CI-trigger: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l.starts_with("self_merge:")),
        "self_merge must never be attempted: {log:?}"
    );
}

#[test]
fn execute_escalate_files_issue_only_no_pr_no_ignore() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    let files = IgnoreFiles::at_root(dir.path());
    let gh = FakeSupplyChainGh::default();
    let adv = fixed_advisory();
    let decision = Decision::Escalate {
        advisory_id: adv.id.clone(),
        reason: "fix exists but not resolvable against Cargo.lock".to_string(),
    };

    let outcome = execute(decision, &adv, &files, &gh).unwrap();
    assert!(matches!(outcome, RemediationOutcome::Escalated { .. }));
    let log = gh.log.borrow();
    assert!(log.iter().any(|l| l.starts_with("create_issue")));
    assert!(
        !log.iter().any(|l| l.starts_with("open_pr")),
        "no PR: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l.starts_with("cargo_update")),
        "no bump: {log:?}"
    );
    // No ignore written (the fixed advisory is not in the files).
    assert!(!files.is_ignored("RUSTSEC-2026-0204").unwrap());
}

#[test]
fn execute_is_idempotent_when_tracking_issue_exists() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    let files = IgnoreFiles::at_root(dir.path());
    let adv = nofix_advisory();
    let signature = failure_signature(&adv.id, &adv.crate_name);
    let gh = FakeSupplyChainGh {
        existing_issues: vec![GhIssue {
            number: 7,
            url: "https://github.com/rysweet/Simard/issues/7".to_string(),
            title: "existing".to_string(),
            body: format!("stewardship-signature: {signature}\n"),
        }],
        ..Default::default()
    };
    let decision = Decision::JustifiedIgnore {
        advisory_id: adv.id.clone(),
        crate_name: adv.crate_name.clone(),
        reason: "no fix".to_string(),
    };

    let outcome = execute(decision, &adv, &files, &gh).unwrap();
    assert!(matches!(outcome, RemediationOutcome::Skipped { .. }));
    let log = gh.log.borrow();
    assert!(
        !log.iter().any(|l| l.starts_with("create_issue")),
        "must not file a duplicate issue: {log:?}"
    );
    assert!(!files.is_ignored("RUSTSEC-2099-0009").unwrap());
}

#[test]
fn execute_no_action_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    write_min_repo(dir.path());
    let files = IgnoreFiles::at_root(dir.path());
    let gh = FakeSupplyChainGh::default();
    let adv = nofix_advisory();
    let outcome = execute(Decision::NoAction, &adv, &files, &gh).unwrap();
    assert!(matches!(outcome, RemediationOutcome::Skipped { .. }));
}
