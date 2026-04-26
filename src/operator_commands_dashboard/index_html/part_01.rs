pub(crate) const PART_01: &str = r#"      </div>
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

  <div class="tab-content" id="tab-terminal">
    <div class="card" style="max-width:980px">
      <h2>Agent Terminal</h2>
      <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.6rem;margin-bottom:.75rem;font-size:.8rem;color:#8b949e">
        Stream the live stdout/stderr of a running subordinate agent. The viewer
        reconnects each time you click <strong>Connect</strong>; close the WS
        with <strong>Disconnect</strong>.
      </div>
      <div style="display:flex;gap:.5rem;align-items:center;flex-wrap:wrap;margin-bottom:.75rem">
        <label for="agent-log-name" style="color:#8b949e;font-size:.85rem">Agent name</label>
        <input id="agent-log-name" type="text" placeholder="e.g. planner" maxlength="64"
               style="padding:.35rem .5rem;background:var(--bg);border:1px solid var(--border);border-radius:4px;color:var(--fg);font-family:monospace;min-width:14rem">
        <button class="btn" id="agent-log-connect" onclick="connectAgentLog()">Connect</button>
        <button class="btn" id="agent-log-disconnect" onclick="disconnectAgentLog()">Disconnect</button>
        <span id="agent-log-status" style="color:#8b949e;font-size:.85rem">Not connected</span>
      </div>
      <div id="xterm-host" style="height:60vh;background:#000;border:1px solid var(--border);border-radius:6px;padding:.25rem"></div>
    </div>
    <div class="card" style="max-width:980px" id="subagent-sessions">
      <h2>Subagent Sessions</h2>
      <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.6rem;margin-bottom:.75rem;font-size:.8rem;color:#8b949e">
        Live and recently-ended engineer subprocesses tracked via tmux.
        Click <strong>Attach</strong> to copy the <code>tmux attach</code>
        command for the corresponding <code>simard-engineer-&lt;id&gt;</code>
        session.
      </div>
      <div id="subagent-sessions-list">
        <span style="color:#8b949e;font-size:.85rem">Loading…</span>
      </div>
    </div>

    <section id="azlin-sessions-panel" class="card" style="max-width:980px;margin-top:1rem">
      <div style="display:flex;justify-content:space-between;align-items:center;flex-wrap:wrap;gap:.5rem">
        <h2 style="margin:0">Azlin Tmux Sessions</h2>
        <div style="display:flex;gap:.5rem;align-items:center;font-size:.85rem;color:#8b949e">
          <span>Last refreshed:</span>
          <span id="tmux-last-refreshed" data-testid="tmux-last-refreshed">—</span>
          <button class="btn" data-testid="tmux-refresh" onclick="fetchTmuxSessions()">Refresh</button>
        </div>
      </div>
      <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.6rem;margin-top:.6rem;font-size:.8rem;color:#8b949e">
        Per-host listing of <code>tmux list-sessions</code> across configured azlin hosts.
        Click <strong>Open</strong> to attach a session into the terminal viewer above.
        Auto-refreshes every 10 s while this tab is active.
      </div>
      <div id="tmux-sessions-body" style="margin-top:.6rem">
        <div style="color:#8b949e;font-size:.85rem">Loading…</div>
      </div>
    </section>
  </div>

  <script>
    /* --- Helpers --- */
    function fmtB(b){if(b<1024)return b+' B';if(b<1048576)return(b/1024).toFixed(1)+' KB';return(b/1048576).toFixed(1)+' MB';}
    function esc(s){if(s==null)return'';const d=document.createElement('div');d.textContent=String(s);return d.innerHTML;}
    async function apiFetch(url,opts){
      const r=await fetch(url,opts);
      if(r.status===401){window.location.href='/login';throw new Error('Session expired — redirecting to login');}
      if(!r.ok){const t=await r.text();throw new Error(t||('HTTP '+r.status));}
      const text=await r.text();
      if(!text)return {};
      return JSON.parse(text);
    }
    function timeAgo(ts){
      if(!ts)return'—';
      const d=new Date(ts);if(isNaN(d))return ts;
      const s=Math.floor((Date.now()-d.getTime())/1000);
      if(s<5)return'just now';if(s<60)return s+'s ago';
      const m=Math.floor(s/60);if(m<60)return m+'m ago';
      const h=Math.floor(m/60);if(h<24)return h+'h ago';
      const days=Math.floor(h/24);return days+'d ago';
    }
    function copyLogContent(id){
      const el=document.getElementById(id);if(!el)return;
      navigator.clipboard.writeText(el.textContent||'').then(
        ()=>{const prev=el.style.borderColor;el.style.borderColor='var(--green)';setTimeout(()=>el.style.borderColor=prev,800);},
        ()=>{}
      );
    }

    /* --- WS-2: Subagent tmux session registry (cached client-side) --- */
    let subagentSessionsCache={live:[],recently_ended:[],byId:{}};
    function rebuildSubagentIndex(){
      const idx={};
      for(const s of (subagentSessionsCache.live||[])){idx[s.agent_id]=s;}
      for(const s of (subagentSessionsCache.recently_ended||[])){if(!idx[s.agent_id])idx[s.agent_id]=s;}
      subagentSessionsCache.byId=idx;
    }
    async function fetchSubagentSessions(){
      try{
        const d=await apiFetch('/api/subagent-sessions');
        subagentSessionsCache.live=d.live||[];
        subagentSessionsCache.recently_ended=d.recently_ended||[];
        rebuildSubagentIndex();
        renderSubagentSessions();
      }catch(e){
        const el=document.getElementById('subagent-sessions-list');
        if(el) el.innerHTML='<span class="err">Failed to load subagent sessions: '+esc(e.message||e)+'</span>';
      }
    }
    function attachCommandFor(s){
      if(s.host && s.host!=='local'){
        return 'ssh '+s.host+' -t tmux attach -t '+s.session_name;
      }
      return 'tmux attach -t '+s.session_name;
    }
    function renderSubagentSessions(){
      const el=document.getElementById('subagent-sessions-list');
      if(!el) return;
      const live=subagentSessionsCache.live||[];
      const ended=subagentSessionsCache.recently_ended||[];
      if(!live.length && !ended.length){
        el.innerHTML='<span style="color:#8b949e;font-size:.85rem">No subagent sessions tracked yet.</span>';
        return;
      }
      const row=(s,status)=>{
        const cmd=attachCommandFor(s);
        return '<div style="display:flex;gap:.5rem;align-items:baseline;padding:.35rem 0;border-bottom:1px solid var(--border);font-size:.85rem">'
          +'<code style="min-width:14rem">'+esc(s.agent_id)+'</code>'
          +'<span style="color:#8b949e;min-width:8rem">'+esc(s.goal_id||'')+'</span>'
          +'<span class="'+(status==='live'?'ok':'warn')+'" style="min-width:5rem">'+status+'</span>'
          +'<span style="flex:1;color:#8b949e;font-size:.75rem">pid '+s.pid+' · '+esc(s.host||'local')+'</span>'
          +'<button class="btn attach-btn" data-cmd="'+esc(cmd)+'" onclick="copyAttachCmd(this)">Attach →</button>'
          +'</div>';
      };
      el.innerHTML=live.map(s=>row(s,'live')).join('')+ended.map(s=>row(s,'ended')).join('');
    }
    function copyAttachCmd(btn){
      const cmd=btn.getAttribute('data-cmd')||'';
      navigator.clipboard.writeText(cmd).then(()=>{
        const prev=btn.textContent;btn.textContent='Copied!';
        setTimeout(()=>{btn.textContent=prev;},900);
      },()=>{});
    }
    /* Shared renderer for Recent Actions outcome.detail strings.
       Detects agent='engineer-...' references and, when a matching tmux
       session is in the registry cache, swaps the literal substring for an
       inline Attach button. Returns an HTML string (caller already escaped
       the detail). */
    function renderActionDetail(detail){
      const safe=esc(detail||'');
      const re=/agent='(engineer-[A-Za-z0-9_-]+)'/;
      const m=safe.match(re);
      if(!m) return safe;
      const agentId=m[1];
      const session=subagentSessionsCache.byId[agentId];
      if(!session) return safe;
      const cmd=attachCommandFor(session);
      const btn=' <button class="btn attach-btn" data-cmd="'+esc(cmd)+'" onclick="copyAttachCmd(this)" style="font-size:.7rem;padding:.05rem .35rem;margin-left:.25rem">Attach →</button>';
      return safe.replace(m[0], m[0]+btn);
    }

    /* --- Active tab tracking for auto-refresh --- */
    let activeTab='overview';
    let tabRefreshTimers={};

    function clearTabTimers(){Object.values(tabRefreshTimers).forEach(clearInterval);tabRefreshTimers={};}

    /* --- Tabs --- */
    document.querySelectorAll('.tab').forEach(tab=>{
      tab.addEventListener('click',()=>{
        document.querySelectorAll('.tab').forEach(t=>t.classList.remove('active'));
        document.querySelectorAll('.tab-content').forEach(c=>c.classList.remove('active'));
        tab.classList.add('active');
        document.getElementById('tab-'+tab.dataset.tab).classList.add('active');
        activeTab=tab.dataset.tab;
        clearTabTimers();
        if(tab.dataset.tab==='logs') {fetchLogs();tabRefreshTimers.logs=setInterval(fetchLogs,15000);}
        if(tab.dataset.tab==='processes') {fetchProcessTree();tabRefreshTimers.proc=setInterval(fetchProcessTree,15000);}
        if(tab.dataset.tab==='memory') {fetchMemoryGraph();fetchMemory();}

        if(tab.dataset.tab==='goals') fetchGoals();
        if(tab.dataset.tab==='costs') fetchCosts();
        if(tab.dataset.tab==='traces') fetchTraces();
        if(tab.dataset.tab==='chat') initChat();
        if(tab.dataset.tab==='workboard') {fetchWorkboard();tabRefreshTimers.wb=setInterval(fetchWorkboard,30000);}
        if(tab.dataset.tab==='thinking') {fetchThinking();tabRefreshTimers.thinking=setInterval(fetchThinking,30000);}
        if(tab.dataset.tab==='terminal') {initAgentLogTerminal();fetchSubagentSessions();tabRefreshTimers.subagent=setInterval(fetchSubagentSessions,5000);fetchTmuxSessions();tabRefreshTimers.tmux=setInterval(fetchTmuxSessions,10000);}
      });
    });
    setInterval(()=>{document.getElementById('clock').textContent=new Date().toLocaleString()},1000);

    /* --- Status --- */
    async function fetchStatus(){
      try{
        const d=await apiFetch('/api/status');
        const dc=d.disk_usage_pct>90?'err':d.disk_usage_pct>70?'warn':'ok';
        const oc=d.ooda_daemon==='running'?'ok':(d.ooda_daemon==='stale'?'warn':'err');
        const shortHash=d.git_hash?d.git_hash.substring(0,7):'';
        const versionLink=d.git_hash?`<a href="https://github.com/rysweet/Simard/commit/${d.git_hash}" target="_blank" style="color:#3fb950;text-decoration:none">v${esc(d.version)}</a> (<code>${shortHash}</code>)`:`v${esc(d.version)}`;
        let healthDetail='';
        if(d.daemon_health){
          const dh=d.daemon_health;
          healthDetail=` (cycle #${dh.cycle_number??'?'}`;
          if(dh.timestamp) healthDetail+=`, ${timeAgo(dh.timestamp)}`;
          healthDetail+=')';
        }
        document.getElementById('status').innerHTML=`
          <div class="stat"><span class="label">Version</span><span class="value">${versionLink}</span></div>
          <div class="stat"><span class="label">OODA Daemon</span><span class="value ${oc}">${esc(d.ooda_daemon)}${healthDetail}</span></div>
          <div class="stat"><span class="label">Active Processes</span><span class="value">${d.active_processes??0}</span></div>
          <div class="stat"><span class="label">Disk Usage</span><span class="value ${dc}">${d.disk_usage_pct??'?'}%</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${timeAgo(d.timestamp)}</span></div>`;
        document.getElementById('header-version').textContent='v'+d.version+' ('+shortHash+')';
      }catch(e){document.getElementById('status').innerHTML='<span class="err">Failed to reach /api/status — is the dashboard server running?</span>';}
    }

    async function fetchAgentOverview(){
      try{
        const d=await apiFetch('/api/activity');
        const el=document.getElementById('agent-live-status');
        const daemon=d.daemon||{};
        const isRunning=daemon.status==='healthy';
        const heartbeat=daemon.last_heartbeat?timeAgo(daemon.last_heartbeat):'never';
        const cycle=daemon.current_cycle||'?';

        // Staleness check: if heartbeat is >10 min old, daemon may be hung
        let isStale=false;
        if(isRunning && daemon.last_heartbeat){
          const hbAge=Date.now()-new Date(daemon.last_heartbeat).getTime();
          isStale=hbAge>10*60*1000;
        }

        // Extract actual actions from the most recent structured cycle report
        let latestActions=[];
        const cycles=d.recent_cycles||[];
        for(const c of cycles){
          const rpt=c.report||{};
          if(rpt.outcomes?.length){
            latestActions=rpt.outcomes;
            break;
          }
        }

        // Find what the agent is currently working on from latest priorities
        let currentFocus='';
        for(const c of cycles){
          const rpt=c.report||{};
          if(rpt.priorities?.length){
            const top=rpt.priorities[0];
            currentFocus=`<strong>${esc(top.goal_id)}</strong> — ${esc(top.reason)} <span style="color:${top.urgency>0.7?'var(--red)':top.urgency>0.4?'var(--yellow)':'var(--green)'}">urgency ${top.urgency.toFixed(2)}</span>`;
            break;
          }
        }

        el.innerHTML=`
          <div style="display:flex;gap:2rem;flex-wrap:wrap;align-items:center;margin-bottom:.75rem">
            <div><span style="font-size:1.5rem;${isRunning&&!isStale?'':'filter:grayscale(1)'}">${isRunning?(isStale?'🟡':'🟢'):'🔴'}</span> <strong style="font-size:1.1rem">${isRunning?(isStale?'Agent Stale':'OODA Loop Active'):'Agent Stopped'}</strong></div>
            <div style="color:#8b949e">Cycle <strong style="color:var(--fg)">#${cycle}</strong> · Last heartbeat <strong style="color:var(--fg)">${heartbeat}</strong>${isStale?' <span style="color:var(--yellow)">(>10 min ago)</span>':''}</div>
          </div>
          ${currentFocus?`<div style="margin-bottom:.75rem"><span style="color:#8b949e">🎯 Top Priority:</span> ${currentFocus}</div>`:''}
          ${latestActions.length?`
            <div style="font-size:.85rem">
              <div style="color:#8b949e;margin-bottom:.3rem;font-weight:600">Last Cycle Actions:</div>
              ${latestActions.map(o=>`
                <div style="padding:.2rem 0;display:flex;gap:.5rem;align-items:baseline">
                  <span>${o.success?'✅':'❌'}</span>
                  <code style="color:var(--accent)">${esc(o.action_kind||'')}</code>
                  <span>${esc(o.action_description||'')}</span>
                  ${o.detail?'<span style="color:#8b949e;font-size:.8rem;max-width:400px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:inline-block">'+esc(o.detail.substring(0,120))+'</span>':''}
                </div>`).join('')}
            </div>`:'<div style="color:#8b949e">No recent actions recorded.</div>'}`;

        // Open PRs
        const prs=d.open_prs||[];
        const prEl=document.getElementById('open-prs-list');
        if(prs.length){
          prEl.innerHTML=prs.slice(0,8).map(pr=>`
            <div style="padding:.3rem 0;border-bottom:1px solid var(--border);font-size:.85rem;display:flex;gap:.5rem;align-items:baseline">
              <a href="${esc(pr.url)}" target="_blank" style="color:var(--accent);text-decoration:none;min-width:3rem">#${pr.number}</a>
              <span style="flex:1">${esc(pr.title)}</span>
              <span style="color:#8b949e;font-size:.75rem">${timeAgo(pr.createdAt)}</span>
            </div>`).join('')+
            (prs.length>8?`<div style="color:#8b949e;font-size:.8rem;margin-top:.3rem">+ ${prs.length-8} more</div>`:'');
        }else{
          prEl.innerHTML='<span style="color:#8b949e">No open PRs</span>';
        }

        // Recent actions from cycle outcomes
        const actEl=document.getElementById('recent-actions-list');
        let allActions=[];
        for(const c of cycles.slice(0,5)){
          const rpt=c.report||{};
          const num=rpt.cycle_number||c.cycle_number||'?';
          for(const o of (rpt.outcomes||[])){
            allActions.push({cycle:num,...o});
          }
        }
        if(allActions.length){
          actEl.innerHTML=allActions.slice(0,15).map(a=>`
            <div style="padding:.25rem 0;border-bottom:1px solid var(--border);font-size:.85rem;display:flex;gap:.5rem;align-items:baseline">
              <span style="color:var(--accent);min-width:2rem;font-weight:600">#${a.cycle}</span>
              <span>${a.success?'✅':'❌'}</span>
              <code>${esc(a.action_kind||'')}</code>
              <span style="flex:1">${renderActionDetail((function(){var arr=Array.from(a.detail||'');var d=arr.length>200?arr.slice(0,200).join('')+'…':arr.join('');return d||a.action_description||'';})())}</span>
            </div>`).join('');
        }else{
          actEl.innerHTML='<span style="color:#8b949e">No structured action history yet. The OODA daemon records actions each cycle.</span>';
        }
      }catch(e){
        console.warn('fetchAgentOverview failed:', e);
        const el=document.getElementById('agent-live-status');
        if(el) el.innerHTML='<span class="err">Failed to load agent status</span>';
      }
    }

    /* --- Issues --- */
    async function fetchIssues(){
      try{
        const data=await apiFetch('/api/issues');
        if(Array.isArray(data)){
          if(!data.length){document.getElementById('issues-list').innerHTML='<li style="color:#8b949e">No open issues 🎉</li>';return;}
          document.getElementById('issues-list').innerHTML=data.map(i=>{
            const labels=(i.labels||[]).map(l=>`<span class="badge" style="margin-left:.3rem">${esc(l.name||l)}</span>`).join('');
            return`<li><span class="issue-num">#${i.number}</span>${esc(i.title)}${labels}</li>`;
          }).join('');
        }else if(data.error){
          document.getElementById('issues-list').innerHTML=`<li class="warn">${esc(data.error)} — is <code>gh</code> authenticated?</li>`;
        }
      }catch(e){document.getElementById('issues-list').innerHTML='<li class="err">Failed to load issues — check network</li>';}
    }

    /* --- Logs --- */
    let allLogLines=[];
    async function fetchLogs(){
      try{
        const d=await apiFetch('/api/logs');
        allLogLines=d.daemon_log_lines||[];
        applyLogFilter();
        // Issue #928: guard each element access so a missing target on the
        // current tab does not abort the whole fetchLogs and leave every
        // panel stuck on "Loading…".
        const tEl=document.getElementById('ooda-transcripts');
        if(tEl){"#;
