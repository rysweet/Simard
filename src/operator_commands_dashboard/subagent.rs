use axum::Json;
use serde_json::{Value, json};

pub(crate) fn file_metrics(path: &std::path::Path) -> (u64, Option<String>) {
    match std::fs::metadata(path) {
        Ok(m) => {
            let size = m.len();
            let modified = m.modified().ok().map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.to_rfc3339()
            });
            (size, modified)
        }
        Err(_) => (0, None),
    }
}

pub(crate) fn count_json_records(path: &std::path::Path) -> u64 {
    let Ok(content) = std::fs::read_to_string(path) else {
        return 0;
    };
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Array(arr)) => arr.len() as u64,
        Ok(Value::Object(map)) => map.len() as u64,
        _ => 0,
    }
}

pub(crate) async fn disk_usage_pct() -> Option<u8> {
    let output = tokio::process::Command::new("df")
        .args(["--output=pcent", "/home"])
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().nth(1)?;
    line.trim().trim_end_matches('%').parse().ok()
}

/// WS-2: list live and recently-ended subagent tmux sessions.
///
/// Returns `{ live: [...], recently_ended: [...] }` sorted by `created_at`
/// descending. The dashboard polls this every 5s to populate the Subagent
/// Sessions card and to drive Attach deep-links in the Recent Actions feed.
pub(crate) async fn subagent_sessions() -> Json<Value> {
    let reg = crate::subagent_sessions::load();
    let mut live: Vec<&crate::subagent_sessions::SubagentSession> = reg
        .sessions
        .iter()
        .filter(|s| s.ended_at.is_none())
        .collect();
    let mut ended: Vec<&crate::subagent_sessions::SubagentSession> = reg
        .sessions
        .iter()
        .filter(|s| s.ended_at.is_some())
        .collect();
    live.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    ended.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    Json(json!({
        "live": live,
        "recently_ended": ended,
    }))
}
