use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response,
};

use serde_json::json;

use crate::error::{SimardError, SimardResult};

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

pub(crate) async fn ws_chat_handler(ws: WebSocketUpgrade) -> response::Response {
    ws.on_upgrade(handle_ws_chat)
}

pub(crate) async fn handle_ws_chat(mut socket: WebSocket) {
    use crate::meeting_backend::{MeetingBackend, MeetingCommand, parse_command};

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
                let text = text.to_string();
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
                            Ok(Ok(Ok(s))) => format!(
                                "Meeting closed. {} messages. Summary: {}",
                                s.message_count, s.summary_text
                            ),
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
                        let help = "Commands: /status, /template [name], /export, /theme <text>, /recap, /preview, /close, /help. Everything else is natural conversation with Simard.";
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": help}).to_string().into(),
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
                    MeetingCommand::Theme(theme) => {
                        backend.push_theme(theme.clone());
                        let content = format!("Theme recorded: {theme}");
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": content})
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
                    MeetingCommand::Conversation(user_text) => {
                        // send_message is synchronous — use spawn_blocking
                        // wrapped with catch_unwind so a panic in the agent
                        // doesn't crash the chat task.
                        let result = tokio::task::spawn_blocking(move || {
                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    backend.send_message(&user_text)
                                }));
                            (backend, outcome)
                        })
                        .await;
                        match result {
                            Ok((returned_backend, Ok(Ok(resp)))) => {
                                backend = returned_backend;
                                let _ = socket
                                    .send(Message::Text(
                                        json!({"role":"assistant","content": resp.content})
                                            .to_string()
                                            .into(),
                                    ))
                                    .await;
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
