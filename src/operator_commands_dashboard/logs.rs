use axum::Json;
use serde_json::{Value, json};

use super::routes::resolve_state_root;

// ---------------------------------------------------------------------------
// Logs endpoint — returns tail of daemon log + OODA transcripts
// ---------------------------------------------------------------------------

pub(crate) async fn logs() -> Json<Value> {
    let state_root = resolve_state_root();

    // Try multiple log sources for daemon output (#414)
    let daemon_log = read_tail("/var/log/simard-daemon.log", 200)
        .or_else(|| {
            let alt_path = state_root.join("simard-daemon.log");
            read_tail(&alt_path.to_string_lossy(), 200)
        })
        .or_else(|| {
            let alt_path = state_root.join("ooda.log");
            read_tail(&alt_path.to_string_lossy(), 200)
        })
        .or_else(|| {
            let alt_path = state_root.join("simard.log");
            read_tail(&alt_path.to_string_lossy(), 200)
        })
        .unwrap_or_default();

    // Try journalctl if no file-based logs found (not a degradation —
    // journalctl is another valid log source, and the UI shows which was used).
    let combined_log = if daemon_log.is_empty() {
        read_journal_logs().await
    } else {
        daemon_log
    };

    let ooda_dir = state_root.join("ooda_transcripts");

    let mut transcripts: Vec<Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&ooda_dir) {
        let mut files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.path()));
        for entry in files.into_iter().take(10) {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let preview = read_tail(&path.to_string_lossy(), 20).unwrap_or_default();
            transcripts.push(json!({
                "name": name,
                "size_bytes": size,
                "preview_lines": preview,
            }));
        }
    }

    let cost_log = {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
        let ledger = std::path::PathBuf::from(home).join(".simard/costs/ledger.jsonl");
        read_tail(&ledger.to_string_lossy(), 50).unwrap_or_default()
    };

    // Collect recent terminal session transcripts from /tmp (#414)
    let mut terminal_transcripts: Vec<Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("simard-terminal-shell-")
            })
            .collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.path()));
        for entry in files.into_iter().take(10) {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let preview = read_tail(&path.to_string_lossy(), 20).unwrap_or_default();
            terminal_transcripts.push(json!({
                "name": name,
                "size_bytes": size,
                "preview_lines": preview,
            }));
        }
    }

    // Collect cycle reports for the logs tab
    let mut cycle_reports: Vec<Value> = Vec::new();
    let cycle_dir = state_root.join("cycle_reports");
    if let Ok(entries) = std::fs::read_dir(&cycle_dir) {
        let mut files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        files.sort_by_key(|e| e.path());
        for entry in files.into_iter() {
            let path = entry.path();
            if let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(report) = serde_json::from_str::<Value>(&content)
            {
                cycle_reports.push(report);
            }
        }
    }

    Json(json!({
        "daemon_log_lines": combined_log,
        "ooda_transcripts": transcripts,
        "terminal_transcripts": terminal_transcripts,
        "cost_log_lines": cost_log,
        "cycle_reports": cycle_reports,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

pub(crate) fn read_tail(path: &str, max_lines: usize) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<String> = content.lines().map(String::from).collect();
    let start = lines.len().saturating_sub(max_lines);
    Some(lines[start..].to_vec())
}

/// Read recent log entries from systemd journal for simard-related units (#414).
pub(crate) async fn read_journal_logs() -> Vec<String> {
    // Try user-level journal first
    let output = tokio::process::Command::new("journalctl")
        .args([
            "--user",
            "--unit=simard*",
            "--no-pager",
            "-n",
            "200",
            "--output=short-iso",
        ])
        .output()
        .await;

    if let Ok(o) = output
        && o.status.success()
    {
        let text = String::from_utf8_lossy(&o.stdout);
        let lines: Vec<String> = text
            .lines()
            .filter(|l| !l.contains("No entries"))
            .map(String::from)
            .collect();
        if !lines.is_empty() {
            return lines;
        }
    }

    // Also try system-level journal (broader scope than user-level).
    let output = tokio::process::Command::new("journalctl")
        .args([
            "--unit=simard*",
            "--no-pager",
            "-n",
            "200",
            "--output=short-iso",
        ])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines()
                .filter(|l| !l.contains("No entries"))
                .map(String::from)
                .collect()
        }
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Active processes panel
// ---------------------------------------------------------------------------

pub(crate) async fn processes() -> Json<Value> {
    let output = tokio::process::Command::new("ps")
        .args(["axo", "pid,ppid,etime,comm,args"])
        .output()
        .await;

    let mut procs: Vec<Value> = Vec::new();
    let mut root_pid: Option<String> = None;

    if let Ok(o) = output {
        let text = String::from_utf8_lossy(&o.stdout);

        // Phase 1: Parse every process into a row.
        struct PsRow {
            pid: String,
            ppid: String,
            etime: String,
            comm: String,
            full_args: String,
        }
        let mut all_rows: Vec<PsRow> = Vec::new();
        // Map parent-pid -> indices of direct children for fast descendant walk.
        let mut children_map: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();

        for line in text.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                let idx = all_rows.len();
                let row = PsRow {
                    pid: parts[0].to_string(),
                    ppid: parts[1].to_string(),
                    etime: parts[2].to_string(),
                    comm: parts[3].to_string(),
                    full_args: parts[4..].join(" "),
                };
                children_map.entry(row.ppid.clone()).or_default().push(idx);
                all_rows.push(row);
            }
        }

        // Phase 2: Locate the OODA daemon – the process whose args contain
        // "simard" AND "ooda" AND "run" (i.e. `simard ooda run`).
        let mut ooda_idx: Option<usize> = None;
        for (i, row) in all_rows.iter().enumerate() {
            let lower = row.full_args.to_lowercase();
            if lower.contains("simard") && lower.contains("ooda") && lower.contains("run") {
                ooda_idx = Some(i);
                break;
            }
        }

        // Phase 3: BFS from the OODA daemon to collect it + all descendants.
        if let Some(start) = ooda_idx {
            let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
            let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
            queue.push_back(start);
            visited.insert(start);
            root_pid = Some(all_rows[start].pid.clone());

            while let Some(idx) = queue.pop_front() {
                let row = &all_rows[idx];
                let is_root = idx == start;
                procs.push(json!({
                    "pid": row.pid,
                    "ppid": row.ppid,
                    "uptime": row.etime,
                    "command": row.comm,
                    "full_args": row.full_args,
                    "is_ooda_root": is_root,
                }));

                if let Some(kids) = children_map.get(&row.pid) {
                    for &child_idx in kids {
                        if visited.insert(child_idx) {
                            queue.push_back(child_idx);
                        }
                    }
                }
            }
        }
    }

    Json(json!({
        "processes": procs,
        "count": procs.len(),
        "root_pid": root_pid,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}
