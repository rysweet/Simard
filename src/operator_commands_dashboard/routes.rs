use axum::{Json, Router, middleware, routing::get};
use serde_json::{Value, json};

use super::auth::require_auth;

pub fn build_router() -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/issues", get(issues))
        .route("/", get(index))
        .layer(middleware::from_fn(require_auth))
}

async fn status() -> Json<Value> {
    let version = env!("CARGO_PKG_VERSION");

    let ooda_running = std::process::Command::new("pgrep")
        .args(["-f", "simard.*ooda run"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let disk = disk_usage_pct().await;

    let child_count = std::process::Command::new("pgrep")
        .args(["-f", "-c", "copilot.*Simard|simard.*ooda|cargo.*simard"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    Json(json!({
        "version": version,
        "ooda_daemon": if ooda_running { "running" } else { "stopped" },
        "active_processes": child_count,
        "disk_usage_pct": disk,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

async fn issues() -> Json<Value> {
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,labels",
        ])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout);
            match serde_json::from_str::<Value>(&raw) {
                Ok(v) => Json(v),
                Err(_) => Json(json!({"error": "failed to parse gh output"})),
            }
        }
        _ => Json(json!({"error": "failed to run gh issue list"})),
    }
}

async fn index() -> axum::response::Html<String> {
    axum::response::Html(INDEX_HTML.to_string())
}

async fn disk_usage_pct() -> Option<u8> {
    let output = tokio::process::Command::new("df")
        .args(["--output=pcent", "/home"])
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().nth(1)?;
    line.trim().trim_end_matches('%').parse().ok()
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Simard Dashboard</title>
  <style>
    :root { --bg: #0d1117; --fg: #c9d1d9; --accent: #58a6ff; --card: #161b22; --border: #30363d; }
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: var(--bg); color: var(--fg); padding: 2rem; }
    h1 { color: var(--accent); margin-bottom: 1.5rem; font-size: 1.5rem; }
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: 1rem; }
    .card { background: var(--card); border: 1px solid var(--border); border-radius: 8px; padding: 1.25rem; }
    .card h2 { color: var(--accent); font-size: 1rem; margin-bottom: 0.75rem; border-bottom: 1px solid var(--border); padding-bottom: 0.5rem; }
    .stat { display: flex; justify-content: space-between; padding: 0.3rem 0; }
    .stat .label { color: #8b949e; }
    .stat .value { font-weight: 600; }
    .ok { color: #3fb950; }
    .warn { color: #d29922; }
    .err { color: #f85149; }
    #issues-list { list-style: none; }
    #issues-list li { padding: 0.3rem 0; border-bottom: 1px solid var(--border); }
    #issues-list li:last-child { border-bottom: none; }
    .issue-num { color: var(--accent); font-weight: 600; margin-right: 0.5rem; }
    .loading { color: #8b949e; font-style: italic; }
  </style>
</head>
<body>
  <h1>🌲 Simard Dashboard</h1>
  <div class="grid">
    <div class="card">
      <h2>System Status</h2>
      <div id="status"><span class="loading">Loading...</span></div>
    </div>
    <div class="card">
      <h2>Open Issues</h2>
      <ul id="issues-list"><li class="loading">Loading...</li></ul>
    </div>
  </div>

  <script>
    async function fetchStatus() {
      try {
        const r = await fetch('/api/status');
        const d = await r.json();
        const diskClass = d.disk_usage_pct > 90 ? 'err' : d.disk_usage_pct > 70 ? 'warn' : 'ok';
        const daemonClass = d.ooda_daemon === 'running' ? 'ok' : 'err';
        document.getElementById('status').innerHTML = `
          <div class="stat"><span class="label">Version</span><span class="value">v${d.version}</span></div>
          <div class="stat"><span class="label">OODA Daemon</span><span class="value ${daemonClass}">${d.ooda_daemon}</span></div>
          <div class="stat"><span class="label">Active Processes</span><span class="value">${d.active_processes ?? 0}</span></div>
          <div class="stat"><span class="label">Disk Usage</span><span class="value ${diskClass}">${d.disk_usage_pct ?? '?'}%</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${new Date(d.timestamp).toLocaleTimeString()}</span></div>
        `;
      } catch (e) { document.getElementById('status').innerHTML = '<span class="err">Failed to load</span>'; }
    }

    async function fetchIssues() {
      try {
        const r = await fetch('/api/issues');
        const issues = await r.json();
        if (Array.isArray(issues)) {
          document.getElementById('issues-list').innerHTML = issues.map(i =>
            `<li><span class="issue-num">#${i.number}</span>${i.title}</li>`
          ).join('');
        }
      } catch (e) { document.getElementById('issues-list').innerHTML = '<li class="err">Failed to load</li>'; }
    }

    fetchStatus(); fetchIssues();
    setInterval(fetchStatus, 30000);
    setInterval(fetchIssues, 60000);
  </script>
</body>
</html>
"#;
