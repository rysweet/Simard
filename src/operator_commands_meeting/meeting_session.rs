use std::io::{self, BufReader};
use std::path::PathBuf;

use crate::bridge_launcher::{cognitive_memory_db_path, find_python_dir, launch_memory_bridge};
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::greeting_banner::print_greeting_banner;
use crate::identity::OperatingMode;
use crate::meeting_repl::run_meeting_repl;
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::operator_commands::prompt_root;

use super::live_context::build_live_meeting_context;

/// Load the meeting system prompt from prompt_assets/simard/meeting_system.md.
fn load_meeting_system_prompt() -> String {
    let path = prompt_root().join("simard/meeting_system.md");
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Attempt to launch the real Python memory bridge for meeting mode.
///
/// Uses the same `BridgeLauncher` infrastructure as engineer mode: locates the
/// `python/` directory, starts `simard_memory_bridge.py`, and connects to Kuzu.
/// Returns `None` if any step fails so the caller can fall back gracefully.
fn launch_real_meeting_bridge() -> Option<CognitiveMemoryBridge> {
    let python_dir = find_python_dir().ok()?;
    let state_root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/simard-state"));
    let _ = std::fs::create_dir_all(&state_root);
    let db_path = cognitive_memory_db_path(&state_root);
    launch_memory_bridge("simard-meeting", &db_path, &python_dir).ok()
}

/// Open an agent session for the meeting REPL using the standard base type
/// infrastructure. Same agent identity, same platform — just meeting mode.
fn open_meeting_agent_session() -> Option<Box<dyn crate::base_types::BaseTypeSession>> {
    crate::session_builder::SessionBuilder::new(OperatingMode::Meeting)
        .node_id("meeting-repl")
        .address("meeting-repl://local")
        .adapter_tag("meeting-rustyclawd")
        .open()
}

/// Open an agent session for the meeting using the configured LLM provider.
///
/// Returns `None` if the provider cannot be initialised — the REPL will then
/// run in note-taking mode.
pub fn run_meeting_repl_command(topic: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Try to launch the real Python memory bridge backed by Kuzu graph database.
    // Falls back to an in-memory stub if the bridge is unavailable (no Python,
    // missing bridge_server.py, etc.).
    let bridge = match launch_real_meeting_bridge() {
        Some(b) => {
            eprintln!("  Memory: cognitive bridge active (Kuzu backend)");
            b
        }
        None => {
            eprintln!(
                "  \u{26a0} Memory bridge unavailable \u{2014} using in-memory stub (memories will not persist to Kuzu)"
            );
            let transport =
                InMemoryBridgeTransport::new("meeting-repl", |method, _params| match method {
                    "memory.record_sensory" => Ok(serde_json::json!({"id": "sen_repl"})),
                    "memory.store_episode" => Ok(serde_json::json!({"id": "epi_repl"})),
                    "memory.store_fact" => Ok(serde_json::json!({"id": "sem_repl"})),
                    "memory.store_prospective" => Ok(serde_json::json!({"id": "pro_repl"})),
                    "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
                    "memory.get_statistics" => Ok(serde_json::json!({
                        "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                        "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
                    })),
                    _ => Err(crate::bridge::BridgeErrorPayload {
                        code: -32601,
                        message: format!("unknown method: {method}"),
                    }),
                });
            CognitiveMemoryBridge::new(Box::new(transport))
        }
    };

    // Display greeting banner with memory bridge context
    print_greeting_banner(Some(&bridge));

    // Open an agent session for conversational meeting mode.
    // Uses the same base type infrastructure as engineer mode.
    let mut agent_session = open_meeting_agent_session();
    let base_prompt = load_meeting_system_prompt();
    let live_context = build_live_meeting_context(&bridge);
    let meeting_system_prompt = format!("{base_prompt}\n\n{live_context}");

    if agent_session.is_some() {
        eprintln!("  Agent: ready");
    } else {
        eprintln!("  ⚠ No agent backend available — meeting will be note-taking only.");
        eprintln!(
            "    Check SIMARD_LLM_PROVIDER and auth config (gh auth status / ANTHROPIC_API_KEY)."
        );
    }

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    let session = match agent_session {
        Some(ref mut boxed_agent) => run_meeting_repl(
            topic,
            &bridge,
            Some(&mut **boxed_agent),
            &meeting_system_prompt,
            &mut reader,
            &mut writer,
        )?,
        None => run_meeting_repl(
            topic,
            &bridge,
            None,
            &meeting_system_prompt,
            &mut reader,
            &mut writer,
        )?,
    };

    println!("Meeting closed.");
    println!("Decisions: {}", session.decisions.len());
    println!("Action items: {}", session.action_items.len());
    println!("Notes: {}", session.notes.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_meeting_system_prompt_does_not_panic() {
        // Uses unwrap_or_default internally so must never panic even when
        // the prompt asset file is absent (e.g. in CI).
        let _prompt = load_meeting_system_prompt();
    }

    #[test]
    fn load_meeting_system_prompt_returns_string() {
        let prompt = load_meeting_system_prompt();
        // May be empty if the file doesn't exist, but must not panic
        let _ = prompt.len();
    }

    #[test]
    fn open_meeting_agent_session_returns_none_without_api_key() {
        // Without ANTHROPIC_API_KEY set, should return None gracefully
        let _result = open_meeting_agent_session();
        // Just verify it doesn't panic; result depends on env
    }
}
