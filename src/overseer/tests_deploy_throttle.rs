//! Test-first contract for the durable, fail-closed self-deploy anti-thrash
//! ledger (#4390).
//!
//! These tests are RED until `src/overseer/deploy_throttle.rs` provides the
//! `DeployAttemptLedger` primitive, its `ThrottleDecision`/`FailClosedReason`
//! enums, and the `DEPLOY_BACKOFF_CAP_SECS` constant described in
//! `docs/reference/overseer-deploy-throttle-api.md`.
//!
//! They pin the four properties the observed thrash (commit `56b10bef5057`
//! failing the canary deploy-gate on five consecutive ticks) requires:
//!
//!   1. **Restart-durable.** A red-canary SHA recorded as failed is still
//!      suppressed after the in-memory ledger is dropped and re-`load`ed from
//!      the same state dir — modelling an overseer restart that resets every
//!      process `static`.
//!   2. **Fail-closed per-SHA.** A ledger this tick cannot trust for the
//!      candidate SHA (corrupt/unknown-version file, or a record present with
//!      no terminal result) refuses that SHA rather than re-admitting it.
//!   3. **Never deadlocks the first deploy.** A *missing* ledger loads empty and
//!      a never-seen SHA is `Allow`ed, so fail-closed is scoped to known-bad
//!      commits only.
//!   4. **Bounded exponential backoff that self-clears.** A failed SHA backs off
//!      exponentially (capped), and a later `record_success` clears the curve so
//!      the throttle is never a permanent hard-stop.

use std::path::Path;

use tempfile::TempDir;

use crate::overseer::deploy_throttle::{
    DEPLOY_BACKOFF_CAP_SECS, DeployAttemptLedger, FailClosedReason, ThrottleDecision,
};
use crate::overseer::deploy_trigger::deploy_min_interval_secs;

const SHA_A: &str = "56b10bef5057aabbccddeeff00112233445566aa";
const SHA_B: &str = "0123456789abcdef0123456789abcdef01234567";

/// The exponential backoff the ledger applies after `n` consecutive failures,
/// mirroring the documented curve `min(base * 2^(n-1), cap)`. Uses the SAME base
/// accessor the implementation reads, so the assertion holds regardless of any
/// `SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS` override in the environment.
fn expected_backoff_secs(n: u32) -> u64 {
    let base = deploy_min_interval_secs();
    let grown = base.saturating_mul(1u64 << (n - 1).min(63));
    grown.min(DEPLOY_BACKOFF_CAP_SECS)
}

fn load(dir: &Path) -> DeployAttemptLedger {
    DeployAttemptLedger::load(dir)
}

// ─────────────────────────── Allow / missing ───────────────────────────────

#[test]
fn missing_ledger_allows_a_never_seen_sha() {
    // A first-ever run (no file on disk) must load an EMPTY ledger and admit a
    // brand-new SHA — otherwise fail-closed would deadlock the literal first
    // autonomous deploy.
    let dir = TempDir::new().unwrap();
    assert!(
        !DeployAttemptLedger::ledger_path(dir.path()).exists(),
        "no ledger file exists before the first record"
    );
    let ledger = load(dir.path());
    assert_eq!(
        ledger.consult(SHA_A, 10_000),
        ThrottleDecision::Allow,
        "a never-seen SHA on an empty ledger is admitted"
    );
}

#[test]
fn ledger_path_is_the_named_json_under_the_state_dir() {
    let dir = TempDir::new().unwrap();
    let path = DeployAttemptLedger::ledger_path(dir.path());
    assert_eq!(
        path.file_name().and_then(|n| n.to_str()),
        Some("deploy-attempt-ledger.json"),
        "the ledger is the documented file name"
    );
    assert_eq!(
        path.parent(),
        Some(dir.path()),
        "the ledger lives directly under the state dir"
    );
}

// ─────────────────────────── backoff after failure ─────────────────────────

#[test]
fn a_failed_sha_backs_off_and_becomes_eligible_again_after_the_window() {
    let dir = TempDir::new().unwrap();
    let mut ledger = load(dir.path());
    let t0 = 1_000_000u64;

    ledger.record_failure(SHA_A, t0).expect("persist failure");
    let window = expected_backoff_secs(1);

    // Inside the window: suppressed with the concrete retry time.
    match ledger.consult(SHA_A, t0 + window - 1) {
        ThrottleDecision::BackingOff {
            target_sha,
            failure_count,
            retry_after_unix_secs,
        } => {
            assert_eq!(target_sha, SHA_A);
            assert_eq!(failure_count, 1, "one recorded failure");
            assert_eq!(
                retry_after_unix_secs,
                t0 + window,
                "retry_after = now_of_failure + backoff window"
            );
        }
        other => panic!("expected BackingOff inside the window, got {other:?}"),
    }

    // At the boundary (now == backoff_until) the SHA is eligible again.
    assert_eq!(
        ledger.consult(SHA_A, t0 + window),
        ThrottleDecision::Allow,
        "eligible once now >= backoff_until (inclusive boundary)"
    );
}

#[test]
fn backoff_grows_exponentially_and_is_capped() {
    let dir = TempDir::new().unwrap();
    let mut ledger = load(dir.path());
    let t = 2_000_000u64;

    // Each successive failure widens the window: base, 2·base, 4·base, …, capped.
    for n in 1..=7u32 {
        ledger.record_failure(SHA_A, t).expect("persist failure");
        let expected = expected_backoff_secs(n);
        match ledger.consult(SHA_A, t) {
            ThrottleDecision::BackingOff {
                failure_count,
                retry_after_unix_secs,
                ..
            } => {
                assert_eq!(
                    failure_count, n,
                    "failure_count tracks consecutive failures"
                );
                assert_eq!(
                    retry_after_unix_secs,
                    t + expected,
                    "backoff #{n} follows the capped exponential curve"
                );
            }
            other => panic!("expected BackingOff after failure #{n}, got {other:?}"),
        }
    }

    // Far enough along the curve the window is pinned to the 6 h cap.
    assert_eq!(
        expected_backoff_secs(7),
        DEPLOY_BACKOFF_CAP_SECS,
        "the curve saturates at the fixed 6 h cap"
    );
}

#[test]
fn record_success_clears_the_backoff_so_a_green_sha_is_eligible_again() {
    let dir = TempDir::new().unwrap();
    let mut ledger = load(dir.path());
    let t0 = 3_000_000u64;

    ledger.record_failure(SHA_A, t0).expect("persist failure");
    assert!(
        matches!(
            ledger.consult(SHA_A, t0),
            ThrottleDecision::BackingOff { .. }
        ),
        "a just-failed SHA is backing off"
    );

    ledger
        .record_success(SHA_A, t0 + 5)
        .expect("persist success");
    assert_eq!(
        ledger.consult(SHA_A, t0 + 5),
        ThrottleDecision::Allow,
        "a successful deploy clears the failure count and backoff immediately"
    );
}

#[test]
fn backoff_is_scoped_per_sha() {
    let dir = TempDir::new().unwrap();
    let mut ledger = load(dir.path());
    let t0 = 4_000_000u64;

    ledger.record_failure(SHA_A, t0).expect("persist failure");
    assert!(
        matches!(
            ledger.consult(SHA_A, t0),
            ThrottleDecision::BackingOff { .. }
        ),
        "the failed SHA backs off"
    );
    assert_eq!(
        ledger.consult(SHA_B, t0),
        ThrottleDecision::Allow,
        "an unrelated, never-failed SHA is unaffected (per-SHA scope)"
    );
}

// ─────────────────────────── restart durability ────────────────────────────

#[test]
fn a_failed_sha_is_still_suppressed_after_a_simulated_overseer_restart() {
    // The core #4390 fix: the throttle must survive the process `static` reset
    // that a self-deploy restart causes. Record a failure, DROP the in-memory
    // ledger (restart), re-`load` from the same dir, and the SHA must STILL be
    // backing off — proving the memory lived on disk, not in the process.
    let dir = TempDir::new().unwrap();
    let t0 = 5_000_000u64;

    {
        let mut ledger = load(dir.path());
        ledger.record_failure(SHA_A, t0).expect("persist failure");
    } // ledger dropped == overseer process exits

    let reloaded = load(dir.path());
    match reloaded.consult(SHA_A, t0 + 60) {
        ThrottleDecision::BackingOff {
            target_sha,
            failure_count,
            ..
        } => {
            assert_eq!(target_sha, SHA_A);
            assert_eq!(
                failure_count, 1,
                "the failure count persisted across restart"
            );
        }
        other => panic!("a red-canary SHA must survive a restart, got {other:?}"),
    }
}

#[test]
fn consecutive_failures_persist_across_restarts_and_keep_widening() {
    // Five red ticks with a restart between each must still escalate the backoff
    // — the exact 56b10bef5057 thrash, but converging instead of re-attempting
    // every tick.
    let dir = TempDir::new().unwrap();
    let t = 6_000_000u64;

    for _ in 0..5 {
        let mut ledger = load(dir.path());
        ledger.record_failure(SHA_A, t).expect("persist failure");
    }

    let ledger = load(dir.path());
    match ledger.consult(SHA_A, t) {
        ThrottleDecision::BackingOff {
            failure_count,
            retry_after_unix_secs,
            ..
        } => {
            assert_eq!(
                failure_count, 5,
                "all five failures accumulated across restarts"
            );
            assert_eq!(retry_after_unix_secs, t + expected_backoff_secs(5));
        }
        other => panic!("expected an escalated BackingOff, got {other:?}"),
    }
}

// ─────────────────────────── fail-closed ───────────────────────────────────

#[test]
fn a_corrupt_ledger_file_fails_closed_for_the_candidate_sha() {
    // A torn / non-JSON file must NOT silently re-admit a commit that had already
    // been persisted as bad. It loads poisoned and refuses the candidate SHA.
    let dir = TempDir::new().unwrap();
    std::fs::write(
        DeployAttemptLedger::ledger_path(dir.path()),
        b"{ this is not valid json",
    )
    .unwrap();

    let ledger = load(dir.path());
    match ledger.consult(SHA_A, 10_000) {
        ThrottleDecision::FailClosed { target_sha, reason } => {
            assert_eq!(target_sha, SHA_A);
            assert_eq!(reason, FailClosedReason::Unreadable);
        }
        other => panic!("a corrupt ledger must fail closed, got {other:?}"),
    }
}

#[test]
fn an_unknown_schema_version_fails_closed() {
    // A greater/unknown `version` is never silently migrated — it loads poisoned
    // (fail-closed) so a schema change can't reset the anti-thrash memory.
    let dir = TempDir::new().unwrap();
    std::fs::write(
        DeployAttemptLedger::ledger_path(dir.path()),
        br#"{"version":9999,"entries":{}}"#,
    )
    .unwrap();

    let ledger = load(dir.path());
    assert!(
        matches!(
            ledger.consult(SHA_A, 10_000),
            ThrottleDecision::FailClosed {
                reason: FailClosedReason::Unreadable,
                ..
            }
        ),
        "an unknown schema version fails closed"
    );
}

#[test]
fn a_record_with_no_terminal_result_is_ambiguous_and_fails_closed() {
    // A SHA that WAS attempted (a record exists) but whose `last_deploy_result`
    // is unset has an ambiguous outcome — the daemon must not re-attempt it even
    // once its backoff window has elapsed.
    let dir = TempDir::new().unwrap();
    let json = format!(
        r#"{{"version":1,"entries":{{"{SHA_A}":{{"failure_count":2,"last_attempt_unix_secs":100,"backoff_until_unix_secs":200,"last_deploy_result":null}}}}}}"#
    );
    std::fs::write(DeployAttemptLedger::ledger_path(dir.path()), json).unwrap();

    let ledger = load(dir.path());
    // now is PAST the recorded backoff window (200): a *failed* result would be
    // `Allow` here, so this proves the ambiguity — not the backoff — drives the
    // refusal.
    match ledger.consult(SHA_A, 10_000) {
        ThrottleDecision::FailClosed { target_sha, reason } => {
            assert_eq!(target_sha, SHA_A);
            assert_eq!(reason, FailClosedReason::Ambiguous);
        }
        other => panic!("an ambiguous record must fail closed, got {other:?}"),
    }
}

#[test]
fn fail_closed_is_scoped_to_the_candidate_sha_not_all_deploys() {
    // Even with a poisoned ledger, fail-closed refuses only the ONE in-flight
    // candidate SHA. It never becomes a global deploy deadlock — the contract is
    // "refuse a commit already known-bad", per-SHA. (Both SHAs resolve to the
    // same poisoned verdict, but each carries its OWN target, so the surfaced
    // stuck-state names the specific commit rather than "all deploys".)
    let dir = TempDir::new().unwrap();
    std::fs::write(
        DeployAttemptLedger::ledger_path(dir.path()),
        b"not json at all",
    )
    .unwrap();

    let ledger = load(dir.path());
    for sha in [SHA_A, SHA_B] {
        match ledger.consult(sha, 10_000) {
            ThrottleDecision::FailClosed { target_sha, .. } => {
                assert_eq!(target_sha, sha, "the refusal names the specific candidate");
            }
            other => panic!("expected per-SHA FailClosed, got {other:?}"),
        }
    }
}

// ─────────────────────────── durable write hygiene ─────────────────────────

#[test]
fn record_failure_persists_a_parseable_ledger_file() {
    let dir = TempDir::new().unwrap();
    let mut ledger = load(dir.path());
    ledger
        .record_failure(SHA_A, 7_000_000)
        .expect("persist failure");

    let path = DeployAttemptLedger::ledger_path(dir.path());
    assert!(
        path.exists(),
        "the ledger file is written on record_failure"
    );
    let bytes = std::fs::read(&path).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).expect("the persisted ledger is valid JSON");
    assert!(
        value.get("entries").and_then(|e| e.get(SHA_A)).is_some(),
        "the failed SHA is recorded in the durable entries map"
    );
}

#[cfg(unix)]
#[test]
fn the_ledger_file_is_written_owner_only_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let mut ledger = load(dir.path());
    ledger
        .record_failure(SHA_A, 8_000_000)
        .expect("persist failure");

    let path = DeployAttemptLedger::ledger_path(dir.path());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the ledger may name a red commit; it must be owner-only readable"
    );
}
