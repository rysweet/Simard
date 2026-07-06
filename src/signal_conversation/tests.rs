//! TDD tests for the Signal channel guardrails (issue #2527).
//!
//! Written **first**: `allowlist::Allowlist::authorize` and the `gating::*`
//! functions are `todo!()` stubs, so these are the "red" phase pinning the
//! required security behavior — a fail-closed sender allowlist and high-risk
//! gating that never auto-executes a mutating command from a text.
//!
//! These run under the default `cargo test` (the `signal` feature is on by
//! default since issue #2576); no live signal-cli or network is involved.

use super::allowlist::{Allowlist, AuthDecision};
use super::config::SignalConfig;
use super::gating::{self, GateDecision, InboundCommand, RiskClass};

const OPERATOR: &str = "+15557654321";
const STRANGER: &str = "+15550000000";

// ── Guardrail (a): sender allowlist, fail-closed ─────────────────────────────

#[test]
fn allowlisted_sender_is_authorized() {
    let al = Allowlist::new(vec![OPERATOR.to_string()], false);
    assert_eq!(al.authorize(OPERATOR), AuthDecision::Authorized);
}

#[test]
fn unknown_sender_is_ignored_when_read_only_off() {
    let al = Allowlist::new(vec![OPERATOR.to_string()], false);
    assert_eq!(al.authorize(STRANGER), AuthDecision::Ignored);
}

#[test]
fn unknown_sender_is_read_only_when_enabled() {
    let al = Allowlist::new(vec![OPERATOR.to_string()], true);
    assert_eq!(al.authorize(STRANGER), AuthDecision::ReadOnly);
}

#[test]
fn empty_allowlist_ignores_everyone_fail_closed() {
    let al = Allowlist::new(vec![], false);
    assert_eq!(al.authorize(OPERATOR), AuthDecision::Ignored);
}

#[test]
fn read_only_unknown_never_grants_command_authority() {
    // Even with read_only_unknown, a non-allowlisted number must never be
    // Authorized to COMMAND — read-only is strictly weaker than authorized.
    let al = Allowlist::new(vec![], true);
    assert_ne!(al.authorize(STRANGER), AuthDecision::Authorized);
}

#[test]
fn allowlist_from_config_uses_config_fields() {
    let cfg = SignalConfig {
        endpoint: "127.0.0.1:7583".to_string(),
        account: "+15551234567".to_string(),
        allowlist: vec![OPERATOR.to_string()],
        read_only_unknown: false,
        own_device_id: None,
    };
    let al = Allowlist::from_config(&cfg);
    assert_eq!(al.authorize(OPERATOR), AuthDecision::Authorized);
    assert_eq!(al.authorize(STRANGER), AuthDecision::Ignored);
}

// ── Guardrail (c): high-risk gating ──────────────────────────────────────────

#[test]
fn parse_inbound_recognizes_the_command_set() {
    assert_eq!(gating::parse_inbound("status"), InboundCommand::Status);
    assert_eq!(gating::parse_inbound("pause"), InboundCommand::Pause);
    assert_eq!(gating::parse_inbound("approve"), InboundCommand::Approve);
    assert_eq!(gating::parse_inbound("deploy"), InboundCommand::Deploy);
    assert_eq!(
        gating::parse_inbound("merge #123"),
        InboundCommand::Merge(123)
    );
}

#[test]
fn parse_inbound_treats_other_text_as_conversation() {
    match gating::parse_inbound("let's talk about the release plan") {
        InboundCommand::Conversation(text) => assert!(text.contains("release plan")),
        other => panic!("expected a Conversation turn, got {other:?}"),
    }
}

#[test]
fn parse_inbound_is_case_insensitive_and_trims() {
    // The command vocabulary is matched case-insensitively (ASCII) after
    // trimming — the optimized parser must preserve exactly this behavior.
    assert_eq!(gating::parse_inbound("  STATUS  "), InboundCommand::Status);
    assert_eq!(gating::parse_inbound("Pause"), InboundCommand::Pause);
    assert_eq!(gating::parse_inbound("ApProVe"), InboundCommand::Approve);
    assert_eq!(gating::parse_inbound("DEPLOY"), InboundCommand::Deploy);
    assert_eq!(
        gating::parse_inbound("Merge #42"),
        InboundCommand::Merge(42)
    );
    assert_eq!(gating::parse_inbound("MERGE 7"), InboundCommand::Merge(7));
}

#[test]
fn parse_inbound_bare_and_malformed_merge_is_conversation() {
    // Bare `merge`, a non-numeric remainder, or text that merely starts with
    // "merge" must fall through to a conversation turn (carried verbatim).
    for text in ["merge", "merge please", "merge #", "merges"] {
        match gating::parse_inbound(text) {
            InboundCommand::Conversation(t) => assert_eq!(t, text),
            other => panic!("expected Conversation for {text:?}, got {other:?}"),
        }
    }
}

#[test]
fn parse_inbound_carries_a_long_turn_verbatim_without_casefolding() {
    // A long conversation turn must be recognized as free text and carried with
    // its original case/whitespace — the parser must not lowercase the body.
    let turn = "Here Is A Long Update About The Release: ".repeat(64);
    match gating::parse_inbound(&turn) {
        InboundCommand::Conversation(t) => assert_eq!(t, turn.trim()),
        other => panic!("expected a Conversation turn, got {other:?}"),
    }
}

#[test]
fn low_risk_commands_classify_low() {
    for cmd in [
        InboundCommand::Status,
        InboundCommand::Pause,
        InboundCommand::Approve,
    ] {
        assert_eq!(gating::classify(&cmd), RiskClass::LowRisk, "{cmd:?}");
    }
}

#[test]
fn high_risk_commands_classify_high() {
    assert_eq!(
        gating::classify(&InboundCommand::Deploy),
        RiskClass::HighRisk
    );
    assert_eq!(
        gating::classify(&InboundCommand::Merge(42)),
        RiskClass::HighRisk
    );
}

#[test]
fn high_risk_commands_never_auto_execute() {
    // The core safety invariant: a high-risk action from a text message must
    // create a pending sign-off, never auto-execute.
    assert_eq!(
        gating::gate(&InboundCommand::Deploy),
        GateDecision::PendingSignOff
    );
    assert_eq!(
        gating::gate(&InboundCommand::Merge(7)),
        GateDecision::PendingSignOff
    );
    assert_ne!(
        gating::gate(&InboundCommand::Deploy),
        GateDecision::AutoExecute
    );
    assert_ne!(
        gating::gate(&InboundCommand::Merge(7)),
        GateDecision::AutoExecute
    );
}

#[test]
fn low_risk_commands_auto_execute() {
    assert_eq!(
        gating::gate(&InboundCommand::Status),
        GateDecision::AutoExecute
    );
    assert_eq!(
        gating::gate(&InboundCommand::Pause),
        GateDecision::AutoExecute
    );
    assert_eq!(
        gating::gate(&InboundCommand::Approve),
        GateDecision::AutoExecute
    );
}

// ── Config loading: env-first, then file, fail-closed allowlist ──────────────

mod config_loading {
    use super::super::config::{
        ENV_ACCOUNT, ENV_ALLOWLIST, ENV_ENDPOINT, ENV_OWN_DEVICE_ID, ENV_READ_ONLY_UNKNOWN,
        SignalConfig,
    };
    use serial_test::serial;

    fn clear_env() {
        // SAFETY: serialized via `#[serial(cognitive_memory)]`.
        unsafe {
            std::env::remove_var(ENV_ENDPOINT);
            std::env::remove_var(ENV_ACCOUNT);
            std::env::remove_var(ENV_ALLOWLIST);
            std::env::remove_var(ENV_READ_ONLY_UNKNOWN);
            std::env::remove_var(ENV_OWN_DEVICE_ID);
        }
    }

    #[test]
    #[serial(cognitive_memory)]
    fn env_supplies_all_fields_and_wins() {
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: serialized.
        unsafe {
            std::env::set_var(ENV_ENDPOINT, "127.0.0.1:7583");
            std::env::set_var(ENV_ACCOUNT, "+15551230000");
            std::env::set_var(ENV_ALLOWLIST, "+15557654321, +15559990000");
            std::env::set_var(ENV_READ_ONLY_UNKNOWN, "true");
        }
        let cfg = SignalConfig::load_from(tmp.path()).unwrap();
        clear_env();

        assert_eq!(cfg.endpoint, "127.0.0.1:7583");
        assert_eq!(cfg.account, "+15551230000");
        assert_eq!(cfg.allowlist, vec!["+15557654321", "+15559990000"]);
        assert!(cfg.read_only_unknown);
        // own_device_id is optional; unset resolves to None (fail-safe: the
        // device-1 gate is the primary loop guard and needs no configuration).
        assert_eq!(cfg.own_device_id, None);
    }

    #[test]
    #[serial(cognitive_memory)]
    fn missing_required_field_is_a_clear_error() {
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        // No env, no config.toml → endpoint/account cannot be resolved.
        let err = SignalConfig::load_from(tmp.path()).unwrap_err();
        assert!(
            format!("{err:?}").contains("signal."),
            "expected a MissingRequiredConfig for a signal.* key, got {err:?}"
        );
    }

    #[test]
    #[serial(cognitive_memory)]
    fn reads_the_signal_table_from_config_toml() {
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            r#"
llm_provider = "copilot"

[signal]
endpoint = "127.0.0.1:9000"
account = "+15551230000"
allowlist = ["+15557654321"]
"#,
        )
        .unwrap();

        let cfg = SignalConfig::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.endpoint, "127.0.0.1:9000");
        assert_eq!(cfg.account, "+15551230000");
        assert_eq!(cfg.allowlist, vec!["+15557654321"]);
        // Fail-closed default: unset read_only_unknown resolves to false.
        assert!(!cfg.read_only_unknown);
    }

    // ── own_device_id: optional loop-prevention defence-in-depth (issue #2575) ──

    #[test]
    #[serial(cognitive_memory)]
    fn own_device_id_absent_resolves_to_none() {
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: serialized.
        unsafe {
            std::env::set_var(ENV_ENDPOINT, "127.0.0.1:7583");
            std::env::set_var(ENV_ACCOUNT, "+15551230000");
        }
        let cfg = SignalConfig::load_from(tmp.path()).unwrap();
        clear_env();
        assert_eq!(cfg.own_device_id, None);
    }

    #[test]
    #[serial(cognitive_memory)]
    fn own_device_id_from_env_is_parsed() {
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: serialized.
        unsafe {
            std::env::set_var(ENV_ENDPOINT, "127.0.0.1:7583");
            std::env::set_var(ENV_ACCOUNT, "+15551230000");
            std::env::set_var(ENV_OWN_DEVICE_ID, "2");
        }
        let cfg = SignalConfig::load_from(tmp.path()).unwrap();
        clear_env();
        assert_eq!(cfg.own_device_id, Some(2));
    }

    #[test]
    #[serial(cognitive_memory)]
    fn own_device_id_reads_from_config_toml() {
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            r#"
[signal]
endpoint = "127.0.0.1:9000"
account = "+15551230000"
own_device_id = 4
"#,
        )
        .unwrap();
        let cfg = SignalConfig::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.own_device_id, Some(4));
    }

    #[test]
    #[serial(cognitive_memory)]
    fn own_device_id_unparseable_is_a_hard_error() {
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: serialized.
        unsafe {
            std::env::set_var(ENV_ENDPOINT, "127.0.0.1:7583");
            std::env::set_var(ENV_ACCOUNT, "+15551230000");
            std::env::set_var(ENV_OWN_DEVICE_ID, "not-a-number");
        }
        let err = SignalConfig::load_from(tmp.path()).unwrap_err();
        clear_env();
        assert!(
            format!("{err:?}").contains("own_device_id"),
            "expected an InvalidConfigValue for signal.own_device_id, got {err:?}"
        );
    }

    #[test]
    #[serial(cognitive_memory)]
    fn own_device_id_below_two_is_a_hard_error() {
        // Device 1 is always the operator's primary phone; signal-cli's linked
        // device is always >= 2. A configured own_device_id < 2 is a mistake, never
        // a silent default.
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: serialized.
        unsafe {
            std::env::set_var(ENV_ENDPOINT, "127.0.0.1:7583");
            std::env::set_var(ENV_ACCOUNT, "+15551230000");
            std::env::set_var(ENV_OWN_DEVICE_ID, "1");
        }
        let err = SignalConfig::load_from(tmp.path()).unwrap_err();
        clear_env();
        assert!(
            format!("{err:?}").contains("own_device_id"),
            "expected a hard error for own_device_id < 2, got {err:?}"
        );
    }
}

// ── Turn-lifecycle parity: Signal inherits idle-liveness (no wall-clock cap) ──

/// Regression guard for issues #2604 and #2607.
///
/// The Signal channel must open its agent session in
/// [`crate::identity::OperatingMode::Meeting`] so every turn runs through the
/// [`crate::meeting_backend::agent_proxy::PersistentAgentProxy`] idle-liveness
/// lifecycle — reap the child only after a generous window of *no output*
/// (every streamed chunk resets the clock), never on elapsed wall-clock time.
/// That is the cross-transport parity #2604 requires and the "no wall-clock
/// turn timeout" #2607 mandates; the hours-scale idle default that governs it
/// is PR #2608.
///
/// If [`super::channel::signal_agent_mode`] is ever flipped to a non-Meeting
/// mode, Signal falls back onto the per-turn adapter and the wall-clock turn
/// timeout that kills long-but-productive turns returns silently. This test
/// fails loudly in that case and points back to the parity requirement.
#[test]
fn signal_opens_agent_in_meeting_mode_for_idle_liveness_parity() {
    use crate::identity::OperatingMode;

    assert_eq!(
        super::channel::signal_agent_mode(),
        OperatingMode::Meeting,
        "Signal must open its agent in Meeting mode so turns inherit \
         PersistentAgentProxy idle-liveness (no wall-clock per-turn cap); \
         see issues #2604 and #2607",
    );
}
