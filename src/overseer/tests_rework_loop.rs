//! TDD tests for the autonomous PR rework RAIL (Deliverable 2, design component
//! C5): `crate::overseer::rework_loop::poll_rework`.
//!
//! The rail is thin and deterministic — ALL judgment ("is this hold fixable, and
//! what must change?") lives in the merge-judge prompt and reaches the rail ONLY
//! as a typed [`MergeVerdictRecord`]. The rail:
//!   1. reads the typed verdict fail-closed (`read_verified`);
//!   2. admits a rework ONLY when `reworkable == true` AND the per-PR attempt cap
//!      is not hit AND it is not a duplicate of an already-dispatched rework AND
//!      the PR is not the Overseer's own (recursion guard);
//!   3. on admission returns `Intervention::ReworkPr { repo, pr, concern_path }`,
//!      having written the concern to a ContextFile at `concern_path`;
//!   4. increments a DURABLE, MONOTONIC per-PR counter; at the cap (or on corrupt
//!      state) returns `Intervention::Escalate` (the human backstop);
//!   5. otherwise returns `Skip(reason)` (no-op).
//!
//! Contract:
//!   - `pub enum ReworkOutcome { Rework(Intervention), Escalate(Intervention),
//!        Skip(String) }`.
//!   - `pub fn poll_rework(state_root: &Path, repo: &str, pr: u32,
//!        pr_author: &str, run_token: &str, max_attempts: u32,
//!        overseer_author_login: Option<&str>) -> ReworkOutcome`.
//!
//! References the not-yet-existent module → FAILS TO COMPILE until C5 lands.

use std::path::{Path, PathBuf};

use crate::overseer::intervention::Intervention;
use crate::overseer::rework_loop::{ReworkOutcome, poll_rework};
use crate::stewardship::merge_verdict_store::{MergeVerdictRecord, VerdictKind, write_record};

const REPO: &str = "rysweet/Simard";
const PR: u32 = 4931;
const HUMAN: &str = "rysweet"; // a normal (human) PR author
const OVERSEER: &str = "simard-overseer-bot"; // the Overseer's distinct identity

fn temp_state_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-rework-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

/// Write a reworkable HOLD verdict for `(REPO, PR)` with the given token/concern.
fn write_reworkable(root: &Path, token: &str, concern: &str) {
    let mut rec = MergeVerdictRecord::new(PR, REPO, VerdictKind::Hold, "fixable hold", token);
    rec.reworkable = Some(true);
    rec.concern = Some(concern.to_string());
    write_record(root, &rec).expect("write reworkable verdict");
}

// ───────────────────────── not reworkable ⇒ no-op ───────────────────────────

#[test]
fn no_verdict_record_is_skip() {
    let root = temp_state_root("none");
    match poll_rework(&root, REPO, PR, HUMAN, "tok", 3, Some(OVERSEER)) {
        ReworkOutcome::Skip(_) => {}
        other => panic!("no record must be Skip, got {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn hold_without_reworkable_flag_is_skip() {
    let root = temp_state_root("plainhold");
    // A plain hold (reworkable defaults to None) must NOT trigger a rework.
    let rec = MergeVerdictRecord::new(PR, REPO, VerdictKind::Hold, "held", "tok");
    write_record(&root, &rec).unwrap();
    assert!(matches!(
        poll_rework(&root, REPO, PR, HUMAN, "tok", 3, Some(OVERSEER)),
        ReworkOutcome::Skip(_)
    ));
    cleanup(&root);
}

#[test]
fn merge_verdict_is_skip() {
    let root = temp_state_root("merge");
    let rec = MergeVerdictRecord::new(PR, REPO, VerdictKind::Merge, "ready", "tok");
    write_record(&root, &rec).unwrap();
    assert!(matches!(
        poll_rework(&root, REPO, PR, HUMAN, "tok", 3, Some(OVERSEER)),
        ReworkOutcome::Skip(_)
    ));
    cleanup(&root);
}

#[test]
fn stale_run_token_fails_closed_to_skip() {
    let root = temp_state_root("stale");
    write_reworkable(&root, "recorded-token", "clamp before multiply");
    // The rail reads with a DIFFERENT expected token ⇒ fail-closed ⇒ Skip.
    assert!(matches!(
        poll_rework(&root, REPO, PR, HUMAN, "another-token", 3, Some(OVERSEER)),
        ReworkOutcome::Skip(_)
    ));
    cleanup(&root);
}

// ───────────────────────── happy path ⇒ ReworkPr + ContextFile ──────────────

#[test]
fn fixable_reworkable_hold_dispatches_rework_with_context_file() {
    let root = temp_state_root("happy");
    let concern = "Clamp the retry backoff before multiplying; add a ceiling unit test.";
    write_reworkable(&root, "tok", concern);

    match poll_rework(&root, REPO, PR, HUMAN, "tok", 3, Some(OVERSEER)) {
        ReworkOutcome::Rework(Intervention::ReworkPr {
            repo,
            pr,
            concern_path,
        }) => {
            assert_eq!(repo, REPO);
            assert_eq!(pr, PR);
            // Concern MUST ride a ContextFile (never argv) — the file exists and
            // holds exactly the recorded concern.
            let body = std::fs::read_to_string(&concern_path)
                .expect("rail must write the concern to a ContextFile");
            assert!(
                body.contains(concern),
                "the concern ContextFile must contain the recorded concern text"
            );
        }
        other => panic!("expected Rework(ReworkPr), got {other:?}"),
    }
    cleanup(&root);
}

// ───────────────────────── dedup ────────────────────────────────────────────

#[test]
fn identical_rework_is_deduped_on_the_next_tick() {
    let root = temp_state_root("dedup");
    write_reworkable(&root, "tok", "same concern");
    // First tick dispatches.
    assert!(matches!(
        poll_rework(&root, REPO, PR, HUMAN, "tok", 3, Some(OVERSEER)),
        ReworkOutcome::Rework(_)
    ));
    // Second tick with the SAME record (same token + concern) must NOT relaunch
    // an identical rework that is already in flight.
    match poll_rework(&root, REPO, PR, HUMAN, "tok", 3, Some(OVERSEER)) {
        ReworkOutcome::Skip(_) => {}
        other => panic!("identical rework must be deduped to Skip, got {other:?}"),
    }
    cleanup(&root);
}

// ───────────────────────── monotonic cap ⇒ Escalate ─────────────────────────

#[test]
fn attempt_counter_is_monotonic_and_cap_hit_escalates() {
    let root = temp_state_root("cap");
    let cap = 2;

    // Attempt 1 (fresh token/concern) ⇒ dispatch.
    write_reworkable(&root, "t1", "concern-1");
    assert!(matches!(
        poll_rework(&root, REPO, PR, HUMAN, "t1", cap, Some(OVERSEER)),
        ReworkOutcome::Rework(_)
    ));

    // Attempt 2 (a NEW judge run: new token + concern) ⇒ dispatch (counter=2).
    write_reworkable(&root, "t2", "concern-2");
    assert!(matches!(
        poll_rework(&root, REPO, PR, HUMAN, "t2", cap, Some(OVERSEER)),
        ReworkOutcome::Rework(_)
    ));

    // Attempt 3 would exceed the cap ⇒ Escalate to a human (final backstop).
    write_reworkable(&root, "t3", "concern-3");
    match poll_rework(&root, REPO, PR, HUMAN, "t3", cap, Some(OVERSEER)) {
        ReworkOutcome::Escalate(Intervention::Escalate { .. }) => {}
        other => panic!("cap hit must Escalate, got {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn corrupt_attempt_state_escalates_never_retries_forever() {
    let root = temp_state_root("corrupt");
    write_reworkable(&root, "tok", "concern");
    // Corrupt the durable per-PR attempt counter under the documented location
    // `<state_root>/overseer/rework_attempts/<owner__name>/<pr>.json`.
    let counter = root
        .join("overseer")
        .join("rework_attempts")
        .join("rysweet__Simard")
        .join(format!("{PR}.json"));
    std::fs::create_dir_all(counter.parent().unwrap()).unwrap();
    std::fs::write(&counter, b"{ not valid json").unwrap();

    match poll_rework(&root, REPO, PR, HUMAN, "tok", 3, Some(OVERSEER)) {
        ReworkOutcome::Escalate(_) => {}
        other => panic!("corrupt attempt state must Escalate, got {other:?}"),
    }
    cleanup(&root);
}

// ───────────────────────── recursion / own-PR guard ─────────────────────────

#[test]
fn refuses_to_rework_the_overseers_own_pr() {
    let root = temp_state_root("ownpr");
    write_reworkable(&root, "tok", "concern");
    // The PR author IS the Overseer identity ⇒ never rework our own output.
    match poll_rework(&root, REPO, PR, OVERSEER, "tok", 3, Some(OVERSEER)) {
        ReworkOutcome::Skip(_) => {}
        other => panic!("own-PR must be refused (Skip), got {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn unconfigured_overseer_identity_fails_closed() {
    let root = temp_state_root("noident");
    write_reworkable(&root, "tok", "concern");
    // No configured Overseer identity ⇒ the recursion guard cannot prove the PR
    // is foreign ⇒ fail CLOSED (refuse), never open.
    match poll_rework(&root, REPO, PR, HUMAN, "tok", 3, None) {
        ReworkOutcome::Skip(_) => {}
        other => panic!("unconfigured identity must fail closed (Skip), got {other:?}"),
    }
    cleanup(&root);
}
