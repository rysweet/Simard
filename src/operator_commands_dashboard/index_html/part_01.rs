pub(crate) const PART_01: &str = r#"
  <div class="tab-content" id="tab-workers">
    <h1 class="page-h1">Workers</h1>
    <p class="page-lede">The background processes and engineer subprocesses Simard is running on this host, with a tree view for spotting stuck workers and a live attach to any agent's terminal.</p>
    <span class="section-anchor" id="section-processes"></span>
    <h2 class="subsection">Processes</h2>
    <div class="card">
      <h2>Active Simard Processes <button class="btn" onclick="fetchProcesses()">Refresh</button> <span id="proc-auto-refresh" style="font-size:.75rem;color:#8b949e;font-weight:normal;margin-left:.5rem">⟳ auto-refreshing</span></h2>
      <div id="proc-count" style="margin-bottom:.5rem;color:#8b949e;font-size:.85rem"></div>
      <div id="proc-table"><span class="loading">Loading…</span></div>
    </div>
    <h2 class="subsection">Engineers</h2>
    <div class="card" style="margin-top:1rem">
      <h2>Process Tree <button class="btn" onclick="fetchProcessTree()">Refresh</button></h2>
      <div id="proc-tree-summary" style="margin-bottom:.5rem;color:#8b949e;font-size:.85rem"></div>
      <div id="proc-tree-container"><span class="loading">Loading…</span></div>
    </div>
    <span class="section-anchor" id="section-terminal"></span>
    <h2 class="subsection">Terminal</h2>
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

  <div class="tab-content" id="tab-pull-requests">
    <h1 class="page-h1">Pull Requests</h1>
    <p class="page-lede">Every pull request Simard is managing — the merge judge's approve, reject, and defer decisions plus the CI, review, and blocker status that shows what is ready to merge.</p>
    <span class="section-anchor" id="section-merge-decisions"></span>
    <h2 class="subsection">Merge Decisions</h2>
    <div class="card" style="max-width:980px;margin-bottom:1rem">
      <h2>Decision Trend</h2>
      <div id="merge-judge-trend"><span style="color:#8b949e;font-size:.85rem">Trend chart will appear once decision data is available.</span></div>
    </div>
    <div class="card" style="max-width:980px">
      <h2>Decision History <button class="btn" onclick="fetchMergeJudge()" style="font-size:.75rem">Refresh</button></h2>
      <div id="merge-judge-panel"><span class="loading">Loading…</span></div>
    </div>
    <span class="section-anchor" id="section-pr-readiness"></span>
    <h2 class="subsection">Readiness</h2>
    <div class="card" style="max-width:1100px">
      <h2>Open Pull Requests <button class="btn" onclick="fetchPrReadiness()" style="font-size:.75rem">Refresh</button></h2>
      <div id="pr-readiness-summary" style="margin-bottom:.75rem"><span class="loading">Loading…</span></div>
      <div id="pr-readiness-panel"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-chat">
    <h1 class="page-h1">Chat</h1>
    <p class="page-lede">Talk to the running Simard agent in real time — anything you say here can become a new goal, and slash-commands like /close, /goals, and /status are available.</p>
    <div class="chat-layout">
      <div id="chat-sidebar">
        <button id="chat-new" onclick="newChat()">+ New chat</button>
        <div id="chat-sessions"></div>
      </div>
      <div class="card" id="chat-panel">
        <h2>Chat</h2>
        <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.5rem .75rem;margin-bottom:.5rem;font-size:.8rem;color:#8b949e">
          <strong style="color:var(--accent)">💡</strong>
          Talk directly with Simard — ask about your goals, check system status, or start a conversation. Commands: <code>/close</code> end session, <code>/goals</code> review goals, <code>/status</code> system status.
        </div>
        <div class="ws-status disconnected" id="ws-status">● Disconnected <button class="btn" onclick="initChat()" style="font-size:.75rem;padding:.1rem .4rem;margin-left:.5rem">Reconnect</button></div>
        <div id="chat-messages"></div>
        <div id="chat-input-row">
          <textarea id="chat-input" rows="1" placeholder="Type a message… (/close to end session)"></textarea>
          <button id="chat-send" onclick="sendChat()">Send</button>
        </div>
      </div>
    </div>
  </div>

  <div class="tab-content" id="tab-overseer">
    <h1 class="page-h1">Overseer</h1>
    <p class="page-lede">What Simard's steward has been doing on its own — what it noticed across the system, what it changed, and, when it chose to wait, why it held back. Refreshes automatically.</p>
    <div class="card" style="max-width:1100px;margin-bottom:1rem">
      <h2>Steward Status <button class="btn" onclick="fetchOverseer()" style="font-size:.75rem">Refresh</button></h2>
      <div id="overseer-status"><span class="loading">Loading…</span></div>
    </div>
    <div class="card" style="max-width:1100px;margin-bottom:1rem">
      <h2>Operator Threads</h2>
      <div id="overseer-threads"><span class="loading">Loading…</span></div>
    </div>
    <div class="card" style="max-width:1100px">
      <h2>Recent Activity</h2>
      <div id="overseer-recent"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-journal">
    <h1 class="page-h1">Journal</h1>
    <p class="page-lede">A plain-language daily diary of what Simard and its steward the Overseer did each day, with a simple table of the code changes proposed. Browse by date and search the full history.</p>
    <div class="card" style="max-width:1100px">
      <div style="display:flex;gap:.5rem;align-items:center;margin-bottom:.75rem;flex-wrap:wrap">
        <input id="journal-search-input" placeholder="Search the journal…" style="flex:1;min-width:220px;padding:6px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px">
        <button class="btn" onclick="searchJournal()">Search</button>
        <button class="btn" onclick="loadJournal()">Refresh</button>
      </div>
      <div style="display:flex;gap:1rem;align-items:flex-start">
        <div id="journal-dates" data-testid="journal-dates" style="width:230px;max-height:70vh;overflow-y:auto;border-right:1px solid #21262d;padding-right:.5rem"><span class="loading">Loading…</span></div>
        <div id="journal-entry" data-testid="journal-entry" style="flex:1;min-height:220px"><span class="loading">Loading…</span></div>
      </div>
    </div>
  </div>
  <script>
    /* --- Journal tab (issue #2606) --- */
    (function(){
      let journalLoaded=false, journalSelected=null;
      function renderJournalDateList(items){
        const box=document.getElementById('journal-dates');
        if(!items.length){box.innerHTML='<span style="color:#8b949e">No entries yet.</span>';return;}
        box.innerHTML=items.map(e=>{
          const sel=e.date===journalSelected?'background:#1f6feb33;':'';
          const badge=e.quiet_day
            ?'<span style="color:#8b949e;font-size:.7rem"> quiet day</span>'
            :(e.pr_count?'<span style="color:#3fb950;font-size:.7rem"> '+e.pr_count+' change'+(e.pr_count===1?'':'s')+'</span>':'');
          return '<div class="journal-date-item" data-date="'+escAttr(e.date)+'" style="cursor:pointer;padding:.35rem .5rem;border-radius:4px;'+sel+'">'+esc(e.date)+badge+'</div>';
        }).join('');
        box.querySelectorAll('.journal-date-item').forEach(el=>{
          el.addEventListener('click',()=>selectJournalDate(el.dataset.date));
        });
      }
      async function selectJournalDate(date){
        journalSelected=date;
        const target=document.getElementById('journal-entry');
        target.innerHTML='<span class="loading">Loading…</span>';
        document.querySelectorAll('#journal-dates .journal-date-item').forEach(el=>{
          el.style.background=el.dataset.date===date?'#1f6feb33':'';
        });
        try{
          const r=await fetch('/api/journal/render/'+encodeURIComponent(date));
          if(r.status===401){window.location.href='/login';return;}
          /* Server-rendered fragment: the narrative and PR table are already
             HTML-escaped server-side, so assigning to innerHTML is XSS-safe. */
          target.innerHTML=await r.text();
          if(typeof annotateJargon==='function') annotateJargon(target);
        }catch(e){target.innerHTML='<span class="err">Could not load this day\u2019s journal.</span>';}
      }
      async function loadJournal(){
        const box=document.getElementById('journal-dates');
        box.innerHTML='<span class="loading">Loading…</span>';
        try{
          const d=await apiFetch('/api/journal/dates');
          const dates=d.dates||[];
          renderJournalDateList(dates);
          if(dates.length){
            const keep=journalSelected&&dates.some(x=>x.date===journalSelected);
            selectJournalDate(keep?journalSelected:dates[0].date);
          }else{
            document.getElementById('journal-entry').innerHTML='<div class="journal-entry"><p class="journal-empty">Simard has not written any journal entries yet. They appear here once the daily journal has run.</p></div>';
          }
          journalLoaded=true;
        }catch(e){box.innerHTML='<span class="err">Could not load the journal — check /api/journal/dates</span>';}
      }
      async function searchJournal(){
        const q=(document.getElementById('journal-search-input').value||'').trim();
        const box=document.getElementById('journal-dates');
        box.innerHTML='<span class="loading">Searching…</span>';
        try{
          const d=await apiFetch('/api/journal/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({query:q})});
          const results=d.results||[];
          if(!results.length){
            box.innerHTML='<span style="color:#8b949e">No entries match \u201c'+esc(q)+'\u201d.</span>';
            document.getElementById('journal-entry').innerHTML='';
            return;
          }
          renderJournalDateList(results);
          selectJournalDate(results[0].date);
        }catch(e){box.innerHTML='<span class="err">Search failed — check /api/journal/search</span>';}
      }
      window.loadJournal=loadJournal;
      window.searchJournal=searchJournal;
      const jt=document.querySelector('.tab[data-tab="journal"]');
      if(jt) jt.addEventListener('click',()=>{ if(!journalLoaded) loadJournal(); });
      const ji=document.getElementById('journal-search-input');
      if(ji) ji.addEventListener('keypress',e=>{if(e.key==='Enter')searchJournal();});
    })();
  </script>

  <div class="tab-content" id="tab-creative-ideas">
    <h1 class="page-h1">Creative Ideas</h1>
    <p class="page-lede">A pool of candidate improvements Simard dreams up for herself, each reviewed for feasibility, worth, and how to measure success. Browse and search by their review status, from brand-new to accepted or parked.</p>
    <div class="card" style="max-width:1100px">
      <div style="display:flex;gap:.5rem;align-items:center;margin-bottom:.75rem;flex-wrap:wrap">
        <input id="ci-search-input" placeholder="Search ideas…" style="flex:1;min-width:200px;padding:6px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px">
        <select id="ci-status-filter" style="padding:6px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px">
          <option value="">All statuses</option>
          <option value="New">New</option>
          <option value="NeedsRevision">Needs revision</option>
          <option value="NeedsHumanReview">Needs human review</option>
          <option value="AcceptedForImplementation">Accepted</option>
          <option value="ImplementationStarted">Implementation started</option>
          <option value="ImplementationCompleted">Completed</option>
          <option value="Deferred">Deferred</option>
          <option value="Rejected">Rejected</option>
        </select>
        <button class="btn" onclick="searchCreativeIdeas()">Search</button>
        <button class="btn" onclick="loadCreativeIdeas()">Refresh</button>
      </div>
      <div id="ci-counts" data-testid="ci-counts" style="display:flex;gap:.4rem;flex-wrap:wrap;margin-bottom:.75rem"></div>
      <div id="ci-list" data-testid="ci-list" style="min-height:220px"><span class="loading">Loading…</span></div>
    </div>
  </div>
  <script>
    /* --- Creative Ideas tab --- */
    (function(){
      let ciLoaded=false;
      function statusColor(s){
        return ({
          New:'#58a6ff', NeedsRevision:'#d29922', NeedsHumanReview:'#f0883e',
          AcceptedForImplementation:'#3fb950', ImplementationStarted:'#2ea043',
          ImplementationCompleted:'#238636', Deferred:'#8b949e', Rejected:'#f85149'
        })[s]||'#8b949e';
      }
      function renderCounts(counts){
        const box=document.getElementById('ci-counts');
        if(!counts){box.innerHTML='';return;}
        box.innerHTML=Object.keys(counts).map(k=>
          '<span style="font-size:.72rem;padding:.15rem .45rem;border-radius:10px;border:1px solid '+statusColor(k)+';color:'+statusColor(k)+'">'+esc(k)+': '+esc(String(counts[k]))+'</span>'
        ).join('');
      }
      function renderIdeas(items){
        const box=document.getElementById('ci-list');
        if(!items||!items.length){box.innerHTML='<span style="color:#8b949e">No ideas match. Simard fills this pool as the Creative Ideas thread runs.</span>';return;}
        box.innerHTML=items.map(it=>{
          const c=statusColor(it.status);
          const metric=it.has_metric?'<span style="color:#3fb950;font-size:.7rem"> · metric: '+esc(it.metric||'yes')+'</span>':'';
          return '<div style="padding:.5rem .6rem;border:1px solid #21262d;border-radius:6px;margin-bottom:.5rem">'
            +'<div style="display:flex;justify-content:space-between;gap:.5rem;align-items:center">'
            +'<strong>'+esc(it.idea)+'</strong>'
            +'<span style="font-size:.7rem;padding:.1rem .4rem;border-radius:10px;background:'+c+'22;color:'+c+';border:1px solid '+c+'">'+esc(it.status)+'</span>'
            +'</div>'
            +'<div style="color:#8b949e;font-size:.78rem;margin-top:.25rem">'+esc(it.rationale||'')+'</div>'
            +'<div style="color:#6e7681;font-size:.7rem;margin-top:.25rem">'+esc(String(it.reviews))+' review(s) · '+esc(String(it.links))+' link(s)'+metric+'</div>'
            +'</div>';
        }).join('');
      }
      async function loadCreativeIdeas(){
        const box=document.getElementById('ci-list');
        box.innerHTML='<span class="loading">Loading…</span>';
        try{
          const d=await apiFetch('/api/creative-ideas');
          renderCounts(d.counts);
          renderIdeas(d.ideas||[]);
          ciLoaded=true;
        }catch(e){box.innerHTML='<span class="err">Could not load ideas — check /api/creative-ideas</span>';}
      }
      async function searchCreativeIdeas(){
        const q=(document.getElementById('ci-search-input').value||'').trim();
        const status=document.getElementById('ci-status-filter').value||'';
        const box=document.getElementById('ci-list');
        box.innerHTML='<span class="loading">Searching…</span>';
        try{
          const d=await apiFetch('/api/creative-ideas/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({query:q,status:status})});
          if(d.error){box.innerHTML='<span class="err">'+esc(d.error)+'</span>';return;}
          renderIdeas(d.results||[]);
        }catch(e){box.innerHTML='<span class="err">Search failed — check /api/creative-ideas/search</span>';}
      }
      window.loadCreativeIdeas=loadCreativeIdeas;
      window.searchCreativeIdeas=searchCreativeIdeas;
      const ct=document.querySelector('.tab[data-tab="creative-ideas"]');
      if(ct) ct.addEventListener('click',()=>{ if(!ciLoaded) loadCreativeIdeas(); });
      const cs=document.getElementById('ci-search-input');
      if(cs) cs.addEventListener('keypress',e=>{if(e.key==='Enter')searchCreativeIdeas();});
      const cf=document.getElementById('ci-status-filter');
      if(cf) cf.addEventListener('change',()=>searchCreativeIdeas());
    })();
  </script>

  {{TAB_META_JS}}
  <script>
    /* --- Helpers --- */
    function fmtB(b){if(b<1024)return b+' B';if(b<1048576)return(b/1024).toFixed(1)+' KB';return(b/1048576).toFixed(1)+' MB';}
    function esc(s){if(s==null)return'';const d=document.createElement('div');d.textContent=String(s);return d.innerHTML;}
    /* Attribute-context escape: esc() handles element-content (& < >) but does
       NOT neutralize the " that would break out of a double-quoted attribute
       value. Use this for any interpolated title="…" / other quoted attribute
       sink so a future dynamic/peer-supplied value cannot inject attributes. */
    function escAttr(s){return esc(s).replace(/"/g,'&quot;');}
    /* apiFetch de-dupes concurrent duplicate GETs (a manual Refresh and a
       background tick for the same endpoint share one in-flight request) and
       records a client-clock freshness stamp per endpoint. Both stores live in
       browser memory for the page's lifetime — nothing is persisted, and the
       de-dupe key never contains a body, token, or PII (#2649). */
    const inFlightFetches=new Map();
    const lastOk={};
    function fetchDedupeKey(url,method){
      let pathname=url,search='';
      try{const u=new URL(url,window.location.origin);pathname=u.pathname;search=u.search;}catch(_){}
      return method+' '+pathname+search;
    }
    async function apiFetchRaw(url,opts){
      const r=await fetch(url,opts);
      if(r.status===401){window.location.href='/login';throw new Error('Session expired — redirecting to login');}
      if(!r.ok){const t=await r.text();throw new Error(t||('HTTP '+r.status));}
      try{const u=new URL(url,window.location.origin);lastOk[u.pathname]=Date.now();}catch(_){lastOk[url]=Date.now();}
      const text=await r.text();
      if(!text)return {};
      return JSON.parse(text);
    }
    async function apiFetch(url,opts){
      const method=((opts&&opts.method)||'GET').toUpperCase();
      /* GET only: mutations and search POSTs are never collapsed. */
      if(method!=='GET')return apiFetchRaw(url,opts);
      const key=fetchDedupeKey(url,method);
      const inflight=inFlightFetches.get(key);
      if(inflight)return inflight;
      const p=apiFetchRaw(url,opts);
      inFlightFetches.set(key,p);
      /* Clear on settle (success AND failure) so a rejected fetch cannot poison
         the key and lock out future requests to that endpoint. */
      p.then(function(){inFlightFetches.delete(key);},function(){inFlightFetches.delete(key);});
      return p;
    }
    window.apiFetch=apiFetch;
    function timeAgo(ts){
      if(!ts)return'—';
      const d=parseTs(ts);if(!d||isNaN(d))return String(ts);
      const s=Math.floor((Date.now()-d.getTime())/1000);
      if(s<5)return'just now';if(s<60)return s+'s ago';
      const m=Math.floor(s/60);if(m<60)return m+'m ago';
      const h=Math.floor(m/60);if(h<24)return h+'h ago';
      const days=Math.floor(h/24);return days+'d ago';
    }
    function parseTs(ts){
      if(ts==null||ts==='')return null;
      if(typeof ts==='number'&&isFinite(ts)){return new Date(ts<1e12?ts*1000:ts);}
      const d=new Date(ts);return isNaN(d)?null:d;
    }
    function formatTime(ts){
      const d=parseTs(ts);if(!d)return ts==null?'—':String(ts);
      try{return d.toLocaleString();}catch(_){return d.toISOString();}
    }
    function humanizeActionKind(kind){
      if(!kind)return'';
      const map={'spawn_engineer':'Launched sub-agent','progress_assessment':'Checked progress','merge_readiness_check':'Evaluated PR readiness','disk_health_check':'Checked disk health','goal_create':'Created a goal','goal_close':'Closed a goal'};
      if(map[kind])return map[kind];
      return kind.replace(/[_-]+/g,' ').replace(/^./,c=>c.toUpperCase());
    }
    /* --- Plain-English humanizers (#2358) ---
       Jargon banned from the static ledes (tab_meta::BANNED_JARGON) is injected
       here so the same ban extends to dynamically rendered cycle/summary text. */
    const BANNED_JARGON={{BANNED_JARGON_JS}};
    function humanizeGoalId(id){
      if(!id)return'';
      const map={'__memory__':'Memory maintenance','__improvement__':'Self-improvement','__system__':'System upkeep','__meta__':'Housekeeping'};
      if(map[id])return map[id];
      const m=String(id).match(/^__(.+?)__$/);
      if(m)return m[1].replace(/[_-]+/g,' ').replace(/^./,c=>c.toUpperCase());
      return id;
    }
    function humanizePeriod(p){
      if(!p)return'';
      const mn=['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
      let m=String(p).match(/^daily:(\d{4})-(\d{2})-(\d{2})$/);
      if(m){
        const y=+m[1],mo=+m[2],day=+m[3],label=mn[mo-1]+' '+day,t=new Date();
        if(t.getFullYear()===y&&t.getMonth()+1===mo&&t.getDate()===day)return'Today ('+label+')';
        const ys=new Date(t.getTime()-86400000);
        if(ys.getFullYear()===y&&ys.getMonth()+1===mo&&ys.getDate()===day)return'Yesterday ('+label+')';
        return label+', '+y;
      }
      if(p==='weekly:last-7-days'||p==='weekly:last_7_days')return'Last 7 days';
      m=String(p).match(/^weekly:(.+)$/);
      if(m)return'Week of '+m[1].replace(/[-_]/g,' ');
      m=String(p).match(/^daily:(.+)$/);
      if(m)return m[1];
      return String(p);
    }
    function humanizeCycleSummary(text){
      if(!text)return'';
      let s=String(text);
      const m=s.match(/^OODA cycle #(\d+):\s*(\d+) priorities,\s*(\d+) actions \((\d+)\/(\d+) succeeded\),\s*goals=(\d+),\s*issues=(\d+),\s*tree=(\w+)/);
      if(m){
        const cyc=m[1],prio=m[2],ok=m[4],tot=m[5],goals=m[6],issues=m[7],tree=m[8];
        const treeTxt=tree==='clean'?'working tree clean':(tree==='dirty'?'uncommitted changes':'tree '+tree);
        const pW=prio==='1'?'priority':'priorities',aW=tot==='1'?'action':'actions',gW=goals==='1'?'goal':'goals',iW=issues==='1'?'issue':'issues';
        return 'Cycle #'+cyc+' — '+prio+' '+pW+' considered, '+ok+' of '+tot+' '+aW+' succeeded · '+goals+' '+gW+' tracked · '+issues+' open '+iW+' · '+treeTxt;
      }
      // Generic fallback for non-canonical / legacy summaries: drop key=value
      // shorthand and any leftover banned-jargon token.
      s=s.replace(/\btree=clean\b/g,'working tree clean')
         .replace(/\btree=dirty\b/g,'uncommitted changes')
         .replace(/\bgoals=(\d+)\b/g,'$1 goals')
         .replace(/\bissues=(\d+)\b/g,'$1 open issues')
         .replace(/\b([A-Za-z_]+)=([\w.-]+)\b/g,'$1 $2')
         .replace(/^OODA cycle/i,'Cycle');
      for(const j of (BANNED_JARGON||[])){if(j)s=s.split(j).join('');}
      return s.replace(/\s{2,}/g,' ').trim();
    }
    /* P2 (#2358): humanize a raw cycle-outcome action-detail string for the
       Overview tab. Strips machine routing prefixes (advance-goal:, no-action:,
       <x>-brain:, brain:), drops parenthetical brain-error noise, maps known
       decision tokens to plain English, removes the "no decision keyword
       found...defaulting to..." boilerplate, and applies the shared
       BANNED_JARGON ban. The agent='engineer-...' substring the Attach button
       keys off is preserved verbatim. Returns PLAIN TEXT only — callers must
       escape it last (escape-last invariant). */
    function humanizeActionDetail(detail){
      if(detail==null)return'';
      let s=String(detail);
      // 1) Drop any (...) group whose body is pure machine noise, mirroring the
      //    server-side goals_status cleanup. Single linear scan -> ReDoS-immune.
      //    A group carrying an agent='engineer-...' reference is always kept so
      //    the Attach-button contract survives (SR-D5).
      const NOISE=['brain-error fallback','goal-action parse failed','ooda-brain','brain:'];
      let out='';
      for(let i=0;i<s.length;i++){
        if(s.charAt(i)==='('){
          let depth=1,j=i+1;
          for(;j<s.length&&depth>0;j++){
            const cj=s.charAt(j);
            if(cj==='(')depth++;else if(cj===')')depth--;
          }
          const body=s.slice(i+1,depth===0?j-1:j);
          const noisy=NOISE.some(m=>body.indexOf(m)>=0)&&body.indexOf("agent='engineer-")<0;
          if(noisy){out+=' ';i=j-1;continue;}
        }
        out+=s.charAt(i);
      }
      s=out;
      // 2) Drop the verbose "no decision keyword found ... defaulting to X"
      //    boilerplate (bounded, non-backtracking).
      s=s.replace(/no decision keyword found[^()]{0,80}?defaulting to\s*\S*/i,' ');
      // 3) Map known machine decision tokens to plain English (allowlist).
      const MAP={'continue_skipping':'continued without acting','spawn_engineer dispatched':'launched a sub-agent','prefix-routed':'chosen by built-in routing rules','no LLM configured':'no language model configured'};
      for(const k in MAP){if(Object.prototype.hasOwnProperty.call(MAP,k))s=s.split(k).join(MAP[k]);}
      // 4) Strip fixed machine routing prefixes / residual markers. The generic
      //    <x>-brain: form is bounded ({1,20}) so it stays non-backtracking.
      s=s.replace(/\b[a-z]{1,20}-brain:\s*/gi,' ');
      for(const t of ['advance-goal:','no-action:','ooda-brain','brain:']){s=s.split(t).join(' ');}
      // 5) Extend the static-lede jargon ban to this dynamic text.
      for(const j of (BANNED_JARGON||[])){if(j)s=s.split(j).join('');}
      // 6) Tidy orphaned punctuation and collapse whitespace.
      return s.replace(/\(\s*\)/g,'').replace(/\s{2,}/g,' ').replace(/^[\s:;,.-]+/,'').trim();
    }
    // P3 (#2358): render a raw second count as a human duration, e.g.
    // 37440s -> "10h 24m", 624m worth of seconds -> "10h 24m", 90s -> "1m".
    function humanizeDuration(secs){
      let s=Math.round(Number(secs)||0);
      if(s<=0)return'0m';
      if(s<60)return s+'s';
      const m=Math.floor(s/60);
      if(m<60)return m+'m';
      const h=Math.floor(m/60),rm=m%60;
      if(h<24)return rm?h+'h '+rm+'m':h+'h';
      const days=Math.floor(h/24),rh=h%24;
      return rh?days+'d '+rh+'h':days+'d';
    }
    // P3 (#2358): turn a bare 0-1 urgency float into a qualitative phrase with
    // an explicit scale, e.g. 0.50 -> "medium urgency (0.50 of 1.0)".
    function urgencyPhrase(u){
      const n=(typeof u==='number'&&isFinite(u))?u:0;
      const word=n>0.7?'high':n>0.4?'medium':'low';
      return word+' urgency ('+n.toFixed(2)+' of 1.0)';
    }
    /* Cluster-topology / event-bus de-jargon: map the machine-internal labels
       on the Overview "Machines & Memory Sharing" card to plain English. Every
       helper preserves the raw machine term (callers surface it as a hover
       tooltip via title=) so power users lose no information. Each returns
       PLAIN TEXT — callers must escape last (escape-last invariant). */
    function humanizeEventTopic(name){
      const MAP={
        fact_promoted:'Facts this machine shared out',
        fact_imported:'Facts received from other machines',
        memory_sync_requested:'Memory-sync requests',
        node_joined:'A machine joined the group',
        node_left:'A machine left the group'
      };
      if(!name)return'';
      if(MAP[name])return MAP[name];
      return String(name).replace(/[_-]+/g,' ').replace(/^./,c=>c.toUpperCase());
    }
    function humanizeSyncProtocol(p){
      if(!p)return'Not sharing (this machine only)';
      const s=String(p);
      if(s.indexOf('DHT')>=0||s.indexOf('gossip')>=0)return'Peer-to-peer (machines share facts directly)';
      return s;
    }
    function humanizeHiveStatus(s){
      if(!s)return'Standalone (this machine only)';
      const v=String(s).toLowerCase();
      if(v==='standalone')return'Standalone (this machine only)';
      if(v==='active')return'Active (sharing with other machines)';
      return String(s);
    }
    function humanizeTopology(t){
      if(!t)return'';
      const v=String(t).toLowerCase();
      if(v==='distributed')return'Supported (can run across machines)';
      if(v==='standalone'||v==='single')return'Single machine';
      return String(t);
    }
    /* Workboard "Task Memory" de-jargon (#2552 finding #4): the Task Memory
       table surfaces raw semantic-fact contents, some of which are goal-board
       snapshots serialized as JSON — e.g.
       {"active":[{"id":…,"status":{"InProgress":{"percent":5}}}]} — which leak
       the raw GoalProgress enum onto the page. These helpers render such a
       snapshot as plain-English lines (goal name + a plain status such as
       "In progress — 5%"). Plain-text facts and any JSON that is not a
       recognized goal board pass through unchanged; callers keep the raw
       content in a title= tooltip so power users lose nothing. Each returns
       PLAIN TEXT — callers must escape last (escape-last invariant). */
    function humanizeGoalProgress(status){
      if(status==null)return'';
      if(typeof status==='string'){
        const M={Proposed:'Proposed',NotStarted:'Not started',InProgress:'In progress',Paused:'Paused',Completed:'Completed',Done:'Done',Blocked:'Blocked'};
        return M[status]||String(status).replace(/[_-]+/g,' ');
      }
      if(typeof status==='object'){
        if(status.InProgress&&typeof status.InProgress.percent==='number')return'In progress — '+status.InProgress.percent+'%';
        if(Object.prototype.hasOwnProperty.call(status,'Blocked')){const r=status.Blocked;return(r&&typeof r==='string')?'Blocked — '+r:'Blocked';}
        if(status.InProgress!=null)return'In progress';
        const k=Object.keys(status)[0];
        if(k)return String(k).replace(/([a-z])([A-Z])/g,'$1 $2').replace(/^./,c=>c.toUpperCase());
      }
      return'';
    }
    /* Issue #20: classify a serialized `GoalProgress` enum to a canonical
       lifecycle KEY by VARIANT — never by parsing the free-form Display/reason
       string (G3, agentic-over-brittle). The key indexes the GOAL_STATUS_COLORS
       allowlist below so goal-supplied text is never interpolated into a style=
       attribute. Accepts both the string variants ("NotStarted", "Completed",
       …) and the struct/tuple variants ({"InProgress":{…}}, {"Blocked":"…"}). */
    function goalLifecycleKey(status){
      const M={Proposed:'proposed',NotStarted:'not-started',InProgress:'in-progress',Blocked:'blocked',Paused:'paused',Completed:'completed',Done:'completed'};
      if(status==null)return'not-started';
      if(typeof status==='string')return M[status]||'not-started';
      if(typeof status==='object'){
        if(Object.prototype.hasOwnProperty.call(status,'Blocked'))return'blocked';
        if(Object.prototype.hasOwnProperty.call(status,'InProgress'))return'in-progress';
        const k=Object.keys(status)[0];
        if(k)return M[k]||'not-started';
      }
      return'not-started';
    }
    /* Hardcoded allowlist: one colour per lifecycle key. Blocked uses amber
       (#d29922), DELIBERATELY different from the activity-Failed red (#f85149)
       so a lifecycle-blocked goal is never mistaken for an activity failure
       (issue #20). Completed=green, in-progress=accent, not-started/proposed=
       grey, paused=muted. */
    const GOAL_STATUS_COLORS={'not-started':'#8b949e','proposed':'#8b949e','in-progress':'var(--accent)','blocked':'#d29922','paused':'#6e7681','completed':'#2ea043'};
    /* Issue #2695 follow-up: classify a NUMERIC goal priority to a canonical
       tier KEY by its value (never by parsing free-form text). The key indexes
       the hardcoded GOAL_PRIORITY_COLORS allowlist below so priority data is
       never interpolated into a style= attribute. Lower number = higher
       priority: <=1 critical, 2 high, 3 medium, 4 low, >=5 minimal. */
    function priorityTierKey(priority){
      const p=Number(priority);
      if(!Number.isFinite(p))return'medium';
      if(p<=1)return'critical';
      if(p===2)return'high';
      if(p===3)return'medium';
      if(p===4)return'low';
      return'minimal';
    }
    /* Plain-English priority TIER LABEL (e.g. "Critical"). Returns PLAIN TEXT —
       callers esc() the RESULT last and append the raw "(pN)" number; this runs
       on the RAW priority, never on already-escaped text (escape-last). */
    function humanizePriority(priority){
      const p=Number(priority);
      if(!Number.isFinite(p))return'\u2014';
      return{critical:'Critical',high:'High',medium:'Medium',low:'Low',minimal:'Minimal'}[priorityTierKey(p)];
    }
    /* Hardcoded allowlist: one colour per priority tier key, a hotter=more-urgent
       heat gradient. Goal-supplied priority is classified to a key first, so only
       these colours ever reach the DOM. The Critical red and Medium amber match
       hues used in other columns (Current Activity / Status) but live in the
       distinct Priority column with their own labels, so they stay unambiguous. */
    const GOAL_PRIORITY_COLORS={critical:'#f85149',high:'#db6d28',medium:'#d29922',low:'#388bfd',minimal:'#8b949e'};
    /* Issue #2695 follow-up: order goals by priority ASCENDING (lower number =
       higher priority = first) with a stable id tiebreak. Applied at BOTH the
       top level and within each parent's children so priority-first ordering
       holds at every level of the tree. Returns a NEW array (non-mutating). */
    function sortGoalsByPriority(goals){
      return (goals||[]).slice().sort((a,b)=>{
        const pa=Number(a&&a.priority), pb=Number(b&&b.priority);
        const na=Number.isFinite(pa)?pa:Infinity, nb=Number.isFinite(pb)?pb:Infinity;
        if(na!==nb)return na-nb;
        return String((a&&a.id)||'').localeCompare(String((b&&b.id)||''));
      });
    }
    /* Issue #2695 follow-up: group decomposed sub-goals under their parent using
       the structured parent_goal_id edge (G3 — never parse the description), and
       order the resulting tree by priority at every level. Returns priority-
       ordered top-level ENTRIES, each either:
         {kind:'goal',  goal, children[], rep}      standalone/active-parent goal
         {kind:'umbrella', header, children[], rep} demoted decompose-parent group
       Nesting rules:
         * parent_goal_id null / self / resolves to neither set -> the goal roots
           the tree (orphans + completed/tombstoned-parent children at root);
         * parent_goal_id matches an ACTIVE, top-level goal -> nest under it;
         * parent_goal_id matches a demoted `decompose-parent` node in `backlog`
           (the normal post-decompose case, where the umbrella left the active
           board) -> nest under a header synthesised from that backlog node.
       Depth is capped at a single grouping level (decomposition is one level) and
       a parent that is itself nested is not a nesting target, so a cyclic/deep
       parent chain can never loop or indent unboundedly — such nodes fall to the
       root. `rep` is the entry's representative priority for top-level ordering:
       an active parent's own priority, or a demoted group's min child priority. */
    function groupGoalsByParent(active,backlog){
      const list=active||[];
      const activeById={};
      for(const g of list){ if(g&&g.id!=null) activeById[String(g.id)]=g; }
      const backlogById={};
      for(const b of (backlog||[])){ if(b&&b.id!=null) backlogById[String(b.id)]=b; }
      const parentKey=g=>{
        const pid=(g&&g.parent_goal_id!=null)?String(g.parent_goal_id):null;
        return (pid!==null&&pid!==String(g&&g.id))?pid:null;
      };
      const resolves=pid=>pid!==null&&(Object.prototype.hasOwnProperty.call(activeById,pid)||Object.prototype.hasOwnProperty.call(backlogById,pid));
      // An active goal can HOST children only when it is itself top-level (its
      // own parent does not resolve) — this caps depth at one level and breaks
      // any parent-chain cycle by refusing to nest under a nested node.
      const isTopLevelActive=g=>!resolves(parentKey(g));
      const childrenOf={};
      const demotedHeaders={};
      const roots=[];
      for(const g of list){
        const pid=parentKey(g);
        if(pid!==null&&Object.prototype.hasOwnProperty.call(activeById,pid)&&isTopLevelActive(activeById[pid])){
          (childrenOf[pid]=childrenOf[pid]||[]).push(g);
        }else if(pid!==null&&Object.prototype.hasOwnProperty.call(backlogById,pid)){
          (childrenOf[pid]=childrenOf[pid]||[]).push(g);
          demotedHeaders[pid]=backlogById[pid];
        }else{
          roots.push(g); // null / self / unresolved / non-top-level parent
        }
      }
      const entries=[];
      for(const g of roots){
        entries.push({
          kind:'goal',
          goal:g,
          children:sortGoalsByPriority(childrenOf[String(g&&g.id)]||[]),
          rep:Number(g&&g.priority)
        });
      }
      for(const pid in demotedHeaders){
        if(!Object.prototype.hasOwnProperty.call(demotedHeaders,pid))continue;
        const kids=sortGoalsByPriority(childrenOf[pid]||[]);
        const rep=kids.reduce((m,c)=>{const p=Number(c&&c.priority);return Number.isFinite(p)?Math.min(m,p):m;},Infinity);
        entries.push({kind:'umbrella',header:demotedHeaders[pid],children:kids,rep:rep});
      }
      return entries.sort((a,b)=>{
        const na=Number.isFinite(a.rep)?a.rep:Infinity, nb=Number.isFinite(b.rep)?b.rep:Infinity;
        if(na!==nb)return na-nb;
        const ia=a.kind==='goal'?String((a.goal&&a.goal.id)||''):String((a.header&&a.header.id)||'');
        const ib=b.kind==='goal'?String((b.goal&&b.goal.id)||''):String((b.header&&b.header.id)||'');
        return ia.localeCompare(ib);
      });
    }
    function humanizeTaskMemory(content){
      const raw=(content==null)?'':String(content);
      const trimmed=raw.trim();
      if(trimmed.charAt(0)!=='{'&&trimmed.charAt(0)!=='[')return raw;
      let obj;
      try{obj=JSON.parse(trimmed);}catch(e){return raw;}
      if(obj&&Array.isArray(obj.active)){
        const lines=obj.active.map(g=>{
          const name=(g&&(g.description||g.name||g.id))||'(unnamed goal)';
          const st=humanizeGoalProgress(g&&g.status);
          return st?(String(name)+' — '+st):String(name);
        });
        let out=lines.length?lines.join(' · '):'No active goals';
        if(Array.isArray(obj.backlog)&&obj.backlog.length)out+=' · '+obj.backlog.length+' in backlog';
        return out;
      }
      return raw;
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

    /* --- Active tab tracking (#2649: background scheduler owns refresh) --- */
    /* activeTab is read by the background scheduler (part_05.rs) so a
       return-to-visible can immediately refresh whatever tab the operator is
       looking at. Per-tab refresh timers are no longer armed or wiped on tab
       activation — the scheduler owns one persistent, visibility-gated timer
       per fetcher for the page's lifetime. */
    let activeTab='overview';
    /* The Workers live PTY cannot be prefetched, so it is initialised lazily on
       the first Workers activation, never by the background scheduler. */
    let workersTerminalInit=false;

    /* --- Tabs --- */
    function updateDocumentTitleForTab(slug){
      var meta=(window.__TAB_META||{})[slug];
      if(meta && meta.title) document.title=meta.title;
    }

    /* #2627 consolidation: the canonical tabs (nine consolidated tabs plus the
       standalone Creative Ideas tab), plus TAB_ALIASES mapping every retired
       17-tab slug to the parent tab that now hosts it as a sub-section. TAB_ALIASES is a fixed allowlist; a #hash deep link is
       validated against ^[a-z-]+$, matched against the allowlist, and falls
       back to 'overview' on any miss. The hash is never concatenated into a
       DOM selector, so a crafted hash cannot inject markup. */
    const CANONICAL_TABS=['overview','goals','activity','workers','pull-requests','resources','chat','overseer','journal','creative-ideas'];
    const TAB_ALIASES={"status":"overview","workboard":"goals","logs":"activity","traces":"activity","thinking":"activity","brain-failures":"activity","processes":"workers","terminal":"workers","merge-decisions":"pull-requests","pr-readiness":"pull-requests","memory":"resources","costs":"resources"};

    /* #2649: the per-tab fetch/refresh chain that used to live here
       (runTabFetches / clearTabTimers) is retired. Background prefetch and
       persistent per-tab refresh are now owned by the TAB_LOADERS registry and
       startBackgroundScheduler() in part_05.rs, so every tab loads on page open
       and stays fresh regardless of which tab is currently visible. */

    /* Activate a canonical tab by slug. `slug` is always one of CANONICAL_TABS
       (callers pass a validated value); `section`, when given, is a retired
       slug whose scroll anchor we jump to inside the parent tab. */
    function activateTab(slug,section){
      if(CANONICAL_TABS.indexOf(slug)<0) slug='overview';
      const panel=document.getElementById('tab-'+slug);
      if(!panel) return;
      document.querySelectorAll('.tab').forEach(t=>t.classList.remove('active'));
      document.querySelectorAll('.tab-content').forEach(c=>c.classList.remove('active'));
      const navBtn=document.querySelector('.tab[data-tab="'+slug+'"]');
      if(navBtn) navBtn.classList.add('active');
      panel.classList.add('active');
      activeTab=slug;
      updateDocumentTitleForTab(slug);
      /* #2649: render from the already-prefetched cache — never block on a
         fetch and never wipe a background timer. Attach the Workers live
         terminal lazily on first activation (it is excluded from background
         prefetch), then kick a non-blocking refresh so the panel the operator
         just opened is current. */
      if(slug==='workers'&&!workersTerminalInit){workersTerminalInit=true;try{initAgentLogTerminal();}catch(_){}}
      if(typeof refreshTab==='function'){refreshTab(slug);}
      if(section){const el=document.getElementById('section-'+section);if(el&&el.scrollIntoView)el.scrollIntoView({block:'start'});}
    }

    /* Resolve an untrusted location.hash to a {tab,section}. Unknown or
       malformed hashes fall back to Overview; the raw value is validated
       before use and never becomes part of a selector. */
    function resolveHashTab(){
      const raw=(location.hash||'').replace(/^#/,'');
      if(!/^[a-z-]+$/.test(raw)) return {tab:'overview',section:null};
      if(CANONICAL_TABS.indexOf(raw)>=0) return {tab:raw,section:null};
      if(Object.prototype.hasOwnProperty.call(TAB_ALIASES,raw)) return {tab:TAB_ALIASES[raw],section:raw};
      return {tab:'overview',section:null};
    }

    document.querySelectorAll('.tab').forEach(tab=>{
      tab.addEventListener('click',()=>{activateTab(tab.dataset.tab);});
    });
    window.addEventListener('hashchange',()=>{const r=resolveHashTab();activateTab(r.tab,r.section);});
    if(location.hash){const r=resolveHashTab();activateTab(r.tab,r.section);}
    setInterval(()=>{document.getElementById('clock').textContent=formatTime(Date.now())},1000);

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
          <div class="stat"><span class="label">Agent Daemon</span><span class="value ${oc}">${esc(d.ooda_daemon)}${healthDetail}</span></div>
          <div class="stat"><span class="label">Active Processes</span><span class="value">${d.active_processes??0}</span></div>
          <div class="stat"><span class="label">Disk Usage</span><span class="value ${dc}">${d.disk_usage_pct??'?'}%</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${timeAgo(d.timestamp)}</span></div>`;
        document.getElementById('header-version').textContent='v'+d.version+' ('+shortHash+')';
      }catch(e){document.getElementById('status').innerHTML='<span class="err">Failed to reach /api/status — is the dashboard server running?</span>';}
    }

    /* --- Cognition: hybrid recall precision (#2491 / #2494) --- */
    function recallVerdictClass(v){
      if(v==='confirmed') return 'ok';
      if(v==='diverging'||v==='regressed') return 'err';
      if(v==='benchmark-only'||v==='live-only') return 'warn';
      return '';
    }
    async function fetchRecallPrecision(){
      const el=document.getElementById('cognition-recall-precision');
      if(!el) return;
      try{
        const d=await apiFetch('/api/cognition/recall-precision');
        const c=d.correlation||{};
        const verdict=c.verdict||'insufficient';
        const bench=d.benchmark;
        const live=d.live;
        const benchLine=bench
          ? `${bench.score.toFixed(4)} <span class="value ${bench.previous_score!=null&&bench.score>bench.previous_score?'ok':''}">(${esc(bench.signal||'n/a')})</span>`
          : '<span class="warn">no benchmark runs yet</span>';
        const liveLine=live
          ? `${live.first.toFixed(4)} → ${live.latest.toFixed(4)} <span class="value">(${live.samples} sample${live.samples===1?'':'s'}, ${live.window_hours}h)</span>`
          : '<span class="warn">no live samples yet</span>';
        let html=`
          <div class="stat"><span class="label">Verdict</span><span class="value ${recallVerdictClass(verdict)}">${esc(verdict)}</span></div>
          <div class="stat"><span class="label">Benchmark</span><span class="value">${benchLine}</span></div>
          <div class="stat"><span class="label">Live trend</span><span class="value">${liveLine}</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${timeAgo(d.generated_at)}</span></div>`;
        if(c.explanation) html+=`<p style="margin:.5rem 0 0;color:#8b949e;font-size:.8rem">${esc(c.explanation)}</p>`;
        if(d.error) html+=`<p class="err" style="margin:.5rem 0 0;font-size:.8rem">${esc(d.error)}</p>`;
        el.innerHTML=html;
      }catch(e){el.innerHTML='<span class="err">Failed to reach /api/cognition/recall-precision</span>';}
    }

    async function fetchAgentOverview(){
      try{
        const d=await apiFetch('/api/activity');
        const el=document.getElementById('agent-live-status');
        const daemon=d.daemon||{};
        const isRunning=daemon.status==='healthy'||daemon.status==='running';
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
            currentFocus=`<strong>${esc(humanizeGoalId(top.goal_id))}</strong> — ${esc(top.reason)} <span style="color:${top.urgency>0.7?'var(--red)':top.urgency>0.4?'var(--yellow)':'var(--green)'}">${urgencyPhrase(top.urgency)}</span>`;
            break;
          }
        }

        el.innerHTML=`
          <div style="display:flex;gap:2rem;flex-wrap:wrap;align-items:center;margin-bottom:.75rem">
            <div><span style="font-size:1.5rem;${isRunning&&!isStale?'':'filter:grayscale(1)'}">${isRunning?(isStale?'🟡':'🟢'):'🔴'}</span> <strong style="font-size:1.1rem">${isRunning?(isStale?'Agent Stale':'Decision Loop Active'):'Agent Stopped'}</strong></div>
            <div style="color:#8b949e">Cycle <strong style="color:var(--fg)">#${cycle}</strong> · Last heartbeat <strong style="color:var(--fg)">${heartbeat}</strong>${isStale?' <span style="color:var(--yellow)">(>10 min ago)</span>':''}</div>
          </div>
          ${currentFocus?`<div style="margin-bottom:.75rem"><span style="color:#8b949e">🎯 Top Priority:</span> ${currentFocus}</div>`:''}
          ${latestActions.length?`
            <div style="font-size:.85rem">
              <div style="color:#8b949e;margin-bottom:.3rem;font-weight:600">Last Cycle Actions:</div>
              ${latestActions.map(o=>`
                <div style="padding:.2rem 0;display:flex;gap:.5rem;align-items:baseline">
                  <span>${o.success?'✅':'❌'}</span>
                  <code style="color:var(--accent)">${esc(humanizeActionKind(o.action_kind))}</code>
                  <span>${esc(o.action_description||'')}</span>
                  ${o.detail?'<span style="color:#8b949e;font-size:.8rem;max-width:400px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:inline-block">'+esc(humanizeActionDetail(o.detail).substring(0,120))+'</span>':''}
                </div>`).join('')}
            </div>`:'<div style="color:#8b949e">No recent actions recorded.</div>'}`;

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
              <code>${esc(humanizeActionKind(a.action_kind))}</code>
              <span style="flex:1">${renderActionDetail((function(){var arr=Array.from(humanizeActionDetail(a.detail));var d=arr.length>200?arr.slice(0,200).join('')+'…':arr.join('');return d||a.action_description||'';})())}</span>
            </div>`).join('');
        }else{
          actEl.innerHTML='<span style="color:#8b949e">No structured action history yet. The agent daemon records actions each cycle.</span>';
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
    let allLogLevels=[];
    async function fetchLogs(){
      try{
        const d=await apiFetch('/api/logs');
        allLogLines=d.daemon_log_lines||[];
        allLogLevels=d.daemon_log_levels||[];
        applyLogFilter();
        // Issue #928: guard each element access so a missing target on the
        // current tab does not abort the whole fetchLogs and leave every
        // panel stuck on "Loading…".
        const tEl=document.getElementById('ooda-transcripts');
        if(tEl){"#;
