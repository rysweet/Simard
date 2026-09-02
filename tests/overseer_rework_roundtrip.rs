//! Integration TDD test (design: `tests/` round-trip) for the Overseer
//! autonomous PR rework loop: **held → record `reworkable` → `ReworkPr` →
//! re-review → merge**, driven entirely through the crate's public API using
//! REAL components (the durable merge-verdict store + the rework rail).
//!
//! The load-bearing assertion is **agentic-first**: the loop advances because of
//! the *recorded typed verdict*, NOT because of any classifier or heuristic in
//! Rust. Flipping the recorded verdict (hold+reworkable → merge) is the ONLY
//! thing that stops the rework loop — the rail reads judgment, it does not make
//! it.
//!
//! This references not-yet-implemented public symbols and FAILS TO COMPILE until
//! the rework loop lands — the intended TDD red state.

use std::path::{Path, PathBuf};

use simard::overseer::intervention::Intervention;
use simard::overseer::rework_loop::{ReworkOutcome, poll_rework};
use simard::stewardship::merge_verdict_store::{MergeVerdictRecord, VerdictKind, write_record};

const REPO: &str = "rysweet/Simard";
const PR: u32 = 4931;
const HUMAN: &str = "rysweet";
const OVERSEER: &str = "simard-overseer-bot";

fn temp_state_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-rework-rt-{}-{}",
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

fn write_reworkable(root: &Path, token: &str, concern: &str) {
    let mut rec = MergeVerdictRecord::new(PR, REPO, VerdictKind::Hold, "fixable hold", token);
    rec.reworkable = Some(true);
    rec.concern = Some(concern.to_string());
    write_record(root, &rec).expect("record a reworkable hold verdict");
}

#[test]
fn held_reworkable_pr_is_reworked_then_stops_when_judge_merges() {
    let root = temp_state_root();
    let concern = "Clamp the retry backoff before multiplying; add a ceiling unit test.";

    // ── 1. The judge recorded a FIXABLE hold. The rail dispatches a rework, and
    //       the judgment came from the RECORD (not Rust logic). ──────────────
    write_reworkable(&root, "run-token-1", concern);
    match poll_rework(&root, REPO, PR, HUMAN, "run-token-1", 3, Some(OVERSEER)) {
        ReworkOutcome::Rework(Intervention::ReworkPr {
            repo,
            pr,
            concern_path,
        }) => {
            assert_eq!(repo, REPO);
            assert_eq!(pr, PR);
            let body = std::fs::read_to_string(&concern_path)
                .expect("the concern rides a ContextFile the rail wrote");
            assert!(
                body.contains(concern),
                "the dispatched rework must carry the RECORDED concern, verbatim"
            );
        }
        other => panic!("a fixable reworkable hold must dispatch ReworkPr, got {other:?}"),
    }

    // ── 2. Engineer reworks the branch; the SAME merge-judge re-reviews on the
    //       next tick and is now satisfied → it records `merge` (new run token).
    //       The rail must NOT dispatch another rework — the loop ends here and
    //       hands off to the normal gated merge authority. ────────────────────
    let merge_rec = MergeVerdictRecord::new(
        PR,
        REPO,
        VerdictKind::Merge,
        "reworked; ready",
        "run-token-2",
    );
    write_record(&root, &merge_rec).expect("record the satisfied merge verdict");
    match poll_rework(&root, REPO, PR, HUMAN, "run-token-2", 3, Some(OVERSEER)) {
        ReworkOutcome::Skip(_) => {}
        other => panic!(
            "once the judge records `merge`, the rework loop must STOP (Skip), got {other:?}"
        ),
    }

    cleanup(&root);
}

#[test]
fn judgment_lives_in_the_record_not_in_rust() {
    // A hold whose recorded verdict is NOT marked reworkable must never be
    // reworked — the rail has no independent "is this fixable?" opinion.
    let root = temp_state_root();
    let plain_hold = MergeVerdictRecord::new(PR, REPO, VerdictKind::Hold, "held", "tok");
    write_record(&root, &plain_hold).unwrap();
    assert!(
        matches!(
            poll_rework(&root, REPO, PR, HUMAN, "tok", 3, Some(OVERSEER)),
            ReworkOutcome::Skip(_)
        ),
        "the rail must not invent a rework the judge never recorded"
    );
    cleanup(&root);
}
