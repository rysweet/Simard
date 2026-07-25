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
/// Delegates to [`memory_ipc::launch_writer_client`] so the daemon-IPC →
/// native-write → read-only ladder lives in one place (issue #1590,
/// spec recommendation C / A2).
fn launch_real_meeting_client() -> Result<Box<dyn CognitiveMemoryOps>, Box<dyn std::error::Error>> {
    let state_root = memory_ipc::default_state_root();
    let memory = memory_ipc::launch_writer_client(&state_root)?;
    // Move the boxed ops out of the WriterClient wrapper so existing call
    // sites that hold `Box<dyn CognitiveMemoryOps>` keep working unchanged.
    Ok(memory.into_box())
}

/// Resolve the cognitive-memory backend into an `Option`, tolerating a
/// failed launch instead of propagating it.
///
/// Root cause of the deploy-gate red-canary crash-loop (issue #4647): the
/// meeting REPL launched the memory backend with `?` *before* it emitted the
/// greeting banner. In headless CI (integration-test gate) the backend is
/// unavailable, so the `?` returned early and stderr never received the
/// `Simard v` line the `meeting_repl_shows_greeting` canary greps for.
///
/// This seam converts the fallible launch into an `Option`: on failure it
/// logs the (sanitized) category via structured tracing and returns `None`,
/// letting the caller emit the greeting banner *first* and only afterward
/// fail-closed. It is NOT a silent fallback — the caller still requires a
/// live backend via `ok_or(...)?` before running the REPL.
fn resolve_meeting_memory<F>(launch: F) -> Option<Box<dyn CognitiveMemoryOps>>
where
    F: FnOnce() -> Result<Box<dyn CognitiveMemoryOps>, Box<dyn std::error::Error>>,
{
    match launch() {
        Ok(memory) => Some(memory),
        Err(error) => {
            tracing::warn!(%error, "meeting memory backend unavailable");
            None
        }
    }
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
    // Resolve the backend without propagating a launch failure, then emit the
    // greeting banner BEFORE failing closed. This guarantees the `Simard v`
    // canary marker reaches stderr even when the memory backend is
    // unavailable in headless CI (issue #4647).
    let memory = resolve_meeting_memory(launch_real_meeting_client);
    print_greeting_banner(memory.as_deref());

    // Fail-closed: the meeting REPL still requires a live memory backend.
    let memory = memory
        .ok_or("Cognitive memory backend unavailable. Check the memory daemon / state root.")?;
    tracing::info!("Cognitive memory active");

    let agent_session = open_meeting_agent_session();
    let base_prompt = load_meeting_system_prompt();
    let live_context = build_live_meeting_context(&*memory)?;
    let meeting_system_prompt = format!("{base_prompt}\n\n{live_context}");

    if agent_session.is_some() {
        tracing::info!("Meeting agent ready");
    } else {
        return Err("No agent backend available. Check SIMARD_LLM_PROVIDER and auth config (gh auth status / ANTHROPIC_API_KEY).".into());
    }

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    let _session = run_meeting_repl(
        topic,
        &*memory,
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
    fn load_meeting_system_prompt_reads_the_prompt_asset_with_missing_file_fallback() {
        // The loader must resolve `simard/meeting_system.md` under the prompt
        // root and fall back to an empty string when the asset is absent —
        // exactly what a direct read with `unwrap_or_default()` produces.
        let expected = std::fs::read_to_string(prompt_root().join("simard/meeting_system.md"))
            .unwrap_or_default();
        assert_eq!(load_meeting_system_prompt(), expected);
    }

    // ── greeting-before-fallible-launch contract (issue #4647) ──────
    //
    // Root cause of the red-canary crash-loop: `run_meeting_repl_command`
    // used to launch the cognitive-memory backend with `?` *before* it
    // printed the greeting banner. In a headless-CI / integration-test
    // environment the memory backend is unavailable, so the `?` returned
    // early and stderr never received the "Simard v" line the deploy
    // gate's `meeting_repl_shows_greeting` canary greps for → unit-test
    // gate red → every self-deploy refused.
    //
    // The fix extracts a seam, `resolve_meeting_memory`, that turns a
    // fallible launch into an `Option` (logging failures via structured
    // tracing instead of propagating them). The banner is then emitted
    // from `Option::as_deref()` — so it prints whether or not the backend
    // came up — and only afterward does the command fail-closed via
    // `ok_or(...)?`. These tests lock that contract at the unit level so
    // the regression cannot silently return.

    /// A failing memory launch must be *tolerated* at the greeting stage:
    /// `resolve_meeting_memory` returns `None` rather than propagating the
    /// error, so the caller can still emit the greeting banner before it
    /// fails closed. This is the exact ordering bug that produced the
    /// red-canary crash-loop.
    #[test]
    fn resolve_meeting_memory_tolerates_launch_failure() {
        let memory = resolve_meeting_memory(|| Err("memory backend unavailable".into()));
        assert!(
            memory.is_none(),
            "a failing memory launch must yield None (greeting continues), not an early return"
        );
    }

    /// When the backend launches successfully the resolved `Option` must
    /// carry it through unchanged, so the meeting REPL keeps its
    /// memory-backed behavior on the happy path.
    #[test]
    fn resolve_meeting_memory_returns_backend_on_success() {
        use crate::journal::test_support::FakeMemory;

        let memory = resolve_meeting_memory(|| Ok(Box::new(FakeMemory::new())));
        assert!(
            memory.is_some(),
            "a successful launch must yield Some(memory) so the REPL stays memory-backed"
        );
    }

    /// The greeting banner rendered with *no* memory must still contain the
    /// `Simard v` marker the deploy-gate canary asserts on. This guarantees
    /// that printing the banner before the fail-closed `ok_or(...)?` is
    /// sufficient to keep the unit-test gate green in headless CI.
    #[test]
    fn greeting_banner_renders_canary_marker_without_memory() {
        let lines = crate::greeting_banner::build_greeting_banner(None);
        let header = lines.first().expect("banner must have a header line");
        assert!(
            header.contains("Simard v"),
            "banner header must carry the 'Simard v' canary marker even without memory: {header}"
        );
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
}
