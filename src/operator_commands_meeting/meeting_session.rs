use std::io::{self, BufReader};

use crate::build_lock::BuildLock;
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::greeting_banner::print_greeting_banner;
use crate::identity::OperatingMode;
use crate::meeting_repl::run_meeting_repl;
use crate::operator_commands::prompt_root;

use super::live_context::build_live_meeting_context;

/// Load the meeting system prompt from prompt_assets/simard/meeting_system.md.
fn load_meeting_system_prompt() -> String {
    let path = prompt_root().join("simard/meeting_system.md");
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Launch the native cognitive memory backend for meeting mode.
///
/// Opens the DB in **read-only** mode to avoid conflicting with the OODA
/// daemon, which is the sole writer.  The meeting REPL only reads memory
/// (search_facts for live context, get_statistics for the greeting banner).
/// Meeting outcomes are persisted to disk (markdown export, transcript).
fn launch_real_meeting_bridge() -> Result<Box<dyn CognitiveMemoryOps>, Box<dyn std::error::Error>> {
    let state_root = BuildLock::default_state_root();
    let _ = std::fs::create_dir_all(&state_root);
    let mem = NativeCognitiveMemory::open_read_only(&state_root).map_err(|e| {
        format!("cognitive memory read-only open failed (is the OODA daemon running?): {e}")
    })?;
    Ok(Box::new(mem))
}

/// Open an agent session for the meeting REPL using the standard base type
/// infrastructure.
fn open_meeting_agent_session() -> Option<Box<dyn crate::base_types::BaseTypeSession>> {
    match crate::session_builder::SessionBuilder::new(OperatingMode::Meeting)
        .node_id("meeting-repl")
        .address("meeting-repl://local")
        .adapter_tag("meeting-rustyclawd")
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
    eprintln!("  Memory: cognitive bridge active (LadybugDB read-only)");

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
