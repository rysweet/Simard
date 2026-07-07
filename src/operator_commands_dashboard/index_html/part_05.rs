pub(crate) const PART_05: &str = r#"      try {
        const data = await apiFetch('/api/azlin/tmux-sessions');
        const hosts = Array.isArray(data.hosts) ? data.hosts : [];
        if(hosts.length === 0){
          body.innerHTML = '<div style="color:#8b949e;font-size:.85rem">No configured hosts.</div>';
        } else {
          body.innerHTML = hosts.map(h => renderTmuxHost(h)).join('');
        }
        const ts = document.getElementById('tmux-last-refreshed');
        if(ts) ts.textContent = data.refreshed_at ? formatTime(data.refreshed_at) : formatTime(Date.now());
      } catch(e) {
        body.innerHTML = '<div style="color:#f85149;font-size:.85rem">Failed to load tmux sessions: '+esc(e.message||e)+'</div>';
      }
    }
    function renderTmuxHost(h){
      const host = String(h.host || '');
      const reachable = !!h.reachable;
      const sessions = Array.isArray(h.sessions) ? h.sessions : [];
      const errText = h.error ? String(h.error) : '';
      const headerColor = reachable ? '#3fb950' : '#f85149';
      const status = reachable ? '● reachable' : '○ unreachable';
      let inner;
      if(!reachable){
        inner = '<div style="color:#8b949e;font-size:.85rem;padding:.5rem">'
              + (errText ? esc(errText) : 'host unreachable')
              + '</div>';
      } else if(sessions.length === 0){
        inner = '<div style="color:#8b949e;font-size:.85rem;padding:.5rem">No tmux sessions on this host.</div>';
      } else {
        const rows = sessions.map(s => {
          const name = String(s.name || '');
          const created = fmtUnixTs(s.created);
          const attached = s.attached ? '✓' : '—';
          const wins = (s.windows == null) ? '—' : String(s.windows);
          const tid = 'tmux-open-'+host+'-'+name;
          return '<tr>'
               + '<td style="padding:.3rem .5rem;font-family:monospace">'+esc(name)+'</td>'
               + '<td style="padding:.3rem .5rem;color:#8b949e">'+esc(created)+'</td>'
               + '<td style="padding:.3rem .5rem;text-align:center">'+attached+'</td>'
               + '<td style="padding:.3rem .5rem;text-align:right">'+esc(wins)+'</td>'
               + '<td style="padding:.3rem .5rem;text-align:right">'
               +   '<button class="btn" data-testid="'+esc(tid)+'" '
               +     'onclick="openTmuxAttach('+JSON.stringify(host)+','+JSON.stringify(name)+')">Open</button>'
               + '</td>'
               + '</tr>';
        }).join('');
        inner = '<table data-testid="tmux-table-'+esc(host)+'" '
              + 'style="width:100%;border-collapse:collapse;font-size:.88rem">'
              + '<thead><tr style="border-bottom:1px solid var(--border);color:#8b949e;text-align:left">'
              + '<th style="padding:.3rem .5rem">Session</th>'
              + '<th style="padding:.3rem .5rem">Created</th>'
              + '<th style="padding:.3rem .5rem;text-align:center">Attached?</th>'
              + '<th style="padding:.3rem .5rem;text-align:right">Windows</th>'
              + '<th style="padding:.3rem .5rem;text-align:right">Action</th>'
              + '</tr></thead><tbody>'
              + rows
              + '</tbody></table>';
      }
      // For unreachable hosts, also expose the host-keyed testid on the wrapper so
      // e2e tests can find error text without a sessions table.
      const wrapperTid = reachable ? '' : ' data-testid="tmux-table-'+esc(host)+'"';
      return '<div'+wrapperTid+' style="margin-top:.6rem;border:1px solid var(--border);border-radius:6px;overflow:hidden">'
           + '<div style="background:#1a1a2e;padding:.4rem .6rem;display:flex;justify-content:space-between;align-items:center">'
           +   '<strong style="font-family:monospace">'+esc(host)+'</strong>'
           +   '<span style="color:'+headerColor+';font-size:.85rem">'+status+'</span>'
           + '</div>'
           + inner
           + '</div>';
    }
    function openTmuxAttach(host, session){
      // Validate identifier shape client-side (mirror of server allow-list).
      const re = /^[A-Za-z0-9_.-]{1,64}$/;
      if(!re.test(host) || !re.test(session)){
        setAgentLogStatus('invalid host or session name', '#f85149');
        return;
      }
      initAgentLogTerminal();
      if(!agentLogTerm) return;
      // Tear down any existing agent-log WS before reusing the xterm instance.
      if(agentLogWS){ try { agentLogWS.close(); } catch(_) {} agentLogWS = null; }
      agentLogTerm.clear();
      // Surface the attached target in the existing status row.
      const nameInput = document.getElementById('agent-log-name');
      if(nameInput) nameInput.value = host + ':' + session;
      setAgentLogStatus('attaching to '+host+':'+session+'…', '#d29922');
      const proto = (window.location.protocol === 'https:') ? 'wss:' : 'ws:';
      const url = proto + '//' + window.location.host
                + '/ws/tmux_attach/' + encodeURIComponent(host)
                + '/' + encodeURIComponent(session);
      let ws;
      try { ws = new WebSocket(url); ws.binaryType = 'arraybuffer'; }
      catch(e){ setAgentLogStatus('connect failed: '+(e&&e.message||e), '#f85149'); return; }
      agentLogWS = ws;
      ws.onopen = () => setAgentLogStatus('attached: '+host+':'+session, '#3fb950');
      ws.onmessage = (ev) => {
        if(!agentLogTerm) return;
        if(typeof ev.data === 'string'){
          agentLogTerm.write(ev.data);
        } else if(ev.data instanceof ArrayBuffer){
          const bytes = new Uint8Array(ev.data);
          // Pass raw bytes through xterm so ANSI escapes render correctly.
          let s = '';
          for(let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
          agentLogTerm.write(s);
        }
      };
      ws.onerror = () => setAgentLogStatus('socket error', '#f85149');
      ws.onclose = () => { setAgentLogStatus('detached', '#8b949e'); if(agentLogWS === ws) agentLogWS = null; };
    }

    /* --- Merge Judge Decisions (#2041) --- */
    async function fetchMergeJudge(){
      const el=document.getElementById('merge-judge-panel');
      const trendEl=document.getElementById('merge-judge-trend');
      if(!el) return;
      try {
        const d = await apiFetch('/api/merge-judge');
        const persistenceAvailable = !!d.persistence_available;
        const decisions = Array.isArray(d.decisions) ? d.decisions : [];
        const summary = d.summary || {};

        // Render approval-rate-over-time trend chart (#2223)
        if(trendEl){
          if(decisions.length>=2){
            const sorted=decisions.slice().sort(function(a,b){return (a.evaluated_at||'').localeCompare(b.evaluated_at||'');});
            const chartW=Math.min(600,Math.max(200,sorted.length*30));
            const chartH=80;
            const pad=2;
            let cumApproved=0,cumTotal=0;
            const rates=sorted.map(function(dec){
              cumTotal++;
              if(dec.verdict==='ready') cumApproved++;
              return {rate:cumTotal>0?cumApproved/cumTotal:0,label:'PR #'+(dec.pr_number||'?'),time:dec.evaluated_at||''};
            });
            const stepX=rates.length>1?(chartW-pad*2)/(rates.length-1):0;
            const points=rates.map(function(r,i){
              const x=pad+i*stepX;
              const y=chartH-pad-(r.rate*(chartH-pad*2));
              return x+','+y;
            }).join(' ');
            const dots=rates.map(function(r,i){
              const x=pad+i*stepX;
              const y=chartH-pad-(r.rate*(chartH-pad*2));
              const pct=Math.round(r.rate*100);
              return '<circle cx="'+x+'" cy="'+y+'" r="3" fill="var(--green)" stroke="var(--bg)" stroke-width="1"><title>'+esc(r.label)+': '+pct+'% approved ('+r.time+')</title></circle>';
            }).join('');
            const finalRate=rates.length?Math.round(rates[rates.length-1].rate*100):0;
            trendEl.innerHTML='<div style="display:flex;align-items:center;gap:1rem;margin-bottom:.5rem">'
              +'<span style="font-size:.85rem;color:#8b949e">Cumulative approval rate over time</span>'
              +'<strong style="color:var(--green)">'+finalRate+'%</strong>'
              +'</div>'
              +'<div style="overflow-x:auto" data-testid="merge-judge-trend-chart">'
              +'<svg width="'+chartW+'" height="'+(chartH+4)+'" style="display:block">'
              +'<line x1="'+pad+'" y1="'+(chartH/2)+'" x2="'+(chartW-pad)+'" y2="'+(chartH/2)+'" stroke="var(--border)" stroke-width="1" stroke-dasharray="4,4"/>'
              +'<polyline points="'+points+'" fill="none" stroke="var(--green)" stroke-width="2" data-testid="merge-trend-line"/>'
              +dots
              +'<line x1="'+pad+'" y1="'+chartH+'" x2="'+(chartW-pad)+'" y2="'+chartH+'" stroke="var(--border)" stroke-width="1"/>'
              +'</svg>'
              +'<div style="display:flex;justify-content:space-between;font-size:.7rem;color:#8b949e;margin-top:.2rem"><span>0%</span><span>50%</span><span>100%</span></div></div>';
          }else if(!persistenceAvailable){
            trendEl.innerHTML='<span style="color:#8b949e;font-size:.85rem">Trend chart will appear once decision data is available.</span>';
          }else{
            trendEl.innerHTML='<span style="color:#8b949e;font-size:.85rem">Need at least 2 decisions to render a trend chart.</span>';
          }
        }

        if(!persistenceAvailable && decisions.length === 0){
          el.innerHTML =
              '<div style="padding:1rem;text-align:center">'
            +   '<div style="font-size:2rem;margin-bottom:.5rem">📋</div>'
            +   '<div style="color:#8b949e;font-size:.95rem;max-width:540px;margin:0 auto;line-height:1.6">'
            +     'No merge-judge decisions have been recorded yet. '
            +     'When the merge judge evaluates a pull request, the verdict — approved, rejected, '
            +     'or deferred — will appear here with the reasoning and timestamp.'
            +   '</div>'
            +   '<div style="color:#8b949e;font-size:.8rem;margin-top:.75rem;padding:.5rem;background:#1a2332;border-radius:4px;display:inline-block">'
            +     '⏳ Verdict persistence is not yet enabled. '
            +     esc(d.persistence_reason || 'See issue #1893.')
            +   '</div>'
            + '</div>';
          return;
        }

        if(decisions.length === 0){
          el.innerHTML = '<div style="color:#8b949e;font-size:.85rem">No decisions recorded in this session.</div>';
          return;
        }

        const summaryHtml =
            '<div style="display:flex;gap:1rem;flex-wrap:wrap;margin-bottom:.75rem;font-size:.85rem">'
          +   '<span>Total: <strong>'+esc(String(summary.total||0))+'</strong></span>'
          +   '<span class="ok">Approved: <strong>'+esc(String(summary.approved||0))+'</strong></span>'
          +   '<span class="err">Rejected: <strong>'+esc(String(summary.rejected||0))+'</strong></span>'
          +   '<span class="warn">Deferred: <strong>'+esc(String(summary.deferred||0))+'</strong></span>'
          + '</div>';

        const rows = decisions.map(function(dec){
          const verdict = String(dec.verdict||'unknown');
          let badge;
          if(verdict === 'ready')          badge = '<span class="ok">✓ approved</span>';
          else if(verdict === 'not_ready') badge = '<span class="err">✗ rejected</span>';
          else if(verdict === 'unclear')   badge = '<span class="warn">? deferred</span>';
          else                             badge = '<span style="color:#8b949e">'+esc(verdict)+'</span>';
          const blockers = Array.isArray(dec.blockers) && dec.blockers.length
            ? '<div style="margin-top:.3rem;font-size:.8rem;color:#8b949e">Blockers: '
              + dec.blockers.map(function(b){return esc(b.section)+' ('+esc(b.severity)+'): '+esc(b.observation);}).join('; ')
              + '</div>'
            : '';
          return '<tr>'
               + '<td><a href="https://github.com/rysweet/Simard/pull/'+esc(String(dec.pr_number))+'" target="_blank" style="color:var(--accent)">#'+esc(String(dec.pr_number))+'</a></td>'
               + '<td>'+badge+'</td>'
               + '<td style="max-width:400px">'+esc(String(dec.rationale||''))+blockers+'</td>'
               + '<td style="color:#8b949e">'+formatTime(dec.evaluated_at)+'</td>'
               + '</tr>';
        }).join('');

        el.innerHTML = summaryHtml
          + '<table class="proc-table" data-testid="merge-judge-table">'
          + '<thead><tr><th>PR</th><th>Verdict</th><th>Reasoning</th><th>Evaluated</th></tr></thead>'
          + '<tbody>' + rows + '</tbody></table>';
      } catch(e) {
        el.innerHTML = '<span class="err">Failed to load merge-judge decisions: '+esc(e.message||e)+'</span>';
      }
    }

    /* --- PR Readiness (#2042) --- */
    async function fetchPrReadiness(){
      const sumEl=document.getElementById('pr-readiness-summary');
      const el=document.getElementById('pr-readiness-panel');
      if(!el) return;
      try {
        const d = await apiFetch('/api/prs');
        const prs = Array.isArray(d.prs) ? d.prs : [];
        const summary = d.summary || {};

        if(d.error){
          sumEl.innerHTML='<div class="err" style="font-size:.85rem">'+esc(d.error)+'</div>';
        } else {
          sumEl.innerHTML=
              '<div style="display:flex;gap:1rem;flex-wrap:wrap;font-size:.85rem">'
            +   '<span>Total: <strong>'+esc(String(summary.total||0))+'</strong></span>'
            +   '<span class="ok">Ready: <strong>'+esc(String(summary.ready||0))+'</strong></span>'
            +   '<span class="err">Blocked: <strong>'+esc(String(summary.blocked||0))+'</strong></span>'
            +   '<span class="warn">Pending: <strong>'+esc(String(summary.pending||0))+'</strong></span>'
            + '</div>';
        }

        if(prs.length === 0){
          el.innerHTML =
              '<div style="padding:1rem;text-align:center">'
            +   '<div style="font-size:2rem;margin-bottom:.5rem">📋</div>'
            +   '<div style="color:#8b949e;font-size:.95rem;max-width:540px;margin:0 auto;line-height:1.6">'
            +     'No open pull requests found. When Simard opens or manages PRs, they will appear here with their readiness status.'
            +   '</div>'
            + '</div>';
          return;
        }

        const rows = prs.map(function(pr){
          let ciBadge;
          if(pr.ci_status==='passing')       ciBadge='<span class="ok">✓ passing</span>';
          else if(pr.ci_status==='failing')  ciBadge='<span class="err">✗ failing</span>';
          else if(pr.ci_status==='pending')  ciBadge='<span class="warn">⏳ pending</span>';
          else                               ciBadge='<span style="color:#8b949e">'+esc(pr.ci_status||'none')+'</span>';
          let reviewBadge;
          if(pr.review_status==='approved')              reviewBadge='<span class="ok">✓ approved</span>';
          else if(pr.review_status==='changes_requested') reviewBadge='<span class="err">✗ changes requested</span>';
          else if(pr.review_status==='review_required')  reviewBadge='<span class="warn">⏳ review required</span>';
          else                                           reviewBadge='<span style="color:#8b949e">'+esc(pr.review_status||'none')+'</span>';
          const blockerList = Array.isArray(pr.blockers) && pr.blockers.length
            ? '<div style="margin-top:.2rem;font-size:.8rem;color:#f85149">'+pr.blockers.map(function(b){return '• '+esc(b);}).join('<br>')+'</div>'
            : '';
          const readyBadge = pr.ready
            ? '<span class="ok" style="font-weight:700">✓ Ready</span>'
            : '<span class="err" style="font-weight:700">✗ Blocked</span>';
          const draft = pr.is_draft ? ' <span class="badge" style="background:#21262d;color:#8b949e">draft</span>' : '';
          const titleEsc = esc(String(pr.title||'').slice(0,80));
          return '<tr data-testid="pr-row-'+pr.number+'">'
               + '<td><a href="'+esc(pr.url||'')+'" target="_blank" style="color:var(--accent)">#'+esc(String(pr.number))+'</a>'+draft+'</td>'
               + '<td style="max-width:320px">'+titleEsc+'</td>'
               + '<td><code>'+esc(String(pr.base_branch||''))+'</code></td>'
               + '<td>'+ciBadge+'</td>'
               + '<td>'+reviewBadge+'</td>'
               + '<td>'+readyBadge+blockerList+'</td>'
               + '<td style="color:#8b949e;font-size:.8rem">'+formatTime(pr.updated_at)+'</td>'
               + '</tr>';
        }).join('');

        el.innerHTML =
            '<table class="proc-table" data-testid="pr-readiness-table">'
          + '<thead><tr><th>PR</th><th>Title</th><th>Base</th><th>CI</th><th>Review</th><th>Status</th><th>Updated</th></tr></thead>'
          + '<tbody>' + rows + '</tbody></table>';
      } catch(e) {
        el.innerHTML = '<span class="err">Failed to load PR readiness: '+esc(e.message||e)+'</span>';
      }
    }

    /* --- Status (unified telemetry snapshot, #2528) --- */
    async function fetchStatusSnapshot(){
      const el=document.getElementById('status-snapshot-panel');
      if(!el) return;
      try {
        const d = await apiFetch('/api/status/snapshot');
        if(d.error){
          el.innerHTML='<span class="err">Failed to load status: '+esc(d.error)+'</span>';
          return;
        }
        const rendered = typeof d.rendered==='string' ? d.rendered : '';
        el.innerHTML='<pre class="status-report" data-testid="status-report" '
          +'style="white-space:pre-wrap;font-family:ui-monospace,Menlo,Consolas,monospace;'
          +'font-size:.82rem;line-height:1.45;color:#c9d1d9;margin:0">'
          +esc(rendered)+'</pre>';
      } catch(e) {
        el.innerHTML='<span class="err">Failed to load status: '+esc(e.message||e)+'</span>';
      }
    }

    /* --- Overseer activity feed (#2419) --- */
    function overseerCadenceHuman(secs){
      secs=Number(secs)||0;
      if(secs<=0) return '—';
      if(secs%3600===0) return (secs/3600)+' h';
      if(secs%60===0) return (secs/60)+' min';
      return secs+' s';
    }
    function overseerTickHuman(r){
      r=r||{};
      const n=k=>Number(r[k])||0;
      const s=x=>x===1?'':'s';
      const did=[];
      if(n('issues_filed')>0) did.push('filed '+n('issues_filed')+' issue'+s(n('issues_filed')));
      if(n('recipes_launched')>0) did.push('launched '+n('recipes_launched')+' workstream'+s(n('recipes_launched')));
      if(n('prs_merged')>0) did.push('merged '+n('prs_merged')+' PR'+s(n('prs_merged')));
      if(n('deploys')>0) did.push('ran '+n('deploys')+' deploy'+s(n('deploys')));
      if(n('escalations')>0) did.push('escalated '+n('escalations')+' to the operator');
      const saw='saw '+n('problems')+' problem'+s(n('problems'));
      let action;
      if(did.length) action=did.join(', ');
      else if(n('held')>0) action='held '+n('held')+' (waiting on a gate)';
      else if(r.panicked) action='tick panicked — isolated';
      else action='observing, no action needed';
      return saw+' · '+action;
    }
    function overseerTickDetails(r){
      r=r||{};
      const obs=Array.isArray(r.observed_details)?r.observed_details:[];
      const act=Array.isArray(r.action_details)?r.action_details:[];
      const rows=[];
      for(const o of obs) rows.push('observed: '+esc(o));
      for(const a of act) rows.push(esc(a));
      if(!rows.length) return '';
      let html='';
      for(const row of rows){
        html+='<div style="color:#8b949e;font-size:.8rem;padding-left:1.2rem;white-space:pre-wrap">'+row+'</div>';
      }
      return html;
    }
    async function fetchOverseer(){
      const statusEl=document.getElementById('overseer-status');
      const threadsEl=document.getElementById('overseer-threads');
      const recentEl=document.getElementById('overseer-recent');
      if(!statusEl||!threadsEl||!recentEl) return;
      try {
        const d = await apiFetch('/api/overseer');
        if(d.error){
          statusEl.innerHTML='<span class="err">Failed to load Overseer activity: '+esc(d.error)+'</span>';
          threadsEl.innerHTML='';recentEl.innerHTML='';
          return;
        }
        const section=d.section||{};
        const avail=String(section.availability||'unavailable');
        const note=section.note?String(section.note):'';
        const data=section.data;
        if(!data || avail!=='ok'){
          const msg=note||'The steward has not recorded any activity yet. It starts writing after the daemon is redeployed.';
          statusEl.innerHTML='<div style="color:#9bb1c4;font-size:.9rem">'+esc(msg)+'</div>';
          threadsEl.innerHTML='<span style="color:#8b949e;font-size:.85rem">No thread status available.</span>';
          recentEl.innerHTML='<span style="color:#8b949e;font-size:.85rem">No recent activity.</span>';
          return;
        }
        const staleTag=section.freshness==='stale'?' <span style="color:#d29922">(stale)</span>':'';
        const summary=note||('Overseer: '+(data.enabled?'enabled':'disabled'));
        statusEl.innerHTML=
          '<div style="font-size:1rem;color:#c9d1d9;margin-bottom:.4rem">'+esc(summary)+staleTag+'</div>'
          +'<div style="font-size:.85rem;color:#8b949e">Runs every '+esc(overseerCadenceHuman(data.cadence_secs))
          +' · acting as '+esc(data.author_login||'—')
          +' · last check '+esc(data.last_tick_at||'never')+'</div>';

        const threads=Array.isArray(data.threads)?data.threads:[];
        if(threads.length===0){
          threadsEl.innerHTML='<span style="color:#8b949e;font-size:.85rem">Only the steward loop is running; no other operator threads reported.</span>';
        } else {
          let rows='';
          for(const t of threads){
            rows+='<tr>'
              +'<td>'+esc(t.id)+'</td>'
              +'<td>'+(t.enabled?'on':'off')+'</td>'
              +'<td>'+esc(t.last_run||'—')+'</td>'
              +'<td>'+esc(t.next_due||'—')+'</td>'
              +'<td>'+esc(t.health||'—')+'</td>'
              +'</tr>';
          }
          threadsEl.innerHTML='<table class="proc-table" data-testid="overseer-threads-table">'
            +'<thead><tr><th>Thread</th><th>Enabled</th><th>Last run</th><th>Next due</th><th>Health</th></tr></thead>'
            +'<tbody>'+rows+'</tbody></table>';
        }

        const recent=Array.isArray(data.recent)?data.recent:[];
        if(recent.length===0){
          recentEl.innerHTML='<span style="color:#8b949e;font-size:.85rem">Enabled and observing — 0 interventions so far.</span>';
        } else {
          let items='';
          for(const rec of recent){
            items+='<div style="padding:.35rem .1rem;border-bottom:1px solid #21262d;font-size:.85rem">'
              +'<span style="color:#8b949e">'+esc(rec.timestamp||'')+'</span> — '
              +'<span style="color:#c9d1d9">'+esc(overseerTickHuman(rec.report))+'</span>'
              +overseerTickDetails(rec.report)
              +'</div>';
          }
          recentEl.innerHTML='<div data-testid="overseer-recent-list">'+items+'</div>';
        }
      } catch(e) {
        statusEl.innerHTML='<span class="err">Failed to load Overseer activity: '+esc(e.message||e)+'</span>';
      }
    }

    /* --- Merge Readiness (#1880) --- */
    async function fetchMergeReadiness(){
      const el=document.getElementById('merge-readiness-panel');
      if(!el) return;
      try {
        const d = await apiFetch('/api/merge-readiness');
        const judgeConfigured = !!d.judge_configured;
        const judgeKind = String(d.judge_kind || 'unknown');
        const badgeColor = judgeConfigured ? '#3fb950' : '#f85149';
        const badgeText = judgeConfigured
          ? 'Judge: configured ('+judgeKind.toUpperCase()+')'
          : 'Judge: unconfigured — RefusingMergeJudge fallback';
        const judgeTooltip = judgeConfigured
          ? 'Agentic merge-readiness judge is wired to an LLM provider.'
          : 'No LLM provider is configured; every merge will be refused. See prompt_assets/simard/merge_readiness_judge.md.';
        const summary = d.summary || {};
        const ghError = d.gh_error ? '<div class="err" style="margin-top:.4rem;font-size:.85rem">gh error: '+esc(d.gh_error)+'</div>' : '';
        const persistenceStub = (d.verdict_persistence && d.verdict_persistence.available === false)
          ? '<div style="color:#8b949e;font-size:.75rem;margin-top:.3rem">Per-PR judge verdicts are not yet persisted; see follow-up issue.</div>'
          : '';
        const prs = Array.isArray(d.open_prs) ? d.open_prs : [];
        let rows;
        if(prs.length === 0){
          rows = '<div style="color:#8b949e;font-size:.85rem;margin-top:.5rem">No open PRs.</div>';
        } else {
          rows = '<table class="proc-table" data-testid="merge-readiness-table" style="margin-top:.5rem">'
               + '<thead><tr>'
               +   '<th>#</th><th>Title</th><th>Base</th><th>Objective</th><th>Blocker</th><th>Judge Verdict</th><th></th>'
               + '</tr></thead><tbody>'
               + prs.map(pr => {
                   const state = String(pr.readiness_state || 'unknown');
                   let badge;
                   if(state === 'ready')        badge = '<span class="ok">✓ ready</span>';
                   else if(state === 'pending') badge = '<span class="warn">⏳ pending</span>';
                   else                         badge = '<span class="err">✗ '+esc(state)+'</span>';
                   const blocker = pr.objective_blocker
                     ? '<span style="color:#8b949e;font-size:.8rem">'+esc(String(pr.objective_blocker).split('\n')[0])+'</span>'
                     : '<span style="color:#8b949e">—</span>';
                   const verdict = (pr.last_judge_verdict == null)
                     ? '<span style="color:#8b949e" title="Merge-judge verdicts are not yet persisted (follow-up issue).">verdict unavailable</span>'
                     : esc(String(pr.last_judge_verdict.verdict || ''));
                   const titleEsc = esc(String(pr.title || '').slice(0, 60));
                   return '<tr>'
                        + '<td><a href="'+esc(pr.url||'')+'" target="_blank" style="color:var(--accent)">#'+esc(String(pr.number))+'</a></td>'
                        + '<td>'+titleEsc+'</td>'
                        + '<td><code>'+esc(String(pr.base_ref_name||''))+'</code></td>'
                        + '<td>'+badge+'</td>'
                        + '<td>'+blocker+'</td>'
                        + '<td>'+verdict+'</td>'
                        + '<td><a href="'+esc(pr.url||'')+'" target="_blank" title="Open on GitHub">↗</a></td>'
                        + '</tr>';
                 }).join('')
               + '</tbody></table>';
        }
        el.innerHTML =
            '<div data-testid="merge-readiness-judge-pill" style="display:flex;align-items:center;gap:.5rem;flex-wrap:wrap">'
          +   '<span class="badge" style="background:'+badgeColor+'22;color:'+badgeColor+'" title="'+esc(judgeTooltip)+'">'+esc(badgeText)+'</span>'
          +   '<span style="color:#8b949e;font-size:.85rem">'
          +     esc(String(summary.objective_ready||0))+' ready · '
          +     esc(String(summary.objective_pending||0))+' pending · '
          +     esc(String(summary.objective_blocked||0))+' blocked · '
          +     esc(String(summary.total_open||0))+' open'
          +   '</span>'
          + '</div>'
          + ghError
          + rows
          + persistenceStub;
      } catch(e) {
        el.innerHTML = '<span class="err">Failed to load merge readiness: '+esc(e.message||e)+'</span>';
      }
    }

    /* --- Background tab prefetch / refresh scheduler (#2649) ----------------
       Every canonical tab loads on page open and stays on its own persistent,
       visibility-gated background refresh, so switching tabs renders already-
       fetched data instead of a slow on-activate fetch. This replaces the
       retired runTabFetches slug-branch chain. Full design:
       docs/reference/dashboard-background-tab-prefetch.md */

    /* Rate-safety controls — they protect the local daemon from a self-
       inflicted request storm, so treat them as a safety limit, not a tweak. */
    const MAX_CONCURRENCY=3;   /* at most 3 background loaders in flight at once */
    const STAGGER_MS=150;      /* minimum delay between dispatching queued loaders */

    /* Single source of truth: slug -> [{fn, intervalMs, paths}]. Only list /
       summary loaders are registered; interactive surfaces (the Workers live
       PTY, the Chat live attach) are deliberately excluded so the scheduler
       never auto-opens a stream nobody is watching. A slug with no entry is
       silently skipped, so this survives tab-set churn without hardcoding. */
    const TAB_LOADERS={
      'overview':[{fn:fetchStatusSnapshot,intervalMs:30000,paths:['/api/status/snapshot']}],
      'goals':[{fn:fetchGoals,intervalMs:30000,paths:['/api/goals']},{fn:fetchWorkboard,intervalMs:30000,paths:['/api/workboard']}],
      'activity':[{fn:fetchLogs,intervalMs:15000,paths:['/api/logs']},{fn:fetchTraces,intervalMs:30000,paths:['/api/traces']},{fn:fetchThinking,intervalMs:30000,paths:['/api/ooda-thinking']},{fn:fetchOodaCycles,intervalMs:30000,paths:['/api/ooda-cycles']},{fn:fetchBrainFailures,intervalMs:30000,paths:['/api/brain-failures']}],
      'workers':[{fn:fetchSubagentSessions,intervalMs:5000,paths:['/api/subagent-sessions']},{fn:fetchTmuxSessions,intervalMs:10000,paths:['/api/azlin/tmux-sessions']},{fn:fetchProcessTree,intervalMs:15000,paths:['/api/processes']}],
      'pull-requests':[{fn:fetchMergeJudge,intervalMs:30000,paths:['/api/merge-judge']},{fn:fetchPrReadiness,intervalMs:30000,paths:['/api/prs']}],
      'resources':[{fn:fetchRecentMemories,intervalMs:120000,paths:['/api/memory/recent']},{fn:fetchMemoryHistory,intervalMs:120000,paths:['/api/memory/history']},{fn:fetchCosts,intervalMs:120000,paths:['/api/costs']}],
      'chat':[{fn:loadChatSessions,intervalMs:120000,paths:['/api/chat/sessions']}],
      'overseer':[{fn:fetchOverseer,intervalMs:30000,paths:['/api/overseer']}],
      'journal':[{fn:loadJournal,intervalMs:120000,paths:['/api/journal/dates']}],
      'creative-ideas':[{fn:loadCreativeIdeas,intervalMs:120000,paths:['/api/creative-ideas']}],
      'memory':[{fn:fetchMemoryGraph,intervalMs:120000,paths:['/api/memory/graph']},{fn:fetchMemory,intervalMs:120000,paths:['/api/memory']}]
    };
    window.TAB_LOADERS=TAB_LOADERS;

    let backgroundFetchers=[];   /* flattened {slug, fn, intervalMs, paths} list */
    let backgroundTimers={};     /* timer key -> setTimeout/setInterval id */
    let backgroundActive=false;  /* false while the browser tab is hidden */
    const tabInflight={};        /* slug -> count of in-flight loaders */

    /* Flatten TAB_LOADERS over CANONICAL_TABS; a slug absent from the registry
       (a not-yet-wired tab) is skipped, not an error. */
    function collectBackgroundFetchers(){
      const out=[];
      for(let i=0;i<CANONICAL_TABS.length;i++){
        const slug=CANONICAL_TABS[i];
        const loaders=TAB_LOADERS[slug];
        if(!loaders)continue;
        for(let j=0;j<loaders.length;j++){
          const l=loaders[j];
          if(typeof l.fn!=='function')continue;
          out.push({slug:slug,fn:l.fn,intervalMs:l.intervalMs,paths:l.paths||[]});
        }
      }
      return out;
    }

    /* Inject (once) a subtle per-tab "Updated <relative>" element so the
       operator can always see how fresh each panel is — no silent staleness.
       textContent only, never innerHTML, so it can never become a markup sink. */
    function ensureFreshnessEl(slug){
      const panel=document.getElementById('tab-'+slug);
      if(!panel)return null;
      let el=panel.querySelector('[data-testid="'+slug+'-updated"]');
      if(!el){
        el=document.createElement('span');
        el.setAttribute('data-testid',slug+'-updated');
        el.className='tab-freshness';
        el.style.cssText='display:block;color:#8b949e;font-size:.75rem;margin:.1rem 0 .5rem';
        const h1=panel.querySelector('h1');
        if(h1&&h1.parentNode)h1.parentNode.insertBefore(el,h1.nextSibling);
        else panel.insertBefore(el,panel.firstChild);
      }
      return el;
    }
    function tabMinLastOk(slug){
      const loaders=TAB_LOADERS[slug];
      if(!loaders)return 0;
      let min=0;
      for(let i=0;i<loaders.length;i++){
        const paths=loaders[i].paths||[];
        for(let j=0;j<paths.length;j++){
          const t=lastOk[paths[j]];
          if(t&&(min===0||t<min))min=t;
        }
      }
      return min;
    }
    function updateFreshness(slug){
      const el=ensureFreshnessEl(slug);
      if(!el)return;
      if(tabInflight[slug]>0){el.textContent='refreshing…';return;}
      const t=tabMinLastOk(slug);
      el.textContent=t?('Updated '+timeAgo(t)):'Updated —';
    }

    /* Run one loader, tracking its tab's in-flight state for the freshness
       badge. Loaders render their own in-panel error state, so a rejection here
       is a safety net: it is logged (slug only, no bodies) and swallowed so a
       single failing loader cannot stall the initial-wave queue drain. */
    function runFetcher(f){
      tabInflight[f.slug]=(tabInflight[f.slug]||0)+1;
      updateFreshness(f.slug);
      let p;
      try{p=Promise.resolve(f.fn());}catch(e){p=Promise.reject(e);}
      return p.catch(function(e){
        if(window.console&&console.warn)console.warn('background loader for "'+f.slug+'" failed');
      }).then(function(){
        tabInflight[f.slug]=Math.max(0,(tabInflight[f.slug]||1)-1);
        updateFreshness(f.slug);
      });
    }

    /* Drain the initial prefetch wave: at most MAX_CONCURRENCY loaders in
       flight, dispatched no faster than one per STAGGER_MS, in nav order, so
       the daemon never sees every endpoint fire at once. */
    function drainInitialWave(fetchers){
      let idx=0,active=0;
      function pump(){
        while(active<MAX_CONCURRENCY&&idx<fetchers.length){
          const f=fetchers[idx++];
          active++;
          runFetcher(f).then(function(){active--;pump();});
          if(idx<fetchers.length){setTimeout(pump,STAGGER_MS);break;}
        }
      }
      pump();
    }

    /* Arm one persistent, phase-jittered interval per fetcher. Jitter spreads
       the ticks so intervals do not realign into a thundering herd. Idempotent:
       clears any existing timers first so re-arming cannot leak. */
    function armBackgroundIntervals(){
      suspendBackgroundIntervals();
      backgroundActive=true;
      for(let i=0;i<backgroundFetchers.length;i++){
        armOneFetcher(backgroundFetchers[i],'bg'+i);
      }
    }
    function armOneFetcher(f,key){
      const jitter=Math.floor(Math.random()*Math.min(750,Math.max(1,f.intervalMs/6)));
      backgroundTimers[key]=setTimeout(function(){
        if(!backgroundActive)return;
        backgroundTimers[key]=setInterval(function(){
          if(backgroundActive)runFetcher(f);
        },f.intervalMs);
      },jitter);
    }
    function suspendBackgroundIntervals(){
      backgroundActive=false;
      for(const k in backgroundTimers){
        clearTimeout(backgroundTimers[k]);
        clearInterval(backgroundTimers[k]);
      }
      backgroundTimers={};
    }

    /* Immediate, non-blocking refresh of one tab's loaders (used on activate
       and on return-to-visible). Shares apiFetch's in-flight de-dupe, so it
       never double-hits an endpoint a background tick is already fetching. */
    function refreshTab(slug){
      const loaders=TAB_LOADERS[slug];
      if(!loaders)return;
      for(let i=0;i<loaders.length;i++){
        const l=loaders[i];
        if(typeof l.fn==='function')runFetcher({slug:slug,fn:l.fn,paths:l.paths||[]});
      }
    }

    /* Single global visibility gate (one gate, not per-tab): a hidden browser
       tab does no background refresh; on return to visible, re-arm every
       interval and immediately refresh whatever tab the operator is viewing. */
    function handleVisibilityChange(){
      if(document.visibilityState==='hidden'){
        suspendBackgroundIntervals();
      }else{
        armBackgroundIntervals();
        refreshTab(activeTab);
      }
    }

    function startBackgroundScheduler(){
      backgroundFetchers=collectBackgroundFetchers();
      document.addEventListener('visibilitychange',handleVisibilityChange);
      if(document.visibilityState==='hidden'){
        /* Opened in a hidden browser tab: do not prefetch or arm timers yet;
           the visibility gate will start everything when it becomes visible. */
        backgroundActive=false;
        return;
      }
      drainInitialWave(backgroundFetchers);
      armBackgroundIntervals();
    }

    /* --- Init --- */
    fetchStatus(); fetchIssues(); fetchDistributed(); fetchAgentOverview(); fetchMergeReadiness(); fetchRecallPrecision();
    setInterval(fetchAgentOverview,30000);
    setInterval(fetchMergeReadiness,30000);
    setInterval(fetchStatus,30000);
    setInterval(fetchRecallPrecision,30000);
    setInterval(fetchIssues,120000);
    /* #2649: overview's status snapshot is now owned by the background scheduler
       (TAB_LOADERS['overview']) — it is prefetched on page load and kept on a
       persistent, visibility-gated 30s refresh, so it is no longer armed here. */
    startBackgroundScheduler();

    /* --- Glossary / Jargon tooltips (#1996) --- */
    const GLOSSARY={
      'decision cycle':'Observe-Orient-Decide-Act — the decision-making loop Simard runs each cycle to decide what to do next.',
      'coordinator':'The meeting coordinator component that manages discussions between Simard and the user, extracting goals and action items.',
      'memory compaction':'The process of merging short-term observations into long-term memory, strengthening important facts and pruning noise.',
      'recipe runner':'The workflow engine that executes multi-step automation recipes (build, test, deploy sequences) as part of goal work.',
      'launched sub-agent':'An action where Simard launches a sub-agent in a separate process to work on a specific task (e.g., fixing a bug or writing code).',
      'agent memory':'Simard\u2019s multi-layered memory system: sensory (raw input), working (active context), event memories (past events), semantic (learned facts), procedural (how-to), and prospective (reminders).',
      'memory store':'The built-in graph database Simard uses to persist memories across sessions, organised by memory type.',
      'semantic fact':'A learned piece of knowledge stored in long-term memory, like a concept or relationship Simard has observed.',
      'event memory':'A record of a specific past event — what happened, when, and the outcome.',
      'procedural memory':'How-to knowledge — step-by-step procedures Simard has learned for completing tasks.',
      'prospective memory':'A planned future action or reminder that Simard intends to act on later.',
      'working memory':'Short-term context currently being used — the facts and plans relevant to whatever Simard is working on right now.',
      'sensory buffer':'Raw, unprocessed recent observations before they are categorised into other memory types.',
      'goal board':'The prioritised list of active goals and backlog items that Simard is tracking.',
      'backlog':'Goals queued for later — not actively being worked on, but available to promote to active status.',
      'hive mind':'Multi-host synchronisation: sharing knowledge across multiple Simard instances running on different machines.',
      'daemon':'The background process that runs Simard\u2019s autonomous decision-making loop continuously.',
      'cycle':'One complete pass through the decision loop — observe the environment, orient priorities, decide on an action, and act on it.',
      'gym':'A training environment where Simard practices and improves its skills on synthetic scenarios.',
      'OTEL':'OpenTelemetry — an open standard for collecting distributed traces, metrics, and logs from software systems.',
      'OpenTelemetry':'An open-source observability framework for generating, collecting, and exporting telemetry data (traces, metrics, logs) from distributed systems.',
    };
    function toggleGlossary(){
      const p=document.getElementById('glossary-panel');
      p.classList.toggle('open');
    }
    function annotateJargon(el){
      if(!el)return;
      const walker=document.createTreeWalker(el,NodeFilter.SHOW_TEXT,null);
      const nodes=[];
      while(walker.nextNode()) nodes.push(walker.currentNode);
      const terms=Object.keys(GLOSSARY).sort((a,b)=>b.length-a.length);
      const pattern=new RegExp('\\b('+terms.map(t=>t.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')).join('|')+')\\b','gi');
      nodes.forEach(node=>{
        if(node.parentElement&&(node.parentElement.tagName==='ABBR'||node.parentElement.tagName==='SCRIPT'||node.parentElement.tagName==='STYLE'||node.parentElement.tagName==='CODE'||node.parentElement.tagName==='INPUT'||node.parentElement.tagName==='TEXTAREA'))return;
        const text=node.textContent;
        if(!pattern.test(text))return;
        pattern.lastIndex=0;
        const frag=document.createDocumentFragment();
        let lastIdx=0;
        let match;
        while((match=pattern.exec(text))!==null){
          if(match.index>lastIdx) frag.appendChild(document.createTextNode(text.slice(lastIdx,match.index)));
          const abbr=document.createElement('abbr');
          const key=Object.keys(GLOSSARY).find(k=>k.toLowerCase()===match[1].toLowerCase())||match[1];
          abbr.title=GLOSSARY[key]||'';
          abbr.textContent=match[0];
          frag.appendChild(abbr);
          lastIdx=pattern.lastIndex;
        }
        if(lastIdx<text.length) frag.appendChild(document.createTextNode(text.slice(lastIdx)));
        if(frag.childNodes.length>1) node.parentElement.replaceChild(frag,node);
      });
    }
    // Run jargon annotation after each tab switch and data fetch
    const origTabClickHandlers=[];
    document.querySelectorAll('.tab').forEach(tab=>{
      const origClick=tab.onclick;
      tab.addEventListener('click',()=>{
        setTimeout(()=>annotateJargon(document.querySelector('.tab-content.active')),300);
      });
    });
    // Annotate overview on first load
    setTimeout(()=>annotateJargon(document.querySelector('.tab-content.active')),500);
  </script>

  <div id="glossary-panel" class="glossary-panel">
    <h3>Glossary <button class="close-btn" onclick="toggleGlossary()">&times;</button></h3>
    <p style="color:#8b949e;font-size:.8rem;margin-bottom:1rem">Hover any <abbr title="Example tooltip">dotted-underlined term</abbr> in the dashboard for a quick explanation, or browse the full list below.</p>
    <dl id="glossary-list"></dl>
  </div>
  <script>
    (function(){
      const dl=document.getElementById('glossary-list');
      if(!dl)return;
      Object.keys(GLOSSARY).sort().forEach(term=>{
        const entry=document.createElement('div');
        entry.className='glossary-entry';
        entry.innerHTML='<dt>'+esc(term)+'</dt><dd>'+esc(GLOSSARY[term])+'</dd>';
        dl.appendChild(entry);
      });
    })();
  </script>
</body>
</html>
"#;
