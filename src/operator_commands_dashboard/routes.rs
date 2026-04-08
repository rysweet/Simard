use axum::{
    Json, Router,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    middleware, response,
    routing::get,
    routing::post,
};
use serde_json::{Value, json};

use super::auth::{require_auth, try_login};

pub fn build_router() -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/issues", get(issues))
        .route("/api/logs", get(logs))
        .route("/api/processes", get(processes))
        .route("/api/memory", get(memory_metrics))
        .route("/ws/chat", get(ws_chat_handler))
        .route("/api/login", post(login))
        .route("/login", get(login_page))
        .route("/", get(index))
        .layer(middleware::from_fn(require_auth))
}

async fn login(Json(body): Json<Value>) -> response::Response {
    let code = body.get("code").and_then(|v| v.as_str()).unwrap_or("");
    match try_login(code) {
        Some(session_token) => response::Response::builder()
            .status(200)
            .header(
                "set-cookie",
                format!(
                    "simard_session={session_token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400"
                ),
            )
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"ok": true}).to_string(),
            ))
            .unwrap(),
        None => response::Response::builder()
            .status(401)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"ok": false, "error": "invalid code"}).to_string(),
            ))
            .unwrap(),
    }
}

async fn login_page() -> response::Html<String> {
    response::Html(LOGIN_HTML.to_string())
}

async fn status() -> Json<Value> {
    let build_number = std::env::var("SIMARD_BUILD_NUMBER").unwrap_or_else(|_| "dev".to_string());
    let version = format!("{}.{}", env!("CARGO_PKG_VERSION"), build_number);

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

// ---------------------------------------------------------------------------
// WebSocket chat — bridges to Simard's meeting facilitator conversation model
// ---------------------------------------------------------------------------

async fn ws_chat_handler(ws: WebSocketUpgrade) -> response::Response {
    ws.on_upgrade(handle_ws_chat)
}

async fn handle_ws_chat(mut socket: WebSocket) {
    use crate::meeting_facilitator::{MeetingSession, MeetingSessionStatus, add_note};

    let mut session = MeetingSession {
        topic: "Dashboard Chat".to_string(),
        decisions: Vec::new(),
        action_items: Vec::new(),
        notes: Vec::new(),
        status: MeetingSessionStatus::Open,
        started_at: chrono::Utc::now().to_rfc3339(),
        participants: vec!["operator".to_string()],
        explicit_questions: Vec::new(),
    };

    let _ = socket
        .send(Message::Text(
            json!({"role":"system","content":"Connected to Simard meeting facilitator. Type a message to begin. Use /close to end."}).to_string().into(),
        ))
        .await;

    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                let text = text.to_string();
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if trimmed.eq_ignore_ascii_case("/close") {
                    session.status = MeetingSessionStatus::Closed;
                    let recap = format!(
                        "Meeting closed. Summary: {} decision(s), {} action item(s), {} note(s).",
                        session.decisions.len(),
                        session.action_items.len(),
                        session.notes.len(),
                    );
                    let _ = socket
                        .send(Message::Text(
                            json!({"role":"system","content": recap}).to_string().into(),
                        ))
                        .await;
                    break;
                }

                let _ = add_note(&mut session, trimmed);

                let reply = json!({
                    "role": "assistant",
                    "content": format!(
                        "Noted. Session has {} decision(s), {} action(s), {} note(s), {} question(s).",
                        session.decisions.len(),
                        session.action_items.len(),
                        session.notes.len(),
                        session.explicit_questions.len(),
                    ),
                    "stats": {
                        "decisions": session.decisions.len(),
                        "action_items": session.action_items.len(),
                        "notes": session.notes.len(),
                        "questions": session.explicit_questions.len(),
                    }
                });
                if socket
                    .send(Message::Text(reply.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Logs endpoint — returns tail of daemon log + OODA transcripts
// ---------------------------------------------------------------------------

async fn logs() -> Json<Value> {
    let state_root = resolve_state_root();

    let daemon_log = read_tail("/var/log/simard-daemon.log", 200)
        .or_else(|| {
            let fallback = state_root.join("simard-daemon.log");
            read_tail(&fallback.to_string_lossy(), 200)
        })
        .unwrap_or_default();

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

    Json(json!({
        "daemon_log_lines": daemon_log,
        "ooda_transcripts": transcripts,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

fn read_tail(path: &str, max_lines: usize) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<String> = content.lines().map(String::from).collect();
    let start = lines.len().saturating_sub(max_lines);
    Some(lines[start..].to_vec())
}

// ---------------------------------------------------------------------------
// Active processes panel
// ---------------------------------------------------------------------------

async fn processes() -> Json<Value> {
    let output = tokio::process::Command::new("ps")
        .args(["axo", "pid,etime,comm,args"])
        .output()
        .await;

    let mut procs: Vec<Value> = Vec::new();

    if let Ok(o) = output {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines().skip(1) {
            let lower = line.to_lowercase();
            if (lower.contains("simard") || lower.contains("ooda") || lower.contains("copilot"))
                && !lower.contains("ps axo")
            {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    procs.push(json!({
                        "pid": parts[0],
                        "uptime": parts[1],
                        "command": parts[2],
                        "full_args": parts[3..].join(" "),
                    }));
                }
            }
        }
    }

    Json(json!({
        "processes": procs,
        "count": procs.len(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// Memory metrics panel
// ---------------------------------------------------------------------------

async fn memory_metrics() -> Json<Value> {
    let state_root = resolve_state_root();

    let memory_path = state_root.join("memory_records.json");
    let evidence_path = state_root.join("evidence_records.json");
    let goal_path = state_root.join("goal_records.json");
    let handoff_path = state_root.join("latest_handoff.json");

    let memory_info = file_metrics(&memory_path);
    let evidence_info = file_metrics(&evidence_path);
    let goal_info = file_metrics(&goal_path);
    let handoff_info = file_metrics(&handoff_path);

    let fact_count = count_json_records(&memory_path);
    let evidence_count = count_json_records(&evidence_path);
    let goal_count = count_json_records(&goal_path);

    let last_consolidation = [&memory_path, &evidence_path, &goal_path]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .filter_map(|m| m.modified().ok())
        .max()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        });

    Json(json!({
        "state_root": state_root.to_string_lossy(),
        "memory_records": {
            "path": memory_path.to_string_lossy().to_string(),
            "count": fact_count,
            "size_bytes": memory_info.0,
            "modified": memory_info.1,
        },
        "evidence_records": {
            "path": evidence_path.to_string_lossy().to_string(),
            "count": evidence_count,
            "size_bytes": evidence_info.0,
            "modified": evidence_info.1,
        },
        "goal_records": {
            "path": goal_path.to_string_lossy().to_string(),
            "count": goal_count,
            "size_bytes": goal_info.0,
            "modified": goal_info.1,
        },
        "handoff": {
            "path": handoff_path.to_string_lossy().to_string(),
            "size_bytes": handoff_info.0,
            "modified": handoff_info.1,
        },
        "total_facts": fact_count + evidence_count + goal_count,
        "last_consolidation": last_consolidation,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

fn resolve_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/simard-state")
        })
}

fn file_metrics(path: &std::path::Path) -> (u64, Option<String>) {
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

fn count_json_records(path: &std::path::Path) -> u64 {
    let Ok(content) = std::fs::read_to_string(path) else {
        return 0;
    };
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Array(arr)) => arr.len() as u64,
        Ok(Value::Object(map)) => map.len() as u64,
        _ => 0,
    }
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

const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Simard — Login</title>
  <style>
    :root { --bg: #0d1117; --fg: #c9d1d9; --accent: #58a6ff; --card: #161b22; --border: #30363d; }
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: var(--bg); color: var(--fg); display: flex; align-items: center; justify-content: center; min-height: 100vh; }
    .login-card { background: var(--card); border: 1px solid var(--border); border-radius: 12px; padding: 2rem; width: 340px; text-align: center; }
    h1 { color: var(--accent); font-size: 1.3rem; margin-bottom: 0.5rem; }
    p { color: #8b949e; font-size: 0.85rem; margin-bottom: 1.5rem; }
    input { width: 100%; padding: 0.6rem; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--fg); font-size: 1.1rem; text-align: center; letter-spacing: 0.15em; }
    input:focus { outline: none; border-color: var(--accent); }
    button { width: 100%; margin-top: 1rem; padding: 0.6rem; border: none; border-radius: 6px; background: var(--accent); color: #0d1117; font-weight: 600; font-size: 0.95rem; cursor: pointer; }
    button:hover { opacity: 0.9; }
    .error { color: #f85149; margin-top: 0.75rem; font-size: 0.85rem; display: none; }
  </style>
</head>
<body>
  <div class="login-card">
    <h1>🌲 Simard</h1>
    <p>Enter the login code from the server terminal</p>
    <form id="login-form">
      <input id="code" type="text" placeholder="code" autocomplete="off" autofocus maxlength="8">
      <button type="submit">Log in</button>
    </form>
    <div class="error" id="error">Invalid code. Check terminal output.</div>
  </div>
  <script>
    document.getElementById('login-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const code = document.getElementById('code').value;
      const r = await fetch('/api/login', { method: 'POST', headers: {'content-type':'application/json'}, body: JSON.stringify({code}) });
      if (r.ok) { window.location.href = '/'; }
      else { document.getElementById('error').style.display = 'block'; }
    });
  </script>
</body>
</html>
"#;

const INDEX_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Simard Dashboard v2</title>
  <style>
    :root { --bg:#0d1117; --fg:#c9d1d9; --accent:#58a6ff; --card:#161b22; --border:#30363d; --green:#3fb950; --yellow:#d29922; --red:#f85149; }
    *{margin:0;padding:0;box-sizing:border-box}
    body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:var(--bg);color:var(--fg)}
    header{display:flex;align-items:center;justify-content:space-between;padding:1rem 2rem;border-bottom:1px solid var(--border)}
    header h1{color:var(--accent);font-size:1.3rem}
    .tabs{display:flex;gap:0;border-bottom:1px solid var(--border);padding:0 2rem}
    .tab{padding:.6rem 1.2rem;cursor:pointer;color:#8b949e;border-bottom:2px solid transparent;font-size:.9rem}
    .tab:hover{color:var(--fg)} .tab.active{color:var(--accent);border-bottom-color:var(--accent)}
    .tab-content{display:none;padding:1.5rem 2rem} .tab-content.active{display:block}
    .grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(340px,1fr));gap:1rem}
    .card{background:var(--card);border:1px solid var(--border);border-radius:8px;padding:1.25rem}
    .card h2{color:var(--accent);font-size:1rem;margin-bottom:.75rem;border-bottom:1px solid var(--border);padding-bottom:.5rem}
    .stat{display:flex;justify-content:space-between;padding:.3rem 0}
    .stat .label{color:#8b949e} .stat .value{font-weight:600}
    .ok{color:var(--green)} .warn{color:var(--yellow)} .err{color:var(--red)}
    #issues-list{list-style:none}
    #issues-list li{padding:.3rem 0;border-bottom:1px solid var(--border)}
    #issues-list li:last-child{border-bottom:none}
    .issue-num{color:var(--accent);font-weight:600;margin-right:.5rem}
    .loading{color:#8b949e;font-style:italic}
    .log-box{background:#010409;border:1px solid var(--border);border-radius:6px;padding:.75rem;font-family:'SF Mono','Fira Code',monospace;font-size:.8rem;max-height:500px;overflow-y:auto;white-space:pre-wrap;word-break:break-all;line-height:1.4;color:#8b949e}
    .transcript-item{background:var(--card);border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem}
    .transcript-item h3{font-size:.85rem;color:var(--accent);margin-bottom:.4rem}
    .proc-table{width:100%;border-collapse:collapse;font-size:.85rem}
    .proc-table th{text-align:left;color:#8b949e;padding:.4rem .6rem;border-bottom:1px solid var(--border)}
    .proc-table td{padding:.4rem .6rem;border-bottom:1px solid var(--border)}
    .proc-table tr:last-child td{border-bottom:none}
    #chat-messages{background:#010409;border:1px solid var(--border);border-radius:6px;padding:.75rem;height:400px;overflow-y:auto;font-size:.9rem;margin-bottom:.75rem}
    .chat-msg{margin-bottom:.5rem} .chat-msg .role{font-weight:700;margin-right:.5rem}
    .chat-msg .role.user{color:var(--accent)} .chat-msg .role.system{color:var(--yellow)} .chat-msg .role.assistant{color:var(--green)}
    #chat-input-row{display:flex;gap:.5rem}
    #chat-input{flex:1;padding:.5rem;border:1px solid var(--border);border-radius:6px;background:var(--card);color:var(--fg);font-size:.9rem;resize:none;height:42px}
    #chat-input:focus{outline:none;border-color:var(--accent)}
    #chat-send{padding:.5rem 1.2rem;border:none;border-radius:6px;background:var(--accent);color:#0d1117;font-weight:600;cursor:pointer}
    #chat-send:hover{opacity:.9}
    .ws-status{font-size:.8rem;color:#8b949e;margin-bottom:.5rem} .ws-status.connected{color:var(--green)} .ws-status.disconnected{color:var(--red)}
    .mem-file{background:var(--card);border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem}
    .mem-file h3{font-size:.85rem;color:var(--accent);margin-bottom:.4rem}
    .badge{display:inline-block;padding:.15rem .5rem;border-radius:10px;font-size:.75rem;font-weight:600;background:#1f6feb33;color:var(--accent)}
    .btn{background:var(--accent);color:#0d1117;border:none;border-radius:4px;padding:.2rem .6rem;cursor:pointer;font-size:.8rem;float:right}
    .btn:hover{opacity:.9}
  </style>
</head>
<body>
  <header>
    <h1>🌲 Simard Dashboard <span style="font-size:.75rem;color:#8b949e">v2</span></h1>
    <span id="clock" style="color:#8b949e;font-size:.85rem"></span>
  </header>
  <div class="tabs">
    <div class="tab active" data-tab="overview">Overview</div>
    <div class="tab" data-tab="logs">Logs</div>
    <div class="tab" data-tab="processes">Processes</div>
    <div class="tab" data-tab="memory">Memory</div>
    <div class="tab" data-tab="chat">Chat</div>
  </div>

  <div class="tab-content active" id="tab-overview">
    <div class="grid">
      <div class="card"><h2>System Status</h2><div id="status"><span class="loading">Loading…</span></div></div>
      <div class="card"><h2>Open Issues</h2><ul id="issues-list"><li class="loading">Loading…</li></ul></div>
    </div>
  </div>

  <div class="tab-content" id="tab-logs">
    <div class="card" style="margin-bottom:1rem">
      <h2>Daemon Log <button class="btn" onclick="fetchLogs()">Refresh</button></h2>
      <div id="daemon-log" class="log-box"><span class="loading">Loading…</span></div>
    </div>
    <h2 style="color:var(--accent);font-size:1rem;margin-bottom:.5rem">OODA Transcripts</h2>
    <div id="ooda-transcripts"><span class="loading">Loading…</span></div>
  </div>

  <div class="tab-content" id="tab-processes">
    <div class="card">
      <h2>Active Simard Processes <button class="btn" onclick="fetchProcesses()">Refresh</button></h2>
      <div id="proc-count" style="margin-bottom:.5rem;color:#8b949e;font-size:.85rem"></div>
      <div id="proc-table"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-memory">
    <div class="grid">
      <div class="card"><h2>Memory Overview</h2><div id="mem-overview"><span class="loading">Loading…</span></div></div>
      <div class="card"><h2>Memory Files</h2><div id="mem-files"><span class="loading">Loading…</span></div></div>
    </div>
  </div>

  <div class="tab-content" id="tab-chat">
    <div class="card" style="max-width:720px">
      <h2>Meeting Chat</h2>
      <div class="ws-status disconnected" id="ws-status">● Disconnected</div>
      <div id="chat-messages"></div>
      <div id="chat-input-row">
        <textarea id="chat-input" placeholder="Type a message… (/close to end session)"></textarea>
        <button id="chat-send" onclick="sendChat()">Send</button>
      </div>
    </div>
  </div>

  <script>
    /* --- Tabs --- */
    document.querySelectorAll('.tab').forEach(tab=>{
      tab.addEventListener('click',()=>{
        document.querySelectorAll('.tab').forEach(t=>t.classList.remove('active'));
        document.querySelectorAll('.tab-content').forEach(c=>c.classList.remove('active'));
        tab.classList.add('active');
        document.getElementById('tab-'+tab.dataset.tab).classList.add('active');
        if(tab.dataset.tab==='logs') fetchLogs();
        if(tab.dataset.tab==='processes') fetchProcesses();
        if(tab.dataset.tab==='memory') fetchMemory();
        if(tab.dataset.tab==='chat') initChat();
      });
    });
    setInterval(()=>{document.getElementById('clock').textContent=new Date().toLocaleTimeString()},1000);

    /* --- Status --- */
    async function fetchStatus(){
      try{
        const r=await fetch('/api/status'); const d=await r.json();
        const dc=d.disk_usage_pct>90?'err':d.disk_usage_pct>70?'warn':'ok';
        const oc=d.ooda_daemon==='running'?'ok':'err';
        document.getElementById('status').innerHTML=`
          <div class="stat"><span class="label">Version</span><span class="value">v${d.version}</span></div>
          <div class="stat"><span class="label">OODA Daemon</span><span class="value ${oc}">${d.ooda_daemon}</span></div>
          <div class="stat"><span class="label">Active Processes</span><span class="value">${d.active_processes??0}</span></div>
          <div class="stat"><span class="label">Disk Usage</span><span class="value ${dc}">${d.disk_usage_pct??'?'}%</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${new Date(d.timestamp).toLocaleTimeString()}</span></div>`;
      }catch(e){document.getElementById('status').innerHTML='<span class="err">Failed to load</span>';}
    }

    /* --- Issues --- */
    async function fetchIssues(){
      try{
        const r=await fetch('/api/issues'); const issues=await r.json();
        if(Array.isArray(issues)){
          document.getElementById('issues-list').innerHTML=issues.map(i=>
            `<li><span class="issue-num">#${i.number}</span>${i.title}</li>`
          ).join('');
        }
      }catch(e){document.getElementById('issues-list').innerHTML='<li class="err">Failed to load</li>';}
    }

    /* --- Logs --- */
    async function fetchLogs(){
      try{
        const r=await fetch('/api/logs'); const d=await r.json();
        const el=document.getElementById('daemon-log');
        el.textContent=d.daemon_log_lines?.length?d.daemon_log_lines.join('\n'):'(no daemon log found)';
        el.scrollTop=el.scrollHeight;
        const tEl=document.getElementById('ooda-transcripts');
        if(d.ooda_transcripts?.length){
          tEl.innerHTML=d.ooda_transcripts.map(t=>`
            <div class="transcript-item">
              <h3>${esc(t.name)} <span class="badge">${fmtB(t.size_bytes)}</span></h3>
              <div class="log-box" style="max-height:200px">${esc((t.preview_lines||[]).join('\n'))||'(empty)'}</div>
            </div>`).join('');
        }else{tEl.innerHTML='<span class="loading">No OODA transcripts found</span>';}
      }catch(e){document.getElementById('daemon-log').textContent='Failed to load logs';}
    }

    /* --- Processes --- */
    async function fetchProcesses(){
      try{
        const r=await fetch('/api/processes'); const d=await r.json();
        document.getElementById('proc-count').textContent=`${d.count} process(es) detected`;
        if(d.processes?.length){
          document.getElementById('proc-table').innerHTML=`
            <table class="proc-table">
              <tr><th>PID</th><th>Uptime</th><th>Command</th><th>Arguments</th></tr>
              ${d.processes.map(p=>`<tr><td>${esc(p.pid)}</td><td>${esc(p.uptime)}</td><td>${esc(p.command)}</td><td style="max-width:400px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(p.full_args)}</td></tr>`).join('')}
            </table>`;
        }else{document.getElementById('proc-table').innerHTML='<span class="loading">No Simard processes found</span>';}
      }catch(e){document.getElementById('proc-table').innerHTML='<span class="err">Failed to load</span>';}
    }

    /* --- Memory --- */
    async function fetchMemory(){
      try{
        const r=await fetch('/api/memory'); const d=await r.json();
        document.getElementById('mem-overview').innerHTML=`
          <div class="stat"><span class="label">Total Facts</span><span class="value">${d.total_facts}</span></div>
          <div class="stat"><span class="label">Last Consolidation</span><span class="value">${d.last_consolidation?new Date(d.last_consolidation).toLocaleString():'N/A'}</span></div>
          <div class="stat"><span class="label">State Root</span><span class="value" style="font-size:.8rem;word-break:break-all">${esc(d.state_root)}</span></div>`;
        const files=[
          {key:'memory_records',label:'Memory Records'},
          {key:'evidence_records',label:'Evidence Records'},
          {key:'goal_records',label:'Goal Records'},
          {key:'handoff',label:'Latest Handoff'}];
        document.getElementById('mem-files').innerHTML=files.map(f=>{
          const info=d[f.key]||{};
          return`<div class="mem-file">
            <h3>${f.label} ${info.count!==undefined?'<span class="badge">'+info.count+' records</span>':''} <span class="badge">${fmtB(info.size_bytes||0)}</span></h3>
            <div class="stat"><span class="label">Modified</span><span class="value">${info.modified?new Date(info.modified).toLocaleString():'N/A'}</span></div>
          </div>`;}).join('');
      }catch(e){document.getElementById('mem-overview').innerHTML='<span class="err">Failed to load</span>';}
    }

    /* --- Chat --- */
    let ws=null,chatInit=false;
    function initChat(){
      if(chatInit&&ws&&ws.readyState===WebSocket.OPEN) return;
      chatInit=true;
      const proto=location.protocol==='https:'?'wss:':'ws:';
      ws=new WebSocket(`${proto}//${location.host}/ws/chat`);
      const st=document.getElementById('ws-status');
      ws.onopen=()=>{st.textContent='● Connected';st.className='ws-status connected';};
      ws.onclose=()=>{st.textContent='● Disconnected';st.className='ws-status disconnected';chatInit=false;};
      ws.onerror=()=>{st.textContent='● Error';st.className='ws-status disconnected';};
      ws.onmessage=ev=>{try{const m=JSON.parse(ev.data);appendMsg(m.role||'system',m.content||ev.data);}catch{appendMsg('system',ev.data);}};
    }
    function sendChat(){
      const inp=document.getElementById('chat-input'); const txt=inp.value.trim();
      if(!txt||!ws||ws.readyState!==WebSocket.OPEN) return;
      appendMsg('user',txt); ws.send(txt); inp.value='';
    }
    function appendMsg(role,content){
      const el=document.getElementById('chat-messages');
      el.innerHTML+=`<div class="chat-msg"><span class="role ${role}">${role}:</span> ${esc(content)}</div>`;
      el.scrollTop=el.scrollHeight;
    }
    document.getElementById('chat-input').addEventListener('keydown',e=>{
      if(e.key==='Enter'&&!e.shiftKey){e.preventDefault();sendChat();}
    });

    /* --- Helpers --- */
    function fmtB(b){if(b<1024)return b+' B';if(b<1048576)return(b/1024).toFixed(1)+' KB';return(b/1048576).toFixed(1)+' MB';}
    function esc(s){const d=document.createElement('div');d.textContent=s;return d.innerHTML;}

    /* --- Init --- */
    fetchStatus(); fetchIssues();
    setInterval(fetchStatus,30000);
    setInterval(fetchIssues,60000);
  </script>
</body>
</html>
"##;
