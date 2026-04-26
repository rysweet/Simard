use axum::{
    Json,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response,
};
use serde_json::{Value, json};

use super::hosts::{host_entry_name, load_hosts};
use super::distributed::strip_ansi_codes;
use axum::extract::Path;

// =====================================================================
// WS-1 AZLIN-TMUX-SESSIONS-LIST
//
// Provides a per-host tmux-session listing companion panel for the
// existing Terminal tab. Reuses:
//   * `load_hosts()` — canonical `~/.simard/hosts.json` source
//   * `azlin connect <host> --no-tmux -- <cmd>` — same exec channel as
//     `distributed()` host-status code (no new SSH transport)
// =====================================================================

/// One tmux session as parsed from
/// `tmux list-sessions -F '#S\t#{session_created}\t#{session_attached}\t#{session_windows}'`.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TmuxSession {
    pub name: String,
    pub created: i64,
    pub attached: bool,
    pub windows: u32,
}

/// Pure parser: tab-split rows, exactly 4 fields, types validated.
/// Tolerates trailing newlines, blank lines, "no server running" stderr,
/// and silently skips any malformed row.
pub(crate) fn parse_tmux_sessions(input: &str) -> Vec<TmuxSession> {
    let mut out = Vec::new();
    for raw_line in input.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 4 {
            continue;
        }
        let name = fields[0].trim();
        if name.is_empty() {
            continue;
        }
        let Ok(created) = fields[1].trim().parse::<i64>() else {
            continue;
        };
        let attached = match fields[2].trim() {
            "1" => true,
            "0" => false,
            _ => continue,
        };
        let Ok(windows) = fields[3].trim().parse::<u32>() else {
            continue;
        };
        out.push(TmuxSession {
            name: name.to_string(),
            created,
            attached,
            windows,
        });
    }
    out
}

/// Per-host tmux timeout (seconds). Matches the spirit of `distributed()`'s
/// short-bound exec; 5 s is enough for `tmux list-sessions` over azlin.
const TMUX_LIST_TIMEOUT_SECS: u64 = 5;

/// Validate a host or tmux-session name for use in route paths and shell
/// args. Allow-list: `^[A-Za-z0-9_.-]{1,64}$`. Returns the input on success.
fn sanitize_tmux_ident(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return None;
    }
    for &b in bytes {
        let ok = b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.';
        if !ok {
            return None;
        }
    }
    Some(s.to_string())
}

/// Run `tmux list-sessions` on a single host via the same azlin exec
/// channel used by `distributed()`. Returns (reachable, sessions, error).
///
/// Reachability semantics:
///   * exec OK + parsed sessions (possibly empty) → reachable=true
///   * exit ≠ 0 with empty stdout (typical "no server running") → reachable=true, sessions=[]
///   * spawn error / timeout / non-empty stderr without stdout → reachable=false
fn run_tmux_list_for_host(host: &str) -> (bool, Vec<TmuxSession>, Option<String>) {
    use std::process::Command;
    let host_owned = host.to_string();
    let output = Command::new("systemd-run")
        .args([
            "--user",
            "--pipe",
            "--quiet",
            "azlin",
            "connect",
            &host_owned,
            "--no-tmux",
            "--",
            "tmux",
            "list-sessions",
            "-F",
            "#S\t#{session_created}\t#{session_attached}\t#{session_windows}",
        ])
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => return (false, vec![], Some(format!("spawn failed: {e}"))),
    };

    let stdout_raw = String::from_utf8_lossy(&output.stdout);
    let stderr_raw = String::from_utf8_lossy(&output.stderr);
    // azlin connect --no-tmux can route remote stdout to local stderr when
    // run without a TTY (rysweet/azlin#980); strip ANSI on both streams.
    let stdout = strip_ansi_codes(&stdout_raw);
    let stderr = strip_ansi_codes(&stderr_raw);

    // Pick whichever stream actually carries tmux's table (tab-separated rows).
    let haystack = if stdout.contains('\t') {
        stdout.clone()
    } else if stderr.contains('\t') {
        stderr.clone()
    } else {
        stdout.clone()
    };

    let sessions = parse_tmux_sessions(&haystack);

    if !sessions.is_empty() {
        return (true, sessions, None);
    }

    // No parsed rows. Distinguish "tmux up, no server" (reachable=true) from
    // unreachable host. The `tmux: no server running` message is the canonical
    // marker; anything else with empty stdout means we never got past azlin.
    let combined_lower = format!("{stdout}\n{stderr}").to_lowercase();
    if combined_lower.contains("no server running") {
        return (true, vec![], None);
    }

    if output.status.success() && stdout.trim().is_empty() && stderr.trim().is_empty() {
        // azlin returned cleanly with no data — treat as reachable, no sessions.
        return (true, vec![], None);
    }

    let mut err = stderr.trim().to_string();
    if err.is_empty() {
        err = stdout.trim().to_string();
    }
    if err.is_empty() {
        err = format!("azlin connect exited with status {}", output.status);
    }
    let truncated: String = err.chars().take(256).collect();
    (false, vec![], Some(truncated))
}

/// GET `/api/azlin/tmux-sessions` — snapshot of tmux sessions across all
/// configured hosts. Always returns 200; per-host failures encoded inline.
pub(crate) async fn azlin_tmux_sessions() -> Json<Value> {
    let hosts = tokio::task::spawn_blocking(load_hosts)
        .await
        .unwrap_or_default();

    let mut tasks = tokio::task::JoinSet::new();
    for entry in &hosts {
        let name = host_entry_name(entry).to_string();
        if name.is_empty() {
            continue;
        }
        // Defense-in-depth: only attempt hosts whose name passes the same
        // allow-list we apply on the WS attach path.
        if sanitize_tmux_ident(&name).is_none() {
            tasks.spawn(async move {
                json!({
                    "host": name,
                    "reachable": false,
                    "sessions": [],
                    "error": "host name failed validation (allowed: A-Z a-z 0-9 _ . -)",
                })
            });
            continue;
        }
        tasks.spawn(async move {
            let host_for_blocking = name.clone();
            let res = tokio::time::timeout(
                std::time::Duration::from_secs(TMUX_LIST_TIMEOUT_SECS),
                tokio::task::spawn_blocking(move || run_tmux_list_for_host(&host_for_blocking)),
            )
            .await;
            match res {
                Ok(Ok((reachable, sessions, err))) => json!({
                    "host": name,
                    "reachable": reachable,
                    "sessions": sessions,
                    "error": err,
                }),
                Ok(Err(e)) => json!({
                    "host": name,
                    "reachable": false,
                    "sessions": [],
                    "error": format!("task join error: {e}"),
                }),
                Err(_) => json!({
                    "host": name,
                    "reachable": false,
                    "sessions": [],
                    "error": format!("timed out after {TMUX_LIST_TIMEOUT_SECS}s"),
                }),
            }
        });
    }

    let mut results: Vec<Value> = Vec::new();
    while let Some(r) = tasks.join_next().await {
        if let Ok(v) = r {
            results.push(v);
        }
    }
    // Stable ordering by host name so the UI doesn't shuffle on each refresh.
    results.sort_by(|a, b| {
        a.get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("host").and_then(|v| v.as_str()).unwrap_or(""))
    });

    Json(json!({
        "hosts": results,
        "refreshed_at": chrono::Utc::now().to_rfc3339(),
    }))
}

/// GET `/ws/tmux_attach/{host}/{session}` — WebSocket bridging xterm.js to
/// `azlin connect <host> --no-tmux -- tmux attach -t <session>`. The same
/// azlin exec channel as the snapshot route — no new SSH path.
pub(crate) async fn ws_tmux_attach_handler(
    Path((host, session)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> response::Response {
    let Some(safe_host) = sanitize_tmux_ident(&host) else {
        return response::Response::builder()
            .status(400)
            .header("content-type", "text/plain; charset=utf-8")
            .body(axum::body::Body::from(
                "invalid host: must match ^[A-Za-z0-9_.-]{1,64}$",
            ))
            .unwrap();
    };
    let Some(safe_session) = sanitize_tmux_ident(&session) else {
        return response::Response::builder()
            .status(400)
            .header("content-type", "text/plain; charset=utf-8")
            .body(axum::body::Body::from(
                "invalid session: must match ^[A-Za-z0-9_.-]{1,64}$",
            ))
            .unwrap();
    };

    // Host whitelist: must appear in load_hosts(). Prevents arbitrary-host
    // exec via crafted URL.
    let hosts = tokio::task::spawn_blocking(load_hosts)
        .await
        .unwrap_or_default();
    let known = hosts
        .iter()
        .any(|h| host_entry_name(h) == safe_host.as_str());
    if !known {
        return response::Response::builder()
            .status(404)
            .header("content-type", "text/plain; charset=utf-8")
            .body(axum::body::Body::from(format!(
                "unknown host '{safe_host}': not in configured hosts",
            )))
            .unwrap();
    }

    ws.on_upgrade(move |socket| handle_tmux_attach_ws(socket, safe_host, safe_session))
}

async fn handle_tmux_attach_ws(mut socket: WebSocket, host: String, session: String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::Command;

    let mut child = match Command::new("systemd-run")
        .args([
            "--user",
            "--pipe",
            "--quiet",
            "azlin",
            "connect",
            &host,
            "--no-tmux",
            "--",
            "tmux",
            "attach",
            "-t",
            &session,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    format!("[simard] failed to spawn azlin connect: {e}\n").into(),
                ))
                .await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = socket
                .send(Message::Text(
                    "[simard] internal error: child stdin unavailable\n".into(),
                ))
                .await;
            return;
        }
    };
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = socket
                .send(Message::Text(
                    "[simard] internal error: child stdout unavailable\n".into(),
                ))
                .await;
            return;
        }
    };
    let mut stderr = child.stderr.take();

    // Single-task duplex: tokio::select! on stdout reads vs ws inbound frames.
    // No socket split required (avoids depending on futures_util directly).
    let mut buf = vec![0u8; 4096];
    loop {
        tokio::select! {
            // Child stdout → ws (binary; raw passthrough preserves ANSI).
            n = stdout.read(&mut buf) => {
                match n {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = buf[..n].to_vec();
                        if socket.send(Message::Binary(chunk.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // ws → child stdin.
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(t)))
                        if stdin.write_all(t.as_bytes()).await.is_err() => {
                            break;
                        }
                    Some(Ok(Message::Binary(b)))
                        if stdin.write_all(&b).await.is_err() => {
                            break;
                        }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }

    // Drain stderr (best-effort) and forward as a final text frame.
    if let Some(mut err) = stderr.take() {
        let mut errbuf = Vec::new();
        let _ = err.read_to_end(&mut errbuf).await;
        if !errbuf.is_empty() {
            let text = String::from_utf8_lossy(&errbuf).to_string();
            let _ = socket.send(Message::Text(text.into())).await;
        }
    }
    let _ = socket.send(Message::Close(None)).await;
    let _ = stdin.shutdown().await;
    let _ = child.kill().await;
}
