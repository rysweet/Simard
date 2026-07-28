//! signal-cli JSON-RPC transport (issue #2527).
//!
//! Simard does **not** implement the Signal protocol. She speaks newline-delimited
//! JSON-RPC 2.0 to a locally-run `signal-cli` daemon (`signal-cli -a <account>
//! daemon --tcp <host:port>`), which owns the account, encryption, and delivery.
//!
//! This module has two halves:
//! - the pure, unit-tested wire helpers [`parse_incoming`] and
//!   [`build_send_request`] (no I/O, no network), and
//! - the [`SignalTransport`] trait plus its live [`JsonRpcTransport`] (a tokio
//!   TCP client, reusing the tokio `net` dependency — no new crate) and the
//!   in-memory [`MockTransport`] used by the channel's tests.
//!
//! # Naming
//!
//! Nothing here is an RPC transport or client. This is the Signal conversation
//! channel's transport; it is unrelated to the cognitive-memory `RpcTransport`.

use std::collections::VecDeque;
use std::future::Future;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::{SimardError, SimardResult};

/// signal-cli always assigns the account owner's **primary phone** device id `1`;
/// every linked device (signal-cli itself, Signal Desktop, an iPad) gets a
/// distinct id `>= 2`. This is the anchor of Note-to-Self loop prevention.
pub(crate) const PRIMARY_DEVICE_ID: u32 = 1;

/// How long a just-sent outbound body suppresses an identical synced-back echo
/// (defence-in-depth loop guard for Note-to-Self setups, issue #2575).
pub(crate) const RECENT_OUTBOUND_TTL: Duration = Duration::from_secs(300);

/// Upper bound on the echo-suppression window so it stays small and bounded.
pub(crate) const RECENT_OUTBOUND_CAP: usize = 64;

/// A parsed inbound Signal message: either a normal `dataMessage` from a separate
/// sender, or a `syncMessage.sentMessage` — a sync of a message the account
/// itself sent (e.g. **Note to Self** on a single-number linked-device setup,
/// issue #2575).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedInbound {
    /// The commanding party's E.164. For a sync-sent message this is the
    /// account's own number, since the account sent the message.
    pub sender: String,
    /// The message body / command text.
    pub body: String,
    /// The originating device id from the envelope, if present. **Never coerced**:
    /// `None` means the envelope carried no `sourceDevice` — it is not defaulted
    /// to `1`, so a missing device can never masquerade as the primary phone.
    pub source_device: Option<u32>,
    /// True iff this came from `syncMessage.sentMessage` — a sync of a message the
    /// account sent (Note to Self, or a message to a third party).
    pub is_sync_sent: bool,
    /// For a sync-sent message, the number the account sent to. Used to tell a
    /// true Note to Self (destined for the account itself) from a sync of a
    /// message to a third party. `None` for a normal `dataMessage`.
    pub sync_destination: Option<String>,
    /// The Signal group id (`groupInfo.groupId`) this message was sent to, if
    /// any — parsed from `dataMessage.groupInfo.groupId` or
    /// `syncMessage.sentMessage.groupInfo.groupId`. Carried through **verbatim**
    /// as delivered (base64/hex, no re-encoding). `None` for a direct
    /// (non-group) message — a missing group can never masquerade as a group,
    /// so the operator-liaison group filter is regression-safe.
    pub group_id: Option<String>,
}

/// Extract `groupInfo.groupId` from a `dataMessage` / `sentMessage` object,
/// returning `None` when the object carries no group (a direct message) or a
/// `groupInfo` without a string `groupId`. Total — never panics on untrusted
/// input.
fn extract_group_id(message: &Value) -> Option<String> {
    message
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Parse one newline-delimited JSON-RPC line from the signal-cli daemon into a
/// [`ParsedInbound`].
///
/// Returns `None` for anything that is not an actionable inbound message —
/// JSON-RPC responses to our own `send` calls, receipts, typing indicators, and
/// unparseable lines are all ignored. signal-cli delivers an incoming message as
/// a `"receive"` notification whose `params.envelope` carries either:
/// - a `dataMessage.message` body (a normal message from a separate sender), or
/// - a `syncMessage.sentMessage` (a sync of a message the **account** sent — the
///   Note-to-Self path for single-number linked-device setups, issue #2575).
///
/// A normal `dataMessage` is parsed exactly as before (`is_sync_sent == false`),
/// so the dedicated-number setup is unchanged. A sync-sent message is surfaced
/// with `is_sync_sent == true`; the channel then applies the primary-phone gate
/// and echo suppression (see [`should_accept_sync_sent`] and
/// [`matches_recent_outbound`]) before treating it as an operator command.
pub fn parse_incoming(line: &str) -> Option<ParsedInbound> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("method")?.as_str()? != "receive" {
        return None;
    }
    let envelope = value.get("params")?.get("envelope")?;
    let sender = envelope
        .get("sourceNumber")
        .and_then(Value::as_str)
        .or_else(|| envelope.get("source").and_then(Value::as_str))?
        .to_string();
    let source_device = envelope
        .get("sourceDevice")
        .and_then(Value::as_u64)
        .map(|d| d as u32);

    // A normal delivered text message (dedicated-number path — unchanged).
    if let Some(data_message) = envelope.get("dataMessage")
        && let Some(text) = data_message.get("message").and_then(Value::as_str)
    {
        if text.is_empty() {
            return None;
        }
        return Some(ParsedInbound {
            sender,
            body: text.to_string(),
            source_device,
            is_sync_sent: false,
            sync_destination: None,
            group_id: extract_group_id(data_message),
        });
    }

    // A sync of a message the account sent (Note to Self on a linked device).
    if let Some(sent) = envelope
        .get("syncMessage")
        .and_then(|m| m.get("sentMessage"))
    {
        let body = sent.get("message").and_then(Value::as_str).unwrap_or("");
        if body.is_empty() {
            return None;
        }
        let sync_destination = sent
            .get("destinationNumber")
            .and_then(Value::as_str)
            .or_else(|| sent.get("destination").and_then(Value::as_str))
            .map(str::to_string);
        return Some(ParsedInbound {
            sender,
            body: body.to_string(),
            source_device,
            is_sync_sent: true,
            sync_destination,
            group_id: extract_group_id(sent),
        });
    }

    // Receipts, typing indicators, and everything else are ignored.
    None
}

/// Decide whether a **sync-sent** (Note-to-Self) message may be accepted as an
/// operator command. This is the pure loop-prevention predicate (issue #2575);
/// all conditions must hold:
///
/// - **Primary-phone gate:** it originated on the operator's phone
///   (`source_device == primary_device_id`, i.e. device 1). Simard's own replies
///   sync back from signal-cli's linked device (id `>= 2`) and are rejected here,
///   closing the loop even with no `own_device_id` configured.
/// - **Own-device rejection (defence-in-depth):** its source device is not
///   signal-cli's own linked-device id, when one is configured.
/// - **True Note to Self:** it was destined for the account itself, not a third
///   party the operator happened to text from their phone.
pub(crate) fn should_accept_sync_sent(
    source_device: Option<u32>,
    own_device_id: Option<u32>,
    destination: Option<&str>,
    account: &str,
    primary_device_id: u32,
) -> bool {
    source_device == Some(primary_device_id)
        && source_device != own_device_id
        && destination == Some(account)
}

/// Whether `body` exactly matches a still-fresh outbound Simard recently sent —
/// used to suppress a synced-back echo of her own message (defence-in-depth loop
/// guard, issue #2575). Entries older than [`RECENT_OUTBOUND_TTL`] never match.
pub(crate) fn matches_recent_outbound(
    body: &str,
    recent: &VecDeque<(String, Instant)>,
    now: Instant,
) -> bool {
    recent
        .iter()
        .any(|(b, t)| b == body && now.saturating_duration_since(*t) <= RECENT_OUTBOUND_TTL)
}

/// Build a newline-delimited JSON-RPC `send` request for the signal-cli daemon.
/// The `account` is included so multi-account daemons route correctly; a
/// single-account daemon ignores it.
pub fn build_send_request(id: u64, account: &str, recipient: &str, text: &str) -> String {
    let req = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "send",
        "params": {
            "account": account,
            "recipient": [recipient],
            "message": text,
        }
    });
    req.to_string()
}

/// Build a newline-delimited JSON-RPC `send` request targeting a Signal **group**
/// (`params.groupId`), not a single recipient. Used by the Overseer operator-
/// liaison to reply on the operator group. `recipient` and `groupId` are mutually
/// exclusive in one request; this variant emits only `groupId`.
pub fn build_send_request_group(id: u64, account: &str, group_id: &str, text: &str) -> String {
    let req = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "send",
        "params": {
            "account": account,
            "groupId": group_id,
            "message": text,
        }
    });
    req.to_string()
}

/// A newline-delimited JSON-RPC line transport to the signal-cli daemon.
///
/// Kept deliberately minimal (read a line, write a line) so the [`SignalConversation`](super::SignalConversation)
/// channel owns all Signal semantics and can be tested against [`MockTransport`]
/// with no live daemon or network.
pub trait SignalTransport {
    /// Read the next line from the daemon. `Ok(None)` means the socket closed.
    fn recv_line(&mut self) -> impl Future<Output = SimardResult<Option<String>>> + Send;

    /// Write one JSON-RPC request line (a newline is appended by the transport).
    fn send_line(&mut self, line: String) -> impl Future<Output = SimardResult<()>> + Send;
}

/// The live tokio-TCP transport to a signal-cli JSON-RPC daemon.
pub struct JsonRpcTransport {
    reader: tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl JsonRpcTransport {
    /// Connect to the signal-cli daemon at `endpoint` (`host:port`).
    pub async fn connect(endpoint: &str) -> SimardResult<Self> {
        let stream = tokio::net::TcpStream::connect(endpoint)
            .await
            .map_err(|e| SimardError::ActionExecutionFailed {
                action: "signal-connect".to_string(),
                reason: format!("could not connect to signal-cli daemon at {endpoint}: {e}"),
            })?;
        let (read_half, write_half) = stream.into_split();
        Ok(Self {
            reader: tokio::io::BufReader::new(read_half),
            writer: write_half,
        })
    }
}

impl SignalTransport for JsonRpcTransport {
    fn recv_line(&mut self) -> impl Future<Output = SimardResult<Option<String>>> + Send {
        use tokio::io::AsyncBufReadExt;
        async move {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await.map_err(|e| {
                SimardError::ActionExecutionFailed {
                    action: "signal-transport".to_string(),
                    reason: format!("read from signal-cli daemon failed: {e}"),
                }
            })?;
            if n == 0 { Ok(None) } else { Ok(Some(line)) }
        }
    }

    fn send_line(&mut self, line: String) -> impl Future<Output = SimardResult<()>> + Send {
        use tokio::io::AsyncWriteExt;
        async move {
            self.writer
                .write_all(line.as_bytes())
                .await
                .and(self.writer.write_all(b"\n").await)
                .and(self.writer.flush().await)
                .map_err(|e| SimardError::ActionExecutionFailed {
                    action: "signal-transport".to_string(),
                    reason: format!("write to signal-cli daemon failed: {e}"),
                })
        }
    }
}

/// An in-memory [`SignalTransport`] for tests: `recv_line` replays scripted lines
/// then ends the stream, and `send_line` captures every written line.
#[cfg(test)]
pub struct MockTransport {
    inbound: std::collections::VecDeque<String>,
    pub sent: Vec<String>,
}

#[cfg(test)]
impl MockTransport {
    /// Build from a script of raw JSON-RPC lines the daemon would emit.
    pub fn with_lines(lines: Vec<String>) -> Self {
        Self {
            inbound: lines.into_iter().collect(),
            sent: Vec::new(),
        }
    }
}

#[cfg(test)]
impl SignalTransport for MockTransport {
    fn recv_line(&mut self) -> impl Future<Output = SimardResult<Option<String>>> + Send {
        let next = self.inbound.pop_front();
        async move { Ok(next) }
    }

    fn send_line(&mut self, line: String) -> impl Future<Output = SimardResult<()>> + Send {
        self.sent.push(line);
        async move { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    use super::*;

    const ACCOUNT: &str = "+15551230000";
    const THIRD_PARTY: &str = "+15559990000";

    // ── parse_incoming: normal dataMessage (regression — unchanged behavior) ──

    #[test]
    fn parse_incoming_extracts_sender_and_text() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"account":"+15551230000","envelope":{"source":"+15557654321","sourceNumber":"+15557654321","timestamp":1,"dataMessage":{"message":"status"}}}}"#;
        let msg = parse_incoming(line).expect("a delivered text message");
        assert_eq!(msg.sender, "+15557654321");
        assert_eq!(msg.body, "status");
        assert!(
            !msg.is_sync_sent,
            "a dataMessage is not a sync-sent message"
        );
        assert_eq!(msg.sync_destination, None);
    }

    #[test]
    fn parse_incoming_prefers_source_number() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"uuid-form","sourceNumber":"+15557654321","dataMessage":{"message":"hi"}}}}"#;
        let msg = parse_incoming(line).unwrap();
        assert_eq!(msg.sender, "+15557654321");
        assert!(!msg.is_sync_sent);
    }

    #[test]
    fn parse_incoming_reads_source_device_when_present() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15557654321","sourceDevice":1,"dataMessage":{"message":"hi"}}}}"#;
        let msg = parse_incoming(line).unwrap();
        assert_eq!(msg.source_device, Some(1));
    }

    #[test]
    fn parse_incoming_missing_source_device_is_none_never_coerced() {
        // A dataMessage without sourceDevice must yield None — never coerced to 1.
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15557654321","dataMessage":{"message":"hi"}}}}"#;
        let msg = parse_incoming(line).unwrap();
        assert_eq!(msg.source_device, None);
    }

    // ── parse_incoming: syncMessage.sentMessage (Note to Self, issue #2575) ──

    #[test]
    fn parse_incoming_parses_sync_sent_note_to_self() {
        // The account sends "status" to itself from the primary phone (device 1);
        // signal-cli delivers this as a sync-sent message, not a dataMessage.
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551230000","sourceNumber":"+15551230000","sourceDevice":1,"syncMessage":{"sentMessage":{"destinationNumber":"+15551230000","message":"status"}}}}}"#;
        let msg = parse_incoming(line).expect("a sync-sent Note-to-Self message");
        assert!(
            msg.is_sync_sent,
            "a syncMessage.sentMessage is a sync-sent message"
        );
        assert_eq!(
            msg.sender, ACCOUNT,
            "the sender is the account's own number"
        );
        assert_eq!(msg.body, "status");
        assert_eq!(
            msg.source_device,
            Some(1),
            "originated on the primary phone"
        );
        assert_eq!(
            msg.sync_destination.as_deref(),
            Some(ACCOUNT),
            "a true Note to Self is destined for the account itself"
        );
    }

    #[test]
    fn parse_incoming_sync_sent_falls_back_to_destination_field() {
        // Some envelopes carry `destination` rather than `destinationNumber`.
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15551230000","sourceDevice":1,"syncMessage":{"sentMessage":{"destination":"+15551230000","message":"pause"}}}}}"#;
        let msg = parse_incoming(line).expect("sync-sent parsed");
        assert!(msg.is_sync_sent);
        assert_eq!(msg.body, "pause");
        assert_eq!(msg.sync_destination.as_deref(), Some(ACCOUNT));
    }

    #[test]
    fn parse_incoming_sync_sent_records_higher_source_device() {
        // A reply Simard emitted from signal-cli's linked device syncs back with a
        // device id >= 2. The parser must surface it faithfully (never coerce to 1).
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15551230000","sourceDevice":3,"syncMessage":{"sentMessage":{"destinationNumber":"+15551230000","message":"echo"}}}}}"#;
        let msg = parse_incoming(line).unwrap();
        assert!(msg.is_sync_sent);
        assert_eq!(msg.source_device, Some(3));
    }

    #[test]
    fn parse_incoming_sync_sent_missing_source_device_is_none() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15551230000","syncMessage":{"sentMessage":{"destinationNumber":"+15551230000","message":"hi"}}}}}"#;
        let msg = parse_incoming(line).unwrap();
        assert!(msg.is_sync_sent);
        assert_eq!(msg.source_device, None);
    }

    // ── parse_incoming: everything else is ignored ──

    #[test]
    fn parse_incoming_ignores_non_receive_lines() {
        // A JSON-RPC response to our own send call — not an inbound message.
        let resp = r#"{"jsonrpc":"2.0","id":7,"result":{"timestamp":123}}"#;
        assert!(parse_incoming(resp).is_none());
    }

    #[test]
    fn parse_incoming_ignores_receipts_without_data_message() {
        let receipt = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15557654321","receiptMessage":{"isDelivery":true}}}}"#;
        assert!(parse_incoming(receipt).is_none());
    }

    #[test]
    fn parse_incoming_ignores_typing_indicators() {
        let typing = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15557654321","typingMessage":{"action":"STARTED"}}}}"#;
        assert!(parse_incoming(typing).is_none());
    }

    #[test]
    fn parse_incoming_ignores_empty_sync_sent_body() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15551230000","sourceDevice":1,"syncMessage":{"sentMessage":{"destinationNumber":"+15551230000","message":""}}}}}"#;
        assert!(
            parse_incoming(line).is_none(),
            "an empty body is not a command"
        );
    }

    #[test]
    fn parse_incoming_ignores_unparseable_lines() {
        assert!(parse_incoming("not json").is_none());
        assert!(parse_incoming("").is_none());
    }

    // ── should_accept_sync_sent: the pure acceptance predicate ──

    #[test]
    fn accept_sync_sent_from_primary_phone_to_account() {
        // device 1 + destination == account + no own-device config → accepted.
        assert!(should_accept_sync_sent(
            Some(1),
            None,
            Some(ACCOUNT),
            ACCOUNT,
            1
        ));
    }

    #[test]
    fn reject_sync_sent_from_linked_device_even_without_own_device_id() {
        // A linked device (>= 2) is rejected by the primary-phone gate alone, even
        // with own_device_id == None — the loop-free guarantee needs no config.
        assert!(!should_accept_sync_sent(
            Some(2),
            None,
            Some(ACCOUNT),
            ACCOUNT,
            1
        ));
        assert!(!should_accept_sync_sent(
            Some(3),
            None,
            Some(ACCOUNT),
            ACCOUNT,
            1
        ));
    }

    #[test]
    fn reject_sync_sent_from_signal_cli_own_device_id() {
        // Defence-in-depth: an explicit own_device_id match is rejected even if it
        // somehow presented as the primary device.
        assert!(!should_accept_sync_sent(
            Some(1),
            Some(1),
            Some(ACCOUNT),
            ACCOUNT,
            1
        ));
    }

    #[test]
    fn reject_sync_sent_to_third_party() {
        // A sync of a message the operator sent to someone else is not a command.
        assert!(!should_accept_sync_sent(
            Some(1),
            None,
            Some(THIRD_PARTY),
            ACCOUNT,
            1
        ));
    }

    #[test]
    fn reject_sync_sent_without_destination() {
        assert!(!should_accept_sync_sent(Some(1), None, None, ACCOUNT, 1));
    }

    #[test]
    fn reject_sync_sent_without_source_device() {
        // A missing source device fails the primary-phone gate (fail-closed).
        assert!(!should_accept_sync_sent(
            None,
            None,
            Some(ACCOUNT),
            ACCOUNT,
            1
        ));
    }

    // ── matches_recent_outbound: echo-suppression window ──

    #[test]
    fn recent_outbound_matches_exact_fresh_body() {
        let t0 = Instant::now();
        let mut recent: VecDeque<(String, Instant)> = VecDeque::new();
        recent.push_back(("PR #7 is merge-ready".to_string(), t0));
        assert!(matches_recent_outbound(
            "PR #7 is merge-ready",
            &recent,
            t0 + Duration::from_secs(1),
        ));
    }

    #[test]
    fn recent_outbound_ignores_expired_entries() {
        let t0 = Instant::now();
        let mut recent: VecDeque<(String, Instant)> = VecDeque::new();
        recent.push_back(("stale".to_string(), t0));
        // Past the TTL, an identical body is no longer suppressed.
        let now = t0 + RECENT_OUTBOUND_TTL + Duration::from_secs(1);
        assert!(!matches_recent_outbound("stale", &recent, now));
    }

    #[test]
    fn recent_outbound_no_match_for_different_body() {
        let t0 = Instant::now();
        let mut recent: VecDeque<(String, Instant)> = VecDeque::new();
        recent.push_back(("one".to_string(), t0));
        assert!(!matches_recent_outbound(
            "two",
            &recent,
            t0 + Duration::from_secs(1)
        ));
    }

    #[test]
    fn recent_outbound_empty_window_never_matches() {
        let recent: VecDeque<(String, Instant)> = VecDeque::new();
        assert!(!matches_recent_outbound(
            "anything",
            &recent,
            Instant::now()
        ));
    }

    // ── build_send_request (unchanged) ──

    #[test]
    fn build_send_request_is_valid_jsonrpc() {
        let line = build_send_request(3, "+15551230000", "+15557654321", "hello");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 3);
        assert_eq!(v["method"], "send");
        assert_eq!(v["params"]["account"], "+15551230000");
        assert_eq!(v["params"]["recipient"][0], "+15557654321");
        assert_eq!(v["params"]["message"], "hello");
    }
}
