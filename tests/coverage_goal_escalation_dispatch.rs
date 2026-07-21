//! Dispatch regression for the coverage-goal escalation triage
//! (`docs/investigation/coverage-goal-escalation-triage-2026-07-21.md`).
//!
//! The escalation-triage playbook
//! (`prompt_assets/simard/overseer/escalation_triage.md`) requires the operator
//! to receive a **single**, jargon-free, human-readable Signal message covering
//! the situation and the decision — never a per-step transcript and never a raw
//! machine marker.
//!
//! This test locks that contract for the coverage-goal course-correction: it
//! builds the exact consolidated message the operator receives, fires it through
//! Simard's mandatory `DualChannelNotifier` (email + Signal, "fire every
//! channel, never drop"), and asserts it is
//!
//! 1. exactly ONE delivery per channel (one consolidated message, not a
//!    four-part transcript),
//! 2. `dispatched()` on both channels (the never-dropped guarantee),
//! 3. free of every internal diagnostic marker, and
//! 4. carrying both the situation and the decision in plain English.

use std::sync::{Arc, Mutex};

use simard::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};

/// Internal markers the operator must NEVER see — mirrors the triage playbook's
/// "no jargon" contract (`OODA-SAFEGUARD` / `UNCLEAR-CRITERIA` / `why=` / 🔒).
const JARGON_TOKENS: &[&str] = &[
    "OODA-SAFEGUARD",
    "UNCLEAR-CRITERIA",
    "GENUINELY-STUCK",
    "evidence=[",
    "why=",
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

/// The single consolidated, jargon-free operator message for the coverage-goal
/// course-correction. This is the ONE Signal message the operator receives —
/// situation + decision in a single delivery.
fn coverage_goal_operator_message() -> OperatorNotification {
    let body = "It had been stuck for over an hour. The reason was simple: Simard had no \
         automatic way to tell when the goal was finished — its finish line was just \
         the sentence \"raise it to 70%,\" which no automated check ever measured, so \
         every cycle kept re-investigating and never shipped anything. Looking closer, \
         the coverage work itself is already done: every part of Simard we set out to \
         cover was raised above 70% and merged, and the tracking list shows nothing \
         left to do. So I fixed the real problem — the missing finish line. I rewrote \
         it into three concrete checks the system can confirm on its own from files \
         already in the repo, and all three already pass. The goal can now certify \
         itself complete on its next check and will be closed out. Nothing is needed \
         from you. (I did not add a build-failing 70% coverage gate — that was \
         rejected before, so the coverage check stays report-only.)";

    OperatorNotification {
        kind: "goal-update",
        headline: "coverage goal fixed and finishing on its own".to_string(),
        problem: body.to_string(),
        next_step: String::new(),
        link: Some(
            "https://github.com/rysweet/Simard/blob/main/Specs/COVERAGE_AUDIT.md".to_string(),
        ),
        repo: "rysweet/Simard".to_string(),
        autonomous: true,
    }
}

#[test]
fn dispatches_exactly_one_jargon_free_operator_message() {
    let (email, email_log) = RecordingChannel::new("email");
    let (signal, signal_log) = RecordingChannel::new("signal");
    let notifier = DualChannelNotifier::new(vec![Box::new(email), Box::new(signal)]);

    let message = coverage_goal_operator_message();

    // The operator-facing body must be plain English — no internal markers.
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
        lower.contains("stuck for over an hour"),
        "the message must describe the situation: {body:?}"
    );
    assert!(
        lower.contains("rewrote it into three concrete checks"),
        "the message must state the decision (rewritten machine-checkable done-gate): {body:?}"
    );
    assert!(
        lower.contains("nothing is needed from you"),
        "the message must tell the operator no action is required: {body:?}"
    );
}
