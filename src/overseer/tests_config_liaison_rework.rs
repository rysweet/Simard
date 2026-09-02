//! TDD tests for the default-OFF env flags that gate the two new Overseer
//! capabilities (design component C8), added to `crate::overseer::config`.
//!
//! Contract these tests pin (all in `crate::overseer::config`):
//!   - Env consts: `SIMARD_OVERSEER_SIGNAL_LIAISON_ENV`,
//!     `SIMARD_OVERSEER_REWORK_ENV`, `SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS_ENV`,
//!     `SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER_ENV`,
//!     `SIMARD_OVERSEER_SIGNAL_GROUP_ID_ENV`.
//!   - `signal_liaison_enabled_from(lookup) -> bool` — DEFAULT **OFF**;
//!     explicit-truthy required to enable; AND gated by the master overseer flag
//!     (an explicitly-disabled Overseer forces it off), mirroring the existing
//!     opt-in/opt-out helper convention.
//!   - `rework_enabled_from(lookup) -> bool` — same default-OFF, truthy-required,
//!     master-gated semantics.
//!   - `rework_max_attempts_from(lookup) -> u32` — DEFAULT `3`, clamped to
//!     `1..=10`; unset/empty/unparseable ⇒ default.
//!   - `signal_operator_number_from(lookup) -> Option<String>` and
//!     `signal_group_id_from(lookup) -> Option<String>` — trimmed; None when
//!     unset or empty.
//!
//! These reference not-yet-added symbols and FAIL TO COMPILE until C8 lands.

use std::collections::HashMap;

use crate::overseer::config::{
    SIMARD_OVERSEER_REWORK_ENV, SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS_ENV,
    SIMARD_OVERSEER_SIGNAL_GROUP_ID_ENV, SIMARD_OVERSEER_SIGNAL_LIAISON_ENV,
    SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER_ENV, rework_enabled_from, rework_max_attempts_from,
    signal_group_id_from, signal_liaison_enabled_from, signal_operator_number_from,
};

const MASTER: &str = "SIMARD_OVERSEER_ENABLED";

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

// ───────────────────────── signal-liaison flag (default OFF) ─────────────────

#[test]
fn signal_liaison_defaults_off_when_unset() {
    // Master unset (acting-Overseer default-ON) but liaison flag absent ⇒ OFF.
    assert!(!signal_liaison_enabled_from(env(&[])));
}

#[test]
fn signal_liaison_requires_explicit_truthy() {
    assert!(signal_liaison_enabled_from(env(&[(
        SIMARD_OVERSEER_SIGNAL_LIAISON_ENV,
        "1"
    )])));
    assert!(signal_liaison_enabled_from(env(&[(
        SIMARD_OVERSEER_SIGNAL_LIAISON_ENV,
        "true"
    )])));
    for junk in ["0", "false", "off", "", "maybe"] {
        assert!(
            !signal_liaison_enabled_from(env(&[(SIMARD_OVERSEER_SIGNAL_LIAISON_ENV, junk)])),
            "value {junk:?} must NOT enable the liaison"
        );
    }
}

#[test]
fn signal_liaison_is_forced_off_by_disabled_master() {
    // Even with the liaison flag truthy, an explicitly-disabled Overseer wins.
    assert!(!signal_liaison_enabled_from(env(&[
        (MASTER, "0"),
        (SIMARD_OVERSEER_SIGNAL_LIAISON_ENV, "1"),
    ])));
}

// ───────────────────────── rework flag (default OFF) ────────────────────────

#[test]
fn rework_defaults_off_when_unset() {
    assert!(!rework_enabled_from(env(&[])));
}

#[test]
fn rework_requires_explicit_truthy() {
    assert!(rework_enabled_from(env(&[(
        SIMARD_OVERSEER_REWORK_ENV,
        "yes"
    )])));
    for junk in ["0", "false", "off", "", "nope"] {
        assert!(
            !rework_enabled_from(env(&[(SIMARD_OVERSEER_REWORK_ENV, junk)])),
            "value {junk:?} must NOT enable rework"
        );
    }
}

#[test]
fn rework_is_forced_off_by_disabled_master() {
    assert!(!rework_enabled_from(env(&[
        (MASTER, "false"),
        (SIMARD_OVERSEER_REWORK_ENV, "1"),
    ])));
}

// ───────────────────────── rework attempt cap (default 3, clamp 1..=10) ──────

#[test]
fn rework_max_attempts_defaults_to_three() {
    assert_eq!(rework_max_attempts_from(env(&[])), 3);
}

#[test]
fn rework_max_attempts_honours_in_range_value() {
    assert_eq!(
        rework_max_attempts_from(env(&[(SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS_ENV, "5")])),
        5
    );
}

#[test]
fn rework_max_attempts_clamps_low_and_high() {
    assert_eq!(
        rework_max_attempts_from(env(&[(SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS_ENV, "0")])),
        1,
        "0 must clamp up to the floor of 1"
    );
    assert_eq!(
        rework_max_attempts_from(env(&[(SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS_ENV, "99")])),
        10,
        "99 must clamp down to the ceiling of 10"
    );
}

#[test]
fn rework_max_attempts_falls_back_on_garbage() {
    for junk in ["", "abc", "-1", "3.5"] {
        assert_eq!(
            rework_max_attempts_from(env(&[(SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS_ENV, junk)])),
            3,
            "unparseable {junk:?} must fall back to the default of 3"
        );
    }
}

// ───────────────────────── operator number / group id ───────────────────────

#[test]
fn operator_number_and_group_id_are_none_when_unset() {
    assert_eq!(signal_operator_number_from(env(&[])), None);
    assert_eq!(signal_group_id_from(env(&[])), None);
}

#[test]
fn operator_number_and_group_id_are_read_and_trimmed() {
    assert_eq!(
        signal_operator_number_from(env(&[(
            SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER_ENV,
            "  +15557654321  "
        )])),
        Some("+15557654321".to_string())
    );
    assert_eq!(
        signal_group_id_from(env(&[(SIMARD_OVERSEER_SIGNAL_GROUP_ID_ENV, "grp-abc==")])),
        Some("grp-abc==".to_string())
    );
}

#[test]
fn empty_operator_number_and_group_id_are_none() {
    assert_eq!(
        signal_operator_number_from(env(&[(SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER_ENV, "   ")])),
        None
    );
    assert_eq!(
        signal_group_id_from(env(&[(SIMARD_OVERSEER_SIGNAL_GROUP_ID_ENV, "")])),
        None
    );
}
