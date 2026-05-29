use super::logs::read_tail;
use super::routes::resolve_state_root;

use axum::{
    extract::{
        Path,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response,
};

// ---------------------------------------------------------------------------
// Issue #947 — Agent terminal widget: WS endpoint, sanitizer, and tail loop.
// ---------------------------------------------------------------------------

/// WebSocket route path for tailing per-agent stdout/stderr logs.
///
/// Registered inside the `require_auth` middleware scope by `build_router`.
pub(crate) const WS_AGENT_LOG_ROUTE: &str = "/ws/agent_log/{agent_name}";

/// Validate `agent_name` against allow-list `^[A-Za-z0-9_-]{1,64}$`.
///
/// This is the sole defense against path traversal (INV-7): any byte that is
/// not in the allow-list (including `/`, `\`, `.`, NUL, control chars, and
/// non-ASCII) causes rejection with `None`. No filesystem-side canonicalization
/// is performed — the regex shape is sufficient to keep names confined to a
/// single path component within `agent_logs/`.
pub(crate) fn sanitize_agent_name(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return None;
    }
    for &b in bytes {
        let ok = b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
        if !ok {
            return None;
        }
    }
    Some(name.to_string())
}

/// Build the per-agent log file path: `<state_root>/agent_logs/<name>.log`.
///
/// Caller is responsible for sanitizing `name` first via
/// [`sanitize_agent_name`]. Combined with the allow-list, the resulting path
/// is guaranteed to be a direct child of `<state_root>/agent_logs/`.
pub(crate) fn agent_log_path(state_root: &std::path::Path, name: &str) -> std::path::PathBuf {
    state_root.join("agent_logs").join(format!("{name}.log"))
}

pub(crate) async fn ws_agent_log_handler(
    Path(agent_name): Path<String>,
    ws: WebSocketUpgrade,
) -> response::Response {
    let Some(safe) = sanitize_agent_name(&agent_name) else {
        return response::Response::builder()
            .status(400)
            .header("content-type", "text/plain; charset=utf-8")
            .body(axum::body::Body::from(
                "invalid agent_name: must match ^[A-Za-z0-9_-]{1,64}$",
            ))
            .unwrap();
    };
    let path = agent_log_path(&resolve_state_root(), &safe);
    ws.on_upgrade(move |socket| handle_agent_log_ws(socket, path))
}

/// Maximum number of lines sent during the initial backfill.
const AGENT_LOG_BACKFILL_LINES: usize = 200;
/// Maximum bytes read per polling tick (DoS bound on burst writes).
const AGENT_LOG_MAX_TICK_BYTES: u64 = 1_048_576; // 1 MiB
/// Polling interval for new bytes appended to the log.
const AGENT_LOG_TICK_MS: u64 = 200;
/// Maximum time to wait for the log file to appear before giving up.
const AGENT_LOG_WAIT_TIMEOUT_MS: u64 = 30_000;

pub(crate) async fn handle_agent_log_ws(mut socket: WebSocket, path: std::path::PathBuf) {
    use std::io::SeekFrom;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    use tokio::time::{Duration, sleep};

    // Phase 1: wait for the log file to appear (supervisor may not have
    // spawned the agent yet). Poll every tick up to the timeout.
    let waited_ms = wait_for_file(&path).await;
    if waited_ms.is_none() {
        let _ = socket
            .send(Message::Text(
                "[simard] no log file for this agent yet (timed out waiting). The agent may not be running.\n"
                    .to_string()
                    .into(),
            ))
            .await;
        let _ = socket.send(Message::Close(None)).await;
        return;
    }

    // Phase 2: backfill the last N lines using the existing helper, so the
    // viewer immediately sees recent context.
    let path_str = path.to_string_lossy().to_string();
    let backfill = read_tail(&path_str, AGENT_LOG_BACKFILL_LINES).unwrap_or_default();
    for line in backfill {
        if socket
            .send(Message::Text(format!("{line}\n").into()))
            .await
            .is_err()
        {
            return;
        }
    }

    // Phase 3: stream new appends. Open the file and seek to its current end
    // so we don't double-deliver the backfill lines.
    let mut file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    format!("[simard] could not open log: {e}\n").into(),
                ))
                .await;
            return;
        }
    };
    let mut pos = file.seek(SeekFrom::End(0)).await.unwrap_or(0);
    // Buffer trailing partial line until we see its newline.
    let mut partial: Vec<u8> = Vec::new();

    loop {
        // If the client sent anything (typically a close), drain it.
        if let Ok(maybe_msg) = tokio::time::timeout(Duration::from_millis(1), socket.recv()).await {
            match maybe_msg {
                Some(Ok(Message::Close(_))) | None => return,
                Some(Err(_)) => return,
                _ => {} // ignore other inbound frames (server→client only)
            }
        }

        // Detect truncation/rotation: if file shrinks below our position,
        // reset to start and drop any partial line buffered.
        let len = match tokio::fs::metadata(&path).await {
            Ok(m) => m.len(),
            Err(_) => {
                // Transient stat failure — try again next tick.
                sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
                continue;
            }
        };
        if len < pos {
            partial.clear();
            pos = 0;
            let _ = socket
                .send(Message::Text(
                    "[simard] log file truncated; resetting tail position\n"
                        .to_string()
                        .into(),
                ))
                .await;
        }

        let available = len.saturating_sub(pos);
        if available > 0 {
            let to_read = available.min(AGENT_LOG_MAX_TICK_BYTES);
            if file.seek(SeekFrom::Start(pos)).await.is_err() {
                sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
                continue;
            }
            let mut buf = vec![0u8; to_read as usize];
            match file.read_exact(&mut buf).await {
                Ok(_) => {
                    pos += to_read;
                    partial.extend_from_slice(&buf);
                    // Emit one frame per complete line.
                    while let Some(nl) = partial.iter().position(|&b| b == b'\n') {
                        let line_bytes = partial.drain(..=nl).collect::<Vec<u8>>();
                        // Strip trailing \n (and \r if present) for the frame;
                        // the client adds its own line break via writeln.
                        let mut line = String::from_utf8_lossy(&line_bytes).into_owned();
                        if line.ends_with('\n') {
                            line.pop();
                        }
                        if line.ends_with('\r') {
                            line.pop();
                        }
                        if socket.send(Message::Text(line.into())).await.is_err() {
                            return;
                        }
                    }
                }
                Err(_) => {
                    sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
                    continue;
                }
            }
        } else {
            sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
        }
    }
}

/// Poll for `path` to exist. Returns `Some(elapsed_ms)` on success or `None`
/// if the timeout is reached.
pub(crate) async fn wait_for_file(path: &std::path::Path) -> Option<u64> {
    use tokio::time::{Duration, Instant, sleep};
    let start = Instant::now();
    loop {
        if tokio::fs::metadata(path).await.is_ok() {
            return Some(start.elapsed().as_millis() as u64);
        }
        if start.elapsed() >= Duration::from_millis(AGENT_LOG_WAIT_TIMEOUT_MS) {
            return None;
        }
        sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- sanitize_agent_name ----------------------------------------------

    #[test]
    fn accepts_alphanumeric() {
        assert_eq!(sanitize_agent_name("agent01"), Some("agent01".to_string()));
    }

    #[test]
    fn accepts_hyphens_and_underscores() {
        assert_eq!(
            sanitize_agent_name("my-agent_v2"),
            Some("my-agent_v2".to_string())
        );
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(sanitize_agent_name(""), None);
    }

    #[test]
    fn rejects_over_64_chars() {
        let long = "a".repeat(65);
        assert_eq!(sanitize_agent_name(&long), None);
    }

    #[test]
    fn accepts_exactly_64_chars() {
        let exact = "a".repeat(64);
        assert!(sanitize_agent_name(&exact).is_some());
    }

    #[test]
    fn rejects_path_traversal_dots() {
        assert_eq!(sanitize_agent_name("../etc/passwd"), None);
        assert_eq!(sanitize_agent_name(".."), None);
    }

    #[test]
    fn rejects_slashes() {
        assert_eq!(sanitize_agent_name("agent/name"), None);
        assert_eq!(sanitize_agent_name("agent\\name"), None);
    }

    #[test]
    fn rejects_spaces() {
        assert_eq!(sanitize_agent_name("agent name"), None);
    }

    #[test]
    fn rejects_null_bytes() {
        assert_eq!(sanitize_agent_name("agent\0name"), None);
    }

    #[test]
    fn rejects_non_ascii() {
        assert_eq!(sanitize_agent_name("agënt"), None);
        assert_eq!(sanitize_agent_name("日本語"), None);
    }

    #[test]
    fn rejects_control_chars() {
        assert_eq!(sanitize_agent_name("agent\nname"), None);
        assert_eq!(sanitize_agent_name("agent\tname"), None);
    }

    // ---- agent_log_path ---------------------------------------------------

    #[test]
    fn agent_log_path_structure() {
        let root = std::path::Path::new("/home/user/.simard/state");
        let path = agent_log_path(root, "my-agent");
        assert_eq!(
            path,
            std::path::PathBuf::from("/home/user/.simard/state/agent_logs/my-agent.log")
        );
    }

    #[test]
    fn agent_log_path_appends_log_extension() {
        let root = std::path::Path::new("/tmp");
        let path = agent_log_path(root, "test");
        assert!(path.to_string_lossy().ends_with(".log"));
    }

    #[test]
    fn agent_log_path_uses_agent_logs_dir() {
        let root = std::path::Path::new("/state");
        let path = agent_log_path(root, "x");
        assert!(path.to_string_lossy().contains("agent_logs"));
    }

    // ---- Constants --------------------------------------------------------

    #[test]
    fn constants_have_sensible_values() {
        const { assert!(AGENT_LOG_BACKFILL_LINES > 0) };
        const { assert!(AGENT_LOG_MAX_TICK_BYTES > 0) };
        const { assert!(AGENT_LOG_TICK_MS > 0) };
        const { assert!(AGENT_LOG_WAIT_TIMEOUT_MS > AGENT_LOG_TICK_MS) };
    }

    #[test]
    fn ws_route_contains_agent_name_param() {
        assert!(WS_AGENT_LOG_ROUTE.contains("{agent_name}"));
    }

    // ---- wait_for_file (async) --------------------------------------------

    #[tokio::test]
    async fn wait_for_file_returns_immediately_if_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exists.log");
        std::fs::write(&path, "data").unwrap();
        let elapsed = wait_for_file(&path).await;
        assert!(elapsed.is_some());
        assert!(elapsed.unwrap() < 1000, "should return nearly immediately");
    }
}
