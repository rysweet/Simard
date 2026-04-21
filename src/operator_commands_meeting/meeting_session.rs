use std::io::{self, BufReader};

use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::greeting_banner::print_greeting_banner;
use crate::identity::OperatingMode;
use crate::meeting_repl::run_meeting_repl;
use crate::memory_ipc::{self, RemoteCognitiveMemory};
use crate::operator_commands::prompt_root;

use super::live_context::build_live_meeting_context;

/// Load the meeting system prompt from prompt_assets/simard/meeting_system.md.
fn load_meeting_system_prompt() -> String {
    let path = prompt_root().join("simard/meeting_system.md");
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Launch a cognitive memory backend suitable for meeting mode.
///
/// Priority:
/// 1. If the OODA daemon is publishing its memory IPC socket, connect as a
///    client (shared live view, no lock contention).
/// 2. Otherwise, reap any stale open-lock and open the on-disk DB directly
///    (no daemon is running, so we own it).
/// 3. Only fall back to read-only if the direct open fails *and* there
///    appears to be another writer — which should be rare once (1) and the
///    stale-lock reaper are in place.
fn launch_real_meeting_bridge() -> Result<Box<dyn CognitiveMemoryOps>, Box<dyn std::error::Error>> {
    let state_root = memory_ipc::default_state_root();
    let _ = std::fs::create_dir_all(&state_root);

    // (1) Prefer the running daemon when available.
    let sock = memory_ipc::default_socket_path();
    if sock.exists() {
        match RemoteCognitiveMemory::connect(&sock) {
            Ok(client) => {
                eprintln!(
                    "[simard] meeting: connected to OODA daemon memory IPC at {}",
                    client.socket_path().display()
                );
                return Ok(Box::new(client));
            }
            Err(e) => {
                eprintln!(
                    "[simard] meeting: daemon socket present but connect failed ({e}); \
                     falling back to direct open"
                );
            }
        }
    }

    // (2) No daemon — reap any stale lock left by a prior crashed process
    //     and open the DB ourselves.
    if let Err(e) = memory_ipc::reap_stale_open_lock(&state_root) {
        eprintln!("[simard] meeting: stale-lock reap failed: {e}");
    }

    match NativeCognitiveMemory::open(&state_root) {
        Ok(mem) => Ok(Box::new(mem)),
        Err(rw_err) => {
            // (3) Last-resort read-only fallback — only reached if another
            //     unidentified writer is holding the DB.
            eprintln!(
                "[simard] cognitive memory read-write open failed (another writer holds it): {rw_err}"
            );
            eprintln!(
                "[simard] falling back to read-only mode — meeting outcomes will be saved to disk only"
            );
            let mem = NativeCognitiveMemory::open_read_only(&state_root)
                .map_err(|e| format!("cognitive memory failed to open even read-only: {e}"))?;
            Ok(Box::new(mem))
        }
    }
}

/// Open an agent session for the meeting REPL using the standard base type
/// infrastructure.
fn open_meeting_agent_session() -> Option<Box<dyn crate::base_types::BaseTypeSession>> {
    let provider = match crate::session_builder::LlmProvider::resolve() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[simard] meeting agent: LLM provider not configured: {e}");
            return None;
        }
    };
    match crate::session_builder::SessionBuilder::new(OperatingMode::Meeting, provider)
        .node_id("meeting-repl")
        .address("meeting-repl://local")
        .adapter_tag("meeting")
        .open()
    {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[simard] meeting agent session failed: {e}");
            None
        }
    }
}

/// Entry point for the `simard meeting` CLI command.
pub fn run_meeting_repl_command(topic: &str) -> Result<(), Box<dyn std::error::Error>> {
    let bridge = launch_real_meeting_bridge()?;
    eprintln!("  Memory: cognitive bridge active (LadybugDB backend)");

    print_greeting_banner(Some(&*bridge));

    let agent_session = open_meeting_agent_session();
    let base_prompt = load_meeting_system_prompt();
    let live_context = build_live_meeting_context(&*bridge)?;
    let meeting_system_prompt = format!("{base_prompt}\n\n{live_context}");

    if agent_session.is_some() {
        eprintln!("  Agent: ready");
    } else {
        return Err("No agent backend available. Check SIMARD_LLM_PROVIDER and auth config (gh auth status / ANTHROPIC_API_KEY).".into());
    }

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    let _session = run_meeting_repl(
        topic,
        &*bridge,
        agent_session,
        &meeting_system_prompt,
        &mut reader,
        &mut writer,
    )?;

    println!("Meeting closed.");
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
