//! TDD (Step 7) — FAILING tests for the P2 merged-but-undeployed fix.
//!
//! P2: the running head does not advance to the merged head, and self-deploy
//! re-fires for the same head (issues #4390 anti-thrash, #4387 dedupe, #4305
//! land the merged-but-undeployed head). These tests specify the genuinely NEW
//! logic layered on the existing time-based `global_deploy_throttle_allow`:
//!
//!   * per-TARGET-SHA dedupe — never re-deploy a SHA that already SUCCEEDED;
//!   * head-advance decision — deploy only when running != merged head;
//!   * file-backed deploy-head state (mirrors `SelfRelaunchState`) round-trips;
//!   * argv-safety — an invalid (non-hex / padded / flag-like) SHA is never
//!     deployed (guards `systemctl`/`gh`/`git` argv option-injection);
//!   * systemd unit-not-loaded classification + reconcile decision.
//!
//! RED until the following are implemented and re-exported at `self_deploy::`:
//!   * `DeployHeadState`, `DeployResult`
//!   * `should_deploy_target_sha`, `needs_head_advance`, `is_valid_deploy_sha`
//!   * `UnitLoadState`, `classify_unit_load`, `should_reconcile_unit`
//!
//! Wire-in: `#[cfg(test)] mod tests_deploy_dedup;` in `src/self_deploy/mod.rs`.

use crate::self_deploy::{
    DeployHeadState, DeployResult, UnitLoadState, classify_unit_load, is_valid_deploy_sha,
    needs_head_advance, should_deploy_target_sha, should_reconcile_unit,
};

const SHA_A: &str = "0123456789abcdef0123456789abcdef01234567"; // 40-hex
const SHA_B: &str = "89abcdef0123456789abcdef0123456789abcdef"; // 40-hex
const SHA_A_256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"; // 64-hex

// ════════════════════════════════════════════════════════════════════════════
// 1. is_valid_deploy_sha — 40/64 lowercase hex only (argv-injection guard)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn accepts_40_and_64_char_lowercase_hex() {
    assert!(is_valid_deploy_sha(SHA_A));
    assert!(is_valid_deploy_sha(SHA_A_256));
}

#[test]
fn rejects_uppercase_wrong_length_and_non_hex() {
    for bad in [
        "",
        "abc",
        "0123456789ABCDEF0123456789abcdef01234567", // uppercase
        "0123456789abcdef0123456789abcdef0123456",  // 39
        "0123456789abcdef0123456789abcdef012345678", // 41
        "z123456789abcdef0123456789abcdef01234567", // non-hex 'z'
    ] {
        assert!(!is_valid_deploy_sha(bad), "must reject invalid SHA {bad:?}");
    }
}

#[test]
fn rejects_flag_like_and_padded_shas() {
    // Leading '-' or surrounding whitespace would let the value be parsed as an
    // option or break argv boundaries — never accepted.
    for bad in [
        "--upload-pack=evil",
        "-0123456789abcdef0123456789abcdef0123456",
        " 0123456789abcdef0123456789abcdef01234567",
        "0123456789abcdef0123456789abcdef01234567 ",
    ] {
        assert!(
            !is_valid_deploy_sha(bad),
            "must reject argv-unsafe SHA {bad:?}"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 2. should_deploy_target_sha — per-SHA dedupe (skip already-SUCCEEDED head)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn deploys_when_no_prior_state() {
    let state = DeployHeadState::default();
    assert!(should_deploy_target_sha(&state, SHA_A));
}

#[test]
fn skips_a_sha_that_already_succeeded() {
    // The #4387 dedupe: a head we already deployed SUCCESSFULLY must not be
    // redeployed every tick.
    let state = DeployHeadState {
        last_deploy_target_sha: Some(SHA_A.to_string()),
        last_deploy_result: Some(DeployResult::Succeeded),
    };
    assert!(
        !should_deploy_target_sha(&state, SHA_A),
        "an already-succeeded SHA must be deduped (no redeploy)"
    );
}

#[test]
fn deploys_a_new_merged_head_even_after_a_prior_success() {
    // #4305: once new work merges, the new head must advance.
    let state = DeployHeadState {
        last_deploy_target_sha: Some(SHA_A.to_string()),
        last_deploy_result: Some(DeployResult::Succeeded),
    };
    assert!(
        should_deploy_target_sha(&state, SHA_B),
        "a different merged head must still deploy"
    );
}

#[test]
fn retries_a_sha_whose_prior_deploy_failed() {
    // Dedupe is scoped to SUCCESS only: a FAILED attempt for the same SHA may
    // retry (the time-based throttle still prevents per-tick thrash).
    let state = DeployHeadState {
        last_deploy_target_sha: Some(SHA_A.to_string()),
        last_deploy_result: Some(DeployResult::Failed),
    };
    assert!(
        should_deploy_target_sha(&state, SHA_A),
        "a previously-failed SHA is allowed to retry"
    );
}

#[test]
fn never_deploys_an_invalid_candidate_sha() {
    let state = DeployHeadState::default();
    assert!(
        !should_deploy_target_sha(&state, "--not-a-sha"),
        "an argv-unsafe candidate SHA must never be deployed (fail-closed)"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 3. DeployHeadState — file-backed JSON round-trip (mirrors SelfRelaunchState)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn deploy_head_state_json_round_trips() {
    let state = DeployHeadState {
        last_deploy_target_sha: Some(SHA_A.to_string()),
        last_deploy_result: Some(DeployResult::Succeeded),
    };
    let json = serde_json::to_string(&state).expect("serialises");
    let back: DeployHeadState = serde_json::from_str(&json).expect("deserialises");
    assert_eq!(back.last_deploy_target_sha.as_deref(), Some(SHA_A));
    assert_eq!(back.last_deploy_result, Some(DeployResult::Succeeded));
}

#[test]
fn deploy_head_state_default_is_empty() {
    let state = DeployHeadState::default();
    assert!(state.last_deploy_target_sha.is_none());
    assert!(state.last_deploy_result.is_none());
}

// ════════════════════════════════════════════════════════════════════════════
// 4. needs_head_advance — reconcile running head to merged head
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn advances_when_running_is_behind_merged() {
    assert!(needs_head_advance(SHA_A, SHA_B));
}

#[test]
fn no_advance_when_running_equals_merged() {
    assert!(!needs_head_advance(SHA_A, SHA_A));
}

#[test]
fn no_advance_to_an_invalid_merged_head() {
    // Never advance the running head to an unverifiable / argv-unsafe target.
    assert!(!needs_head_advance(SHA_A, "--evil"));
    assert!(!needs_head_advance(SHA_A, ""));
}

// ════════════════════════════════════════════════════════════════════════════
// 5. systemd unit-not-loaded classification + reconcile decision
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn classifies_unit_loaded_on_success() {
    assert_eq!(classify_unit_load(true, "enabled"), UnitLoadState::Loaded);
}

#[test]
fn classifies_unit_not_loaded_on_not_found() {
    for out in [
        "Unit simard-ooda.service not loaded",
        "not found",
        "no such unit",
    ] {
        assert_eq!(
            classify_unit_load(false, out),
            UnitLoadState::NotLoaded,
            "output {out:?} indicates the unit is not loaded"
        );
    }
}

#[test]
fn classifies_known_but_disabled_unit_as_loaded() {
    // A non-zero exit for a KNOWN-but-not-enabled unit (static/disabled) is not
    // "not loaded" — matches the existing systemd_unit_present heuristic.
    assert_eq!(classify_unit_load(false, "static"), UnitLoadState::Loaded);
    assert_eq!(classify_unit_load(false, "disabled"), UnitLoadState::Loaded);
}

#[test]
fn reconciles_only_when_unit_not_loaded() {
    assert!(should_reconcile_unit(UnitLoadState::NotLoaded));
    assert!(!should_reconcile_unit(UnitLoadState::Loaded));
}
