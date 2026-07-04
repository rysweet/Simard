//! The one unified conversation loop.
//!
//! [`run_conversation`] is the single meeting-conversation driver that the new
//! [`SignalConversation`](crate::signal_conversation) channel and the
//! [`MockConversationChannel`](super::MockConversationChannel) drive through (and
//! that any future channel can reuse). It calls `channel.recv()`, parses with the
//! existing `parse_command`, routes conversation turns and record commands
//! through the shared [`apply_record`], fires the per-channel `on_recorded` hook
//! for structured captures, and drives `/close`.
//!
//! The engine ([`MeetingBackend`]) stays synchronous and unchanged. Its
//! `send_message` / `close` / `apply_template` calls complete synchronously
//! *before* any `.await`, so no `&mut MeetingBackend` is ever held across an
//! await point.

use crate::error::SimardResult;
use crate::meeting_backend::MeetingBackend;
use crate::meeting_backend::command::{MeetingCommand, parse_command};

use super::{ConversationChannel, OutKind, Outbound, apply_record};

/// Drive one operator↔Simard conversation to completion over `channel`, using
/// `backend` as the (synchronous) meeting engine. Returns when the operator
/// closes the meeting or the channel reaches end-of-stream.
pub async fn run_conversation<C: ConversationChannel>(
    channel: &mut C,
    backend: &mut MeetingBackend,
) -> SimardResult<()> {
    while let Some(inbound) = channel.recv().await? {
        match parse_command(&inbound.text) {
            MeetingCommand::Close => {
                let out = close_backend(backend);
                channel.send(out).await?;
                break;
            }
            MeetingCommand::Conversation(text) => {
                if text.is_empty() {
                    continue;
                }
                channel.send(conversation_turn(backend, &text)).await?;
            }
            MeetingCommand::Export => {
                channel.send(export(backend)).await?;
            }
            MeetingCommand::Template(name) => {
                channel.send(template(backend, &name)).await?;
            }
            other => {
                if let Some(rec) = apply_record(backend, &other) {
                    let is_record = rec.kind == OutKind::Recorded;
                    channel
                        .send(Outbound {
                            kind: rec.kind,
                            text: rec.text,
                        })
                        .await?;
                    // The per-channel post-record hook fires only for actual
                    // structured captures, never for read-only status views —
                    // this is what keeps the CLI capture tally off `/status`.
                    if is_record {
                        channel.on_recorded(backend).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Run one conversational (LLM) turn and render the reply as an `Assistant`
/// outbound, or an `Error` outbound if the engine turn fails.
fn conversation_turn(backend: &mut MeetingBackend, text: &str) -> Outbound {
    match backend.send_message(text) {
        Ok(resp) => Outbound {
            kind: OutKind::Assistant,
            text: resp.content,
        },
        Err(e) => Outbound {
            kind: OutKind::Error,
            text: format!("[error: {e}]"),
        },
    }
}

/// Close the meeting, writing the handoff bundle exactly as before, and render a
/// closing summary. Errors surface as an `Error` outbound; the loop ends either
/// way because the session is over.
fn close_backend(backend: &mut MeetingBackend) -> Outbound {
    match backend.close() {
        Ok(s) => {
            let mut body = format!(
                "Meeting closed. {} messages. Summary: {}",
                s.message_count, s.summary_text
            );
            if let Some(dir) = &s.bundle_dir {
                body.push_str(&format!("\nBundle: {dir}"));
            }
            if let Some(md) = &s.markdown_report_path {
                body.push_str(&format!("\nReport: {md}"));
            }
            Outbound {
                kind: OutKind::Status,
                text: body,
            }
        }
        Err(e) => Outbound {
            kind: OutKind::Error,
            text: format!("Meeting closed with error: {e}"),
        },
    }
}

/// Export the current transcript to markdown and render the resulting path.
fn export(backend: &MeetingBackend) -> Outbound {
    use crate::meeting_backend::persist::write_markdown_export;
    match write_markdown_export(backend.topic(), backend.started_at(), backend.history()) {
        Ok(path) => Outbound {
            kind: OutKind::Status,
            text: format!("Meeting exported to: {}", path.display()),
        },
        Err(e) => Outbound {
            kind: OutKind::Error,
            text: format!("[export error: {e}]"),
        },
    }
}

/// Apply a meeting template: list templates for an empty name, otherwise record
/// the agenda on the session and fire a single LLM context-injection turn.
fn template(backend: &mut MeetingBackend, name: &str) -> Outbound {
    use crate::meeting_backend::persist::{TEMPLATES, find_template};
    if name.is_empty() {
        let mut listing = String::from("Available templates:\n");
        for t in TEMPLATES {
            listing.push_str(&format!("  {} — {}\n", t.name, t.description));
        }
        listing.push_str("\nUsage: /template <name>");
        return Outbound {
            kind: OutKind::Status,
            text: listing,
        };
    }
    let Some(tmpl) = find_template(name) else {
        return Outbound {
            kind: OutKind::Status,
            text: format!("Unknown template: {name}. Available: standup, 1on1, retro, planning"),
        };
    };
    backend.apply_template(tmpl.name, tmpl.agenda);
    let ctx = format!(
        "The operator has selected the '{}' meeting template. \
         Please follow this agenda:\n{}",
        tmpl.name, tmpl.agenda
    );
    match backend.send_message(&ctx) {
        Ok(resp) => Outbound {
            kind: OutKind::Assistant,
            text: resp.content,
        },
        Err(e) => Outbound {
            kind: OutKind::Error,
            text: format!("[template error: {e}]"),
        },
    }
}
