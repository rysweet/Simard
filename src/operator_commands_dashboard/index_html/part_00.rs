pub(crate) const PART_00: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{{DEFAULT_TITLE}}</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/xterm@5.3.0/css/xterm.min.css">
  <script src="https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.min.js"></script>
  <style>
    :root { --bg:#0d1117; --fg:#c9d1d9; --accent:#58a6ff; --card:#161b22; --border:#30363d; --green:#3fb950; --yellow:#d29922; --red:#f85149; }
    *{margin:0;padding:0;box-sizing:border-box}
    body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:var(--bg);color:var(--fg)}
    header{display:flex;align-items:center;justify-content:space-between;padding:1rem 2rem;border-bottom:1px solid var(--border)}
    header .brand{color:var(--accent);font-size:1.3rem;font-weight:600}
    .tabs{display:flex;gap:0;border-bottom:1px solid var(--border);padding:0 2rem}
    .tab{padding:.6rem 1.2rem;cursor:pointer;color:#8b949e;border-bottom:2px solid transparent;font-size:.9rem}
    .tab:hover{color:var(--fg)} .tab.active{color:var(--accent);border-bottom-color:var(--accent)}
    .tab-content{display:none;padding:1.5rem 2rem} .tab-content.active{display:block}
    .page-h1{color:var(--accent);font-size:1.4rem;margin:0 0 .35rem 0;line-height:1.2}
    .page-lede{padding:.5rem .75rem;margin:0 0 1rem;background:#1a2332;border-left:3px solid var(--accent);border-radius:4px;color:#9bb1c4;font-size:.85rem;line-height:1.5}
    .subsection{color:var(--accent);font-size:1.1rem;margin:1.75rem 0 .75rem;padding-bottom:.35rem;border-bottom:1px solid var(--border);font-weight:600}
    .subsection:first-of-type{margin-top:.5rem}
    .subsection-lede{color:#9bb1c4;font-size:.82rem;line-height:1.5;margin:0 0 .75rem}
    .section-anchor{display:block;height:0;overflow:hidden;scroll-margin-top:80px}
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
    .loading{color:#8b949e;font-style:italic;display:inline-flex;align-items:center;gap:.5rem}
    .loading::before{content:'';width:1rem;height:1rem;border:2px solid #30363d;border-top-color:#58a6ff;border-radius:50%;animation:spin .8s linear infinite;flex-shrink:0}
    @keyframes spin{to{transform:rotate(360deg)}}
    .log-box{background:#010409;border:1px solid var(--border);border-radius:6px;padding:.75rem;font-family:'SF Mono','Fira Code',monospace;font-size:.8rem;max-height:500px;overflow-y:auto;white-space:pre-wrap;word-break:break-all;line-height:1.4;color:#8b949e}
    .transcript-item{background:var(--card);border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem}
    .transcript-item h3{font-size:.85rem;color:var(--accent);margin-bottom:.4rem}
    .proc-table{width:100%;border-collapse:collapse;font-size:.85rem}
    .proc-table th{text-align:left;color:#8b949e;padding:.4rem .6rem;border-bottom:1px solid var(--border)}
    .proc-table td{padding:.4rem .6rem;border-bottom:1px solid var(--border)}
    .proc-table tr:last-child td{border-bottom:none}
    .proc-tree .proc-row{display:flex;gap:.5rem;align-items:baseline;padding:.25rem .5rem;border-bottom:1px solid var(--border);font-family:monospace;font-size:.82rem}
    .proc-tree .proc-row:hover{background:rgba(88,166,255,0.05)}
    .proc-tree .proc-pid{color:var(--accent);min-width:4rem;font-weight:600}
    .proc-tree .proc-uptime{min-width:6rem}
    .proc-tree .proc-kids.collapsed{display:none}
    .proc-tree .proc-kids{border-left:1px solid #30363d;margin-left:8px}
    #chat-messages{background:#010409;border:1px solid var(--border);border-radius:6px;padding:.6rem;flex:1;min-height:0;overflow-y:auto;font-size:.9rem;margin-bottom:.5rem}
    .chat-msg{margin-bottom:.5rem} .chat-msg .role{font-weight:700;margin-right:.5rem}
    .chat-msg .role.user{color:var(--accent)} .chat-msg .role.system{color:var(--yellow)} .chat-msg .role.assistant{color:var(--green)}
    .chat-typing{padding:.25rem 0;color:#8b949e}
    #tab-chat.active{display:flex;flex-direction:column;height:calc(100vh - 106px);padding:.4rem 1.25rem .6rem}
    #tab-chat .page-h1{margin-bottom:.1rem}
    #tab-chat .page-lede{margin:0 0 .4rem;padding:.3rem .6rem}
    .chat-layout{display:flex;gap:1rem;flex:1;min-height:0}
    #chat-sidebar{flex:0 0 240px;display:flex;flex-direction:column;background:var(--card);border:1px solid var(--border);border-radius:8px;padding:.6rem;min-height:0}
    #chat-new{width:100%;padding:.5rem;border:none;border-radius:6px;background:var(--accent);color:#0d1117;font-weight:600;cursor:pointer;margin-bottom:.5rem;font-size:.85rem}
    #chat-new:hover{opacity:.9}
    #chat-sessions{flex:1;overflow-y:auto;min-height:0}
    .chat-session-item{padding:.45rem .5rem;border-radius:6px;cursor:pointer;border:1px solid transparent;margin-bottom:.25rem}
    .chat-session-item:hover{background:rgba(88,166,255,0.08)}
    .chat-session-item.active{background:rgba(88,166,255,0.12);border-color:var(--accent)}
    .chat-session-item .cs-title{color:var(--fg);font-weight:600;font-size:.8rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
    .chat-session-item .cs-time{color:#8b949e;font-size:.7rem;margin-top:.1rem}
    .chat-session-empty{color:#8b949e;font-size:.8rem;padding:.5rem;line-height:1.4}
    #chat-panel{flex:1;display:flex;flex-direction:column;min-height:0;padding:.7rem}
    #chat-panel h2{margin-bottom:.4rem}
    #chat-panel .ws-status{margin-bottom:.35rem}
    .typing-dots span{animation:blink 1.4s infinite both;font-size:1.2em}
    .typing-dots span{animation:blink 1.4s infinite both;font-size:1.2em}
    .typing-dots span:nth-child(2){animation-delay:.2s}
    .typing-dots span:nth-child(3){animation-delay:.4s}
    @keyframes blink{0%,80%,100%{opacity:0}40%{opacity:1}}
    #chat-send:disabled{opacity:.5;cursor:not-allowed}
    #chat-input-row{display:flex;gap:.5rem;align-items:flex-end}
    /* max-height here is coupled to the JS auto-grow clamp in part_04.rs (150) */
    #chat-input{flex:1;padding:.5rem;border:1px solid var(--border);border-radius:6px;background:var(--card);color:var(--fg);font-size:.9rem;resize:none;min-height:42px;max-height:150px;overflow-y:auto;box-sizing:border-box;line-height:1.4}
    #chat-input:focus{outline:none;border-color:var(--accent)}
    #chat-send{padding:.5rem 1.2rem;border:none;border-radius:6px;background:var(--accent);color:#0d1117;font-weight:600;cursor:pointer}
    #chat-send:hover{opacity:.9}
    .ws-status{font-size:.8rem;color:#8b949e;margin-bottom:.5rem} .ws-status.connected{color:var(--green)} .ws-status.disconnected{color:var(--red)}
    .mem-file{background:var(--card);border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem}
    .mem-file h3{font-size:.85rem;color:var(--accent);margin-bottom:.4rem}
    .badge{display:inline-block;padding:.15rem .5rem;border-radius:10px;font-size:.75rem;font-weight:600;background:#1f6feb33;color:var(--accent)}
    .btn{background:var(--accent);color:#0d1117;border:none;border-radius:4px;padding:.2rem .6rem;cursor:pointer;font-size:.8rem;float:right}
    .btn:hover{opacity:.9}
    .thinking-cycle{border:1px solid var(--border);border-radius:8px;padding:1rem;margin-bottom:1rem;background:var(--card)}
    .thinking-cycle.legacy{opacity:0.7}
    .cycle-header{display:flex;align-items:center;gap:.75rem;margin-bottom:.75rem;padding-bottom:.5rem;border-bottom:1px solid var(--border)}
    .cycle-num{font-weight:700;font-size:1rem;color:var(--accent)}
    .cycle-summary-inline{font-size:.85rem;color:#8b949e}
    .cycle-badge{font-size:.7rem;padding:2px 6px;border-radius:4px;background:#21262d;color:#8b949e}
    .phase{margin-bottom:.75rem;padding-left:1rem;border-left:3px solid var(--border)}
    .phase.observe{border-left-color:var(--accent)}
    .phase.orient{border-left-color:var(--yellow)}
    .phase.decide{border-left-color:#a371f7}
    .phase.act{border-left-color:var(--green)}
    .phase-label{font-weight:600;font-size:.9rem;margin-bottom:.3rem}
    .phase-content{font-size:.85rem;color:#c9d1d9}
    .phase-content div{margin-bottom:.2rem}
    .goal-line{padding-left:.5rem;color:#8b949e}
    .priority-line{padding-left:.5rem}
    .urgency{margin-right:.3rem}
    .outcome{padding:.4rem;border-radius:4px;margin-bottom:.3rem}
    .outcome.success{background:rgba(63,185,80,0.1)}
    .outcome.failure{background:rgba(248,81,73,0.1)}
    .outcome-detail{font-size:.8rem;color:#8b949e;margin-top:.2rem;padding-left:1rem;font-family:monospace;white-space:pre-wrap;max-height:100px;overflow-y:auto}
    abbr[title]{text-decoration:underline dotted var(--accent);text-underline-offset:2px;cursor:help;color:var(--fg)}
    abbr[title]:hover{color:var(--accent)}
    .glossary-toggle{cursor:pointer;color:var(--accent);font-size:.85rem;padding:.2rem .5rem;border:1px solid var(--accent);border-radius:4px;background:transparent;margin-left:.5rem}
    .glossary-toggle:hover{background:rgba(88,166,255,0.1)}
    .glossary-panel{display:none;position:fixed;right:0;top:0;width:340px;height:100vh;background:var(--card);border-left:1px solid var(--border);padding:1.5rem;overflow-y:auto;z-index:1000;box-shadow:-4px 0 12px rgba(0,0,0,0.3)}
    .glossary-panel.open{display:block}
    .glossary-panel h3{color:var(--accent);margin-bottom:1rem;display:flex;justify-content:space-between;align-items:center}
    .glossary-panel .close-btn{cursor:pointer;color:#8b949e;font-size:1.2rem;background:none;border:none}
    .glossary-entry{padding:.5rem 0;border-bottom:1px solid var(--border)}
    .glossary-entry dt{color:var(--accent);font-weight:600;font-size:.9rem;margin-bottom:.2rem}
    .glossary-entry dd{color:#8b949e;font-size:.85rem;margin:0;line-height:1.4}
  </style>
</head>
<body>
  <header>
    <div class="brand">🌲 Simard Dashboard</div>
    <div style="display:flex;align-items:center;gap:1rem">
      <span id="header-version" style="font-size:.75rem;color:#8b949e"></span>
      <a href="https://github.com/rysweet/Simard" target="_blank" style="color:#8b949e;text-decoration:none;font-size:.85rem;padding:.2rem .4rem" title="Source on GitHub">⟨/⟩ Source</a>
      <a href="https://github.com/rysweet/Simard/releases/latest" target="_blank" style="color:#3fb950;text-decoration:none;font-size:.85rem;border:1px solid #3fb950;padding:.2rem .6rem;border-radius:4px">📦 Releases</a>
      <button class="glossary-toggle" onclick="toggleGlossary()" title="Show glossary of Simard terms">📖 Glossary</button>
      {{FEEDBACK_WIDGET_BUTTON}}
      <span id="clock" style="color:#8b949e;font-size:.85rem"></span>
    </div>
  </header>
  {{TAB_NAV}}


  <div class="tab-content active" id="tab-overview">
    <h1 class="page-h1">Overview</h1>
    <p class="page-lede">A live look at what the Simard daemon is doing right now, plus system health, open work items, aggregate run counters, and any other Simard hosts in your cluster.</p>
    <h2 class="subsection">Summary</h2>
    <div class="card" style="margin-bottom:1rem;border:1px solid #238636;background:linear-gradient(135deg,#0d1117,#0f1a12)">
      <h2 style="color:#3fb950;margin-bottom:.75rem">🤖 Simard — Autonomous Agent</h2>
      <div id="agent-live-status"><span class="loading">Loading agent status…</span></div>
    </div>
    <div class="grid">
      <div class="card">
        <h2>Recent Actions <button class="btn" onclick="fetchStatus()" style="font-size:.75rem">Refresh</button></h2>
        <div id="recent-actions-list"><span class="loading">Loading…</span></div>
      </div>
    </div>
    <h2 class="subsection">Health</h2>
    <div class="grid">
      <div class="card" data-testid="merge-readiness-card" style="grid-column:1 / -1">
        <h2>Merge Readiness <button class="btn" onclick="fetchMergeReadiness()" style="font-size:.75rem">Refresh</button></h2>
        <div id="merge-readiness-panel"><span class="loading">Loading…</span></div>
      </div>
      <div class="card"><h2>System Status</h2><div id="status"><span class="loading">Loading…</span></div></div>
      <div class="card" data-testid="cognition-recall-precision-card">
        <h2>Cognition: Recall Precision <button class="btn" onclick="fetchRecallPrecision()" style="font-size:.75rem">Refresh</button></h2>
        <p class="card-lede" style="margin:0 0 .75rem;color:#8b949e;font-size:.8rem;line-height:1.5">Hybrid measurement (issue #2491 / #2494): recall precision@k is trusted only when it improves on the fixed benchmark <strong>and</strong> trends the same way live. The verdict crosses both rails.</p>
        <div id="cognition-recall-precision"><span class="loading">Loading…</span></div>
      </div>
      <div class="card"><h2>Open Issues</h2><ul id="issues-list"><li class="loading">Loading…</li></ul></div>
      <div class="card">
        <h2>Machines &amp; Memory Sharing <button class="btn" onclick="fetchDistributed()">Refresh</button></h2>
        <p class="card-lede" style="margin:0 0 .75rem;color:#8b949e;font-size:.8rem;line-height:1.5">Whether Simard is running on a single machine or coordinating a group of them, and how those machines share what they've learned. On a single-machine setup the counters below stay at zero.</p>
        <div id="cluster-topology"><span class="loading">Loading…</span></div>
      </div>
      <div class="card">
        <h2>Remote VMs</h2>
        <div id="remote-vms"><span class="loading">Loading…</span></div>
      </div>
      <div class="card">
        <h2>Azlin Hosts</h2>
        <div id="hosts-list"><span class="loading">Loading…</span></div>
        <div style="margin-top:1rem;display:flex;gap:0.5rem;align-items:center;flex-wrap:wrap">
          <input id="host-name" placeholder="VM name" style="padding:4px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px;width:12rem">
          <input id="host-rg" placeholder="Resource group" value="rysweet-linux-vm-pool" style="padding:4px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px;width:16rem">
          <button class="btn" onclick="addHost()">Add Host</button>
          <span id="host-status"></span>
        </div>
      </div>
    </div>
    <span class="section-anchor" id="section-status"></span>
    <h2 class="subsection">Stats</h2>
    <div class="card" style="max-width:1100px">
      <h2>Operational Status <button class="btn" onclick="fetchStatusSnapshot()" style="font-size:.75rem">Refresh</button></h2>
      <div id="status-snapshot-panel"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-goals">
    <h1 class="page-h1">Goals</h1>
    <p class="page-lede">The things you have asked Simard to accomplish — active goals in progress now, the queued backlog, and a work board of the tasks it is moving through.</p>
    <h2 class="subsection">Goals</h2>
    <div class="card" style="margin-bottom:1rem">
      <h2>Active Goals
        <button class="btn" onclick="fetchGoals()">Refresh</button>
        <button class="btn" onclick="seedGoals()" style="margin-left:.5rem">Seed Default Goals</button>
        <button class="btn" onclick="showAddGoalForm()" style="margin-left:.5rem">+ Add Goal</button>
      </h2>
      <div id="add-goal-form" style="display:none;margin-bottom:1rem;padding:.75rem;background:var(--bg);border:1px solid var(--border);border-radius:6px">
        <div style="display:flex;gap:.5rem;margin-bottom:.5rem">
          <input id="new-goal-desc" placeholder="Goal description" style="flex:1;padding:.4rem;background:var(--card);color:var(--fg);border:1px solid var(--border);border-radius:4px">
          <select id="new-goal-type" style="padding:.4rem;background:var(--card);color:var(--fg);border:1px solid var(--border);border-radius:4px">
            <option value="active">Active</option>
            <option value="backlog">Backlog</option>
          </select>
          <input id="new-goal-priority" type="number" min="1" max="5" value="3" style="width:50px;padding:.4rem;background:var(--card);color:var(--fg);border:1px solid var(--border);border-radius:4px" placeholder="Pri">
        </div>
        <div style="display:flex;gap:.5rem">
          <button class="btn" onclick="submitGoal()">Add</button>
          <button class="btn" onclick="document.getElementById('add-goal-form').style.display='none'" style="background:#21262d">Cancel</button>
        </div>
      </div>
      <div id="goals-tag-filter" style="margin:.4rem 0;font-size:.8rem;color:#8b949e"></div>
      <div id="goals-active"><span class="loading">Loading…</span></div>
    </div>
    <div class="card">
      <h2>Backlog</h2>
      <div id="goals-backlog"><span class="loading">Loading…</span></div>
    </div>
    <span class="section-anchor" id="section-workboard"></span>
    <h2 class="subsection">Work Board</h2>
    <div id="wb-header" style="display:flex;align-items:center;gap:1.5rem;margin-bottom:1rem;flex-wrap:wrap">
      <div id="wb-cycle-indicator" style="display:flex;align-items:center;gap:.5rem">
        <span id="wb-phase-dot" style="width:12px;height:12px;border-radius:50%;display:inline-block;background:#8b949e"></span>
        <span id="wb-cycle-label" style="font-weight:700;color:var(--accent)">Cycle —</span>
        <span id="wb-phase-label" style="color:#8b949e;font-size:.85rem"></span>
      </div>
      <div style="color:#8b949e;font-size:.85rem"><span id="wb-uptime">—</span> uptime</div>
      <div style="color:#8b949e;font-size:.85rem">Next cycle: <span id="wb-eta" style="color:var(--fg);font-weight:600">—</span></div>
      <button class="btn" onclick="fetchWorkboard()">Refresh</button>
    </div>

    <h3 style="color:var(--accent);margin-bottom:.5rem;font-size:.95rem">Goals</h3>
    <div id="wb-kanban" style="display:grid;grid-template-columns:repeat(4,1fr);gap:.75rem;margin-bottom:1.25rem">
      <div class="card" style="min-height:80px"><h2 style="font-size:.85rem">Queued</h2><div id="wb-col-queued"></div></div>
      <div class="card" style="min-height:80px"><h2 style="font-size:.85rem">In Progress</h2><div id="wb-col-inprogress"></div></div>
      <div class="card" style="min-height:80px"><h2 style="font-size:.85rem">Blocked</h2><div id="wb-col-blocked"></div></div>
      <div class="card" style="min-height:80px"><h2 style="font-size:.85rem">Done</h2><div id="wb-col-done"></div></div>
    </div>

    <div class="grid" style="margin-bottom:1.25rem">
      <div class="card">
        <h2>Active Engineers</h2>
        <div id="wb-engineers"><span style="color:#8b949e">No spawned engineers</span></div>
      </div>
      <div class="card">
        <h2>Recent Actions</h2>
        <div id="wb-actions" style="max-height:300px;overflow-y:auto"><span style="color:#8b949e">No recent actions</span></div>
      </div>
    </div>

    <div class="card" style="margin-bottom:1.25rem">
      <h2 style="cursor:pointer" onclick="document.getElementById('wb-wm-body').style.display=document.getElementById('wb-wm-body').style.display==='none'?'block':'none'">Working Memory <span style="font-weight:normal;color:#8b949e;font-size:.8rem" id="wb-wm-count">0 slots</span> <span style="font-size:.75rem;color:#8b949e">▾</span></h2>
      <div id="wb-wm-body">
        <div id="wb-wm-list" style="font-size:.85rem;color:#8b949e">No active working memory</div>
      </div>
    </div>

    <div class="card" style="margin-bottom:1.25rem">
      <h2 style="cursor:pointer" onclick="document.getElementById('wb-facts-body').style.display=document.getElementById('wb-facts-body').style.display==='none'?'block':'none'">Task Memory <span style="font-weight:normal;color:#8b949e;font-size:.8rem" id="wb-facts-count">0 facts</span> <span style="font-size:.75rem;color:#8b949e">▾</span></h2>
      <div id="wb-facts-body">
        <div id="wb-facts-list" style="font-size:.85rem;color:#8b949e">No facts loaded</div>
      </div>
    </div>

    <div class="card">
      <h2>Cognitive Statistics</h2>
      <div id="wb-cog-stats" style="font-size:.85rem;color:#8b949e">Loading…</div>
    </div>
  </div>

  <div class="tab-content" id="tab-activity">
    <h1 class="page-h1">Activity</h1>
    <p class="page-lede">Everything Simard has been doing recently in one place — the background service log, step-by-step decision traces, the live thinking stream, and where its brain failed to parse a response.</p>
    <span class="section-anchor" id="section-logs"></span>
    <h2 class="subsection">Logs</h2>
    <div class="card" style="margin-bottom:1rem">
      <h2>Background Service Log <button class="btn" onclick="fetchLogs()">Refresh</button> <button class="btn" onclick="copyLogContent('daemon-log')" style="margin-left:.3rem">📋 Copy</button></h2>
      <p style="color:#8b949e;font-size:.82rem;margin:.15rem 0 .5rem">Live activity from the always-on background process that runs Simard. Use the level menu to show only problems — pick <strong>Errors</strong> to surface failures, or <strong>All levels</strong> to see everything.</p>
      <div style="margin-bottom:.5rem;display:flex;gap:.5rem;align-items:center">
        <input id="log-filter" placeholder="Search the log…" style="flex:1;padding:4px 8px;background:var(--bg);border:1px solid var(--border);color:var(--fg);border-radius:4px;font-size:.85rem">
        <select id="log-level-filter" title="Show only lines at this severity level" style="padding:4px;background:var(--bg);border:1px solid var(--border);color:var(--fg);border-radius:4px;font-size:.85rem">
          <option value="">All levels</option>
          <option value="error">Errors</option>
          <option value="warn">Warnings</option>
          <option value="info">Info</option>
        </select>
        <span id="log-line-count" style="color:#8b949e;font-size:.8rem"></span>
      </div>
      <div id="daemon-log" class="log-box"><span class="loading">Loading…</span></div>
    </div>
    <div class="card" style="margin-bottom:1rem">
      <h2>Cost Ledger <button class="btn" onclick="copyLogContent('cost-log-box')">📋 Copy</button></h2>
      <div id="cost-log-box" class="log-box" style="max-height:200px"><span class="loading">Loading…</span></div>
    </div>
    <div class="card" style="margin-bottom:1rem">
      <h2>Cycle Reports</h2>
      <div id="cycle-reports"><span class="loading">Loading…</span></div>
    </div>
    <h2 style="color:var(--accent);font-size:1rem;margin-bottom:.5rem">Agent Transcripts</h2>
    <div id="ooda-transcripts"><span class="loading">Loading…</span></div>
    <h2 style="color:var(--accent);font-size:1rem;margin:.75rem 0 .5rem">Terminal Session Transcripts</h2>
    <div id="terminal-transcripts"><span class="loading">Loading…</span></div>
    <span class="section-anchor" id="section-traces"></span>
    <h2 class="subsection">Traces</h2>
    <div class="card" style="margin-bottom:1rem">
      <h2>OTEL Traces <button class="btn" onclick="fetchTraces()">Refresh</button></h2>
      <div id="otel-status" style="margin-bottom:.75rem"><span class="loading">Loading…</span></div>
      <div id="trace-list" class="log-box" style="max-height:600px;overflow-y:auto"><span class="loading">Loading…</span></div>
    </div>
    <div class="card">
      <h2>Setup</h2>
      <p style="color:#8b949e;font-size:.85rem">To enable full OTEL tracing, set <code>OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317</code> and run an OTEL collector (e.g. Jaeger, Grafana Tempo).</p>
      <p style="color:#8b949e;font-size:.85rem;margin-top:.5rem">For systemd: <code>systemctl --user edit simard-ooda</code> and add the env var in an [Service] override.</p>
    </div>
    <span class="section-anchor" id="section-thinking"></span>
    <h2 class="subsection">Thinking</h2>
    <div class="card" style="margin-bottom:1rem">
      <h2>Cycle History <button class="btn" onclick="fetchOodaCycles()">Refresh</button></h2>
      <div id="ooda-cycle-trend" style="margin-bottom:.75rem"><span class="loading">Loading…</span></div>
      <div id="ooda-cycle-history"><span class="loading">Loading…</span></div>
    </div>
    <div class="card">
      <h2>Agent Internal Reasoning <button class="btn" onclick="fetchThinking()">Refresh</button></h2>
      <div id="thinking-timeline"><span class="loading">Loading…</span></div>
    </div>
    <span class="section-anchor" id="section-brain-failures"></span>
    <h2 class="subsection">Failures</h2>
    <div class="card" style="margin-bottom:1rem">
      <h2>Summary <button class="btn" onclick="fetchBrainFailures()">Refresh</button></h2>
      <div id="brain-failures-summary"><span class="loading">Loading…</span></div>
    </div>
    <div class="card">
      <h2>Recent Brain Failures</h2>
      <div id="brain-failures-list"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-resources">
    <h1 class="page-h1">Resources</h1>
    <p class="page-lede">What Simard has learned and remembered alongside what it costs to run — searchable memory of facts, events, and plans, plus token and dollar spending by model and provider.</p>
    <span class="section-anchor" id="section-memory"></span>
    <h2 class="subsection">Memory</h2>
    <div class="card" style="margin-bottom:1rem;border:1px solid #238636;background:linear-gradient(135deg,#0d1117,#0f1a12)">
      <div style="display:flex;align-items:center;gap:1.5rem;flex-wrap:wrap">
        <div style="text-align:center;min-width:120px">
          <div id="mem-recent-count" style="font-size:2.5rem;font-weight:700;color:#3fb950;line-height:1">—</div>
          <div style="font-size:.85rem;color:#8b949e;margin-top:.25rem">items remembered<br>in the last hour</div>
        </div>
        <div style="flex:1;min-width:200px">
          <div style="display:flex;align-items:center;gap:.75rem;margin-bottom:.5rem">
            <h2 style="margin:0;color:#3fb950;font-size:1rem">What Simard Remembers</h2>
            <span id="mem-recent-total" style="color:#8b949e;font-size:.8rem"></span>
            <button class="btn" onclick="fetchRecentMemories()" style="font-size:.75rem;margin-left:auto">Refresh</button>
          </div>
          <div id="mem-recent-list" style="max-height:340px;overflow-y:auto"><span class="loading">Loading recent memories…</span></div>
        </div>
      </div>
    </div>

    <div class="card" id="mem-growth-card" data-testid="memory-growth-card" style="margin-bottom:1rem;border:1px solid #1f6feb;background:linear-gradient(135deg,#0d1117,#0d1520)">
      <div style="display:flex;align-items:center;gap:1rem;margin-bottom:.75rem">
        <h2 style="margin:0;color:#58a6ff;font-size:1rem">Memory Growth</h2>
        <span id="mem-growth-trend" style="font-size:.85rem;font-weight:600;color:#8b949e" data-testid="memory-growth-trend"></span>
        <button class="btn" onclick="fetchMemoryHistory()" style="font-size:.75rem;margin-left:auto">Refresh</button>
      </div>
      <div id="mem-growth-deltas" data-testid="memory-growth-deltas" style="display:flex;gap:.75rem;flex-wrap:wrap;margin-bottom:.75rem">
        <span class="loading" style="font-size:.85rem">Loading growth data…</span>
      </div>
      <div style="display:flex;gap:1rem;align-items:flex-end;flex-wrap:wrap">
        <div style="flex:1;min-width:200px">
          <div style="font-size:.75rem;color:#8b949e;margin-bottom:.25rem">Long-term memory trend (last observed samples)</div>
          <svg id="mem-growth-sparkline" data-testid="memory-growth-sparkline" width="100%" height="48" viewBox="0 0 400 48" preserveAspectRatio="none" style="background:#161b22;border-radius:4px;border:1px solid #21262d"></svg>
        </div>
        <div id="mem-growth-rate" data-testid="memory-growth-rate" style="text-align:center;min-width:100px">
          <div style="font-size:1.5rem;font-weight:700;color:#58a6ff;line-height:1">—</div>
          <div style="font-size:.75rem;color:#8b949e;margin-top:.15rem">memories/hour</div>
        </div>
      </div>
    </div>

    <details id="mem-advanced-toggle">
      <summary style="cursor:pointer;color:var(--accent);font-size:.85rem;margin-bottom:1rem;user-select:none">▸ Show advanced memory view (graph, search, raw data)</summary>

    <div style="display:flex;align-items:center;gap:1rem;margin-bottom:1rem">
      <h2 style="margin:0">Memory Graph</h2>
      <span id="mem-graph-stats" style="color:#8b949e;font-size:.8rem;margin-left:auto"></span>
      <button class="btn" onclick="fetchMemoryGraph()" style="font-size:.75rem">Refresh Graph</button>
    </div>

    <div id="mem-graph-panel">
      <div class="card" style="margin-bottom:.5rem;padding:.5rem .75rem">
        <div style="display:flex;gap:1rem;flex-wrap:wrap;align-items:center;font-size:.8rem">
          <label style="color:#f0883e"><input type="checkbox" class="mem-filter" data-type="WorkingMemory" checked> Currently thinking about</label>
          <label style="color:#58a6ff"><input type="checkbox" class="mem-filter" data-type="SemanticFact" checked> Facts learned</label>
          <label style="color:#3fb950"><input type="checkbox" class="mem-filter" data-type="EpisodicMemory" checked> Events remembered</label>
          <label style="color:#a371f7"><input type="checkbox" class="mem-filter" data-type="ProceduralMemory" checked> Known procedures</label>
          <label style="color:#d29922"><input type="checkbox" class="mem-filter" data-type="ProspectiveMemory" checked> Planned actions</label>
          <label style="color:#8b949e"><input type="checkbox" class="mem-filter" data-type="SensoryBuffer" checked> Recent observations</label>
        </div>
      </div>
      <div style="display:flex;gap:1rem">
        <div class="card" style="flex:1;padding:0;position:relative;min-height:60vh">
          <canvas id="mem-graph-canvas" style="width:100%;height:60vh;display:block;cursor:grab"></canvas>
          <div id="mem-graph-tooltip" style="display:none;position:absolute;background:#161b22;border:1px solid #30363d;border-radius:6px;padding:.5rem .75rem;font-size:.8rem;max-width:320px;pointer-events:none;z-index:10;word-break:break-word"></div>
        </div>
        <div id="mem-graph-detail" class="card" style="width:280px;display:none">
          <h2 id="mg-detail-title">Node Details</h2>
          <div id="mg-detail-body"></div>
        </div>
      </div>
    </div>

    <div style="display:flex;gap:1rem;margin-top:1rem">
      <div class="card" style="flex:1">
        <h2>Memory Search</h2>
        <div style="display:flex;gap:.5rem;align-items:center;margin-bottom:.75rem">
          <input id="mem-search-input" placeholder="Search memories…" style="flex:1;padding:6px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px">
          <button class="btn" onclick="searchMemory()">Search</button>
        </div>
        <div id="mem-search-results"></div>
      </div>
      <div class="card" style="flex:1"><h2>Memory Overview</h2><div id="mem-overview"><span class="loading">Loading…</span></div></div>
      <div class="card" style="flex:1"><h2>Memory Files</h2><div id="mem-files"><span class="loading">Loading…</span></div></div>
    </div>
    </details>
    <span class="section-anchor" id="section-costs"></span>
    <h2 class="subsection">Costs</h2>
    <div class="grid">
      <div class="card"><h2>Daily Costs <button class="btn" onclick="fetchCosts()">Refresh</button></h2><div id="costs-daily"><span class="loading">Loading…</span></div></div>
      <div class="card"><h2>Weekly Costs</h2><div id="costs-weekly"><span class="loading">Loading…</span></div></div>
      <div class="card"><h2>Budget Settings</h2>
        <div style="display:flex;gap:1rem;align-items:center;flex-wrap:wrap">
          <label>Daily $<input id="budget-daily" type="number" step="0.01" style="width:8rem;padding:4px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px"></label>
          <label>Weekly $<input id="budget-weekly" type="number" step="0.01" style="width:8rem;padding:4px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px"></label>
          <button class="btn" onclick="saveBudget()">Save</button>
          <span id="budget-status"></span>
        </div>
      </div>
    </div>
  </div>
"#;
