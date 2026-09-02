//! `signal-notify` operator-probe command (issue #4276 follow-up).
//!
//! The agentic escalation-triage recipe
//! (`prompt_assets/simard/overseer/escalation_triage.md`) must send the operator
//! a plain-English Signal message per step — and, when it asks the operator a
//! question, actually DELIVER that question rather than merely composing it. The
//! recipe runs as an agent with no direct Rust API, so it needs a THIN CLI seam
//! that posts one plain-English line to the running local Signal JSON-RPC service
//! through the SAME transport the Overseer's own operator notifications use
//! ([`JsonRpcSignalSender`]).
//!
//! This command is that seam. It:
//! - reads the live Signal principals from the environment
//!   ([`SignalRpcConfig::from_env`]: `SIMARD_SIGNAL_RPC_ACCOUNT` +
//!   `SIMARD_SIGNAL_RPC_RECIPIENT`, addr defaulting to the local service),
//! - wraps the body in the reserved anti-self-ingest marker so the inbound Signal
//!   processor deterministically skips Simard's own notice when it syncs back to a
//!   linked device (exactly like [`SignalNotifyChannel`]), and
//! - POSTs it, surfacing any transport/protocol error as `Err` (NEVER a silent
//!   drop) so the recipe learns the operator was not reached.
//!
//! [`SignalNotifyChannel`]: crate::overseer::notify::SignalNotifyChannel

use crate::overseer::notify::{JsonRpcSignalSender, SignalRpcConfig, SignalSender};

/// The exact text a `signal-notify` line puts on the wire. Mirrors
/// `crate::overseer::notify::signal_wire_body`: under the `signal` feature the
/// body carries the reserved [`OPERATOR_NOTIFY_MARKER`] so the inbound processor
/// skips this self-notice; with `signal` off there is no inbound processor and
/// the plain body is sent unchanged.
///
/// [`OPERATOR_NOTIFY_MARKER`]: crate::signal_conversation::gating::OPERATOR_NOTIFY_MARKER
#[cfg(feature = "signal")]
pub(crate) fn operator_notify_wire_body(body: &str) -> String {
    crate::signal_conversation::gating::wrap_operator_notification(body)
}

#[cfg(not(feature = "signal"))]
pub(crate) fn operator_notify_wire_body(body: &str) -> String {
    body.to_string()
}

/// Send one already-validated plain-English operator notice through `sender`,
/// wrapping it in the anti-self-ingest marker first. Split out from
/// [`run_signal_notify_probe`] so it is unit-testable against a captured
/// [`SignalSender`] with no live daemon.
pub(crate) fn send_operator_notice(sender: &dyn SignalSender, message: &str) -> Result<(), String> {
    sender.send_text(&operator_notify_wire_body(message))
}

/// `simard_operator_probe signal-notify <message>` — deliver ONE plain-English
/// operator notice to the running local Signal service.
///
/// Fails LOUD (returns `Err`) when the Signal principals are unset or the send
/// does not complete, so a caller (e.g. the escalation-triage recipe) never
/// mistakes an undelivered message for a delivered one.
pub fn run_signal_notify_probe(message: &str) -> Result<(), Box<dyn std::error::Error>> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err("signal-notify message must not be empty".into());
    }

    let config = SignalRpcConfig::from_env();
    if !config.is_configured() {
        return Err("signal-notify requires SIMARD_SIGNAL_RPC_ACCOUNT and \
             SIMARD_SIGNAL_RPC_RECIPIENT to be set"
            .into());
    }

    let addr = config.addr.clone();
    let sender = JsonRpcSignalSender::new(config);
    send_operator_notice(&sender, trimmed).map_err(|reason| {
        tracing::warn!(
            target: "operator::signal_notify",
            %addr,
            error = %reason,
            "signal-notify failed to reach the operator"
        );
        Box::<dyn std::error::Error>::from(format!("signal-notify send failed: {reason}"))
    })?;

    tracing::info!(
        target: "operator::signal_notify",
        %addr,
        chars = trimmed.chars().count(),
        "signal-notify delivered one plain-English operator notice"
    );
    println!("Probe mode: signal-notify");
    println!("Signal service: {addr}");
    println!(
        "Delivered: operator notice ({} chars)",
        trimmed.chars().count()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A [`SignalSender`] that captures the wire text, or fails on demand, so the
    /// send path is exercised with no live daemon.
    struct CapturingSender {
        sent: Mutex<Vec<String>>,
        fail_with: Option<String>,
    }

    impl CapturingSender {
        fn ok() -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
                fail_with: None,
            }
        }

        fn failing(reason: &str) -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
                fail_with: Some(reason.to_string()),
            }
        }
    }

    impl SignalSender for CapturingSender {
        fn send_text(&self, text: &str) -> Result<(), String> {
            if let Some(reason) = &self.fail_with {
                return Err(reason.clone());
            }
            self.sent.lock().unwrap().push(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn send_operator_notice_delivers_wrapped_body() {
        let sender = CapturingSender::ok();
        send_operator_notice(&sender, "Pick which finish line counts.").unwrap();
        let sent = sender.sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "exactly one notice must be sent");
        assert!(
            sent[0].contains("Pick which finish line counts."),
            "the plain-English body must be on the wire; got {:?}",
            sent[0]
        );
    }

    #[cfg(feature = "signal")]
    #[test]
    fn send_operator_notice_wraps_with_anti_self_ingest_marker() {
        use crate::signal_conversation::gating::{
            OPERATOR_NOTIFY_MARKER, is_operator_notification,
        };
        let sender = CapturingSender::ok();
        send_operator_notice(&sender, "Done — nothing needed from you.").unwrap();
        let sent = sender.sent.lock().unwrap();
        assert!(
            sent[0].starts_with(OPERATOR_NOTIFY_MARKER),
            "the wire body must start with the anti-self-ingest marker; got {:?}",
            sent[0]
        );
        assert!(
            is_operator_notification(&sent[0]),
            "the wire body must be recognised as an operator notification"
        );
    }

    #[test]
    fn send_operator_notice_surfaces_transport_error() {
        let sender = CapturingSender::failing("connect refused");
        let err = send_operator_notice(&sender, "hello").unwrap_err();
        assert!(
            err.contains("connect refused"),
            "the transport error must surface (never a silent drop); got {err}"
        );
    }

    #[test]
    fn run_signal_notify_probe_rejects_empty_message() {
        let err = run_signal_notify_probe("   ").unwrap_err();
        assert!(
            err.to_string().contains("must not be empty"),
            "an empty message must be rejected; got {err}"
        );
    }
}
