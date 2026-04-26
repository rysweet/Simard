pub(crate) const PART_03: &str = r#"        const d=await apiFetch('/api/goals');
        if(d.active?.length){
          document.getElementById('goals-active').innerHTML=`<table class="proc-table">
            <tr><th>Priority</th><th>ID</th><th>Description</th><th>Status</th><th>Current Activity</th><th>Actions</th></tr>
            ${d.active.map(g=>{
              let wipHtml='—';
              if(g.current_activity||g.wip_refs?.length){
                let parts=[];
                if(g.current_activity) parts.push('<div style="font-size:.8rem">'+esc(g.current_activity)+'</div>');
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
              <td>${esc(g.status)}</td>
              <td>${wipHtml}</td>
              <td>
                <button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="demoteGoal('${esc(g.id)}')">▼ Backlog</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px" onclick="updateGoalStatus('${esc(g.id)}')">Status</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px;color:#f85149" onclick="removeGoal('${esc(g.id)}')">✕</button>
              </td>
            </tr>`;}).join('')}
          </table>
          <div style="margin-top:.5rem;color:#8b949e;font-size:.8rem">${d.active_count} active goal(s)</div>`;
        }else{document.getElementById('goals-active').innerHTML='<span style="color:#8b949e">No active goals. Use "Seed Default Goals" or run the OODA daemon to generate goals from meetings.</span>';}
        if(d.backlog?.length){
          document.getElementById('goals-backlog').innerHTML=`<table class="proc-table">
            <tr><th>ID</th><th>Description</th><th>Source</th><th>Score</th><th>Actions</th></tr>
            ${d.backlog.map(b=>`<tr>
              <td><code>${esc(b.id)}</code></td>
              <td>${esc(b.description)}</td>
              <td>${esc(b.source||'')}</td>
              <td>${b.score??'—'}</td>
              <td>
                <button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="promoteGoal('${esc(b.id)}')">▲ Promote</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px" onclick="removeGoal('${esc(b.id)}')">✕</button>
              </td>
            </tr>`).join('')}
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
          document.getElementById('trace-list').innerHTML=d.spans.map(s=>{
            const data=s.data;
            const ts=data.timestamp||data.__REALTIME_TIMESTAMP||data._SOURCE_REALTIME_TIMESTAMP||'';
            const msg=data.MESSAGE||data.message||data.description||data.model||JSON.stringify(data).substring(0,200);
            return`<div style="border-bottom:1px solid var(--border);padding:4px 0;font-size:.82rem">
              <span style="color:#8b949e">[${esc(s.source)}]</span>
              ${ts?'<span style="color:var(--accent);margin:0 .5rem">'+esc(String(ts).substring(0,19))+'</span>':''}
              <span>${esc(String(msg))}</span>
            </div>`;
          }).join('');
        }else{document.getElementById('trace-list').innerHTML='<span style="color:#8b949e">No trace data yet. Run the OODA daemon or make API calls to generate traces.</span>';}
      }catch(e){document.getElementById('trace-list').innerHTML='<span class="err">Failed to load traces — check /api/traces</span>';}
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
          'W:'+(s.working||0)+' S:'+(s.semantic||0)+' E:'+(s.episodic||0)+' P:'+(s.procedural||0)+' Pr:'+(s.prospective||0)+' Se:'+(s.sensory||0);
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
            const isTokens=k.toLowerCase().includes('token');"#;
