//! TDD tests for the Signal **group** transport gap (design component C1) — the
//! largest real gap the operator-liaison closes.
//!
//! Previously `transport::parse_incoming` extracted only `sourceNumber` and
//! `build_send_request` targeted a single recipient. The liaison needs group
//! inbound *and* outbound:
//!
//!   - **Inbound.** `ParsedInbound` gains `group_id: Option<String>`, parsed from
//!     `params.envelope.dataMessage.groupInfo.groupId` AND
//!     `params.envelope.syncMessage.sentMessage.groupInfo.groupId`. A non-group
//!     (direct) message ⇒ `group_id: None` (regression-safe). Parsing stays
//!     total and never panics on untrusted input.
//!   - **Outbound.** A new `build_send_request_group(id, account, group_id, text)`
//!     emits a JSON-RPC `send` with `params.groupId` (NOT a single `recipient`),
//!     so replies land in the operator group. The single-recipient path is
//!     unchanged.
//!
//! These reference the NEW `ParsedInbound.group_id` field and
//! `build_send_request_group`, so they FAIL TO COMPILE until C1 lands — the
//! intended TDD red state.

use serde_json::Value;

use crate::signal_conversation::transport::{build_send_request_group, parse_incoming};

const GROUP_ID: &str = "cGxheS9ncm91cCsx=="; // opaque base64: holds '/', '+', '='

// ─────────────────────── inbound group id parsing ───────────────────────────

#[test]
fn parse_incoming_extracts_group_id_from_data_message() {
    let line = format!(
        r#"{{"jsonrpc":"2.0","method":"receive","params":{{"envelope":{{"sourceNumber":"+15557654321","dataMessage":{{"message":"please look at the canary","groupInfo":{{"groupId":{GROUP_ID:?}}}}}}}}}}}"#
    );
    let msg = parse_incoming(&line).expect("a delivered group message");
    assert_eq!(msg.sender, "+15557654321");
    assert_eq!(msg.body, "please look at the canary");
    assert_eq!(
        msg.group_id.as_deref(),
        Some(GROUP_ID),
        "dataMessage.groupInfo.groupId must be parsed into ParsedInbound.group_id"
    );
    assert!(!msg.is_sync_sent);
}

#[test]
fn parse_incoming_extracts_group_id_from_sync_sent_message() {
    let line = format!(
        r#"{{"jsonrpc":"2.0","method":"receive","params":{{"envelope":{{"sourceNumber":"+15551230000","sourceDevice":1,"syncMessage":{{"sentMessage":{{"destinationNumber":"+15551230000","message":"go ahead and fix it","groupInfo":{{"groupId":{GROUP_ID:?}}}}}}}}}}}}}"#
    );
    let msg = parse_incoming(&line).expect("a sync-sent group message");
    assert!(msg.is_sync_sent);
    assert_eq!(msg.body, "go ahead and fix it");
    assert_eq!(
        msg.group_id.as_deref(),
        Some(GROUP_ID),
        "syncMessage.sentMessage.groupInfo.groupId must be parsed into group_id"
    );
}

#[test]
fn parse_incoming_direct_message_has_no_group_id_regression() {
    // A normal direct (non-group) dataMessage must parse with group_id: None —
    // the dedicated/direct path is unchanged and can never masquerade as a group.
    let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15557654321","dataMessage":{"message":"status"}}}}"#;
    let msg = parse_incoming(line).expect("a direct message");
    assert_eq!(msg.body, "status");
    assert_eq!(
        msg.group_id, None,
        "a non-group message must yield group_id: None (regression-safe)"
    );
}

#[test]
fn parse_incoming_group_info_without_group_id_is_none_never_panics() {
    // Untrusted input: a groupInfo object missing the groupId key must yield
    // None, not a panic.
    let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15557654321","dataMessage":{"message":"hi","groupInfo":{}}}}}"#;
    let msg = parse_incoming(line).expect("parsed");
    assert_eq!(msg.group_id, None);
}

// ─────────────────────── outbound group send request ────────────────────────

#[test]
fn build_send_request_group_targets_group_not_recipient() {
    let line = build_send_request_group(42, "+15551230000", GROUP_ID, "on it");
    let v: Value = serde_json::from_str(&line).expect("valid JSON-RPC");
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 42);
    assert_eq!(v["method"], "send");
    let params = &v["params"];
    assert_eq!(params["account"], "+15551230000");
    assert_eq!(params["message"], "on it");
    assert_eq!(
        params["groupId"], GROUP_ID,
        "a group send must set params.groupId to the target group"
    );
    assert!(
        params.get("recipient").is_none(),
        "a group send must NOT carry a single `recipient`"
    );
}

#[test]
fn build_send_request_group_is_single_line() {
    // The daemon reads newline-delimited requests; the request body itself must
    // be a single line (no embedded newline that would split the frame).
    let line = build_send_request_group(1, "+15551230000", GROUP_ID, "multi\nword");
    assert!(
        !line.contains('\n'),
        "the serialized request must not contain a raw newline"
    );
}
