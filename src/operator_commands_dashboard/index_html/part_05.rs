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

    /* --- Init --- */
    fetchStatus(); fetchIssues(); fetchDistributed(); fetchAgentOverview(); fetchMergeReadiness();
    setInterval(fetchAgentOverview,30000);
    setInterval(fetchMergeReadiness,30000);
    setInterval(fetchStatus,30000);
    setInterval(fetchIssues,120000);
    /* Hash-based tab activation (supports /glossary → /#glossary deep link) */
    (function(){
      const hash=window.location.hash.replace('#','');
      if(hash){const tab=document.querySelector('.tab[data-tab="'+hash+'"]');if(tab)tab.click();}
    })();
  </script>
</body>
</html>
"#;
