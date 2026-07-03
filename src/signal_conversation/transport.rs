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
//! Nothing here is named `adapter`/`Adapter`. This is the Signal conversation
//! channel's transport; it is unrelated to the cognitive-memory `ServerTransport`.

use std::future::Future;

use serde_json::{Value, json};

use crate::error::{SimardError, SimardResult};

/// Parse one newline-delimited JSON-RPC line from the signal-cli daemon into an
/// inbound `(sender_e164, message_text)` pair.
///
/// Returns `None` for anything that is not a delivered text message — JSON-RPC
/// responses to our own `send` calls, receipts, typing indicators, sync
/// messages, and unparseable lines are all ignored. signal-cli delivers an
/// incoming message as a `"receive"` notification whose `params.envelope`
/// carries the source number and a `dataMessage.message` body.
pub fn parse_incoming(line: &str) -> Option<(String, String)> {
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
    let text = envelope
        .get("dataMessage")?
        .get("message")?
        .as_str()?
        .to_string();
    if text.is_empty() {
        return None;
    }
    Some((sender, text))
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
    use super::*;

    #[test]
    fn parse_incoming_extracts_sender_and_text() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"account":"+15551230000","envelope":{"source":"+15557654321","sourceNumber":"+15557654321","timestamp":1,"dataMessage":{"message":"status"}}}}"#;
        let (sender, text) = parse_incoming(line).expect("a delivered text message");
        assert_eq!(sender, "+15557654321");
        assert_eq!(text, "status");
    }

    #[test]
    fn parse_incoming_prefers_source_number() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"uuid-form","sourceNumber":"+15557654321","dataMessage":{"message":"hi"}}}}"#;
        let (sender, _) = parse_incoming(line).unwrap();
        assert_eq!(sender, "+15557654321");
    }

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
    fn parse_incoming_ignores_unparseable_lines() {
        assert!(parse_incoming("not json").is_none());
        assert!(parse_incoming("").is_none());
    }

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
