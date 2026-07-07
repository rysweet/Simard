//! TDD (RED) tests for the agent-facing `simard memory remember` /
//! `remember-procedure` write tool (issue #2679).
//!
//! This CLI subcommand is the mechanism by which the distiller agentic step
//! commits facts DIRECTLY to cognitive memory — one process invocation writes
//! exactly one fact through the authoritative IPC gate. There is no `{ "facts":
//! [...] }` envelope, so nothing Simard hand-parses.
//!
//! Contract pinned here:
//!
//!   * **Scalar flags, no envelope.** `--concept`, `--content`,
//!     `--source-episode-id` (repeatable), `--confidence`, `--tags`,
//!     `--pass-id`, optional positional state-root. One process = one fact.
//!   * **Routes ONLY through the daemon socket** (`socket_path_for` →
//!     `RemoteCognitiveMemory::connect` → `StoreFactGated`). There is
//!     deliberately NO direct-open fallback: a direct open would bypass the
//!     server-side gate (the single authoritative write boundary), so if no
//!     daemon is reachable the tool exits **3** rather than silently writing
//!     ungated.
//!   * **Exit codes:** 0 = stored, 2 = usage/arg error, 3 = no reachable memory
//!     daemon, 4 = the gate quarantined the fact.
//!
//! These tests reference `parse_remember_fact_args`, `RememberFactArgs`, and
//! `run_remember_fact`, none of which exist yet — the intended TDD red signal.
//! The `remember` / `remember-procedure` subcommands are also not yet routed in
//! `dispatch_memory_command`, so the dispatch-recognition test currently takes
//! the "unsupported command" path (runtime red).

use serial_test::serial;

use super::{
    RememberFactArgs, dispatch_memory_command, parse_remember_fact_args, resolve_pass_id,
    run_remember_fact,
};

/// RAII env guard so the daemon-down test cannot be perturbed by a
/// `SIMARD_MEMORY_SOCKET` override pointing at a live socket.
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}
impl EnvGuard {
    fn unset(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prev }
    }

    /// Set `key` to `value` for the guard's lifetime, restoring the prior value
    /// (or unset state) on drop. Used to inject `SIMARD_DISTILL_PASS_ID` /
    /// `SIMARD_MEMORY_SOCKET` for the real-distill-path regression test.
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prev }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

// ───────────────────────────────────────────────────────────────────────────
// Argument parsing — scalar flags, no envelope
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn parse_accepts_a_full_fact_invocation() {
    let args: RememberFactArgs = parse_remember_fact_args(argv(&[
        "--concept=bug-pattern",
        "--content=empty outcome list panics cycle",
        "--source-episode-id=epi_00007",
        "--confidence=0.8",
        "--tags=bug-pattern,regression",
        "--pass-id=pass-abc123",
    ]))
    .expect("a fully-specified fact invocation must parse");

    assert_eq!(args.concept, "bug-pattern");
    assert_eq!(args.content, "empty outcome list panics cycle");
    assert_eq!(args.source_episode_ids, vec!["epi_00007".to_string()]);
    assert_eq!(args.confidence, Some(0.8));
    assert_eq!(
        args.tags,
        vec!["bug-pattern".to_string(), "regression".to_string()]
    );
    assert_eq!(args.pass_id.as_deref(), Some("pass-abc123"));
    assert!(args.state_root.is_none(), "no positional state-root given");
}

#[test]
fn parse_accepts_repeated_source_episode_ids() {
    // Procedures/facts may derive from multiple episodes; the flag repeats.
    let args = parse_remember_fact_args(argv(&[
        "--concept=lesson-learned",
        "--content=three or more words here",
        "--source-episode-id=epi_1",
        "--source-episode-id=epi_2",
        "--source-episode-id=epi_3",
    ]))
    .expect("repeated --source-episode-id must accumulate");
    assert_eq!(
        args.source_episode_ids,
        vec![
            "epi_1".to_string(),
            "epi_2".to_string(),
            "epi_3".to_string()
        ]
    );
    // Confidence is optional — the server derives it regardless.
    assert_eq!(args.confidence, None);
}

#[test]
fn parse_reads_a_positional_state_root() {
    let args = parse_remember_fact_args(argv(&[
        "--concept=pr-pattern",
        "--content=squash fixups before merge",
        "--source-episode-id=epi_9",
        "/tmp/some-state-root",
    ]))
    .expect("a positional state-root must parse");
    assert_eq!(
        args.state_root.as_deref(),
        Some(std::path::Path::new("/tmp/some-state-root"))
    );
}

#[test]
fn parse_rejects_missing_concept() {
    let err = parse_remember_fact_args(argv(&[
        "--content=orphan content with no concept",
        "--source-episode-id=epi_1",
    ]))
    .expect_err("a fact without --concept must be a usage error");
    assert!(
        err.contains("concept"),
        "usage error must name the missing flag: {err}"
    );
}

#[test]
fn parse_rejects_missing_content() {
    let err = parse_remember_fact_args(argv(&[
        "--concept=bug-pattern",
        "--source-episode-id=epi_1",
    ]))
    .expect_err("a fact without --content must be a usage error");
    assert!(
        err.contains("content"),
        "usage error must name the missing flag: {err}"
    );
}

#[test]
fn parse_rejects_non_numeric_confidence() {
    let err = parse_remember_fact_args(argv(&[
        "--concept=bug-pattern",
        "--content=three or more words here",
        "--source-episode-id=epi_1",
        "--confidence=not-a-number",
    ]))
    .expect_err("a non-numeric --confidence must be a usage error");
    assert!(
        err.contains("confidence"),
        "usage error must name --confidence: {err}"
    );
}

#[test]
fn parse_rejects_unknown_flag() {
    let err = parse_remember_fact_args(argv(&[
        "--concept=bug-pattern",
        "--content=three or more words here",
        "--frobnicate=1",
    ]))
    .expect_err("an unknown flag must be a usage error");
    assert!(
        err.contains("frobnicate"),
        "usage error must name the unknown flag: {err}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Exit codes
// ───────────────────────────────────────────────────────────────────────────

/// A usage error (missing required flag) exits 2 — distinct from a
/// gate/transport failure so a mis-invoking agent is diagnosable.
#[test]
fn run_returns_exit_2_on_usage_error() {
    let code = run_remember_fact(argv(&["--content=no concept given"]));
    assert_eq!(code, 2, "missing --concept must exit 2 (usage), got {code}");
}

/// With a well-formed invocation but NO reachable daemon, the tool must exit 3
/// rather than fall back to a direct (ungated) open. This is the "no bypass"
/// invariant: the authoritative gate lives in the daemon, so no daemon means no
/// gated write path.
#[test]
#[serial(cognitive_memory)]
fn run_returns_exit_3_when_no_daemon_is_reachable() {
    let _unset = EnvGuard::unset("SIMARD_MEMORY_SOCKET");
    let tmp = tempfile::tempdir().expect("tempdir");
    // No server was spawned in this TempDir, so <state_root>/memory.sock does
    // not exist and connect must fail.
    assert!(
        !tmp.path().join("memory.sock").exists(),
        "precondition: the hermetic state-root must have no live socket"
    );

    let code = run_remember_fact(argv(&[
        "--concept=bug-pattern",
        "--content=three or more words here",
        "--source-episode-id=epi_1",
        tmp.path().to_str().unwrap(),
    ]));
    assert_eq!(
        code, 3,
        "a well-formed remember with no reachable daemon must exit 3 (no ungated fallback), got {code}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Dispatch routing — `remember` is a recognized subcommand
// ───────────────────────────────────────────────────────────────────────────

/// `simard memory remember --help` must be recognized (print help, return Ok),
/// proving the subcommand is routed and NOT rejected as an unsupported command.
#[test]
fn dispatch_recognizes_remember_help() {
    let res = dispatch_memory_command(argv(&["remember", "--help"]).into_iter());
    assert!(
        res.is_ok(),
        "`memory remember --help` must be a recognized, successful invocation"
    );
}

/// An unknown subcommand is still rejected — the router must not have gone
/// permissive.
#[test]
fn dispatch_still_rejects_unknown_subcommand() {
    let err = dispatch_memory_command(argv(&["frobnicate"]).into_iter())
        .expect_err("unknown subcommand must error")
        .to_string();
    assert!(err.contains("frobnicate"), "{err}");
}

// ───────────────────────────────────────────────────────────────────────────
// Pass-id resolution — the distill runner tags the ledger via env, not a flag
// (issue #2679 silent-metrics regression)
// ───────────────────────────────────────────────────────────────────────────

/// Precedence contract for [`resolve_pass_id`]: a non-empty explicit `--pass-id`
/// wins; otherwise the `SIMARD_DISTILL_PASS_ID` env var (exported by the distill
/// runner) is the fallback; empty/absent in both yields "" (no ledger tagging).
#[test]
#[serial(cognitive_memory)]
fn resolve_pass_id_prefers_flag_then_env() {
    let _guard = EnvGuard::unset("SIMARD_DISTILL_PASS_ID");

    // Neither source set → empty (no ledger participation).
    assert_eq!(resolve_pass_id(None), "");
    assert_eq!(resolve_pass_id(Some("")), "");

    // Explicit flag wins even when the env var is also set.
    let _env = EnvGuard::set("SIMARD_DISTILL_PASS_ID", "from-env");
    assert_eq!(resolve_pass_id(Some("from-flag")), "from-flag");

    // No (usable) flag → fall back to the env var the runner exports.
    assert_eq!(resolve_pass_id(None), "from-env");
    assert_eq!(resolve_pass_id(Some("")), "from-env");
}

/// Real distill-path regression (issue #2679): the distiller agent runs
/// `simard memory remember` with ONLY the content flags — the pass id reaches
/// the CLI solely through the `SIMARD_DISTILL_PASS_ID` env var the runner
/// exports. This drives the actual [`run_remember_fact`] entry point against a
/// live gate and proves the env-var fallback tags the server's per-pass ledger,
/// so `drain_pass_ledger` returns the real accepted-fact count on production.
///
/// Before the fallback, `run_remember_fact` resolved an EMPTY pass id here, the
/// server's `ledger_record_stored` no-op'd it, and the drain returned 0 — the
/// silent-degradation bug that made every distill pass report `fact_count = 0` /
/// `reduction_pct = 100%` while facts were actually stored. Every prior test
/// passed `--pass-id` explicitly, so none exercised this production path.
#[test]
#[serial(cognitive_memory)]
fn remember_without_flag_tags_ledger_from_env_pass_id() {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::memory_ipc::{RemoteCognitiveMemory, spawn_server};

    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("memory.sock");
    let mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory db"));

    // Seed a real episode so the cited fact is grounded and the gate accepts it.
    let episode_id = mem
        .store_episode(
            "empty outcome list panicked the cycle",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");

    let _handle = spawn_server(sock.clone(), Arc::clone(&mem)).expect("spawn server");
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(sock.exists(), "server socket must come up");

    // Point the CLI at THIS server and supply the pass id ONLY via the env var
    // the distill runner exports — exactly the production invocation shape.
    let pass_id = format!("env-path-{}", std::process::id());
    let _sock_env = EnvGuard::set("SIMARD_MEMORY_SOCKET", sock.to_str().unwrap());
    let _pass_env = EnvGuard::set("SIMARD_DISTILL_PASS_ID", &pass_id);

    // NOTE: no `--pass-id` flag — the recipe never tells the agent to pass one.
    let code = run_remember_fact(argv(&[
        "--concept=bug-pattern",
        "--content=empty outcome list panics cycle",
        &format!("--source-episode-id={episode_id}"),
    ]));
    assert_eq!(
        code, 0,
        "a grounded fact against a reachable daemon must store (exit 0), got {code}"
    );

    // The env-var pass id must have tagged the ledger; a 0 here is the #2679
    // silent-metrics regression (facts stored but the pass reports none).
    let client = RemoteCognitiveMemory::connect(&sock).expect("connect");
    assert_eq!(
        client.drain_pass_ledger(&pass_id).expect("drain"),
        1,
        "the env-var pass id must tag the write ledger even with no --pass-id flag"
    );
}
