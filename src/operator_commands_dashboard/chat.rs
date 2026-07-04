use axum::{
    extract::{
        Query,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response,
};

use serde::Deserialize;
use serde_json::json;

use crate::error::{SimardError, SimardResult};
use crate::meeting_backend::{ConversationMessage, Role};

/// Maximum size (bytes) of a single inbound chat WebSocket frame / user
/// message. The channel is text chat, not file transfer; oversized frames are
/// refused, never persisted.
const MAX_CHAT_FRAME_BYTES: usize = 64 * 1024;

/// Target character count per streamed assistant `chunk` frame. The completed
/// reply is split into fixed char windows so the client renders it
/// incrementally (server-side chunking — see docs/reference/dashboard-chat.md).
const STREAM_CHUNK_CHARS: usize = 48;

/// Query parameters for `GET /ws/chat`. `session_id` selects an existing
/// session to resume; when absent a fresh session is minted lazily on the
/// first user message.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ChatWsParams {
    #[serde(default)]
    session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// WebSocket chat — bridges to Simard's meeting facilitator conversation model
// ---------------------------------------------------------------------------

/// Load the meeting system prompt from disk.
fn load_dashboard_meeting_prompt() -> SimardResult<String> {
    let candidates = [
        // Runtime: next to the binary
        std::env::current_exe().ok().and_then(|p| {
            p.parent()
                .map(|d| d.join("prompt_assets/simard/meeting_system.md"))
        }),
        // Runtime: repo checkout (common on the Simard VM)
        Some(
            std::path::PathBuf::from(
                std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string()),
            )
            .join("src/Simard/prompt_assets/simard/meeting_system.md"),
        ),
        // Build-time: source tree via CARGO_MANIFEST_DIR (dev only)
        Some(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("prompt_assets/simard/meeting_system.md"),
        ),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(&candidate) {
            return Ok(content);
        }
    }
    Err(SimardError::PromptNotFound {
        name: "meeting_system.md".into(),
    })
}

/// Open an agent session for the dashboard chat.
/// Uses the same config-driven provider as the CLI meeting REPL
/// (resolved via `RuntimeConfig`: env var → `~/.simard/config.toml`).
fn open_dashboard_agent_session() -> Option<Box<dyn crate::base_types::BaseTypeSession>> {
    let provider = match crate::session_builder::LlmProvider::resolve() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[simard] dashboard chat: LLM provider not configured: {e}");
            return None;
        }
    };
    match crate::session_builder::SessionBuilder::new(
        crate::identity::OperatingMode::Meeting,
        provider,
    )
    .node_id("dashboard-chat")
    .address("dashboard-chat://local")
    .adapter_tag("meeting-dashboard")
    .open()
    {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[simard] dashboard chat session failed: {e}");
            None
        }
    }
}

pub(crate) async fn ws_chat_handler(
    Query(params): Query<ChatWsParams>,
    ws: WebSocketUpgrade,
) -> response::Response {
    // Validate any requested session_id BEFORE upgrading — mirroring the
    // agent-log WS handler: a malformed id is a 400, never a path join.
    let requested = params.session_id.filter(|s| !s.is_empty());
    if let Some(ref id) = requested
        && !super::chat_store::validate_session_id(id)
    {
        return response::Response::builder()
            .status(400)
            .header("content-type", "text/plain; charset=utf-8")
            .body(axum::body::Body::from(
                "invalid session_id: must match ^[A-Za-z0-9_-]{1,64}$",
            ))
            .unwrap();
    }
    ws.max_message_size(MAX_CHAT_FRAME_BYTES)
        .max_frame_size(MAX_CHAT_FRAME_BYTES)
        .on_upgrade(move |socket| handle_ws_chat(socket, requested))
}

/// Extract the user text from an inbound frame. Accepts the structured
/// `{"content":"…"}` shape sent by the dashboard client, and falls back to a
/// bare text frame (the raw message string) for backward compatibility.
fn extract_user_text(raw: &str) -> String {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw)
        && let Some(serde_json::Value::String(content)) = map.get("content")
    {
        return content.clone();
    }
    raw.to_string()
}

/// Append one conversational turn to the durable chat store off the async
/// runtime. Persistence failures are logged and swallowed so a disk hiccup
/// never breaks the live conversation.
async fn persist_turn(state_root: &std::path::Path, session_id: &str, role: Role, content: String) {
    let sr = state_root.to_path_buf();
    let sid = session_id.to_string();
    let message = ConversationMessage {
        role,
        content,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    let _ = tokio::task::spawn_blocking(move || {
        if let Err(e) = super::chat_store::append_turn_at(&sr, &sid, &message) {
            eprintln!("[simard] chat persist failed: {e}");
        }
    })
    .await;
}

/// Deliver a completed assistant reply as an ordered run of `chunk` frames
/// terminated by a single `done` frame (server-side chunking). Incremental
/// *appearance* is achieved over the existing socket without a token-level
/// model API; the wire protocol stays forward-compatible with true streaming.
async fn stream_assistant(socket: &mut WebSocket, content: &str) {
    let chars: Vec<char> = content.chars().collect();
    for window in chars.chunks(STREAM_CHUNK_CHARS) {
        let piece: String = window.iter().collect();
        if socket
            .send(Message::Text(
                json!({ "type": "chunk", "content": piece })
                    .to_string()
                    .into(),
            ))
            .await
            .is_err()
        {
            return;
        }
        // A small delay so text visibly streams; each delay is tiny and the
        // chunk size bounds the frame count for long replies.
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    let _ = socket
        .send(Message::Text(json!({ "type": "done" }).to_string().into()))
        .await;
}

pub(crate) async fn handle_ws_chat(mut socket: WebSocket, requested_session_id: Option<String>) {
    use crate::conversation_channel::apply_record;
    use crate::meeting_backend::{
        MeetingBackend, MeetingCommand, parse_command, render_help_plain, unknown_command_notice,
    };

    let state_root = super::routes::resolve_state_root();

    // Resolve the session this connection is bound to: resume an existing id,
    // or mint a fresh time-ordered id (used from the `ready` handshake onward
    // and persisted lazily on the first user message).
    let is_resume = requested_session_id.is_some();
    let session_id = requested_session_id.unwrap_or_else(super::chat_store::new_session_id);

    // Handshake: announce the bound session id + streaming capability before
    // any other traffic, so the client can adapt to streaming/fallback.
    let _ = socket
        .send(Message::Text(
            json!({
                "type": "ready",
                "session_id": session_id,
                "streaming": true,
                "protocol_version": 1,
            })
            .to_string()
            .into(),
        ))
        .await;

    // Use the full agent session (SessionBuilder) for chat.
    // The lightweight piped-subprocess path is disabled — it spawns
    // `amplihack copilot --subprocess-safe` which hangs indefinitely
    // because the Copilot CLI doesn't support non-interactive piped mode.
    let agent_session: Option<Box<dyn crate::base_types::BaseTypeSession>> =
        tokio::task::spawn_blocking(open_dashboard_agent_session)
            .await
            .ok()
            .flatten();

    let agent = match agent_session {
        Some(full) => {
            eprintln!("[simard] chat using full agent backend");
            full
        }
        None => {
            eprintln!("[simard][ERROR] no chat backend available — agent session failed to open");
            let _ = socket
                .send(Message::Text(
                    json!({"role":"system","content":"No agent backend available. Check SIMARD_LLM_PROVIDER and auth config."})
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
    };

    let system_prompt = match load_dashboard_meeting_prompt() {
        Ok(prompt) => prompt,
        Err(e) => {
            eprintln!("[simard] dashboard chat: {e}");
            let _ = socket
                .send(Message::Text(
                    json!({"role":"error","content": e.to_string()})
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
    };
    let mut backend = MeetingBackend::new_session("Dashboard Chat", agent, None, system_prompt);

    // Resume: replay persisted history into the backend (agent context) and
    // the UI (restore frame) before accepting new input, so replies stay
    // contextually coherent across reloads and process restarts.
    if is_resume {
        let sr = state_root.clone();
        let sid = session_id.clone();
        let loaded =
            tokio::task::spawn_blocking(move || super::chat_store::load_session_at(&sr, &sid))
                .await;
        if let Ok(Ok(Some(session))) = loaded {
            backend.restore(session.history.clone());
            let _ = socket
                .send(Message::Text(
                    json!({ "type": "restore", "messages": session.history })
                        .to_string()
                        .into(),
                ))
                .await;
        }
    }

    let _ = socket
        .send(Message::Text(
            json!({"role":"system","content":"Connected to Simard. Speak naturally — /help for commands, /close to end."})
                .to_string()
                .into(),
        ))
        .await;

    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                let raw = text.to_string();
                if raw.len() > MAX_CHAT_FRAME_BYTES {
                    // Defense in depth: max_message_size already bounds frames.
                    let _ = socket
                        .send(Message::Text(
                            json!({"role":"system","content":"[message too large — refused]"})
                                .to_string()
                                .into(),
                        ))
                        .await;
                    continue;
                }
                let text = extract_user_text(&raw);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let cmd = parse_command(trimmed);
                match cmd {
                    MeetingCommand::Close => {
                        // Close runs synchronous LLM call — use spawn_blocking
                        // wrapped with catch_unwind so a panic inside summary
                        // generation surfaces as a chat message, not a crash.
                        let summary = tokio::task::spawn_blocking(move || {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                                backend.close()
                            }))
                        })
                        .await;
                        let recap = match summary {
                            Ok(Ok(Ok(s))) => {
                                // Issue #1954: include `next_owner` and a
                                // compact artifact list in the recap so
                                // dashboard operators can navigate straight
                                // to the bundle / transcript / report.
                                let mut body = format!(
                                    "Meeting closed. {} messages. Summary: {}",
                                    s.message_count, s.summary_text
                                );
                                if let Some(ref dir) = s.bundle_dir {
                                    body.push_str(&format!("\nBundle: {dir}"));
                                }
                                if let Some(ref md) = s.markdown_report_path {
                                    body.push_str(&format!("\nReport: {md}"));
                                }
                                body
                            }
                            Ok(Ok(Err(e))) => format!("Meeting closed with error: {e}"),
                            Ok(Err(_panic)) => {
                                eprintln!("[simard][PANIC] ws_chat close panicked");
                                "Meeting close failed: internal panic (recovered)".to_string()
                            }
                            Err(e) => format!("Meeting close failed: {e}"),
                        };
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": recap}).to_string().into(),
                            ))
                            .await;
                        break;
                    }
                    MeetingCommand::Help => {
                        let help = render_help_plain();
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": help}).to_string().into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Unknown { input, suggestion } => {
                        // Mistyped command: surface a did-you-mean hint (or the
                        // full grouped help) instead of forwarding to the LLM.
                        // Issue #2321.
                        let mut content = unknown_command_notice(&input, suggestion.as_deref());
                        if suggestion.is_none() {
                            content.push_str("\n\n");
                            content.push_str(&render_help_plain());
                        }
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": content})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Status => {
                        let status = backend.status();
                        let info = format!(
                            "Topic: {}\nMessages: {}\nStarted: {}\nOpen: {}",
                            status.topic, status.message_count, status.started_at, status.is_open
                        );
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": info}).to_string().into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Template(name) => {
                        use crate::meeting_backend::persist::{TEMPLATES, find_template};
                        let content = if name.is_empty() {
                            let mut listing = "Available templates:\n".to_string();
                            for t in TEMPLATES {
                                listing.push_str(&format!("  {} — {}\n", t.name, t.description));
                            }
                            listing.push_str("\nUsage: /template <name>");
                            listing
                        } else if let Some(tmpl) = find_template(&name) {
                            tmpl.agenda.to_string()
                        } else {
                            format!(
                                "Unknown template: {name}. Available: standup, 1on1, retro, planning"
                            )
                        };
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": content})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Export => {
                        use crate::meeting_backend::persist::write_markdown_export;
                        let content = match write_markdown_export(
                            backend.topic(),
                            backend.started_at(),
                            backend.history(),
                        ) {
                            Ok(path) => format!("Meeting exported to: {}", path.display()),
                            Err(e) => format!("[export error: {e}]"),
                        };
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": content})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    // Every structured-capture command renders identically on the
                    // dashboard: apply the backend mutation via the shared
                    // `apply_record` and echo its canonical acknowledgement as a
                    // `system` chat line. Binding the whole command (`cmd @ …`)
                    // passes it straight through with no clone-and-reconstruct.
                    cmd @ (MeetingCommand::Theme(_)
                    | MeetingCommand::Decision { .. }
                    | MeetingCommand::Action(_)
                    | MeetingCommand::Question(_)
                    | MeetingCommand::Owner(_)
                    | MeetingCommand::Goal(_)
                    | MeetingCommand::Risk(_)
                    | MeetingCommand::Disagree(_)) => {
                        let rec = apply_record(&mut backend, &cmd).expect("record command");
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": rec.text})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Recap => {
                        let status = backend.status();
                        let themes = backend.explicit_themes();
                        let mut recap = format!(
                            "── Meeting Recap ──\nTopic: {}\nMessages: {}\nStarted: {}",
                            status.topic, status.message_count, status.started_at
                        );
                        if !themes.is_empty() {
                            recap.push_str(&format!("\nThemes: {}", themes.join(", ")));
                        }
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": recap}).to_string().into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Preview => {
                        let status = backend.status();
                        let themes = backend.explicit_themes();
                        let preview = format!(
                            "── Handoff Preview ──\nTopic: {}\nMessages so far: {}\nThemes: {}",
                            status.topic,
                            status.message_count,
                            if themes.is_empty() {
                                "none yet".to_string()
                            } else {
                                themes.join(", ")
                            }
                        );
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": preview})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    MeetingCommand::State => {
                        // Re-display the running list of decisions, open
                        // questions, and action items (issue #1646). Reuses
                        // the existing extractors — no duplicate parsing.
                        use crate::meeting_backend::persist::{
                            extract_action_items, extract_decisions, extract_open_questions,
                        };
                        let messages = backend.history();
                        let decisions = extract_decisions(messages);
                        let open_questions = extract_open_questions(messages);
                        let action_items = extract_action_items(messages);

                        let mut body = String::new();
                        body.push_str("── Decisions ──\n");
                        if decisions.is_empty() {
                            body.push_str("  _(none)_\n");
                        } else {
                            for (i, d) in decisions.iter().enumerate() {
                                body.push_str(&format!("  {}. {d}\n", i + 1));
                            }
                        }
                        body.push_str("\n── Open Questions ──\n");
                        if open_questions.is_empty() {
                            body.push_str("  _(none)_\n");
                        } else {
                            for q in &open_questions {
                                let tag = if q.explicit { " *(explicit)*" } else { "" };
                                body.push_str(&format!("  - {}{tag}\n", q.text));
                            }
                        }
                        body.push_str("\n── Action Items ──\n");
                        if action_items.is_empty() {
                            body.push_str("  _(none)_\n");
                        } else {
                            for (i, item) in action_items.iter().enumerate() {
                                let mut line = format!("  {}. {}", i + 1, item.description);
                                if let Some(ref who) = item.assignee {
                                    line.push_str(&format!(" [→ {who}]"));
                                }
                                if let Some(ref when) = item.deadline {
                                    line.push_str(&format!(" ({when})"));
                                }
                                body.push_str(&line);
                                body.push('\n');
                            }
                        }
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": body}).to_string().into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Conversation(user_text) => {
                        // Persist the user turn first so the session is created
                        // lazily (its title keys off the first user message)
                        // before the agent replies.
                        persist_turn(&state_root, &session_id, Role::User, user_text.clone()).await;

                        // send_message is synchronous — use spawn_blocking
                        // wrapped with catch_unwind so a panic in the agent
                        // doesn't crash the chat task.
                        let for_agent = user_text.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    backend.send_message(&for_agent)
                                }));
                            (backend, outcome)
                        })
                        .await;
                        match result {
                            Ok((returned_backend, Ok(Ok(resp)))) => {
                                backend = returned_backend;
                                // Persist the assistant reply as one turn, then
                                // stream it incrementally to the client.
                                persist_turn(
                                    &state_root,
                                    &session_id,
                                    Role::Assistant,
                                    resp.content.clone(),
                                )
                                .await;
                                stream_assistant(&mut socket, &resp.content).await;
                            }
                            Ok((returned_backend, Ok(Err(e)))) => {
                                backend = returned_backend;
                                let _ = socket
                                    .send(Message::Text(
                                        json!({"role":"system","content": format!("[error: {e}]")})
                                            .to_string()
                                            .into(),
                                    ))
                                    .await;
                            }
                            Ok((returned_backend, Err(_panic))) => {
                                eprintln!("[simard][PANIC] ws_chat send_message panicked");
                                backend = returned_backend;
                                let _ = socket
                                    .send(Message::Text(
                                        json!({"role":"system","content":"[error: agent panicked — recovered, conversation continues]"})
                                            .to_string()
                                            .into(),
                                    ))
                                    .await;
                            }
                            Err(e) => {
                                let _ = socket
                                    .send(Message::Text(
                                        json!({"role":"system","content": format!("[internal error: {e}]")})
                                            .to_string()
                                            .into(),
                                    ))
                                    .await;
                                break;
                            }
                        }
                    }
                }
            }
            Message::Close(_) => {
                // Clean up on disconnect
                let _ = tokio::task::spawn_blocking(move || backend.close()).await;
                break;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- load_dashboard_meeting_prompt ------------------------------------

    #[test]
    fn load_meeting_prompt_returns_content_when_file_exists() {
        // CARGO_MANIFEST_DIR is set during `cargo test`, and the prompt file
        // lives at <manifest>/prompt_assets/simard/meeting_system.md.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let prompt_path = manifest.join("prompt_assets/simard/meeting_system.md");
        if prompt_path.exists() {
            let result = load_dashboard_meeting_prompt();
            assert!(result.is_ok(), "should find prompt via CARGO_MANIFEST_DIR");
            let content = result.unwrap();
            assert!(!content.is_empty(), "prompt content should be non-empty");
            assert!(
                content.len() > 50,
                "prompt should be a real document, got {} bytes",
                content.len()
            );
        }
    }

    #[test]
    fn load_meeting_prompt_returns_prompt_not_found_when_missing() {
        // When none of the candidate paths exist, we get PromptNotFound.
        // This test can only verify the error type if ALL candidates miss.
        // If the file does exist on this machine, the function will succeed.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let prompt_path = manifest.join("prompt_assets/simard/meeting_system.md");
        if !prompt_path.exists() {
            let result = load_dashboard_meeting_prompt();
            assert!(
                result.is_err(),
                "should return error when no prompt file found"
            );
        }
    }

    #[test]
    fn load_meeting_prompt_candidate_paths_are_reasonable() {
        // Validate that the candidate path construction doesn't panic
        // and produces paths with the expected suffix.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let expected_suffix = "prompt_assets/simard/meeting_system.md";
        let build_candidate = manifest.join(expected_suffix);
        assert!(
            build_candidate.to_string_lossy().ends_with(expected_suffix),
            "build-time candidate should end with {expected_suffix}"
        );
    }

    // ---- open_dashboard_agent_session ------------------------------------

    #[test]
    // #2360: `open_dashboard_agent_session()` resolves the LLM provider from the
    // env-derived state root (`SIMARD_LLM_PROVIDER`, then
    // `<state_root>/config.toml`). Two isolation needs:
    //   1. A `HermeticState` points the state root at a fresh, empty temp dir so
    //      the assertion is deterministic regardless of the host's real
    //      `~/.simard/config.toml` (which on a dev box may set `llm_provider`).
    //   2. The `cognitive_memory` key (required by `HermeticState`, and shared
    //      with the provider-env mutators in ooda_actions / disk_health /
    //      self_improve) keeps a concurrent `SIMARD_LLM_PROVIDER` mutation from
    //      tearing this read.
    #[serial_test::serial(cognitive_memory)]
    fn open_agent_session_returns_none_without_provider_config() {
        // Empty hermetic state root → no config.toml → no provider configured
        // there. Combined with SIMARD_LLM_PROVIDER being unset, provider
        // resolution must fail and the session must be None.
        let _hermetic = crate::test_support::HermeticState::new();
        let session = open_dashboard_agent_session();
        if std::env::var("SIMARD_LLM_PROVIDER").is_err() {
            assert!(
                session.is_none(),
                "should return None without LLM provider configured"
            );
        }
    }

    // ---- WebSocket chat command routing -----------------------------------
    // We can't easily create a real WebSocket in unit tests, but we can
    // verify the parse_command integration contract.

    #[test]
    fn chat_recognizes_close_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        assert!(matches!(parse_command("/close"), MeetingCommand::Close));
    }

    #[test]
    fn chat_recognizes_help_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        assert!(matches!(parse_command("/help"), MeetingCommand::Help));
    }

    #[test]
    fn chat_routes_unknown_command_with_suggestion() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("/colse") {
            MeetingCommand::Unknown { input, suggestion } => {
                assert_eq!(input, "/colse");
                assert_eq!(suggestion.as_deref(), Some("/close"));
            }
            other => panic!("expected Unknown, got: {other:?}"),
        }
    }

    #[test]
    fn chat_help_uses_grouped_reference() {
        use crate::meeting_backend::render_help_plain;
        let help = render_help_plain();
        assert!(help.contains("Meeting control"));
        assert!(help.contains("Capture"));
        assert!(help.contains("Templates"));
    }

    #[test]
    fn chat_recognizes_status_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        assert!(matches!(parse_command("/status"), MeetingCommand::Status));
    }

    #[test]
    fn chat_recognizes_template_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("/template standup") {
            MeetingCommand::Template(name) => assert_eq!(name, "standup"),
            other => panic!("expected Template, got: {other:?}"),
        }
    }

    #[test]
    fn chat_recognizes_theme_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("/theme technical debt") {
            MeetingCommand::Theme(text) => assert_eq!(text, "technical debt"),
            other => panic!("expected Theme, got: {other:?}"),
        }
    }

    #[test]
    fn chat_recognizes_decision_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("/decision Use Rust for the rewrite") {
            MeetingCommand::Decision { text, .. } => {
                assert!(text.contains("Use Rust"));
            }
            other => panic!("expected Decision, got: {other:?}"),
        }
    }

    #[test]
    fn chat_recognizes_action_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("/action Fix the CI pipeline") {
            MeetingCommand::Action(text) => assert!(text.contains("Fix the CI")),
            other => panic!("expected Action, got: {other:?}"),
        }
    }

    #[test]
    fn chat_recognizes_question_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("/question What's the timeline?") {
            MeetingCommand::Question(text) => assert!(text.contains("timeline")),
            other => panic!("expected Question, got: {other:?}"),
        }
    }

    #[test]
    fn chat_recognizes_owner_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("/owner alice") {
            MeetingCommand::Owner(text) => assert_eq!(text, "alice"),
            other => panic!("expected Owner, got: {other:?}"),
        }
    }

    #[test]
    fn chat_recognizes_recap_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        assert!(matches!(parse_command("/recap"), MeetingCommand::Recap));
    }

    #[test]
    fn chat_recognizes_preview_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        assert!(matches!(parse_command("/preview"), MeetingCommand::Preview));
    }

    #[test]
    fn chat_recognizes_state_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        assert!(matches!(parse_command("/state"), MeetingCommand::State));
    }

    #[test]
    fn chat_routes_plain_text_to_conversation() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("Hello, how are you?") {
            MeetingCommand::Conversation(text) => assert_eq!(text, "Hello, how are you?"),
            other => panic!("expected Conversation, got: {other:?}"),
        }
    }

    #[test]
    fn chat_routes_export_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        assert!(matches!(parse_command("/export"), MeetingCommand::Export));
    }

    #[test]
    fn chat_routes_goal_command() {
        use crate::meeting_backend::{MeetingCommand, parse_command};
        match parse_command("/goal Improve test coverage") {
            MeetingCommand::Goal(text) => assert!(text.contains("Improve")),
            other => panic!("expected Goal, got: {other:?}"),
        }
    }
}
