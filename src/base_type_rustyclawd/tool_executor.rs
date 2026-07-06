use std::time::{Duration, Instant};

use rustyclawd_core::client::ClientError;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// Env var (seconds) setting the idle-liveness window for the RustyClawd `Bash`
/// tool — the maximum time a tool child may produce **no output** before it is
/// treated as genuinely hung and reaped (issue #2607). This is NOT a wall-clock
/// per-call cap: every streamed chunk resets the clock, so a long-but-productive
/// command runs unbounded regardless of total runtime. `0` disables idle
/// detection entirely (fully unbounded escape hatch). Mirrors the meeting agent
/// proxy's `SIMARD_MEETING_IDLE_LIVENESS_SECS`.
const IDLE_LIVENESS_ENV: &str = "SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS";

/// One streamed output chunk from a `Bash` tool child, tagged by pipe so the
/// idle-liveness loop can accumulate stdout and stderr separately while treating
/// either as activity that resets the idle clock.
enum BashChunk {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

/// Resolve the idle-liveness window for a `Bash` tool child (issue #2607).
///
/// Pure and env-free so the override/fallback semantics are unit-testable
/// without mutating process-global environment state. The env var
/// [`IDLE_LIVENESS_ENV`] (value in **seconds**) wins; the per-call `timeout`
/// input (in **milliseconds**, model-supplied) is the fallback and is
/// reinterpreted as an idle window, never a total-runtime budget.
///
/// - `Some("<n>")` with `n > 0` → `Some(n secs)` idle window.
/// - `Some("0")` → `None` (idle detection disabled — fully unbounded escape hatch).
/// - `None` (unset) or malformed → `Some(per_call_ms)` (the per-call fallback).
fn resolve_idle_window(env_raw: Option<&str>, per_call_ms: u64) -> Option<Duration> {
    match env_raw {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(_) => Some(Duration::from_millis(per_call_ms)),
        },
        None => Some(Duration::from_millis(per_call_ms)),
    }
}

/// SIGKILL an entire process group given its leader PID (the group id equals the
/// leader's PID when the child was spawned via `setsid`, i.e.
/// [`rustyclawd_tools::ProcessSpawnConfig::with_isolation`]). Numeric-PID
/// signalling via `libc::kill` matches the repo's shell-free signal policy and
/// mirrors `meeting_backend::agent_proxy`'s reaper (issue #2607): on an idle
/// reap the whole subtree — the shell plus anything it forked — is killed so no
/// descendant is left holding the stdout/stderr pipes open.
fn kill_process_group(leader_pid: u32) {
    let pid = leader_pid as i32;
    // Guard against pathological ids: `-0` targets the caller's own group and
    // `-1` broadcasts to every process. Real child PIDs are always > 1.
    if pid <= 1 {
        return;
    }
    // SAFETY: `libc::kill` is FFI but well-defined for any (pid, signal). The
    // negated group-leader PID targets exactly the child's own process group,
    // which we created via `with_isolation()` (setsid); it cannot reach this
    // process.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

/// Execute a tool call locally using process spawning.
pub(super) async fn execute_tool_locally(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> Result<serde_json::Value, ClientError> {
    match tool_name {
        "Bash" => {
            let command = tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // The model-supplied `timeout` (ms) is the fallback idle-liveness
            // window when the env override is unset — a tolerated gap of NO
            // output, never a total-runtime budget (issue #2607). A command
            // that keeps producing output past its nominal `timeout` is NOT
            // killed.
            let timeout_ms = tool_input
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(120_000);
            let idle_window =
                resolve_idle_window(std::env::var(IDLE_LIVENESS_ENV).ok().as_deref(), timeout_ms);

            let mut cmd = tokio::process::Command::new("sh");
            cmd.args(["-c", command]);
            // Pipe stdout/stderr so tool output doesn't leak to the terminal.
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            // Isolate the child into its own session/process group (setsid) so
            // the idle reaper can SIGKILL the whole subtree — the shell plus
            // anything it forked — leaving no descendant orphaned holding the
            // pipes open. With setsid the child's PID equals its PGID.
            let config = rustyclawd_tools::ProcessSpawnConfig::with_isolation();
            let mut child = rustyclawd_tools::spawn_with_isolation(cmd, &config)
                .await
                .map_err(|e| ClientError::Unknown(format!("spawn failed: {e}")))?;

            // Capture the PID up front: after the child is waited/reaped `id()`
            // returns None. This PID is also the process-group id (setsid).
            let child_pid = child.id();

            let stdout = child.stdout.take().ok_or_else(|| {
                ClientError::Unknown("failed to capture bash tool stdout".to_string())
            })?;
            let stderr = child.stderr.take().ok_or_else(|| {
                ClientError::Unknown("failed to capture bash tool stderr".to_string())
            })?;

            // Reader tasks forward each line over a channel; the channel
            // disconnects EXACTLY when BOTH pipes reach EOF (all writers closed)
            // — the authoritative "all output received" signal.
            let (tx, mut rx) = mpsc::channel::<BashChunk>(256);
            let tx_out = tx.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf).await {
                        Ok(0) => break,
                        Ok(_) => {
                            if tx_out.send(BashChunk::Stdout(buf.clone())).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
            let tx_err = tx.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf).await {
                        Ok(0) => break,
                        Ok(_) => {
                            if tx_err.send(BashChunk::Stderr(buf.clone())).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
            // Drop our own sender so `rx` closes once both readers finish.
            drop(tx);

            // Idle-liveness loop (issue #2607). `last_activity` resets on EVERY
            // chunk, so a command that keeps producing output runs unbounded
            // regardless of total runtime — only sustained silence for the whole
            // window reaps the child. When `idle_window` is `None` (escape hatch
            // `0`) the idle branch is never evaluated: fully unbounded.
            let mut out_buf: Vec<u8> = Vec::new();
            let mut err_buf: Vec<u8> = Vec::new();
            let mut last_activity = Instant::now();
            let mut streams_open = true;
            let mut hung = false;
            let mut exit_status: Option<std::process::ExitStatus> = None;

            loop {
                // A child silent for the whole window is genuinely hung — reap.
                if let Some(idle) = idle_window
                    && last_activity.elapsed() >= idle
                {
                    hung = true;
                    break;
                }

                if !streams_open {
                    // Both pipes closed. Finish once the child is reaped; if it
                    // closed its pipes yet keeps running, keep polling so the
                    // idle deadline above can still fire (unless unbounded).
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            exit_status = Some(status);
                            break;
                        }
                        Ok(None) => {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            continue;
                        }
                        Err(e) => {
                            return Err(ClientError::Unknown(format!("process error: {e}")));
                        }
                    }
                }

                match idle_window {
                    Some(idle) => {
                        let remaining = idle.saturating_sub(last_activity.elapsed());
                        tokio::select! {
                            biased;
                            // Output always wins over the idle timer so a
                            // producing child is never mistaken for a hung one.
                            chunk = rx.recv() => match chunk {
                                Some(BashChunk::Stdout(b)) => {
                                    out_buf.extend_from_slice(&b);
                                    last_activity = Instant::now();
                                }
                                Some(BashChunk::Stderr(b)) => {
                                    err_buf.extend_from_slice(&b);
                                    last_activity = Instant::now();
                                }
                                None => streams_open = false,
                            },
                            _ = tokio::time::sleep(remaining) => {
                                // No output within the remaining window — loop so
                                // the idle check above can reap the child.
                            }
                        }
                    }
                    None => match rx.recv().await {
                        Some(BashChunk::Stdout(b)) => {
                            out_buf.extend_from_slice(&b);
                            last_activity = Instant::now();
                        }
                        Some(BashChunk::Stderr(b)) => {
                            err_buf.extend_from_slice(&b);
                            last_activity = Instant::now();
                        }
                        None => streams_open = false,
                    },
                }
            }

            if hung {
                // Kill the whole process group (shell + descendants) so nothing
                // is orphaned holding the pipes open. `start_kill()`/`wait()` is
                // a fallback for the direct child.
                if let Some(pid) = child_pid {
                    kill_process_group(pid);
                }
                let _ = child.start_kill();
                let _ = child.wait().await;
                let window = idle_window.unwrap_or_default();
                return Err(ClientError::Timeout(format!(
                    "bash tool idle for {window:?} with no output; reaped genuinely-hung \
                     child (idle-liveness, {IDLE_LIVENESS_ENV})"
                )));
            }

            let status = match exit_status {
                Some(status) => status,
                None => child
                    .wait()
                    .await
                    .map_err(|e| ClientError::Unknown(format!("process error: {e}")))?,
            };

            Ok(serde_json::json!({
                "stdout": String::from_utf8_lossy(&out_buf),
                "stderr": String::from_utf8_lossy(&err_buf),
                "exit_code": status.code().unwrap_or(-1),
            }))
        }
        "Read" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::read_to_string(path).await {
                Ok(contents) => Ok(serde_json::json!({ "content": contents })),
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        "Write" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let content = tool_input
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::write(path, content).await {
                Ok(()) => Ok(serde_json::json!({ "status": "ok" })),
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        "Edit" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let old = tool_input
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new = tool_input
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::read_to_string(path).await {
                Ok(contents) => {
                    let replaced = contents.replacen(old, new, 1);
                    match tokio::fs::write(path, &replaced).await {
                        Ok(()) => Ok(serde_json::json!({ "status": "ok" })),
                        Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
                    }
                }
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        _ => Ok(serde_json::json!({ "error": format!("unknown tool: {tool_name}") })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn execute_tool_locally_unknown_tool_returns_error_json() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("UnknownTool", &input)
            .await
            .expect("tool execution should succeed");
        let error = result
            .get("error")
            .and_then(|v| v.as_str())
            .expect("expected field present");
        assert!(error.contains("unknown tool"));
        assert!(error.contains("UnknownTool"));
    }

    #[tokio::test]
    async fn execute_tool_locally_read_nonexistent_file_returns_error() {
        let input = serde_json::json!({ "file_path": "/nonexistent/path/to/file.txt" });
        let result = execute_tool_locally("Read", &input)
            .await
            .expect("tool execution should succeed");
        assert!(
            result.get("error").is_some(),
            "should return error for missing file"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_write_to_invalid_path_returns_error() {
        let input =
            serde_json::json!({ "file_path": "/nonexistent/dir/file.txt", "content": "hello" });
        let result = execute_tool_locally("Write", &input)
            .await
            .expect("tool execution should succeed");
        assert!(
            result.get("error").is_some(),
            "should return error for invalid path"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_edit_nonexistent_file_returns_error() {
        let input = serde_json::json!({
            "file_path": "/nonexistent/dir/file.txt",
            "old_string": "old",
            "new_string": "new"
        });
        let result = execute_tool_locally("Edit", &input)
            .await
            .expect("tool execution should succeed");
        assert!(
            result.get("error").is_some(),
            "should return error for missing file"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_read_with_empty_path_returns_error() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("Read", &input)
            .await
            .expect("tool execution should succeed");
        assert!(
            result.get("error").is_some(),
            "empty path should yield error"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_missing_command_runs_empty_string() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        // Running empty command succeeds (sh -c "")
        assert!(result.get("exit_code").is_some());
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_echo_captures_stdout() {
        let input = serde_json::json!({ "command": "echo hello_test_42" });
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        assert!(stdout.contains("hello_test_42"));
        let exit_code = result
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .expect("expected numeric field");
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_failing_command_has_nonzero_exit() {
        let input = serde_json::json!({ "command": "false" });
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        let exit_code = result
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .expect("expected numeric field");
        assert_ne!(exit_code, 0);
    }

    #[tokio::test]
    async fn execute_tool_locally_write_and_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("simard-test-rw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("test_roundtrip.txt");
        let path_str = file_path.to_str().expect("path to str");

        let write_input =
            serde_json::json!({ "file_path": path_str, "content": "roundtrip_content" });
        let write_result = execute_tool_locally("Write", &write_input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(
            write_result.get("status").and_then(|v| v.as_str()),
            Some("ok")
        );

        let read_input = serde_json::json!({ "file_path": path_str });
        let read_result = execute_tool_locally("Read", &read_input)
            .await
            .expect("tool execution should succeed");
        let content = read_result
            .get("content")
            .and_then(|v| v.as_str())
            .expect("expected field present");
        assert_eq!(content, "roundtrip_content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_edit_replaces_content() {
        let dir = std::env::temp_dir().join(format!("simard-test-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("test_edit.txt");
        let path_str = file_path.to_str().expect("path to str");

        std::fs::write(&file_path, "hello world").expect("write test file");

        let edit_input = serde_json::json!({
            "file_path": path_str,
            "old_string": "hello",
            "new_string": "goodbye"
        });
        let edit_result = execute_tool_locally("Edit", &edit_input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(
            edit_result.get("status").and_then(|v| v.as_str()),
            Some("ok")
        );

        let content = std::fs::read_to_string(&file_path).expect("read test file");
        assert_eq!(content, "goodbye world");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_with_timeout_param() {
        let input = serde_json::json!({ "command": "echo timeout_test", "timeout": 5000 });
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        assert!(stdout.contains("timeout_test"));
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_stderr_capture() {
        let input = serde_json::json!({ "command": "echo stderr_test >&2" });
        let result = execute_tool_locally("Bash", &input)
            .await
            .expect("tool execution should succeed");
        let stderr = result.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        assert!(stderr.contains("stderr_test"));
    }

    #[tokio::test]
    async fn execute_tool_locally_write_empty_content() {
        let dir =
            std::env::temp_dir().join(format!("simard-test-empty-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("empty.txt");
        let input = serde_json::json!({ "file_path": file_path.to_str().expect("path to str"), "content": "" });
        let result = execute_tool_locally("Write", &input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(result.get("status").and_then(|v| v.as_str()), Some("ok"));
        let content = std::fs::read_to_string(&file_path).expect("read test file");
        assert!(content.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_edit_no_match_still_writes() {
        let dir =
            std::env::temp_dir().join(format!("simard-test-edit-nomatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("edit_nomatch.txt");
        std::fs::write(&file_path, "original content").expect("write test file");
        let input = serde_json::json!({
            "file_path": file_path.to_str().expect("path to str"),
            "old_string": "nonexistent",
            "new_string": "replacement"
        });
        let result = execute_tool_locally("Edit", &input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(result.get("status").and_then(|v| v.as_str()), Some("ok"));
        // Content should be unchanged since old_string wasn't found
        let content = std::fs::read_to_string(&file_path).expect("read test file");
        assert_eq!(content, "original content");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_read_existing_file() {
        let dir = std::env::temp_dir().join(format!("simard-test-read-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("readable.txt");
        std::fs::write(&file_path, "test content here").expect("write test file");
        let input = serde_json::json!({ "file_path": file_path.to_str().expect("path to str") });
        let result = execute_tool_locally("Read", &input)
            .await
            .expect("tool execution should succeed");
        let content = result
            .get("content")
            .and_then(|v| v.as_str())
            .expect("expected field present");
        assert_eq!(content, "test content here");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_write_with_missing_content_writes_empty() {
        let dir =
            std::env::temp_dir().join(format!("simard-test-write-nocon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file_path = dir.join("no_content.txt");
        let input = serde_json::json!({ "file_path": file_path.to_str().expect("path to str") });
        let result = execute_tool_locally("Write", &input)
            .await
            .expect("tool execution should succeed");
        assert_eq!(result.get("status").and_then(|v| v.as_str()), Some("ok"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ────────────────────────────────────────────────────────────────────
    // Issue #2607 — idle-liveness for the RustyClawd Bash tool (TDD, red).
    //
    // Contract these tests pin down (see
    // docs/reference/rustyclawd-bash-tool-idle-liveness.md):
    //
    //   * The Bash arm must NOT impose a wall-clock cap on the child. A
    //     command that keeps producing output runs unbounded; only a child
    //     that emits NO output for the idle-liveness window is reaped, and the
    //     whole process group is killed so nothing is orphaned.
    //   * The window is resolved by a pure helper
    //         fn resolve_idle_window(env_raw: Option<&str>, per_call_ms: u64)
    //             -> Option<Duration>
    //     where the env var `IDLE_LIVENESS_ENV`
    //     (= "SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS", value in SECONDS) wins;
    //     `0` => `None` (fully unbounded escape hatch); unset/malformed falls
    //     back to the per-call `timeout` input (in MILLISECONDS, default
    //     120_000).
    //
    // These reference `resolve_idle_window` / `IDLE_LIVENESS_ENV`, which do not
    // exist yet, and assert behavior the current wall-clock implementation does
    // not provide — so this module is RED until the idle-liveness Bash arm and
    // its resolver land in the implementation step.
    // ────────────────────────────────────────────────────────────────────

    // ---- pure window resolver (env-free, deterministic) ----

    #[test]
    fn idle_liveness_env_var_name_is_stable() {
        assert_eq!(
            IDLE_LIVENESS_ENV, "SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS",
            "the documented escape-hatch env var name is part of the operator contract"
        );
    }

    #[test]
    fn resolve_idle_window_positive_env_override_wins() {
        assert_eq!(
            resolve_idle_window(Some("600"), 120_000),
            Some(Duration::from_secs(600)),
            "a positive env value sets the idle window in SECONDS"
        );
        assert_eq!(
            resolve_idle_window(Some("  45  "), 120_000),
            Some(Duration::from_secs(45)),
            "surrounding whitespace must be tolerated"
        );
    }

    #[test]
    fn resolve_idle_window_zero_disables_reaping() {
        assert_eq!(
            resolve_idle_window(Some("0"), 120_000),
            None,
            "0 is the explicit unbounded escape hatch — idle detection disabled"
        );
        assert_eq!(
            resolve_idle_window(Some("0"), 1_000),
            None,
            "0 wins regardless of the per-call timeout"
        );
    }

    #[test]
    fn resolve_idle_window_unset_falls_back_to_per_call() {
        assert_eq!(
            resolve_idle_window(None, 120_000),
            Some(Duration::from_millis(120_000)),
            "unset env falls back to the per-call timeout (default 120s)"
        );
        assert_eq!(
            resolve_idle_window(None, 1_000),
            Some(Duration::from_millis(1_000)),
            "the per-call timeout (ms) governs the idle window when env is unset"
        );
    }

    #[test]
    fn resolve_idle_window_malformed_falls_back_to_per_call() {
        assert_eq!(
            resolve_idle_window(Some("not-a-number"), 2_000),
            Some(Duration::from_millis(2_000)),
            "a malformed env value degrades to the per-call timeout, never a wall-clock kill"
        );
    }

    // ---- behavioral guard tests (the three #2607 acceptance scenarios) ----
    //
    // These drive the real `execute_tool_locally` Bash arm. They spawn short
    // subprocesses and are serialized under `cognitive_memory` because they
    // read/mutate the process-global `IDLE_LIVENESS_ENV` (glibc setenv/getenv
    // are not thread-safe; see docs/testing/cognitive-memory-serial-isolation.md).

    /// Drive an async future to completion on a private current-thread runtime
    /// so these can be plain `#[test]` fns (compatible with `#[serial]`).
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread tokio runtime")
            .block_on(fut)
    }

    /// PIDs whose `/proc/<pid>/cmdline` still contains `marker` — i.e. live
    /// (non-zombie) processes from the spawned command tree. A zombie has an
    /// empty cmdline, so a reaped child never matches.
    fn live_pids_matching(marker: &str) -> Vec<i32> {
        let mut hits = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return hits;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
                continue;
            };
            if let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) {
                let cmd = String::from_utf8_lossy(&raw).replace('\0', " ");
                if cmd.contains(marker) {
                    hits.push(pid);
                }
            }
        }
        hits
    }

    /// Best-effort numeric-PID SIGKILL so a red-state (unreaped) orphan never
    /// survives the test run. Numeric signalling only — no name-based killers.
    fn kill_pids(pids: &[i32]) {
        for &pid in pids {
            if pid > 1 {
                // SAFETY: FFI kill of a specific PID discovered from /proc above;
                // the positive PID targets exactly that process, never a group.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }

    /// Scenario 1: a command that keeps producing output past the (short) window
    /// must NOT be killed. Proves total runtime exceeding the window does not
    /// trigger a wall-clock kill — only sustained silence does.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn bash_producing_output_past_the_window_is_never_killed_2607() {
        // Per-call `timeout` = 1s idle window (env unset ⇒ per-call governs).
        // The command runs ~1.5s total while never idling more than ~0.1s, so a
        // wall-clock cap would SIGKILL it whereas idle-liveness must let it finish.
        let input = serde_json::json!({
            "command": "for i in $(seq 15); do echo tick; sleep 0.1; done",
            "timeout": 1000
        });

        let prev = std::env::var(IDLE_LIVENESS_ENV).ok();
        // SAFETY: serialized via serial(cognitive_memory); no concurrent env access.
        unsafe {
            std::env::remove_var(IDLE_LIVENESS_ENV);
        }

        let result = block_on(execute_tool_locally("Bash", &input));

        // Restore prior env before any assertion can panic.
        unsafe {
            match prev {
                Some(v) => std::env::set_var(IDLE_LIVENESS_ENV, v),
                None => std::env::remove_var(IDLE_LIVENESS_ENV),
            }
        }

        let value =
            result.expect("a continuously-producing command must NOT be reaped by idle-liveness");
        let exit_code = value
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .expect("exit_code field present");
        assert_eq!(
            exit_code, 0,
            "the productive command should complete successfully"
        );
        let stdout = value.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let ticks = stdout.matches("tick").count();
        assert!(
            ticks >= 15,
            "all 15 ticks must be captured (got {ticks}); output must not be truncated by a kill"
        );
    }

    /// Scenario 2: a genuinely idle/hung child IS reaped after the idle window,
    /// the error honestly identifies an IDLE reap, and the whole process group
    /// is killed (no orphan leaks).
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn bash_idle_child_is_reaped_with_honest_error_and_no_orphan_2607() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let marker = format!("simard-2607-orphan-{}-{}", std::process::id(), nanos);
        // `echo start` produces output immediately (resets the clock), then the
        // child is silent well past the 0.8s idle window. The trailing
        // `echo {marker}` keeps `sleep` from being the shell's last
        // (exec-optimized) command, so a live orphan retains {marker} in its argv.
        let command = format!("echo start; sleep 999; echo {marker}");
        let input = serde_json::json!({ "command": command, "timeout": 800 });

        let prev = std::env::var(IDLE_LIVENESS_ENV).ok();
        // SAFETY: serialized via serial(cognitive_memory).
        unsafe {
            std::env::remove_var(IDLE_LIVENESS_ENV);
        }

        let result = block_on(execute_tool_locally("Bash", &input));

        // Give the reaper a moment to tear down the process group, then capture
        // any survivors and clean them up numerically (red-state safety).
        std::thread::sleep(Duration::from_millis(300));
        let leaked = live_pids_matching(&marker);
        kill_pids(&leaked);

        // Restore prior env before any assertion can panic.
        unsafe {
            match prev {
                Some(v) => std::env::set_var(IDLE_LIVENESS_ENV, v),
                None => std::env::remove_var(IDLE_LIVENESS_ENV),
            }
        }

        let err = result.expect_err("an idle/hung child must be reaped and surface an error");
        assert!(
            matches!(err, ClientError::Timeout(_)),
            "a genuine idle reap surfaces as ClientError::Timeout; got: {err}"
        );
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("idle"),
            "the error must identify an IDLE reap (honest idle-timeout, not a productive kill); got: {msg}"
        );
        assert!(
            leaked.is_empty(),
            "idle reap must kill the whole process group — leaked orphan PIDs: {leaked:?}"
        );
    }

    /// Scenario 3: `SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS=0` disables reaping
    /// entirely (fully unbounded). An idle command that would be reaped under
    /// the per-call window instead runs to natural completion.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn bash_env_zero_disables_idle_reaping_2607() {
        // Pure contract: 0 => unbounded regardless of the per-call timeout.
        assert_eq!(resolve_idle_window(Some("0"), 300), None);

        // Behavioral: with the escape hatch set to 0, a command idle far longer
        // than the 0.3s per-call window is NOT reaped and completes normally.
        let input = serde_json::json!({ "command": "sleep 1; echo done0_2607", "timeout": 300 });

        let prev = std::env::var(IDLE_LIVENESS_ENV).ok();
        // SAFETY: serialized via serial(cognitive_memory).
        unsafe {
            std::env::set_var(IDLE_LIVENESS_ENV, "0");
        }

        let result = block_on(execute_tool_locally("Bash", &input));

        // Restore prior env before any assertion can panic.
        unsafe {
            match prev {
                Some(v) => std::env::set_var(IDLE_LIVENESS_ENV, v),
                None => std::env::remove_var(IDLE_LIVENESS_ENV),
            }
        }

        let value = result.expect("with the 0 escape hatch an idle command must NOT be reaped");
        let exit_code = value
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .expect("exit_code field present");
        assert_eq!(
            exit_code, 0,
            "the unbounded command should complete normally"
        );
        let stdout = value.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            stdout.contains("done0_2607"),
            "the command must run to completion (unbounded), producing its final output"
        );
    }
}
