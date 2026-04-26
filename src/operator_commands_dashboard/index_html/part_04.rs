pub(crate) const PART_04: &str = r#"            let fmt;
            if(typeof v==='number'){
              if(isCost) fmt='$'+v.toFixed(4);
              else if(isTokens) fmt=v.toLocaleString()+' tokens';
              else fmt=v.toLocaleString();
            }else{fmt=String(v);}
            return `<div class="stat"><span class="label">${esc(fmtLabel(k))}</span><span class="value">${fmt}</span></div>`;
          }).join('');
        }
        document.getElementById('costs-daily').innerHTML=renderSummary(d.daily);
        document.getElementById('costs-weekly').innerHTML=renderSummary(d.weekly);
      }catch(e){document.getElementById('costs-daily').innerHTML='<span class="err">Failed to load cost data</span>';}
    }
    async function fetchBudget(){
      try{
        const d=await apiFetch('/api/budget');
        document.getElementById('budget-daily').value=d.daily_budget_usd||500;
        document.getElementById('budget-weekly').value=d.weekly_budget_usd||2500;
      }catch(e){}
    }
    async function saveBudget(){
      const daily=parseFloat(document.getElementById('budget-daily').value)||500;
      const weekly=parseFloat(document.getElementById('budget-weekly').value)||2500;
      try{
        const d=await apiFetch('/api/budget',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({daily_budget_usd:daily,weekly_budget_usd:weekly})});
        const el=document.getElementById('budget-status');
        el.textContent=d.status==='ok'?'✓ Saved':'Error: '+(d.error||'unknown');
        el.style.color=d.status==='ok'?'var(--green)':'var(--red)';
        setTimeout(()=>{el.textContent='';el.style.color='';},3000);
      }catch(e){document.getElementById('budget-status').textContent='Network error';}
    }
    fetchBudget();

    /* --- Chat --- */
    let ws=null,chatInit=false;
    function initChat(){
      if(ws){try{ws.close();}catch(e){}}
      chatInit=true;
      const proto=location.protocol==='https:'?'wss:':'ws:';
      ws=new WebSocket(`${proto}//${location.host}/ws/chat`);
      const st=document.getElementById('ws-status');
      st.innerHTML='<span style="color:var(--yellow)">● Connecting…</span>';
      ws.onopen=()=>{st.innerHTML='<span style="color:var(--green)">● Connected</span>';};
      ws.onclose=()=>{
        st.innerHTML='<span style="color:var(--red)">● Disconnected</span> <button class="btn" onclick="initChat()" style="font-size:.75rem;padding:.1rem .4rem;margin-left:.5rem">Reconnect</button>';
        chatInit=false;removeTypingIndicator();setChatBusy(false);
      };
      ws.onerror=()=>{
        st.innerHTML='<span style="color:var(--red)">● Error</span> <button class="btn" onclick="initChat()" style="font-size:.75rem;padding:.1rem .4rem;margin-left:.5rem">Retry</button>';
        removeTypingIndicator();setChatBusy(false);
      };
      ws.onmessage=ev=>{removeTypingIndicator();setChatBusy(false);try{const m=JSON.parse(ev.data);appendMsg(m.role||'system',m.content||ev.data);}catch(ex){appendMsg('system',ev.data);}};
    }
    function sendChat(){
      const inp=document.getElementById('chat-input'); const txt=inp.value.trim();
      if(!txt) return;
      if(!ws||ws.readyState!==WebSocket.OPEN){
        appendMsg('system','Not connected. Click Reconnect to establish a session.');
        return;
      }
      appendMsg('user',txt); ws.send(txt); inp.value='';
      showTypingIndicator(); setChatBusy(true);
    }
    function showTypingIndicator(){
      removeTypingIndicator();
      const el=document.getElementById('chat-messages');
      const div=document.createElement('div');
      div.id='typing-indicator';
      div.className='chat-msg';
      div.innerHTML='<span class="role assistant">simard:</span> <span class="typing-dots"><span>.</span><span>.</span><span>.</span></span>';
      el.appendChild(div);
      el.scrollTop=el.scrollHeight;
    }
    function removeTypingIndicator(){
      const ind=document.getElementById('typing-indicator');
      if(ind) ind.remove();
    }
    function setChatBusy(busy){
      document.getElementById('chat-send').disabled=busy;
      document.getElementById('chat-input').disabled=busy;
    }
    function appendMsg(role,content){
      const el=document.getElementById('chat-messages');
      const div=document.createElement('div');
      div.className='chat-msg';
      const roleSpan=document.createElement('span');
      roleSpan.className='role '+role;
      roleSpan.textContent=role+':';
      div.appendChild(roleSpan);
      div.appendChild(document.createTextNode(' '+content));
      el.appendChild(div);
      el.scrollTop=el.scrollHeight;
    }
    document.getElementById('chat-input').addEventListener('keydown',e=>{
      if(e.key==='Enter'&&!e.shiftKey){e.preventDefault();sendChat();}
    });


    /* --- Workboard --- */
    const phaseColors={act:'var(--green)',orient:'var(--yellow)',observe:'var(--accent)',decide:'#a371f7',sleep:'#8b949e',unknown:'#8b949e'};
    function fmtDuration(s){if(s<60)return s+'s';const m=Math.floor(s/60);if(m<60)return m+'m '+s%60+'s';const h=Math.floor(m/60);return h+'h '+m%60+'m';}
    function wbGoalCard(g){
      const pct=g.progress_pct||0;
      const barColor=g.status==='done'?'var(--green)':g.status.startsWith('blocked')?'var(--red)':'var(--accent)';
      return`<div style="background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:.6rem;margin-bottom:.5rem">
        <div style="font-weight:600;font-size:.85rem;margin-bottom:.3rem">${esc(g.name)}</div>
        <div style="font-size:.75rem;color:#8b949e;margin-bottom:.4rem">${esc(g.description||'')}</div>
        <div style="background:#21262d;border-radius:3px;height:6px;margin-bottom:.3rem">
          <div style="background:${barColor};height:100%;border-radius:3px;width:${pct}%;transition:width .3s"></div>
        </div>
        <div style="font-size:.7rem;color:#8b949e">${pct}% complete${g.assigned_to?' · '+esc(g.assigned_to):''}</div>
      </div>`;
    }
    async function fetchWorkboard(){
      try{
        const d=await apiFetch('/api/workboard');
        // Header
        const phase=d.cycle?.phase||'unknown';
        document.getElementById('wb-phase-dot').style.background=phaseColors[phase]||phaseColors.unknown;
        document.getElementById('wb-cycle-label').textContent='Cycle #'+(d.cycle?.number||'—');
        document.getElementById('wb-phase-label').textContent=phase;
        document.getElementById('wb-uptime').textContent=fmtDuration(d.uptime_seconds||0);
        document.getElementById('wb-eta').textContent=d.next_cycle_eta_seconds>0?fmtDuration(d.next_cycle_eta_seconds):'now';
        // Kanban columns
        const cols={queued:[],in_progress:[],blocked:[],done:[]};
        (d.goals||[]).forEach(g=>{
          if(g.status==='done') cols.done.push(g);
          else if(g.status==='queued') cols.queued.push(g);
          else if(g.status.startsWith('blocked')) cols.blocked.push(g);
          else cols.in_progress.push(g);
        });
        document.getElementById('wb-col-queued').innerHTML=cols.queued.length?cols.queued.map(wbGoalCard).join(''):'<span style="color:#8b949e;font-size:.8rem">—</span>';
        document.getElementById('wb-col-inprogress').innerHTML=cols.in_progress.length?cols.in_progress.map(wbGoalCard).join(''):'<span style="color:#8b949e;font-size:.8rem">—</span>';
        document.getElementById('wb-col-blocked').innerHTML=cols.blocked.length?cols.blocked.map(wbGoalCard).join(''):'<span style="color:#8b949e;font-size:.8rem">—</span>';
        document.getElementById('wb-col-done').innerHTML=cols.done.length?cols.done.map(wbGoalCard).join(''):'<span style="color:#8b949e;font-size:.8rem">—</span>';
        // Engineers
        if(d.spawned_engineers?.length){
          document.getElementById('wb-engineers').innerHTML=d.spawned_engineers.map(e=>{
            const sc=e.alive?'ok':'err';
            return`<div style="display:flex;align-items:center;gap:.75rem;padding:.4rem 0;border-bottom:1px solid var(--border)">
              <span class="${sc}" style="font-weight:600">PID ${e.pid}</span>
              <span style="flex:1">${esc(e.task)}</span>
              <span class="${sc}" style="font-size:.8rem">${e.alive?'alive':'exited'}</span>
              <span style="color:#8b949e;font-size:.75rem">${timeAgo(e.started_at)}</span>
            </div>`;
          }).join('');
        }else{document.getElementById('wb-engineers').innerHTML='<span style="color:#8b949e;font-size:.85rem">No spawned engineers</span>';}
        // Recent actions timeline
        if(d.recent_actions?.length){
          document.getElementById('wb-actions').innerHTML=d.recent_actions.map(a=>{
            const isCurrent=a.action==='current';
            return`<div style="display:flex;gap:.5rem;padding:.35rem 0;border-bottom:1px solid var(--border);font-size:.85rem">
              <span style="color:var(--accent);min-width:2.5rem;font-weight:600">#${a.cycle}</span>
              <span style="min-width:5rem;color:${isCurrent?'var(--green)':'#8b949e'}">${esc(a.action)}</span>
              <span style="flex:1">${renderActionDetail(a.result)}</span>
              ${a.at?'<span style="color:#8b949e;font-size:.75rem">'+timeAgo(a.at)+'</span>':''}
            </div>`;
          }).join('');
        }else{document.getElementById('wb-actions').innerHTML='<span style="color:#8b949e;font-size:.85rem">No recent actions</span>';}
        // Task memory (rich facts)
        const tm=d.task_memory||{};
        document.getElementById('wb-facts-count').textContent=(tm.facts_count||0)+' facts';
        if(tm.recent_facts?.length){
          document.getElementById('wb-facts-list').innerHTML=tm.recent_facts.map(f=>{
            const conf=typeof f.confidence==='number'?(' <span style="color:#8b949e;font-size:.75rem">('+Math.round(f.confidence*100)+'%)</span>'):'';
            const tags=(f.tags||[]).map(t=>'<span style="background:var(--border);padding:0 .3rem;border-radius:3px;font-size:.7rem;margin-left:.3rem">'+esc(t)+'</span>').join('');
            return'<div style="padding:.25rem 0;border-bottom:1px solid var(--border)"><strong style="color:var(--accent);font-size:.8rem">'+esc(f.concept||'')+'</strong>'+conf+tags+'<div>'+esc(f.content||'')+'</div></div>';
          }).join('');
        }else{document.getElementById('wb-facts-list').innerHTML='<span style="color:#8b949e">No recent facts in memory</span>';}
        // Working memory
        const wm=d.working_memory||[];
        document.getElementById('wb-wm-count').textContent=wm.length+' slots';
        if(wm.length){
          document.getElementById('wb-wm-list').innerHTML=wm.map(s=>{
            return'<div style="padding:.25rem 0;border-bottom:1px solid var(--border)"><span style="color:var(--accent);font-weight:600;font-size:.8rem">'+esc(s.slot_type)+'</span> <span style="color:#8b949e;font-size:.75rem">['+esc(s.task_id)+'] rel='+((s.relevance||0).toFixed(2))+'</span><div>'+esc(s.content)+'</div></div>';
          }).join('');
        }else{document.getElementById('wb-wm-list').innerHTML='<span style="color:#8b949e">No active working memory</span>';}
        // Cognitive statistics
        const cs=d.cognitive_statistics;
        if(cs){
          document.getElementById('wb-cog-stats').innerHTML=[
            ['Sensory',cs.sensory_count],['Working',cs.working_count],['Episodic',cs.episodic_count],
            ['Semantic',cs.semantic_count],['Procedural',cs.procedural_count],['Prospective',cs.prospective_count],['Total',cs.total]
          ].map(([k,v])=>'<span style="margin-right:1rem"><strong>'+k+':</strong> '+(v||0)+'</span>').join('');
        }else{document.getElementById('wb-cog-stats').innerHTML='<span style="color:#8b949e">No cognitive memory available</span>';}
      }catch(e){document.getElementById('wb-engineers').innerHTML='<span class="err">Failed to load workboard data</span>';}
    }

    /* --- Thinking --- */
    async function fetchThinking(){
      try{
        const d=await apiFetch('/api/ooda-thinking');
        const el=document.getElementById('thinking-timeline');
        if(!d.reports?.length){el.innerHTML='<span style="color:#8b949e">No cycle reports yet. The OODA daemon generates these during autonomous work.</span>';return;}
        el.innerHTML=d.reports.map(rpt=>{
          if(rpt.legacy){
            return `<div class="thinking-cycle legacy">
              <div class="cycle-header"><span class="cycle-num">Cycle #${rpt.cycle_number}</span><span class="cycle-badge">legacy</span></div>
              <div class="cycle-summary">${esc(rpt.summary)}</div>
            </div>`;
          }
          const phases=[];
          if(rpt.observation){
            const obs=rpt.observation;
            phases.push(`<div class="phase observe">
              <div class="phase-label">👁 Observe</div>
              <div class="phase-content">
                <div>${obs.goal_count} goals tracked</div>
                ${obs.goals?.map(g=>`<div class="goal-line">• ${esc(g.id)}: ${esc(g.progress)}</div>`).join('')||''}
                ${obs.gym_health?`<div>Gym: ${(obs.gym_health.pass_rate*100).toFixed(0)}% pass rate (${obs.gym_health.scenario_count} scenarios)</div>`:''}
                ${obs.environment?`<div>Env: ${obs.environment.open_issues} issues, ${obs.environment.recent_commits} recent commits${obs.environment.git_status?'':' (clean)'}</div>`:''}
              </div>
            </div>`);
          }
          if(rpt.priorities?.length){
            phases.push(`<div class="phase orient">
              <div class="phase-label">🧭 Orient</div>
              <div class="phase-content">
                ${rpt.priorities.map(p=>`<div class="priority-line">
                  <span class="urgency" style="color:${p.urgency>0.7?'var(--red)':p.urgency>0.4?'var(--yellow)':'var(--green)'}">●</span>
                  <strong>${esc(p.goal_id)}</strong> (urgency: ${p.urgency.toFixed(2)}) — ${esc(p.reason)}
                </div>`).join('')}
              </div>
            </div>`);
          }
          if(rpt.planned_actions?.length){
            phases.push(`<div class="phase decide">
              <div class="phase-label">🎯 Decide</div>
              <div class="phase-content">
                ${rpt.planned_actions.map(a=>`<div>→ <code>${esc(a.kind)}</code> ${a.goal_id?'['+esc(a.goal_id)+']':''} ${esc(a.description)}</div>`).join('')}
              </div>
            </div>`);
          }
          if(rpt.outcomes?.length){
            phases.push(`<div class="phase act">
              <div class="phase-label">⚡ Act</div>
              <div class="phase-content">
                ${rpt.outcomes.map(o=>{
                  const se=o.spawn_engineer;
                  let seBlock='';
                  if(se){
                    const statusColor=se.status==='live'?'var(--green)':se.status==='skipped'?'var(--yellow)':se.status==='denied'?'var(--yellow)':'var(--red)';
                    const agent=se.subordinate_agent;
                    const agentLink=agent?`<a href='javascript:void(0)' onclick="openAgentLog('${esc(agent)}');return false;"><code>${esc(agent)}</code></a>`:'<em>(no agent)</em>';
                    seBlock=`<div class="spawn-engineer-block" style="margin-top:.35rem;padding:.4rem .55rem;border-left:3px solid ${statusColor};background:rgba(255,255,255,0.03);border-radius:4px">
                      <div><span style="color:${statusColor}">●</span> <strong>spawn_engineer</strong> · ${esc(se.last_action||'')} · <span style="color:${statusColor}">${esc(se.status||'')}</span></div>
                      <div>subordinate: ${agentLink}${se.goal_id?` · goal <code>${esc(se.goal_id)}</code>`:''}</div>
                      ${se.task_summary?`<div>task: ${esc(se.task_summary)}</div>`:''}
                    </div>`;
                  }
                  const det=o.detail||'';
                  const detLow=det.toLowerCase();
                  const hasArtifact=detLow.indexOf('pr #')>=0||detLow.indexOf('commit')>=0;
                  const isAssessmentOnly=detLow.indexOf('assessed')>=0&&detLow.indexOf('verified=0')>=0;
                  const linkIcon=hasArtifact?'<span style="color:#2ea043;margin-right:4px" title="produced artifact">🔗</span>':'';
                  const assessBadge=(!hasArtifact&&isAssessmentOnly)?' <span class="badge-assessment" style="background:#fb8500;color:#fff;padding:1px 6px;border-radius:3px;font-size:11px;margin-left:6px">assessment only</span>':'';
                  return `<div class="outcome ${o.success?'success':'failure'}">
                    ${o.success?'✅':'❌'} <code>${esc(o.action_kind)}</code> — ${esc(o.action_description)}${assessBadge}
                    <div class="outcome-detail">${linkIcon}${esc(det.substring(0,300))}${det.length>300?'…':''}</div>
                    ${seBlock}
                  </div>`;
                }).join('')}
              </div>
            </div>`);
          }
          return `<div class="thinking-cycle">
            <div class="cycle-header">
              <span class="cycle-num">Cycle #${rpt.cycle_number}</span>
              <span class="cycle-summary-inline">${esc(rpt.summary||'')}</span>
            </div>
            <div class="cycle-phases">${phases.join('')}</div>
          </div>`;
        }).join('');
      }catch(e){document.getElementById('thinking-timeline').innerHTML='<span class="err">Failed to load: '+esc(e.toString())+'</span>';}
    }

    /* --- Agent log terminal (issue #947) --- */
    let agentLogTerm = null;
    let agentLogWS = null;
    /* Issue #946: jump from a Thinking-tab spawn_engineer outcome straight to
       the agent terminal viewer. Switches tabs, populates the agent-name
       input, and clicks Connect. */
    function openAgentLog(name){
      const tab = document.querySelector('.tab[data-tab="terminal"]');
      if(tab) tab.click();
      const input = document.getElementById('agent-log-name');
      if(input) input.value = name || '';
      // initAgentLogTerminal is invoked by the tab click handler; defer
      // connect a tick so xterm has been mounted.
      setTimeout(()=>{ try{ connectAgentLog(); }catch(e){} }, 50);
    }
    function setAgentLogStatus(text, color){
      const el = document.getElementById('agent-log-status');
      if(!el) return;
      el.textContent = text;
      el.style.color = color || '#8b949e';
    }
    function initAgentLogTerminal(){
      if(agentLogTerm) return;
      if(typeof Terminal === 'undefined'){
        setAgentLogStatus('xterm.js failed to load (CDN unreachable)', '#f85149');
        return;
      }
      agentLogTerm = new Terminal({
        convertEol: true,
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
        fontSize: 13,
        theme: { background: '#000000', foreground: '#c9d1d9' },
      });
      agentLogTerm.open(document.getElementById('xterm-host'));
    }
    function connectAgentLog(){
      initAgentLogTerminal();
      if(!agentLogTerm) return;
      const raw = (document.getElementById('agent-log-name').value || '').trim();
      // Client-side allow-list mirrors the server sanitizer (^[A-Za-z0-9_-]{1,64}$).
      if(!/^[A-Za-z0-9_-]{1,64}$/.test(raw)){
        setAgentLogStatus('invalid agent name (allowed: letters, digits, _ and -, up to 64 chars)', '#f85149');
        return;
      }
      if(agentLogWS){ try { agentLogWS.close(); } catch(_) {} agentLogWS = null; }
      agentLogTerm.clear();
      const proto = (window.location.protocol === 'https:') ? 'wss:' : 'ws:';
      const url = proto + '//' + window.location.host + '/ws/agent_log/' + encodeURIComponent(raw);
      setAgentLogStatus('connecting…', '#d29922');
      let ws;
      try { ws = new WebSocket(url); }
      catch(e){ setAgentLogStatus('connect failed: ' + (e && e.message || e), '#f85149'); return; }
      agentLogWS = ws;
      ws.onopen = () => setAgentLogStatus('● connected to ' + raw, '#3fb950');
      ws.onmessage = (ev) => {
        // Plain text frames; one frame per line (server already stripped \n).
        if(typeof ev.data === 'string' && agentLogTerm){ agentLogTerm.writeln(ev.data); }
      };
      ws.onerror = () => setAgentLogStatus('socket error', '#f85149');
      ws.onclose = () => { setAgentLogStatus('disconnected', '#8b949e'); if(agentLogWS === ws) agentLogWS = null; };
    }
    function disconnectAgentLog(){
      if(agentLogWS){ try { agentLogWS.close(); } catch(_) {} agentLogWS = null; }
      setAgentLogStatus('disconnected', '#8b949e');
    }

    /* --- Azlin tmux sessions panel (WS-1) --- */
    function fmtUnixTs(ts){
      if(typeof ts !== 'number' || !isFinite(ts) || ts <= 0) return '—';
      try { return new Date(ts*1000).toLocaleString(); } catch(_) { return String(ts); }
    }
    async function fetchTmuxSessions(){
      const body = document.getElementById('tmux-sessions-body');
      if(!body) return;"#;
