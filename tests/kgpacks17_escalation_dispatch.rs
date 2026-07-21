//! Dispatch regression for the agent-kgpacks-rs issue #17 escalation triage
//! (`docs/investigation/kgpacks-rs-escalation-triage-2026-07-21.md`).
//!
//! The escalation-triage playbook
//! (`prompt_assets/simard/overseer/escalation_triage.md`) requires the operator
//! to receive a **single**, jargon-free, human-readable Signal message covering
//! the situation and the decision — never a per-step transcript and never a raw
//! machine marker.
//!
//! This test locks that contract for the #17 course-correction (decision:
//! complete a goal already delivered by merged PRs). It builds the exact
//! consolidated message the operator receives, fires it through Simard's
//! mandatory `DualChannelNotifier` (email + Signal, "fire every channel, never
//! drop"), and asserts it is
//!
//! 1. exactly ONE delivery per channel (one consolidated message, not a
//!    per-step transcript),
//! 2. `dispatched()` on both channels (the never-dropped guarantee),
//! 3. free of every internal diagnostic marker, and
//! 4. carrying both the situation and the decision in plain English.

use std::sync::{Arc, Mutex};

use simard::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};

/// Internal markers the operator must NEVER see — mirrors the triage playbook's
/// "no jargon" contract (`OODA-SAFEGUARD` / `UNCLEAR-CRITERIA` / `why=` / 🔒),
/// plus the raw per-goal upstream-dependency reason marker.
const JARGON_TOKENS: &[&str] = &[
    "OODA-SAFEGUARD",
    "UNCLEAR-CRITERIA",
    "GENUINELY-STUCK",
    "evidence=[",
    "why=",
    "health-review:per-goal-upstream-dependency",
    "eval recall parity",
    "done-gate",
    "\u{1F512}", // the 🔒 lock marker
];

/// A recording notify channel: captures every notification it is asked to
/// deliver and reports it as delivered.
struct RecordingChannel {
    name: String,
    seen: Arc<Mutex<Vec<OperatorNotification>>>,
}

impl RecordingChannel {
    fn new(name: &str) -> (Self, Arc<Mutex<Vec<OperatorNotification>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                name: name.to_string(),
                seen: seen.clone(),
            },
            seen,
        )
    }
}

impl NotifyChannel for RecordingChannel {
    fn name(&self) -> &str {
        &self.name
    }
    fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
        self.seen.lock().unwrap().push(n.clone());
        ChannelDelivery::Sent
    }
}

/// The single consolidated, jargon-free operator message for the #17
/// course-correction. This is the ONE Signal message the operator receives —
/// situation + decision in a single delivery.
fn kgpacks17_operator_message() -> OperatorNotification {
    let body = "It kept getting stuck on the same thing every cycle. Simard thought this \
         task couldn't be finished because its finish check compares against a baseline \
         that a separate task (issue #16) was supposed to produce first — and it believed \
         that baseline task hadn't been started yet, so it kept re-checking and never \
         shipped anything. Looking closer, both pieces of work are already done and \
         merged: the baseline task shipped and was closed on July 6, and the \
         quantization task itself shipped and was closed on July 7 — the code was merged \
         with the new smaller format left switched off on purpose (exactly as the task \
         said to do when a full comparison wasn't available yet), along with a written \
         report. Both are now closed as completed. So the goal was stuck waiting on \
         something that was already finished. I'm marking it complete — the work it \
         asked for has already shipped. Nothing is needed from you.";

    OperatorNotification {
        kind: "goal-update",
        headline: "embedding-quantization goal was already delivered — marking it complete"
            .to_string(),
        problem: body.to_string(),
        next_step: String::new(),
        link: Some("https://github.com/rysweet/agent-kgpacks-rs/pull/40".to_string()),
        repo: "rysweet/agent-kgpacks-rs".to_string(),
        autonomous: true,
    }
}

#[test]
fn dispatches_exactly_one_jargon_free_operator_message() {
    let (email, email_log) = RecordingChannel::new("email");
    let (signal, signal_log) = RecordingChannel::new("signal");
    let notifier = DualChannelNotifier::new(vec![Box::new(email), Box::new(signal)]);

    let message = kgpacks17_operator_message();

    // The operator-facing body must be plain English — no internal markers and
    // none of the raw diagnostic WHY / reason-marker vocabulary.
    let body = message.plain_text();
    for token in JARGON_TOKENS {
        assert!(
            !body.contains(token),
            "operator message must be jargon-free, but contains {token:?}: {body:?}"
        );
    }

    // Fire the single message once through the mandatory dual-channel notifier.
    let report = notifier.notify(&message);

    // It fired on both channels and every channel delivered (never dropped).
    assert!(
        report.dispatched(),
        "the message must be dispatched: {report:?}"
    );
    assert!(
        report.all_sent(),
        "both channels must deliver the message: {report:?}"
    );

    // Exactly ONE message reached each channel — a single consolidated delivery,
    // not a per-step transcript.
    assert_eq!(
        email_log.lock().unwrap().len(),
        1,
        "operator must receive exactly one email message, not a transcript"
    );
    assert_eq!(
        signal_log.lock().unwrap().len(),
        1,
        "operator must receive exactly one Signal message, not a transcript"
    );

    // The one message carries both the SITUATION and the DECISION in plain
    // English.
    let lower = body.to_lowercase();
    assert!(
        lower.contains("kept getting stuck"),
        "the message must describe the situation: {body:?}"
    );
    assert!(
        lower.contains("already done and merged"),
        "the message must state the finding (work already shipped): {body:?}"
    );
    assert!(
        lower.contains("marking it complete"),
        "the message must state the decision (complete the delivered goal): {body:?}"
    );
    assert!(
        lower.contains("nothing is needed from you"),
        "the message must tell the operator no action is required: {body:?}"
    );
}
