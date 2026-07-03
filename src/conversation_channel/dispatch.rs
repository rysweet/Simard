//! The shared, pure, synchronous record/status dispatcher.
//!
//! [`apply_record`] is the per-channel record logic that the CLI REPL and the
//! dashboard chat loop previously duplicated, extracted **once**. It performs a
//! record command's backend mutation and returns the *canonical* acknowledgement
//! text + [`OutKind`]; it does **not** render and does **not** perform any I/O or
//! LLM turn. Commands that call `send_message`/`close`, write files, or fire an
//! LLM turn (`Conversation`, `Close`, `Export`, `Template`) return `None` and are
//! handled by [`super::driver::run_conversation`] via `spawn_blocking`.
//!
//! The record acknowledgement strings returned here are exactly the ones the CLI
//! and dashboard already produced — the extraction changes no observable
//! behavior for the `/goal`, `/decision`, `/action`, `/question`, `/theme`,
//! `/owner`, `/risk`, and `/disagree` commands. The read-only `Status` /
//! `State` / `Recap` / `Preview` / `Help` / `Unknown` text is rendered here for
//! the channels that drive through [`super::driver::run_conversation`] (Signal +
//! the mock); the CLI and dashboard keep their own richer rendering of those
//! read-only views.

use crate::meeting_backend::MeetingBackend;
use crate::meeting_backend::command::{MeetingCommand, render_help_plain, unknown_command_notice};
use crate::meeting_backend::persist::{
    extract_action_items, extract_decisions, extract_open_questions,
};

use super::OutKind;

/// The canonical result of a record/status command: message text + kind.
/// Rendering is left to the channel; the record text is identical across
/// channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub kind: OutKind,
    pub text: String,
}

/// Apply a record/status command's backend mutation and return its canonical
/// message. Returns `None` for commands the driver handles directly
/// (`Conversation`, `Close`, `Export`, `Template`) because they perform an LLM
/// turn or file I/O and so cannot be part of this pure, synchronous applier.
pub fn apply_record(backend: &mut MeetingBackend, cmd: &MeetingCommand) -> Option<Recorded> {
    match cmd {
        // ── Structured capture (mutating) — canonical acks, byte-for-byte the
        //    strings the CLI and dashboard already emit. ──
        MeetingCommand::Theme(text) => {
            backend.push_theme(text.clone());
            Some(recorded(format!("Theme recorded: {text}")))
        }
        MeetingCommand::Decision { text, rationale } => {
            backend.push_explicit_decision(text, rationale.as_deref());
            let msg = if let Some(r) = rationale {
                format!("Decision recorded: {text} (rationale: {r})")
            } else {
                format!("Decision recorded: {text}")
            };
            Some(recorded(msg))
        }
        MeetingCommand::Action(text) => {
            backend.push_explicit_action_item(text);
            Some(recorded(format!("Action recorded: {text}")))
        }
        MeetingCommand::Question(text) => {
            backend.push_explicit_question(text);
            Some(recorded(format!("Question recorded: {text}")))
        }
        MeetingCommand::Owner(text) => {
            backend.push_next_owner(text);
            Some(recorded(format!("Next owner recorded: {text}")))
        }
        MeetingCommand::Goal(text) => {
            backend.set_goal(text);
            Some(recorded(format!("Goal recorded: {text}")))
        }
        MeetingCommand::Risk(text) => {
            backend.push_explicit_risk(text);
            Some(recorded(format!("Risk recorded: {text}")))
        }
        MeetingCommand::Disagree(text) => {
            backend.push_explicit_disagreement(text);
            Some(recorded(format!("Disagreement recorded: {text}")))
        }

        // ── Read-only views (no mutation) — `Status` kind. ──
        MeetingCommand::Status => Some(status(render_status(backend))),
        MeetingCommand::Recap => Some(status(render_recap(backend))),
        MeetingCommand::Preview => Some(status(render_preview(backend))),
        MeetingCommand::State => Some(status(render_state(backend))),
        MeetingCommand::Help => Some(status(render_help_plain())),
        MeetingCommand::Unknown { input, suggestion } => {
            Some(status(unknown_command_notice(input, suggestion.as_deref())))
        }

        // ── Driver-handled (LLM turn / file I/O / lifecycle). ──
        MeetingCommand::Conversation(_)
        | MeetingCommand::Close
        | MeetingCommand::Export
        | MeetingCommand::Template(_) => None,
    }
}

fn recorded(text: String) -> Recorded {
    Recorded {
        kind: OutKind::Recorded,
        text,
    }
}

fn status(text: String) -> Recorded {
    Recorded {
        kind: OutKind::Status,
        text,
    }
}

fn render_status(backend: &MeetingBackend) -> String {
    let s = backend.status();
    format!(
        "Topic: {}\nMessages: {}\nStarted: {}\nOpen: {}",
        s.topic, s.message_count, s.started_at, s.is_open
    )
}

fn render_recap(backend: &MeetingBackend) -> String {
    let s = backend.status();
    let mut out = format!(
        "── Meeting Recap ──\nTopic: {}\nMessages: {}\nStarted: {}",
        s.topic, s.message_count, s.started_at
    );
    let themes = backend.explicit_themes();
    if !themes.is_empty() {
        out.push_str(&format!("\nThemes: {}", themes.join(", ")));
    }
    out
}

fn render_preview(backend: &MeetingBackend) -> String {
    let s = backend.status();
    let themes = backend.explicit_themes();
    format!(
        "── Handoff Preview ──\nTopic: {}\nMessages so far: {}\nThemes: {}",
        s.topic,
        s.message_count,
        if themes.is_empty() {
            "none yet".to_string()
        } else {
            themes.join(", ")
        }
    )
}

fn render_state(backend: &MeetingBackend) -> String {
    let messages = backend.history();
    let decisions = extract_decisions(messages);
    let open_questions = extract_open_questions(messages);
    let action_items = extract_action_items(messages);

    let mut body = String::from("── Decisions ──\n");
    if decisions.is_empty() {
        body.push_str("  (none)\n");
    } else {
        for (i, d) in decisions.iter().enumerate() {
            body.push_str(&format!("  {}. {d}\n", i + 1));
        }
    }
    body.push_str("\n── Open Questions ──\n");
    if open_questions.is_empty() {
        body.push_str("  (none)\n");
    } else {
        for q in &open_questions {
            body.push_str(&format!("  - {}\n", q.text));
        }
    }
    body.push_str("\n── Action Items ──\n");
    if action_items.is_empty() {
        body.push_str("  (none)\n");
    } else {
        for (i, item) in action_items.iter().enumerate() {
            body.push_str(&format!("  {}. {}\n", i + 1, item.description));
        }
    }
    body
}
