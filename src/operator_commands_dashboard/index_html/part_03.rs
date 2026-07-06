pub(crate) const PART_03: &str = r#"        const d=await apiFetch('/api/goals');
        if(d.active?.length){
          document.getElementById('goals-active').innerHTML=`<table class="proc-table">
            <tr><th>Priority</th><th>ID</th><th>Description</th><th>Status</th><th>Current Activity</th><th>Actions</th></tr>
            ${d.active.map(g=>{
              const chipColors={'Working':'#2ea043','Skipped':'#8b949e','Failed':'#f85149','Spawned engineer':'#a371f7','Waiting':'#6e7681'};
              const chip=g.status_chip||'Waiting';
              const chipColor=chipColors[chip]||'#6e7681';
              const chipHtml='<span style="display:inline-block;padding:1px 8px;border-radius:10px;background:'+chipColor+';color:#fff;font-size:.7rem;font-weight:600;white-space:nowrap">'+esc(chip)+'</span>';
              const detailText=g.detail||'';
              const detailHtml=detailText?'<span style="font-size:.8rem;margin-left:6px">'+esc(detailText)+'</span>':'';
              // Issue #20: render each goal's LIVE lifecycle status as a
              // distinctly-colored badge driven by the additive serialized-enum
              // g.status_progress (falling back to the legacy g.status string
              // only if the field is absent). This replaces the old raw
              // status-string cell that — paired with the red activity chip —
              // made every goal read as failed/blocked.
              const lifeColor=GOAL_STATUS_COLORS[goalLifecycleKey(g.status_progress)];
              const lifeLabel=(g.status_progress!=null)?humanizeGoalProgress(g.status_progress):(g.status||'—');
              const statusBadge='<span style="display:inline-block;padding:1px 8px;border-radius:10px;background:'+lifeColor+';color:#fff;font-size:.72rem;font-weight:600">'+esc(lifeLabel)+'</span>';
              const full=g.detail_full||'';
              const isFailed=(chip==='Failed');
              const expandHtml=(full&&full!==detailText)?'<details style="display:inline;margin-left:6px"'+(isFailed?' open':'')+' ><summary style="display:inline;cursor:pointer;color:'+(isFailed?'#f85149':'#8b949e')+';font-size:.7rem">'+(isFailed?'error details':'show full log')+'</summary><pre style="margin:.3rem 0 0;white-space:pre-wrap;font-size:.75rem;color:'+(isFailed?'#f85149':'#8b949e')+'">'+esc(full)+'</pre></details>':'';
              let wipHtml='—';
              if(chip!=='Waiting'||detailText||g.wip_refs?.length){
                let parts=[];
                parts.push('<div style="font-size:.8rem;line-height:1.4">'+chipHtml+' '+detailHtml+expandHtml+'</div>');
                if(g.wip_refs?.length) parts.push(g.wip_refs.map(r=>{
                  const icon=r.kind==='pr'?'🔀':r.kind==='issue'?'🐛':r.kind==='branch'?'🌿':r.kind==='session'?'💻':'📌';
                  return r.url?'<a href="'+esc(r.url)+'" target="_blank" style="color:var(--accent);text-decoration:none;font-size:.8rem">'+icon+' '+esc(r.label)+'</a>':'<span style="font-size:.8rem">'+icon+' '+esc(r.label)+'</span>';
                }).join('<br>'));
                wipHtml=parts.join('');
              }
              return `<tr>
              <td style="text-align:center">${g.priority??'—'}</td>
              <td><code>${esc(g.id)}</code></td>
              <td>${esc(g.description)}</td>
              <td>${statusBadge}</td>
              <td>${wipHtml}</td>
              <td>
                <button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="demoteGoal('${esc(g.id)}')">▼ Backlog</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px" onclick="updateGoalStatus('${esc(g.id)}')">Status</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px;color:#f85149" onclick="removeGoal('${esc(g.id)}')">✕</button>
              </td>
            </tr>`;}).join('')}
          </table>
          <div style="margin-top:.5rem;color:#8b949e;font-size:.8rem">${d.active_count} active goal(s)</div>`;
        }else{document.getElementById('goals-active').innerHTML='<span style="color:#8b949e">No active goals. Use "Seed Default Goals" or run the agent daemon to generate goals from meetings.</span>';}
        if(d.backlog?.length){
          document.getElementById('goals-backlog').innerHTML=`<table class="proc-table">
            <tr><th>Title</th><th>Description</th><th>Source</th><th>Score</th><th>Actions</th></tr>
            ${d.backlog.map(b=>{
              const title=esc(b.display_id||b.id||'');
              const isMemId=/^(sem|epi|wrk|pro|sns)_[0-9a-f]{8,}/.test(b.id||'');
              const titleCell=isMemId?title:'<code>'+title+'</code>';
              return`<tr>
              <td>${titleCell}</td>
              <td>${esc(b.description)}</td>
              <td style="font-size:.8rem;color:#8b949e">${esc(b.source||'')}</td>
              <td>${typeof b.score==='number'?Math.round(b.score*100)+'%':(b.score??'—')}</td>
              <td>
                <button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="promoteGoal('${esc(b.id)}')">▲ Promote</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px" onclick="removeGoal('${esc(b.id)}')">✕</button>
              </td>
            </tr>`}).join('')}
          </table>`;
        }else{document.getElementById('goals-backlog').innerHTML='<span style="color:#8b949e">No backlog items</span>';}
      }catch(e){document.getElementById('goals-active').innerHTML='<span class="err">Failed to load goals — check state root</span>';}
    }

    async function seedGoals(){
      if(!confirm('Seed default goals? This only works if no active goals exist.'))return;
      try{
        const d=await apiFetch('/api/goals/seed',{method:'POST'});
        if(d.status==='ok'||d.status==='already_seeded'){
          fetchGoals();
        }else{
          alert('Seed failed: '+(d.error||'unknown'));
        }
      }catch(e){alert('Seed failed: '+e);}
    }

    function showAddGoalForm(){document.getElementById('add-goal-form').style.display='block';document.getElementById('new-goal-desc').focus();}

    async function submitGoal(){
      const desc=document.getElementById('new-goal-desc').value.trim();
      if(!desc){alert('Description required');return;}
      const type=document.getElementById('new-goal-type').value;
      const priority=parseInt(document.getElementById('new-goal-priority').value)||3;
      try{
        const d=await apiFetch('/api/goals',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({description:desc,type:type,priority:priority})});
        if(d.status==='ok'){document.getElementById('add-goal-form').style.display='none';document.getElementById('new-goal-desc').value='';fetchGoals();}
        else{alert(d.error||'Failed');}
      }catch(e){alert('Error: '+e);}
    }

    async function removeGoal(id){
      if(!confirm('Remove goal "'+id+'"?'))return;
      try{
        const d=await apiFetch('/api/goals/'+encodeURIComponent(id),{method:'DELETE'});
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    async function promoteGoal(id){
      try{
        const d=await apiFetch('/api/goals/promote/'+encodeURIComponent(id),{method:'POST'});
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    async function demoteGoal(id){
      if(!confirm('Move "'+id+'" to backlog?'))return;
      try{
        const d=await apiFetch('/api/goals/demote/'+encodeURIComponent(id),{method:'POST'});
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    async function updateGoalStatus(id){
      const status=prompt('New status (not-started, in-progress, blocked, completed):');
      if(!status)return;
      try{
        const d=await apiFetch('/api/goals/'+encodeURIComponent(id)+'/status',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({status:status})});
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    /* --- Traces --- */
    async function fetchTraces(){
      try{
        const d=await apiFetch('/api/traces');
        const status=d.otel_enabled
          ?`<span class="ok">OTEL enabled</span> → <code>${esc(d.otel_endpoint||'')}</code>`
          :'<span class="warn">OTEL not configured</span> — set OTEL_EXPORTER_OTLP_ENDPOINT to enable';
        document.getElementById('otel-status').innerHTML=`
          <div class="stat"><span class="label">OTEL Status</span><span class="value">${status}</span></div>
          <div class="stat"><span class="label">Collected Entries</span><span class="value">${d.span_count}</span></div>`;
        if(d.spans?.length){
          document.getElementById('trace-list').innerHTML=d.spans.map(s=>s&&s.source==='cost'?renderCostTrace(s.data):renderGenericTrace(s)).join('');
        }else{document.getElementById('trace-list').innerHTML='<span style="color:#8b949e">No trace data yet. Run the agent daemon or make API calls to generate traces.</span>';}
      }catch(e){document.getElementById('trace-list').innerHTML='<span class="err">Failed to load traces — check /api/traces</span>';}
    }

    /* Map the raw cost-ledger `model` token to a plain-language call label. */
    function costModelLabel(m){
      const map={'copilot':'Copilot SDK call','copilot-meeting':'Copilot meeting turn','copilot-lightweight':'Copilot lightweight call','direct-invoke':'Direct agent call','session-builder':'Session-builder call','lightweight-chat':'Lightweight chat call'};
      return map[m]||(m?('LLM call ('+m+')'):'LLM call');
    }
    /* Render an estimated USD cost with enough precision to stay meaningful. */
    function fmtCostUsd(v){
      if(typeof v!=='number'||!isFinite(v))return'—';
      if(v===0)return'$0';
      return v<0.01?('$'+v.toFixed(4)):('$'+v.toFixed(2));
    }
    function shortSession(id){
      if(!id)return'';
      return String(id).replace(/^session-/,'').slice(0,8);
    }
    /* A single cost-ledger trace row: When (relative+absolute), What
       (call type / model / tokens / cost), and Who (context + session).
       `abs` is parse-guarded (mirrors renderGenericTrace) so formatTime's
       raw-passthrough branch can never feed an unescaped quote into the
       double-quoted `title` attribute below (#2351). */
    function renderCostTrace(data){
      data=data||{};
      const parsed=parseTs(data.timestamp);
      const when=data.timestamp?timeAgo(data.timestamp):'—';
      const abs=parsed?formatTime(data.timestamp):'';
      const pt=Number(data.prompt_tokens_est)||0;
      const ct=Number(data.completion_tokens_est)||0;
      const total=pt+ct;
      const model=data.model||'unknown';
      const dot=' <span style="color:#6e7681">·</span> ';
      let what='<strong>'+esc(costModelLabel(model))+'</strong>'+dot+esc(model);
      if(total>0) what+=dot+'~'+total.toLocaleString()+' tokens';
      what+=dot+'<span style="color:var(--green);font-weight:600">'+esc(fmtCostUsd(data.cost_usd_est))+'</span>';
      const whenHtml='<span title="'+esc(abs)+'" style="color:#8b949e;font-size:.75rem;white-space:nowrap;margin-left:.5rem">'+esc(when)+'</span>';
      let who=[];
      if(data.context) who.push(esc(String(data.context)));
      const sid=shortSession(data.session_id);
      if(sid) who.push('session '+esc(sid));
      if(total>0) who.push(pt.toLocaleString()+' in / '+ct.toLocaleString()+' out');
      const whoHtml=who.length?'<div style="color:#6e7681;font-size:.72rem;margin-top:2px">'+who.join(' <span style="color:#30363d">·</span> ')+'</div>':'';
      return '<div style="border-bottom:1px solid var(--border);padding:6px 0;font-size:.82rem;white-space:normal;line-height:1.5">'
        +'<span style="display:inline-block;padding:0 6px;border-radius:8px;background:#1f6feb33;color:#58a6ff;font-size:.7rem;font-weight:600;vertical-align:middle">cost</span> '
        +'<span style="vertical-align:middle">'+what+'</span> '+whenHtml+whoHtml+'</div>';
    }
    /* Non-cost trace rows (journald / in-process spans): keep the source
       badge but route timestamps through the shared timeAgo/formatTime
       helpers and drop the indent-padding that leaked into the pre-wrap box. */
    function renderGenericTrace(s){
      s=s||{};const data=s.data||{};
      const rawTs=data.timestamp||data.timestamp_epoch_ms||data.__REALTIME_TIMESTAMP||data._SOURCE_REALTIME_TIMESTAMP||'';
      const parsed=parseTs(rawTs);
      const when=parsed?timeAgo(rawTs):(rawTs?String(rawTs).substring(0,19):'');
      const abs=parsed?formatTime(rawTs):'';
      const msg=data.MESSAGE||data.message||data.description||data.name||data.model||JSON.stringify(data).substring(0,200);
      const whenHtml=when?'<span title="'+esc(abs)+'" style="color:var(--accent);margin:0 .5rem;font-size:.75rem;white-space:nowrap">'+esc(when)+'</span>':'';
      return '<div style="border-bottom:1px solid var(--border);padding:6px 0;font-size:.82rem;white-space:normal;line-height:1.5">'
        +'<span style="color:#8b949e">['+esc(s.source)+']</span>'+whenHtml+'<span>'+esc(String(msg))+'</span></div>';
    }

    /* --- Memory Growth History (#2136) --- */
    async function fetchMemoryHistory(){
      const deltasEl=document.getElementById('mem-growth-deltas');
      const trendEl=document.getElementById('mem-growth-trend');
      const rateEl=document.getElementById('mem-growth-rate');
      const sparkEl=document.getElementById('mem-growth-sparkline');
      try{
        const d=await apiFetch('/api/memory/history');
        if(d.error){
          deltasEl.innerHTML='<span class="err" style="font-size:.85rem">'+esc(d.error)+'</span>';
          trendEl.textContent='';
          return;
        }
        // Trend badge
        const trendIcons={growing:'↑ Growing',shrinking:'↓ Shrinking',stable:'→ Stable',unknown:'—'};
        const trendColors={growing:'#3fb950',shrinking:'#f85149',stable:'#d29922',unknown:'#8b949e'};
        // Derive the badge from the same signed long-term rate the panel shows
        // below, so "↑ Growing" can never sit next to a negative rate (#2358).
        const ltRate=(d.rate_per_hour&&typeof d.rate_per_hour.long_term_total==='number')?d.rate_per_hour.long_term_total:null;
        let trend;
        if(ltRate!==null){
          trend=Math.abs(ltRate)<0.1?'stable':(ltRate>0?'growing':'shrinking');
        }else{
          trend=d.trend||'unknown';
        }
        trendEl.textContent=trendIcons[trend]||'—';
        trendEl.style.color=trendColors[trend]||'#8b949e';

        // Delta badges
        if(d.deltas){
          const dl=d.deltas;
          const cats=[
            {key:'episodic',label:'Episodic',color:'#3fb950'},
            {key:'semantic',label:'Semantic',color:'#58a6ff'},
            {key:'procedural',label:'Procedural',color:'#a371f7'},
            {key:'prospective',label:'Prospective',color:'#d29922'},
            {key:'working',label:'Working',color:'#f0883e'},
            {key:'sensory',label:'Sensory',color:'#8b949e'},
          ];
          const intervalSecs=dl.interval_secs||0;
          const intervalLabel=intervalSecs>0?' ('+humanizeDuration(intervalSecs)+')':'';
          deltasEl.innerHTML=cats.map(c=>{
            const v=dl[c.key]||0;
            const sign=v>0?'+':'';
            const bg=v!==0?c.color+'22':'#21262d';
            const fg=v!==0?c.color:'#484f58';
            return '<span data-testid="mem-delta-'+c.key+'" style="display:inline-block;padding:2px 8px;border-radius:4px;font-size:.8rem;font-weight:600;background:'+bg+';color:'+fg+'">'+c.label+' '+sign+v+'</span>';
          }).join('')+'<span style="font-size:.7rem;color:#484f58;align-self:center">since prev sample'+intervalLabel+'</span>';
        }else{
          deltasEl.innerHTML='<span style="color:#8b949e;font-size:.85rem">Not enough samples yet — growth data appears after two snapshots</span>';
        }

        // Growth rate
        if(d.rate_per_hour){
          const r=d.rate_per_hour.long_term_total||0;
          const rDisp=Math.abs(r)<0.1?'0':r.toFixed(1);
          rateEl.innerHTML='<div style="font-size:1.5rem;font-weight:700;color:#58a6ff;line-height:1">'+rDisp+'</div><div style="font-size:.75rem;color:#8b949e;margin-top:.15rem">long-term mem/hr</div>';
        }

        // SVG sparkline from snapshots
        const snaps=d.snapshots||[];
        if(snaps.length>=2){
          const vals=snaps.map(s=>s.long_term_total||0);
          const minV=Math.min(...vals);
          const maxV=Math.max(...vals);
          const range=maxV-minV||1;
          const w=400,h=48,pad=2;
          const accentColor='#58a6ff';
          const pts=vals.map((v,i)=>{
            const x=(i/(vals.length-1))*w;
            const y=h-pad-((v-minV)/range)*(h-2*pad);
            return x.toFixed(1)+','+y.toFixed(1);
          });
          const polyPts=pts.join(' ');
          const fillPts=polyPts+','+w+','+(h-pad)+' 0,'+(h-pad);
          sparkEl.innerHTML='<defs><linearGradient id=\'mem-spark-grad\' x1=\'0\' y1=\'0\' x2=\'0\' y2=\'1\'><stop offset=\'0%\' stop-color=\''+accentColor+'\' stop-opacity=\'0.3\'/><stop offset=\'100%\' stop-color=\''+accentColor+'\' stop-opacity=\'0.02\'/></linearGradient></defs>'
            +'<polyline points=\''+polyPts+'\' fill=\'none\' stroke=\''+accentColor+'\' stroke-width=\'1.5\' vector-effect=\'non-scaling-stroke\'/>'
            +'<polyline points=\''+fillPts+'\' fill=\'url(#mem-spark-grad)\' stroke=\'none\'/>';
        }else{
          sparkEl.innerHTML='<text x=\'200\' y=\'28\' text-anchor=\'middle\' fill=\'#484f58\' font-size=\'12\'>Collecting samples…</text>';
        }
      }catch(e){
        deltasEl.innerHTML='<span class="err" style="font-size:.85rem">Failed to load growth data</span>';
      }
    }

    /* --- Recent Memories (plain-English view, #1997) --- */
    async function fetchRecentMemories(){
      const countEl=document.getElementById('mem-recent-count');
      const totalEl=document.getElementById('mem-recent-total');
      const listEl=document.getElementById('mem-recent-list');
      listEl.innerHTML='<span class="loading">Loading recent memories…</span>';
      try{
        const d=await apiFetch('/api/memory/recent');
        if(d.error){listEl.innerHTML='<span class="err">'+esc(d.error)+'</span>';countEl.textContent='—';return;}
        countEl.textContent=d.last_hour_count;
        totalEl.textContent=(d.total||0).toLocaleString()+' total';
        if(!d.items||d.items.length===0){
          const total=d.total||0;
          listEl.innerHTML=total>0
            ?'<span style="color:#8b949e">No new memories in the last hour — '+total.toLocaleString()+' total stored.</span>'
            :'<span style="color:#8b949e">No memories stored yet. Simard will remember things as it works.</span>';
          return;
        }
        const catColors={'Learned fact':'#58a6ff','Past event':'#3fb950','Current task context':'#f0883e','How-to knowledge':'#a371f7','Planned reminder':'#d29922','Recent observation':'#8b949e'};
        listEl.innerHTML=d.items.map(item=>{
          const color=catColors[item.category]||'#8b949e';
          const ts=item.timestamp?timeAgo(item.timestamp):'';
          const summaryText=esc(item.summary).substring(0,200);
          return`<div style="border-bottom:1px solid var(--border);padding:.45rem 0;font-size:.85rem">
            <span style="display:inline-block;padding:1px 6px;border-radius:3px;font-size:.7rem;font-weight:600;background:${color}22;color:${color};margin-right:.5rem">${esc(item.category)}</span>
            <span style="color:#8b949e;font-size:.75rem;float:right">${ts}</span>
            <div style="margin-top:.2rem;color:var(--fg)">${summaryText}</div>
          </div>`;
        }).join('');
      }catch(e){
        countEl.textContent='—';
        listEl.innerHTML='<span class="err">Failed to load — check /api/memory/recent</span>';
      }
    }

    /* --- Memory Search --- */
    async function searchMemory(){
      const q=document.getElementById('mem-search-input').value.trim();
      if(!q){document.getElementById('mem-search-results').innerHTML='<span class="warn">Enter a search term</span>';return;}
      document.getElementById('mem-search-results').innerHTML='<span class="loading">Searching…</span>';
      try{
        const d=await apiFetch('/api/memory/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({query:q})});
        if(d.results?.length){
          document.getElementById('mem-search-results').innerHTML=`
            <p style="color:#8b949e;font-size:.85rem">${d.result_count} result(s) for "${esc(d.query)}"</p>
            ${d.results.map(sr=>`<div style="border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem">
              <span class="badge">${esc(sr.source)}</span>
              <pre style="margin:.5rem 0 0;white-space:pre-wrap;font-size:.8rem;color:var(--fg)">${esc(JSON.stringify(sr.data,null,2).substring(0,500))}</pre>
            </div>`).join('')}`;
        }else{
          document.getElementById('mem-search-results').innerHTML=`<span style="color:#8b949e">No results for "${esc(q)}" — try broader terms</span>`;
        }
      }catch(e){document.getElementById('mem-search-results').innerHTML='<span class="err">Search failed — check /api/memory/search</span>';}
    }
    document.getElementById('mem-search-input')?.addEventListener('keypress',e=>{if(e.key==='Enter')searchMemory();});

    /* --- Memory Graph Visualization --- */
    let mgNodes=[],mgEdges=[],mgFiltered=[],mgFilteredEdges=[];
    let mgDrag=null,mgPinned=null;
    let mgOffX=0,mgOffY=0,mgScale=1,mgPanX=0,mgPanY=0;
    const mgColors={WorkingMemory:'#f0883e',SemanticFact:'#58a6ff',EpisodicMemory:'#3fb950',ProceduralMemory:'#a371f7',ProspectiveMemory:'#d29922',SensoryBuffer:'#8b949e'};

    function mgApplyFilters(){
      const checks={};
      document.querySelectorAll('.mem-filter').forEach(cb=>{checks[cb.dataset.type]=cb.checked;});
      mgFiltered=mgNodes.filter(n=>{
        if(checks[n.type]===false)return false;
        const lbl=(n.label||'').toLowerCase();
        if(lbl.indexOf('goal-board:snapshot')>=0)return false;
        return true;
      });
      const ids=new Set(mgFiltered.map(n=>n.id));
      mgFilteredEdges=mgEdges.filter(e=>ids.has(e.source)&&ids.has(e.target));
      mgRender();
    }
    document.querySelectorAll('.mem-filter').forEach(cb=>cb.addEventListener('change',mgApplyFilters));

    async function fetchMemoryGraph(){
      try{
        const d=await apiFetch('/api/memory/graph');
        if(d.error){document.getElementById('mem-graph-stats').textContent='Error: '+d.error;return;}
        const s=d.stats||{};
        document.getElementById('mem-graph-stats').textContent=
          'Thinking:'+(s.working||0)+' Facts:'+(s.semantic||0)+' Events:'+(s.episodic||0)+' Procedures:'+(s.procedural||0)+' Planned:'+(s.prospective||0)+' Observed:'+(s.sensory||0);
        mgNodes=(d.nodes||[]);mgEdges=(d.edges||[]);
        mgInitLayout();mgApplyFilters();mgSimulate();
      }catch(e){document.getElementById('mem-graph-stats').textContent='Load failed';}
    }

    function mgInitLayout(){
      const canvas=document.getElementById('mem-graph-canvas');
      const w=canvas.clientWidth||800,h=canvas.clientHeight||500;
      mgPanX=0;mgPanY=0;mgScale=1;
      const n=mgNodes.length||1;
      mgNodes.forEach((nd,i)=>{
        const angle=(2*Math.PI*i)/n;
        const radius=Math.min(w,h)*0.3;
        nd.x=w/2+radius*Math.cos(angle);
        nd.y=h/2+radius*Math.sin(angle);
        nd.vx=0;nd.vy=0;nd.pinned=false;
      });
    }

    function mgSimulate(){
      const canvas=document.getElementById('mem-graph-canvas');
      const dt=0.3,repulsion=800,springLen=100,springK=0.02,gravity=0.01,damping=0.85;
      const cx=(canvas.clientWidth||800)/2,cy=(canvas.clientHeight||500)/2;
      for(let iter=0;iter<120;iter++){
        for(let i=0;i<mgFiltered.length;i++){
          if(mgFiltered[i].pinned)continue;
          let fx=0,fy=0;
          for(let j=0;j<mgFiltered.length;j++){
            if(i===j)continue;
            let dx=mgFiltered[i].x-mgFiltered[j].x,dy=mgFiltered[i].y-mgFiltered[j].y;
            let dist=Math.sqrt(dx*dx+dy*dy)||1;
            let f=repulsion/(dist*dist);
            fx+=f*dx/dist;fy+=f*dy/dist;
          }
          fx+=(cx-mgFiltered[i].x)*gravity;
          fy+=(cy-mgFiltered[i].y)*gravity;
          mgFiltered[i].vx=(mgFiltered[i].vx+fx*dt)*damping;
          mgFiltered[i].vy=(mgFiltered[i].vy+fy*dt)*damping;
          mgFiltered[i].x+=mgFiltered[i].vx*dt;
          mgFiltered[i].y+=mgFiltered[i].vy*dt;
        }
        const nodeMap={};mgFiltered.forEach(n=>{nodeMap[n.id]=n;});
        mgFilteredEdges.forEach(e=>{
          const a=nodeMap[e.source],b=nodeMap[e.target];
          if(!a||!b)return;
          let dx=b.x-a.x,dy=b.y-a.y;
          let dist=Math.sqrt(dx*dx+dy*dy)||1;
          let f=(dist-springLen)*springK;
          let fx2=f*dx/dist,fy2=f*dy/dist;
          if(!a.pinned){a.vx+=fx2*dt;a.vy+=fy2*dt;}
          if(!b.pinned){b.vx-=fx2*dt;b.vy-=fy2*dt;}
        });
      }
      mgRender();
    }

    function mgRender(){
      const canvas=document.getElementById('mem-graph-canvas');
      if(!canvas)return;
      canvas.width=canvas.clientWidth*(window.devicePixelRatio||1);
      canvas.height=canvas.clientHeight*(window.devicePixelRatio||1);
      const ctx=canvas.getContext('2d');
      const dpr=window.devicePixelRatio||1;
      ctx.scale(dpr,dpr);
      ctx.clearRect(0,0,canvas.clientWidth,canvas.clientHeight);
      ctx.save();ctx.translate(mgPanX,mgPanY);ctx.scale(mgScale,mgScale);
      const nodeMap={};mgFiltered.forEach(n=>{nodeMap[n.id]=n;});
      mgFilteredEdges.forEach(e=>{
        const a=nodeMap[e.source],b=nodeMap[e.target];
        if(!a||!b)return;
        ctx.beginPath();ctx.moveTo(a.x,a.y);ctx.lineTo(b.x,b.y);
        ctx.strokeStyle='rgba(88,166,255,0.35)';ctx.lineWidth=1.5;ctx.stroke();
      });
      const r=8;
      mgFiltered.forEach(n=>{
        const lblLow=(n.label||'').toLowerCase();
        const isGoal=lblLow.indexOf('goal')>=0;
        const nr=isGoal?12:r;
        ctx.beginPath();ctx.arc(n.x,n.y,n===mgPinned?nr+3:nr,0,Math.PI*2);
        ctx.fillStyle=isGoal?'#FFD700':(mgColors[n.type]||'#8b949e');
        if(n===mgPinned){ctx.lineWidth=2;ctx.strokeStyle='#fff';ctx.stroke();}
        ctx.fill();
        const lbl=n.label||'';
        if(lbl.length>0&&mgScale>0.5){
          ctx.fillStyle='#c9d1d9';ctx.font='10px sans-serif';ctx.textAlign='center';
          ctx.fillText(lbl.substring(0,30),n.x,n.y-nr-4);
        }
      });
      ctx.restore();
    }

    (function(){
      const mgCanvas=document.getElementById('mem-graph-canvas');
      if(!mgCanvas)return;
      function mgHitTest(mx,my){
        const x=(mx-mgPanX)/mgScale,y=(my-mgPanY)/mgScale;
        for(const n of mgFiltered){if((n.x-x)**2+(n.y-y)**2<144)return n;}
        return null;
      }
      mgCanvas.addEventListener('mousemove',function(e){
        const rect=mgCanvas.getBoundingClientRect();
        const mx=e.clientX-rect.left,my=e.clientY-rect.top;
        if(mgDrag){mgDrag.x=(mx-mgOffX-mgPanX)/mgScale;mgDrag.y=(my-mgOffY-mgPanY)/mgScale;mgRender();return;}
        const node=mgHitTest(mx,my);
        const tip=document.getElementById('mem-graph-tooltip');
        if(node){
          mgCanvas.style.cursor='pointer';tip.style.display='block';
          tip.style.left=Math.min(mx+12,mgCanvas.clientWidth-330)+'px';tip.style.top=(my+12)+'px';
          tip.innerHTML='<strong style="color:'+(mgColors[node.type]||'#ccc')+'">'+esc(node.type)+'</strong><br>'+esc((node.content||'').substring(0,200));
        }else{mgCanvas.style.cursor='grab';tip.style.display='none';}
      });
      mgCanvas.addEventListener('mousedown',function(e){
        const rect=mgCanvas.getBoundingClientRect();const mx=e.clientX-rect.left,my=e.clientY-rect.top;
        const node=mgHitTest(mx,my);
        if(node){mgDrag=node;mgCanvas.style.cursor='grabbing';mgOffX=mx-node.x*mgScale-mgPanX;mgOffY=my-node.y*mgScale-mgPanY;}
        else{
          const startPX=mgPanX,startPY=mgPanY,sx=e.clientX,sy=e.clientY;
          function onMove(ev){mgPanX=startPX+(ev.clientX-sx);mgPanY=startPY+(ev.clientY-sy);mgRender();}
          function onUp(){window.removeEventListener('mousemove',onMove);window.removeEventListener('mouseup',onUp);}
          window.addEventListener('mousemove',onMove);window.addEventListener('mouseup',onUp);
        }
      });
      mgCanvas.addEventListener('mouseup',function(){mgDrag=null;mgCanvas.style.cursor='grab';});
      mgCanvas.addEventListener('click',function(e){
        const rect=mgCanvas.getBoundingClientRect();const node=mgHitTest(e.clientX-rect.left,e.clientY-rect.top);
        if(node){
          mgPinned=node;node.pinned=true;
          document.getElementById('mem-graph-detail').style.display='block';
          document.getElementById('mg-detail-title').textContent=node.type;
          document.getElementById('mg-detail-body').innerHTML=
            '<div class="stat"><span class="label">ID</span><span class="value" style="font-size:.75rem;word-break:break-all">'+esc(node.id)+'</span></div>'+
            '<div class="stat"><span class="label">Label</span><span class="value">'+esc(node.label)+'</span></div>'+
            '<div style="margin-top:.5rem;font-size:.8rem;color:#c9d1d9;white-space:pre-wrap;max-height:300px;overflow-y:auto">'+esc(node.content||'')+'</div>';
          mgRender();
        }else{
          if(mgPinned){mgPinned.pinned=false;mgPinned=null;}
          document.getElementById('mem-graph-detail').style.display='none';mgRender();
        }
      });
      mgCanvas.addEventListener('wheel',function(e){
        e.preventDefault();const rect=mgCanvas.getBoundingClientRect();
        const mx=e.clientX-rect.left,my=e.clientY-rect.top;
        const factor=e.deltaY<0?1.1:0.9;
        mgPanX=mx-(mx-mgPanX)*factor;mgPanY=my-(my-mgPanY)*factor;
        mgScale*=factor;mgRender();
      },{passive:false});
    })();

    /* --- Costs --- */
    function fmtLabel(k){
      const map={
        'period':'Period','entry_count':'API Calls',
        'total_prompt_tokens':'Prompt Tokens','total_completion_tokens':'Completion Tokens',
        'total_cost_usd':'Estimated Cost'};
      return map[k]||k.replace(/_/g,' ').replace(/\b\w/g,c=>c.toUpperCase());
    }
    async function fetchCosts(){
      try{
        const d=await apiFetch('/api/costs');
        function renderSummary(s){
          if(!s||s.error) return `<span class="err">${esc(s?.error||'No cost data — is cost tracking configured?')}</span>`;
          return Object.entries(s).map(([k,v])=>{
            if(v==null)return'';
            if(typeof v==='object')return`<div class="stat"><span class="label">${esc(fmtLabel(k))}</span><span class="value" style="font-size:.8rem">${esc(JSON.stringify(v))}</span></div>`;
            const isCost=k.toLowerCase().includes('cost_usd');
            const isTokens=k.toLowerCase().includes('token');
            const isPeriod=k==='period';"#;
