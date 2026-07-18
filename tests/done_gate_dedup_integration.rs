//! Integration test for idempotent done-gate PR emission — Problem 4
//! (issues [#4166]/[#4189]).
//!
//! Models the exact production failure through the PUBLIC typed-OODA ledger
//! surface: when the OODA loop advances the SAME goal across consecutive cycles
//! — after an engineer terminates but leaves its PR open — the goal→engineer
//! dispatch must emit **at most one** open done-gate PR per goal, while a
//! DISTINCT goal still dispatches normally.
//!
//! The `goal_pr_emissions` ledger is the primary, authoritative guard: once an
//! engineer records an `Open` emission for a goal-key, a later cycle finds it
//! and declines to dispatch a second engineer / open a second PR.
//!
//! GREEN against `record_goal_pr_emission` / `find_open_goal_pr_emission` and
//! the schema-v2 migration.
//!
//! [#4166]: https://github.com/rysweet/Simard/issues/4166
//! [#4189]: https://github.com/rysweet/Simard/issues/4189

use simard::typed_ooda::{CapabilityHandler, CapabilityPolicy, EmissionState};

const REPO: &str = "rysweet/Simard";

fn open_ledger(dir: &std::path::Path) -> CapabilityHandler {
    CapabilityHandler::open(
        dir.join("outcomes.sqlite3"),
        CapabilityPolicy::new("done-gate-dedup-it"),
    )
    .expect("open capability handler")
}

/// One OODA cycle's dispatch decision for a goal, modelling the third
/// `dispatch_spawn_engineer` guard against the durable ledger:
///
///   - If an OPEN emission already exists for the goal-key ⇒ idempotent skip
///     (no engineer dispatched, no PR opened).
///   - Otherwise ⇒ dispatch an engineer that opens `next_pr` and records the
///     emission, exactly as the engineer PR-emission contract does.
///
/// Returns `true` iff a dispatch (and therefore a new PR) occurred this cycle.
fn advance_goal_cycle(
    ledger: &CapabilityHandler,
    goal_key: &str,
    goal_id: &str,
    next_pr: u32,
    now_millis: i64,
) -> bool {
    if ledger
        .find_open_goal_pr_emission(goal_key)
        .expect("ledger lookup")
        .is_some()
    {
        return false; // guard hit: idempotent no-op.
    }

    // No open emission ⇒ dispatch one engineer that opens exactly one PR and
    // records the emission after `gh pr create`.
    ledger
        .record_goal_pr_emission(
            goal_key,
            goal_id,
            REPO,
            next_pr,
            &format!("https://github.com/{REPO}/pull/{next_pr}"),
            &format!("engineer/{goal_key}-{goal_id}"),
            EmissionState::Open,
            now_millis,
        )
        .expect("record emission");
    true
}

#[test]
fn same_goal_across_two_cycles_emits_exactly_one_pr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = open_ledger(dir.path());

    // A stable goal-identity key (opaque to the ledger). In production this is
    // `goal_dedup_key("coin-benchmark", REPO)`.
    let goal_key = "4f2a9c1e7b3d0a58";

    // Cycle 1: no open emission ⇒ dispatch, open PR #4326.
    let dispatched_c1 = advance_goal_cycle(&ledger, goal_key, "coin-benchmark", 4326, 1_000);
    assert!(dispatched_c1, "first cycle must dispatch and open a PR");

    // The engineer terminates here (in production its `engineer_claims` row is
    // DELETEd) — but PR #4326 remains open and the emission ledger RETAINS it.

    // Cycle 2: the open emission is found ⇒ NO second dispatch, NO second PR.
    let dispatched_c2 = advance_goal_cycle(&ledger, goal_key, "coin-benchmark", 4329, 2_000);
    assert!(
        !dispatched_c2,
        "second cycle must be an idempotent no-op — one goal, one open PR",
    );

    // The single tracked open PR is still #4326 (never re-emitted as #4329).
    let tracked = ledger
        .find_open_goal_pr_emission(goal_key)
        .expect("lookup")
        .expect("emission still tracked");
    assert_eq!(tracked.pr_number, 4326);
}

#[test]
fn distinct_goals_each_emit_their_own_pr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = open_ledger(dir.path());

    let coin_key = "4f2a9c1e7b3d0a58";
    let kgpacks_key = "00112233445566ff";
    assert_ne!(coin_key, kgpacks_key);

    let coin = advance_goal_cycle(&ledger, coin_key, "coin-benchmark", 4326, 1_000);
    let kgpacks = advance_goal_cycle(&ledger, kgpacks_key, "kgpacks-parity", 4324, 1_100);

    assert!(coin, "coin-benchmark must dispatch its own PR");
    assert!(
        kgpacks,
        "kgpacks-parity is a DISTINCT goal and must still dispatch — the guard only collapses same-goal re-emission",
    );

    assert_eq!(
        ledger
            .find_open_goal_pr_emission(coin_key)
            .expect("lookup")
            .expect("coin tracked")
            .pr_number,
        4326,
    );
    assert_eq!(
        ledger
            .find_open_goal_pr_emission(kgpacks_key)
            .expect("lookup")
            .expect("kgpacks tracked")
            .pr_number,
        4324,
    );
}

#[test]
fn reopened_goal_dispatches_again_after_previous_pr_closes() {
    // Once the previous PR is no longer open (merged/closed), a later cycle is
    // free to dispatch again — the guard suppresses only while a PR is OPEN.
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = open_ledger(dir.path());
    let goal_key = "abcabcabcabcabc0";

    assert!(advance_goal_cycle(
        &ledger,
        goal_key,
        "flaky-goal",
        500,
        1_000
    ));

    // The PR merges: transition the emission out of `open`.
    ledger
        .record_goal_pr_emission(
            goal_key,
            "flaky-goal",
            REPO,
            500,
            &format!("https://github.com/{REPO}/pull/500"),
            &format!("engineer/{goal_key}-flaky-goal"),
            EmissionState::Merged,
            2_000,
        )
        .expect("mark merged");

    // With no OPEN emission, the next cycle dispatches a fresh PR.
    assert!(
        advance_goal_cycle(&ledger, goal_key, "flaky-goal", 900, 3_000),
        "a goal whose previous PR is no longer open must be able to dispatch again",
    );
}
