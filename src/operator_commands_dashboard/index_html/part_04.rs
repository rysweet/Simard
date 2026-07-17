pub(crate) const PART_04: &str = r#"            let fmt;
            if(isPeriod){fmt=humanizePeriod(String(v));}
            else if(typeof v==='number'){
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
    // Persist the active chat session id across page reloads so the
    // conversation stays continuous (issue #2581). Without this the id lived
    // only in memory, so any reload reset it to null and silently started a
    // fresh, empty session — the agent "forgot" the whole conversation.
    const CHAT_SESSION_KEY='simardChatSession';
    function loadStoredChatSession(){ try{ return localStorage.getItem(CHAT_SESSION_KEY)||null; }catch(e){ return null; } }
    function storeChatSession(id){ try{ if(id) localStorage.setItem(CHAT_SESSION_KEY,id); else localStorage.removeItem(CHAT_SESSION_KEY); }catch(e){} }
    let ws=null, currentSessionId=loadStoredChatSession(), streamSpan=null, streamText='';

    async function loadChatSessions(){
      const box=document.getElementById('chat-sessions');
      if(!box) return;
      try{
        const d=await apiFetch('/api/chat/sessions');
        const sessions=(d&&d.sessions)||[];
        box.textContent='';
        if(sessions.length===0){
          const empty=document.createElement('div');
          empty.className='chat-session-empty';
          empty.textContent='No saved chats yet. Start a new conversation.';
          box.appendChild(empty);
          return;
        }
        sessions.forEach(s=>{
          const item=document.createElement('div');
          item.className='chat-session-item'+(s.id===currentSessionId?' active':'');
          item.dataset.id=s.id;
          const title=document.createElement('div');
          title.className='cs-title';
          title.textContent=s.title||s.id;
          const time=document.createElement('div');
          time.className='cs-time';
          time.textContent=timeAgo(s.updated_at);
          item.appendChild(title);
          item.appendChild(time);
          item.addEventListener('click',()=>openSession(s.id));
          box.appendChild(item);
        });
        maybeResumeChat();
      }catch(e){
        box.textContent='';
        const err=document.createElement('div');
        err.className='chat-session-empty';
        err.textContent='Failed to load chats.';
        box.appendChild(err);
      }
    }

    // After a page reload, transparently reconnect to the last conversation
    // (persisted in localStorage) so the chat stays continuous instead of
    // starting a fresh, empty session (issue #2581). Only connects when there
    // is no live socket, and only for a session that still exists on disk
    // (verified against the freshly-rendered session list).
    function maybeResumeChat(){
      if(ws && (ws.readyState===WebSocket.OPEN || ws.readyState===WebSocket.CONNECTING)) return;
      if(!currentSessionId) return;
      let found=false;
      document.querySelectorAll('.chat-session-item').forEach(el=>{
        const match=el.dataset.id===currentSessionId;
        el.classList.toggle('active', match);
        if(match) found=true;
      });
      if(!found){ storeChatSession(null); currentSessionId=null; return; }
      initChat(currentSessionId);
    }

    async function openSession(id){
      currentSessionId=id;
      storeChatSession(id);
      document.querySelectorAll('.chat-session-item').forEach(el=>{
        el.classList.toggle('active', el.dataset.id===id);
      });
      clearMessages();
      try{
        const d=await apiFetch('/api/chat/sessions/'+encodeURIComponent(id));
        (d.history||[]).forEach(m=>appendMsg(m.role||'system', m.content||''));
      }catch(e){ appendMsg('system','Failed to load session history.'); }
      initChat(id);
    }

    function newChat(){
      currentSessionId=null;
      storeChatSession(null);
      document.querySelectorAll('.chat-session-item.active').forEach(el=>el.classList.remove('active'));
      clearMessages();
      initChat(null);
    }

    function initChat(sessionId){
      if(sessionId!==undefined) currentSessionId=sessionId;
      if(ws){try{ws.close();}catch(e){}}
      const proto=location.protocol==='https:'?'wss:':'ws:';
      const qs=currentSessionId?('?session_id='+encodeURIComponent(currentSessionId)):'';
      ws=new WebSocket(`${proto}//${location.host}/ws/chat${qs}`);
      const st=document.getElementById('ws-status');
      st.className='ws-status';
      st.innerHTML='<span style="color:var(--yellow)">● Connecting…</span>';
      ws.onopen=()=>{st.innerHTML='<span style="color:var(--green)">● Connected</span>';};
      ws.onclose=()=>{
        st.innerHTML='<span style="color:var(--red)">● Disconnected</span> <button class="btn" onclick="initChat()" style="font-size:.75rem;padding:.1rem .4rem;margin-left:.5rem">Reconnect</button>';
        removeTypingIndicator();setChatBusy(false);finalizeStream();
      };
      ws.onerror=()=>{
        st.innerHTML='<span style="color:var(--red)">● Error</span> <button class="btn" onclick="initChat()" style="font-size:.75rem;padding:.1rem .4rem;margin-left:.5rem">Retry</button>';
        removeTypingIndicator();setChatBusy(false);
      };
      ws.onmessage=onChatFrame;
    }

    function onChatFrame(ev){
      let m;
      try{ m=JSON.parse(ev.data); }
      catch(ex){ removeTypingIndicator();setChatBusy(false);appendMsg('system',ev.data); return; }
      // Handshake: bind the session id + streaming capability.
      if(m && m.type==='ready'){ if(m.session_id){ currentSessionId=m.session_id; storeChatSession(currentSessionId); } return; }
      // Resume: replay persisted history into the panel.
      if(m && m.type==='restore'){
        clearMessages();
        (m.messages||[]).forEach(msg=>appendMsg(msg.role||'system', msg.content||''));
        removeTypingIndicator();setChatBusy(false);
        return;
      }
      // Streaming: coalesce chunk frames into one assistant bubble.
      if(m && m.type==='chunk'){ removeTypingIndicator(); appendChunk(m.content||''); return; }
      if(m && m.type==='done'){ finalizeStream(m.content); setChatBusy(false); return; }
      // Legacy / fallback frames: {role, content} rendered in one update.
      removeTypingIndicator();setChatBusy(false);finalizeStream();
      appendMsg((m&&m.role)||'system', (m&&m.content!==undefined)?m.content:ev.data);
    }

    function sendChat(){
      const inp=document.getElementById('chat-input'); const txt=inp.value.trim();
      if(!txt) return;
      if(!ws||ws.readyState!==WebSocket.OPEN){
        appendMsg('system','Not connected. Click Reconnect to establish a session.');
        return;
      }
      appendMsg('user',txt); ws.send(txt); inp.value=''; inp.style.height='';
      showTypingIndicator(); setChatBusy(true);
    }

    function appendChunk(text){
      if(!streamSpan){
        const el=document.getElementById('chat-messages');
        const div=document.createElement('div');
        div.className='chat-msg';
        const roleSpan=document.createElement('span');
        roleSpan.className='role assistant';
        roleSpan.textContent='assistant:';
        div.appendChild(roleSpan);
        streamSpan=document.createElement('span');
        div.appendChild(streamSpan);
        el.appendChild(div);
        streamText='';
      }
      streamText+=text;
      streamSpan.textContent=' '+streamText;
      const el=document.getElementById('chat-messages');
      el.scrollTop=el.scrollHeight;
    }
    function finalizeStream(finalText){
      // On `done`, replace the streamed preview with the authoritative
      // (sanitized) reply when the server supplies one (issue #2581); otherwise
      // keep whatever was streamed (server-side chunking fallback).
      if(streamSpan && finalText!==undefined && finalText!==null && finalText!==''){
        streamSpan.textContent=' '+finalText;
      }
      streamSpan=null; streamText='';
    }

    function clearMessages(){
      finalizeStream();
      const el=document.getElementById('chat-messages');
      if(el) el.textContent='';
    }
    function showTypingIndicator(){
      removeTypingIndicator();
      const el=document.getElementById('chat-messages');
      const div=document.createElement('div');
      div.id='typing-indicator';
      div.className='chat-typing';
      div.innerHTML='<span class="typing-dots"><span>.</span><span>.</span><span>.</span></span>';
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
    /* Auto-grow the textarea up to 150px (coupled to max-height in part_00.rs), then scroll. */
    document.getElementById('chat-input').addEventListener('input',e=>{
      const inp=e.target; inp.style.height='auto'; inp.style.height=Math.min(inp.scrollHeight,150)+'px';
    });


    /* --- Workboard --- */
    const phaseColors={act:'var(--green)',orient:'var(--yellow)',observe:'var(--accent)',decide:'#a371f7',sleep:'#8b949e',unknown:'#8b949e'};
    function fmtDuration(s){if(s<60)return s+'s';const m=Math.floor(s/60);if(m<60)return m+'m '+s%60+'s';const h=Math.floor(m/60);return h+'h '+m%60+'m';}
    function wbGoalCard(g){
      const pct=g.progress_pct||0;
      const isBlocked=g.status.startsWith('blocked');
      /* Issue #4178: a lifecycle-BLOCKED goal uses amber (var(--yellow)
         #d29922), NEVER the activity-failure red (var(--red) #f85149). This
         mirrors issue #20's GOAL_STATUS_COLORS decision on the Goals tab so a
         blocked goal is never mistaken for a failed one anywhere in the UI. */
      const barColor=g.status==='done'?'var(--green)':isBlocked?'var(--yellow)':'var(--accent)';
      /* Surface WHY the goal is blocked (issue #4178). Prefer the additive
         clean `block_reason`; fall back to stripping the legacy `blocked: `
         prefix so older payloads still render a reason. */
      const reason=isBlocked?(g.block_reason||g.status.replace(/^blocked:\s*/,'')):'';
      const blockedRow=isBlocked&&reason
        ?`<div style="font-size:.72rem;color:var(--yellow);margin-bottom:.3rem"><strong>Blocked — </strong>${esc(reason)}</div>`
        :'';
      return`<div style="background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:.6rem;margin-bottom:.5rem">
        <div style="font-weight:600;font-size:.85rem;margin-bottom:.3rem">${esc(g.name)}</div>
        <div style="font-size:.75rem;color:#8b949e;margin-bottom:.4rem">${esc(g.description||'')}</div>
        ${blockedRow}
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
              <span style="min-width:5rem;color:${isCurrent?'var(--green)':'#8b949e'}">${esc(humanizeActionKind(a.action))}</span>
              <span style="flex:1" title="${escAttr(a.result||'')}">${renderActionDetail(humanizeActionDetail(a.result))}</span>
              ${a.at?'<span style="color:#8b949e;font-size:.75rem">'+timeAgo(a.at)+'</span>':''}
            </div>`;
          }).join('');
        }else{document.getElementById('wb-actions').innerHTML='<span style="color:#8b949e;font-size:.85rem">No recent actions</span>';}
        // Task memory (structured table — #1683)
        const tm=d.task_memory||{};
        document.getElementById('wb-facts-count').textContent=(tm.facts_count||0)+' facts';
        if(tm.recent_facts?.length){
          document.getElementById('wb-facts-list').innerHTML='<table class="proc-table"><tr><th>Category</th><th>Content</th><th>Confidence</th><th>Tags</th></tr>'
            +tm.recent_facts.map(f=>{
            const cat=esc(f.category||f.concept||'');
            const conf=typeof f.confidence==='number'?Math.round(f.confidence*100)+'%':'—';
            const tags=(f.tags||[]).map(t=>'<span style="background:var(--border);padding:0 .3rem;border-radius:3px;font-size:.7rem;margin-right:.3rem">'+esc(t)+'</span>').join('')||'—';
            const rawContent=(f.content||'');
            const humanizedContent=humanizeTaskMemory(rawContent);
            const content=esc(humanizedContent.substring(0,200));
            const contentTitle=(humanizedContent!==rawContent)?(' title="'+escAttr(rawContent)+'"'):'';
            return'<tr><td style="white-space:nowrap;color:var(--accent);font-weight:600;font-size:.8rem">'+cat+'</td><td style="font-size:.85rem"'+contentTitle+'>'+content+'</td><td style="text-align:center;font-size:.8rem">'+conf+'</td><td>'+tags+'</td></tr>';
          }).join('')+'</table>';
        }else{document.getElementById('wb-facts-list').innerHTML='<span style="color:#8b949e">No recent facts in memory</span>';}
        // Working memory (human-readable — #1683). The slot count comes from
        // the global working_count statistic (the same value the Memory tab
        // shows) rather than the length of the per-goal slot list, so the
        // badge can no longer read "0 slots" while the Memory tab reports a
        // populated working memory (#1679). The per-goal list below is a
        // best-effort detail view; when it is empty but the global count is
        // non-zero, point the operator at the Memory tab instead of claiming
        // there is no working memory.
        const wm=d.working_memory||[];
        const wmCount=(d.cognitive_statistics&&d.cognitive_statistics.working_count!=null)?d.cognitive_statistics.working_count:wm.length;
        document.getElementById('wb-wm-count').textContent=wmCount+' slot'+(wmCount===1?'':'s');
        if(wm.length){
          document.getElementById('wb-wm-list').innerHTML='<table class="proc-table"><tr><th>Type</th><th>Content</th><th>Related Goal</th><th>Relevance</th></tr>'
            +wm.map(s=>{
            const relColor=s.relevance>=0.8?'var(--green)':s.relevance>=0.5?'var(--yellow)':'#8b949e';
            return'<tr><td style="white-space:nowrap;color:var(--accent);font-weight:600;font-size:.8rem">'+esc(s.type_label||'Note')+'</td><td style="font-size:.85rem">'+esc((s.content||'').substring(0,200))+'</td><td style="font-size:.8rem;color:#8b949e">'+esc(s.goal||'—')+'</td><td style="text-align:center"><span style="color:'+relColor+';font-weight:600;font-size:.8rem">'+esc(s.relevance_label||'—')+'</span></td></tr>';
          }).join('')+'</table>';
        }else if(wmCount>0){
          document.getElementById('wb-wm-list').innerHTML='<span style="color:#8b949e">'+wmCount+' working-memory slot'+(wmCount===1?'':'s')+' active — open the Memory tab to inspect them.</span>';
        }else{document.getElementById('wb-wm-list').innerHTML='<span style="color:#8b949e">No active working memory</span>';}
        // Cognitive statistics
        const cs=d.cognitive_statistics;
        if(cs){
          document.getElementById('wb-cog-stats').innerHTML=[
            ['Recent observations',cs.sensory_count],['Currently thinking about',cs.working_count],['Events remembered',cs.episodic_count],
            ['Facts learned',cs.semantic_count],['Known procedures',cs.procedural_count],['Planned actions',cs.prospective_count],['Total',cs.total]
          ].map(([k,v])=>'<span style="margin-right:1rem"><strong>'+k+':</strong> '+(v||0)+'</span>').join('');
        }else{document.getElementById('wb-cog-stats').innerHTML='<span style="color:#8b949e">No agent memory available</span>';}
      }catch(e){document.getElementById('wb-engineers').innerHTML='<span class="err">Failed to load workboard data</span>';}
    }

    /* --- Thinking --- */
    async function fetchThinking(){
      try{
        const d=await apiFetch('/api/ooda-thinking');
        const el=document.getElementById('thinking-timeline');
        if(!d.reports?.length){el.innerHTML='<span style="color:#8b949e">No cycle reports yet. The agent daemon generates these during autonomous work.</span>';return;}
        el.innerHTML=d.reports.map(renderCycleEntry).join('');
      }catch(e){document.getElementById('thinking-timeline').innerHTML='<span class="err">Failed to load: '+esc(e.toString())+'</span>';}
    }

    /* Shared per-cycle renderer (#26): renders ONE collapsed cycle report — the
       same object shape `/api/ooda-thinking` (→ reports) and `/api/logs`
       (→ cycle_reports) now BOTH return from the single server-side reader. Used
       by both the Thinking tab timeline and the Activity tab's "Cycle Reports"
       card so the two views can never diverge into a stale, detail-less copy. */
    function renderCycleEntry(rpt){
          if(rpt.legacy){
            return `<div class="thinking-cycle legacy">
              <div class="cycle-header"><span class="cycle-num">Cycle #${rpt.cycle_number}</span><span class="cycle-badge">legacy</span></div>
              <div class="cycle-summary">${esc(humanizeCycleSummary(rpt.summary))}</div>
            </div>`;
          }
          /* Issue #2580: disposition-aware rendering. A run of identical
             deferrals to an active engineer is collapsed by the server into a
             single entry with a repeat_count; render it as one clean line so
             the tab shows forward progress, not the same line over and over. A
             genuine stuck loop (loop_suspected) is called out in red. */
          const disp=rpt.disposition||'';
          const rc=rpt.repeat_count||1;
          const cFirst=rpt.cycle_number_first||rpt.cycle_number;
          const cLast=rpt.cycle_number_last||rpt.cycle_number;
          const rangeTxt=(cFirst!==cLast)?('Cycles #'+cLast+'–#'+cFirst):('Cycle #'+rpt.cycle_number);
          let dispoBadge='';
          if(disp==='deferring'){dispoBadge='<span class="cycle-badge" style="background:#2ea043;color:#fff">deferring'+(rc>1?' ×'+rc:'')+'</span>';}
          else if(disp==='progressing'){dispoBadge='<span class="cycle-badge" style="background:#1f6feb;color:#fff">progress</span>';}
          if(rpt.loop_suspected){dispoBadge+='<span class="cycle-badge" style="background:var(--red);color:#fff" title="the same non-progressing decision repeated without the work advancing">⚠ possible loop ×'+rc+'</span>';}
          if(disp==='deferring'){
            return `<div class="thinking-cycle">
              <div class="cycle-header"><span class="cycle-num">${rangeTxt}</span>${dispoBadge}</div>
              <div class="cycle-summary-inline">${esc(rpt.collapsed_summary||'Deferring to an active engineer')}</div>
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
                  <strong>${esc(humanizeGoalId(p.goal_id))}</strong> · ${urgencyPhrase(p.urgency)} — ${esc(p.reason)}
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
                      <div><span style="color:${statusColor}">●</span> <strong>Launched sub-agent</strong> · ${esc(se.last_action||'')} · <span style="color:${statusColor}">${esc(se.status||'')}</span></div>
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
              <span class="cycle-num">${rangeTxt}</span>
              ${dispoBadge}
              <span class="cycle-summary-inline">${esc(humanizeCycleSummary(rpt.summary||''))}</span>
            </div>
            <div class="cycle-phases">${phases.join('')}</div>
          </div>`;
    }

    /* --- OODA Cycle History (issue #2135) --- */
    // Honest cycle-count label: the endpoint returns `total_cycles` (rows in the
    // bounded window) and `latest_cycle_number` (the authoritative cumulative
    // cycle count, #1680). When the daemon has run more cycles than the window
    // shows, say so explicitly instead of rendering the capped window size as
    // the lifetime total.
    function cycleCountLabel(d){
      const shown=d.total_cycles||0;
      const lifetime=d.latest_cycle_number||0;
      return lifetime>shown
        ? `Showing last ${shown} of ${lifetime} cycles run`
        : `${shown} cycles recorded`;
    }
    async function fetchOodaCycles(){
      try{
        const d=await apiFetch('/api/ooda-cycles');
        const trendEl=document.getElementById('ooda-cycle-trend');
        const histEl=document.getElementById('ooda-cycle-history');
        const cycles=d.cycles||[];
        const trend=d.duration_trend||{};
        const dir=trend.direction||'insufficient_data';
        // Issue #21: when there is not enough per-cycle duration data to compute
        // a trend, render only an honest cycle count — never a permanently
        // broken "not enough data" chart/placeholder.
        if(dir==='insufficient_data'){
          trendEl.innerHTML=`<div style="color:#8b949e;font-size:.85rem">${cycleCountLabel(d)}</div>`;
        }else{
          const trendColors={improving:'var(--green)',degrading:'var(--red)',stable:'var(--yellow)'};
          const trendLabels={improving:'↓ Improving',degrading:'↑ Degrading',stable:'→ Stable'};
          const trendColor=trendColors[dir]||'#8b949e';
          let trendHtml=`<div style="display:flex;gap:1.5rem;align-items:center;flex-wrap:wrap">
            <div><strong style="color:${trendColor}">${trendLabels[dir]||dir}</strong></div>
            <div style="color:#8b949e;font-size:.85rem">${cycleCountLabel(d)}</div>`;
          if(trend.recent_avg_secs!=null){
            trendHtml+=`<div style="font-size:.85rem">Recent avg: <strong>${trend.recent_avg_secs}s</strong></div>
              <div style="font-size:.85rem">Older avg: <strong>${trend.older_avg_secs}s</strong></div>
              <div style="font-size:.85rem">Change: <strong style="color:${trendColor}">${trend.change_pct>0?'+':''}${trend.change_pct}%</strong></div>`;
          }
          trendHtml+='</div>';
          trendEl.innerHTML=trendHtml;
        }
        // Duration trend line chart (inline SVG) — #2223. Hidden entirely while
        // there is not enough duration data (issue #21), so the tab never shows
        // a permanently-flat "not enough data" chart.
        if(dir!=='insufficient_data' && cycles.length){
          const durations=cycles.map(c=>c.duration_secs||0).reverse();
          const nums=cycles.map(c=>c.cycle_number).reverse();
          const maxD=Math.max(...durations,1);
          const barW=Math.max(6,Math.min(24,Math.floor(600/durations.length)));
          const chartH=80;
          const borderClr='var(--border)';
          const bars=durations.map((d,i)=>{
            const h=Math.max(2,(d/maxD)*chartH);
            const x=i*(barW+2);
            const color=d===0?borderClr:'var(--accent)';
            return `<rect x="${x}" y="${chartH-h}" width="${barW}" height="${h}" fill="${color}" rx="1" opacity="0.4"><title>Cycle ${nums[i]}: ${d}s</title></rect>`;
          }).join('');
          const svgW=durations.length*(barW+2);
          // Compute trend line points
          const linePoints=durations.map((d,i)=>{
            const x=i*(barW+2)+barW/2;
            const y=chartH-Math.max(2,(d/maxD)*chartH)+1;
            return `${x},${y}`;
          }).join(' ');
          // Moving average line (window=3) for smoothed trend
          const maWindow=Math.min(3,Math.max(1,Math.floor(durations.length/3)));
          const maPoints=durations.map((d,i)=>{
            const start=Math.max(0,i-maWindow+1);
            const slice=durations.slice(start,i+1);
            const avg=slice.reduce((a,b)=>a+b,0)/slice.length;
            const x=i*(barW+2)+barW/2;
            const y=chartH-Math.max(2,(avg/maxD)*chartH)+1;
            return `${x},${y}`;
          }).join(' ');
          trendEl.innerHTML+=`<div style="margin-top:.5rem;overflow-x:auto" data-testid="ooda-cycle-trend-chart"><svg width="${svgW}" height="${chartH+16}" style="display:block"><g>${bars}</g><polyline points="${linePoints}" fill="none" stroke="var(--accent)" stroke-width="1.5" opacity="0.7"/><polyline points="${maPoints}" fill="none" stroke="var(--green)" stroke-width="2" stroke-dasharray="4,2" data-testid="ooda-trend-line"><title>Moving average trend</title></polyline><line x1="0" y1="${chartH}" x2="${svgW}" y2="${chartH}" stroke="${borderClr}" stroke-width="1"/></svg><div style="display:flex;gap:1rem;font-size:.75rem;color:#8b949e;margin-top:.3rem"><span><span style="color:var(--accent)">━</span> Duration per cycle</span><span><span style="color:var(--green)">╌</span> Moving average</span></div></div>`;
        }
        // History table
        if(!cycles.length){histEl.innerHTML='<span style="color:#8b949e">No cycle history available. Run the agent daemon to generate cycle data.</span>';return;}
        histEl.innerHTML=`<div style="overflow-x:auto"><table class="proc-table">
          <tr><th>#</th><th>Phase</th><th>Duration</th><th>Actions</th><th>Summary</th><th>Time</th></tr>
          ${cycles.map(c=>{
            const phaseColors={act:'var(--green)',decide:'#a371f7',orient:'var(--yellow)',observe:'var(--accent)',unknown:'#8b949e'};
            const pColor=phaseColors[c.phase]||'#8b949e';
            const dur=c.duration_secs!=null?c.duration_secs+'s':'—';
            /* Issue #21: prefer the server's difference-carrying collapsed_summary
               (the decided action / deferral / decision text) over the raw
               count-boilerplate, so forward progress stands out from a stuck loop. */
            const summary=c.collapsed_summary||humanizeCycleSummary(c.summary||'');
            const shortSummary=summary.length>120?summary.substring(0,120)+'…':summary;
            /* A collapsed run renders as one row labelled with a repeat count
               and cycle range (A=oldest, B=newest); a single cycle keeps its
               plain number. */
            const rc=c.repeat_count||1;
            const cFirst=c.cycle_number_first||c.cycle_number;
            const cLast=c.cycle_number_last||c.cycle_number;
            const cycleLabel=(rc>1)?('×'+rc+' (cycles #'+cLast+'–#'+cFirst+')'):('#'+c.cycle_number);
            return `<tr>
              <td style="font-weight:600;color:var(--accent);white-space:nowrap">${esc(cycleLabel)}</td>
              <td><span style="color:${pColor}">${esc(c.phase)}</span></td>
              <td>${dur}</td>
              <td>${c.action_count||0}</td>
              <td style="font-size:.8rem;max-width:400px">${esc(shortSummary)}</td>
              <td style="color:#8b949e;font-size:.8rem;white-space:nowrap">${c.timestamp?timeAgo(c.timestamp):'—'}</td>
            </tr>`;}).join('')}
        </table></div>`;
      }catch(e){
        const el=document.getElementById('ooda-cycle-history');
        if(el) el.innerHTML='<span class="err">Failed to load cycle history: '+esc(e.toString())+'</span>';
      }
    }

    /* --- Brain Failures (issue #2043) --- */
    async function fetchBrainFailures(){
      try{
        const d=await apiFetch('/api/brain-failures');
        const sumEl=document.getElementById('brain-failures-summary');
        const listEl=document.getElementById('brain-failures-list');
        const s=d.summary||{};
        const scanned=s.cycles_scanned||0;
        /* Issue #2580: headline the CURRENT bounded window + rate, never a
           stale cumulative total. Zero current failures renders green. */
        const rec=d.recent||{};
        const life=d.lifetime||{};
        const win=rec.window_minutes||60;
        const recentTotal=rec.total||0;
        const recentParse=rec.parse_failure||0;
        const recentFallback=rec.fallback||0;
        const rate=Number(rec.rate_per_hour||0);
        const lifetimeParse=life.parse_failure_count||0;
        const statusClass=rec.status==='err'?'err':(rec.status==='warn'?'warn':'ok');
        const statusText=recentTotal===0
          ?('No brain failures in the last '+win+' min')
          :(recentTotal+' failure'+(recentTotal===1?'':'s')+' in the last '+win+' min');
        sumEl.innerHTML=`
          <div class="stat"><span class="label">Current status (last ${win} min)</span><span class="value ${statusClass}">${statusText}</span></div>
          <div class="stat"><span class="label">Current failure rate</span><span class="value ${statusClass}">${rate.toFixed(1)} / hr</span></div>
          <div class="stat"><span class="label">Parse failures (last ${win} min)</span><span class="value ${recentParse>0?'err':'ok'}">${recentParse}</span></div>
          <div class="stat"><span class="label">Deterministic fallbacks (last ${win} min)</span><span class="value ${recentFallback>0?'warn':'ok'}">${recentFallback}</span></div>
          <div class="stat"><span class="label">All-time parse failures (cumulative, not current)</span><span class="value" style="opacity:.55">${lifetimeParse}</span></div>
          <div class="stat"><span class="label">Last checked</span><span class="value">${timeAgo(d.timestamp)}</span></div>`;
        const failures=d.failures||[];
        if(!failures.length){
          listEl.innerHTML='<div style="color:#8b949e;padding:.5rem 0">No brain failures in the last '+scanned+' cycles. The daemon\'s language-model brain has been responding correctly.</div>';
          return;
        }
        listEl.innerHTML=failures.map(f=>{
          const typeIcon=f.failure_type==='parse_failure'?'🔴':'🟡';
          const escBadge=f.escalated?'<span style="background:var(--red);color:#fff;padding:1px 6px;border-radius:3px;font-size:11px;margin-left:6px">escalated to issue</span>':'';
          const recoveryBadge=f.recovery_succeeded?'<span style="color:var(--green);font-size:.8rem">✓ recovered via fallback</span>':'<span style="color:var(--red);font-size:.8rem">✗ no recovery</span>';
          let detail='';
          if(f.parse_failure_detail){
            const pf=f.parse_failure_detail;
            detail=`<div style="margin-top:.35rem;padding:.4rem .55rem;border-left:3px solid var(--red);background:rgba(255,255,255,0.03);border-radius:4px;font-size:.8rem">
              <div><strong>Error:</strong> ${esc(pf.error_message||'')}</div>
              <div><strong>Prompt:</strong> ${esc(pf.prompt_name||'')} (version: ${esc(pf.prompt_version||'none')})</div>
              <div><strong>Consecutive failures:</strong> ${pf.consecutive_count||0}</div>
              ${pf.raw_response_truncated?'<details style="margin-top:.25rem"><summary style="cursor:pointer;color:#8b949e">Raw model response</summary><pre style="white-space:pre-wrap;max-height:200px;overflow:auto;margin-top:.25rem;padding:.35rem;background:#0d1117;border:1px solid var(--border);border-radius:4px;font-size:.75rem">'+esc(pf.raw_response_truncated)+'</pre></details>':''}
            </div>`;
          }
          return `<div style="padding:.6rem 0;border-bottom:1px solid var(--border)">
            <div style="display:flex;gap:.5rem;align-items:baseline;flex-wrap:wrap">
              <span>${typeIcon}</span>
              <strong>${esc(f.failure_type_plain||f.failure_type)}</strong>${escBadge}
              <span style="color:#8b949e;font-size:.8rem">Cycle #${f.cycle_number} · ${timeAgo(f.timestamp)}</span>
              <span style="margin-left:auto">${recoveryBadge}</span>
            </div>
            <div style="font-size:.85rem;color:#9bb1c4;margin-top:.2rem"><strong>Component:</strong> ${esc(f.phase_plain||f.phase)}</div>
            <div style="font-size:.85rem;color:#9bb1c4;margin-top:.15rem">${esc(f.description||'')}</div>
            ${f.rationale?'<div style="font-size:.8rem;color:#8b949e;margin-top:.15rem"><em>Rationale: '+esc(f.rationale)+'</em></div>':''}
            ${detail}
          </div>`;
        }).join('');
      }catch(e){
        document.getElementById('brain-failures-summary').innerHTML='<span class="err">Failed to load: '+esc(e.toString())+'</span>';
        document.getElementById('brain-failures-list').innerHTML='';
      }
    }

    /* --- Agent log terminal (issue #947) --- */
    let agentLogTerm = null;
    let agentLogWS = null;
    /* Issue #946 / #2627: jump from an engineer-spawn outcome straight to the
       agent terminal viewer. The Terminal view is now the Terminal sub-section
       of the consolidated Workers tab, so activate Workers and scroll to it. */
    function openAgentLog(name){
      if(typeof activateTab==='function'){ activateTab('workers','terminal'); }
      else { const tab=document.querySelector('.tab[data-tab="workers"]'); if(tab) tab.click(); }
      const input = document.getElementById('agent-log-name');
      if(input) input.value = name || '';
      // initAgentLogTerminal is invoked by the tab activation; defer
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
      return formatTime(ts);
    }
    async function fetchTmuxSessions(){
      const body = document.getElementById('tmux-sessions-body');
      if(!body) return;"#;
