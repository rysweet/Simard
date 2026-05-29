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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- file_metrics -----------------------------------------------------

    #[test]
    fn file_metrics_returns_size_and_modified_for_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "hello world").unwrap();

        let (size, modified) = file_metrics(&path);
        assert_eq!(size, 11);
        assert!(modified.is_some(), "modified timestamp should be present");
    }

    #[test]
    fn file_metrics_returns_zero_for_nonexistent_file() {
        let path = std::path::Path::new("/tmp/nonexistent_file_abc123xyz");
        let (size, modified) = file_metrics(path);
        assert_eq!(size, 0);
        assert!(modified.is_none());
    }

    #[test]
    fn file_metrics_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        std::fs::write(&path, "").unwrap();

        let (size, modified) = file_metrics(&path);
        assert_eq!(size, 0);
        assert!(modified.is_some());
    }

    // ---- count_json_records -----------------------------------------------

    #[test]
    fn count_json_array_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        std::fs::write(&path, r#"[{"a":1},{"b":2},{"c":3}]"#).unwrap();

        assert_eq!(count_json_records(&path), 3);
    }

    #[test]
    fn count_json_object_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        std::fs::write(&path, r#"{"key1":"v","key2":"v"}"#).unwrap();

        assert_eq!(count_json_records(&path), 2);
    }

    #[test]
    fn count_returns_zero_for_scalar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scalar.json");
        std::fs::write(&path, r#""just a string""#).unwrap();

        assert_eq!(count_json_records(&path), 0);
    }

    #[test]
    fn count_returns_zero_for_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not json at all").unwrap();

        assert_eq!(count_json_records(&path), 0);
    }

    #[test]
    fn count_returns_zero_for_nonexistent_file() {
        let path = std::path::Path::new("/tmp/nonexistent_json_abc123xyz");
        assert_eq!(count_json_records(path), 0);
    }

    #[test]
    fn count_empty_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.json");
        std::fs::write(&path, "[]").unwrap();

        assert_eq!(count_json_records(&path), 0);
    }

    #[test]
    fn count_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty_obj.json");
        std::fs::write(&path, "{}").unwrap();

        assert_eq!(count_json_records(&path), 0);
    }
}
