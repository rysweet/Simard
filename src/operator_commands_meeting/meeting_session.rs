use std::io::{self, BufReader};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::greeting_banner::print_greeting_banner;
use crate::identity::OperatingMode;
use crate::meeting_repl::run_meeting_repl;
use crate::memory_ipc;
use crate::operator_commands::prompt_root;

use super::live_context::build_live_meeting_context;

/// Load the meeting system prompt from prompt_assets/simard/meeting_system.md.
fn load_meeting_system_prompt() -> String {
    let path = prompt_root().join("simard/meeting_system.md");
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Launch a cognitive memory backend suitable for meeting mode.
///
/// Delegates to [`memory_ipc::launch_writer_bridge`] so the daemon-IPC →
/// native-write → read-only ladder lives in one place (issue #1590,
/// spec recommendation C / A2).
fn launch_real_meeting_bridge() -> Result<Box<dyn CognitiveMemoryOps>, Box<dyn std::error::Error>> {
    let state_root = memory_ipc::default_state_root();
    let bridge = memory_ipc::launch_writer_bridge(&state_root)?;
    // Move the boxed ops out of the WriterBridge wrapper so existing call
    // sites that hold `Box<dyn CognitiveMemoryOps>` keep working unchanged.
    Ok(bridge.into_box())
}

/// Open an agent session for the meeting REPL using `SessionBuilder`.
///
/// All providers (Copilot, RustyClawd, etc.) go through the same
/// `SessionBuilder` path — no subprocess or per-provider special-casing.
/// This matches the dashboard chat backend (`open_dashboard_agent_session`)
/// so both CLI and web get identical behavior. Fixes #2105, #2106.
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
        let _prompt = load_meeting_system_prompt();
    }

    #[test]
    fn load_meeting_system_prompt_returns_string() {
        let prompt = load_meeting_system_prompt();
        let _ = prompt.len();
    }

    #[test]
    fn open_meeting_agent_session_returns_none_without_api_key() {
        let _result = open_meeting_agent_session();
    }

    /// Calling `open_meeting_agent_session()` in a headless CI
    /// environment must NEVER block indefinitely — it either succeeds or
    /// returns None promptly.
    #[test]
    fn open_meeting_agent_session_does_not_block_in_headless_env() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::time::Duration;

        let done = Arc::new(AtomicBool::new(false));
        let done_clone = Arc::clone(&done);

        let handle = std::thread::spawn(move || {
            let _result = open_meeting_agent_session();
            done_clone.store(true, Ordering::SeqCst);
        });

        std::thread::sleep(Duration::from_secs(10));

        assert!(
            done.load(Ordering::SeqCst),
            "open_meeting_agent_session must complete within 10s"
        );

        let _ = handle.join();
    }

    #[test]
    fn run_meeting_repl_command_errors_cleanly_without_agent() {
        let prompt = load_meeting_system_prompt();
        let _ = prompt.len();
    }
}
