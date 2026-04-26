pub(crate) const PART_02: &str = r#"          if(d.ooda_transcripts?.length){
            tEl.innerHTML=d.ooda_transcripts.map(t=>`
              <div class="transcript-item">
                <h3>${esc(t.name)} <span class="badge">${fmtB(t.size_bytes)}</span></h3>
                <div class="log-box" style="max-height:200px">${esc((t.preview_lines||[]).join('\n'))||'(empty)'}</div>
              </div>`).join('');
          }else{tEl.innerHTML='<span style="color:#8b949e">No OODA transcripts found in state root.</span>';}
        }
        // Render cycle reports
        const crEl=document.getElementById('cycle-reports');
        if(crEl){
          if(d.cycle_reports?.length){
            crEl.innerHTML=d.cycle_reports.map(c=>{
              const num=c.cycle_number;
              const text=c.summary||JSON.stringify(c.report||{});
              return`<div class="transcript-item">
                <h3>Cycle #${num}</h3>
                <div class="log-box" style="max-height:100px">${esc(text)}</div>
              </div>`;
            }).join('');
          }else{crEl.innerHTML='<span style="color:#8b949e">No cycle reports found. Run the OODA daemon to generate cycle data.</span>';}
        }
        const ttEl=document.getElementById('terminal-transcripts');
        if(ttEl){
          if(d.terminal_transcripts?.length){
            ttEl.innerHTML=d.terminal_transcripts.map(t=>`
              <div class="transcript-item">
                <h3>${esc(t.name)} <span class="badge">${fmtB(t.size_bytes)}</span></h3>
                <div class="log-box" style="max-height:200px">${esc((t.preview_lines||[]).join('\n'))||'(empty)'}</div>
              </div>`).join('');
          }else{ttEl.innerHTML='<span style="color:#8b949e">No terminal session transcripts found.</span>';}
        }
        const costEl=document.getElementById('cost-log-box');
        if(costEl){
          if(d.cost_log_lines?.length){
            costEl.textContent=d.cost_log_lines.join('\n');
            costEl.scrollTop=costEl.scrollHeight;
          }else{costEl.innerHTML='<span style="color:#8b949e">No cost ledger entries</span>';}
        }
      }catch(e){const dl=document.getElementById('daemon-log'); if(dl){dl.textContent='Failed to load logs — check /api/logs endpoint';}}
    }
    function applyLogFilter(){
      const filter=(document.getElementById('log-filter')?.value||'').toLowerCase();
      const level=(document.getElementById('log-level-filter')?.value||'').toLowerCase();
      let lines=allLogLines;
      if(filter) lines=lines.filter(l=>l.toLowerCase().includes(filter));
      if(level) lines=lines.filter(l=>l.toLowerCase().includes(level));
      const el=document.getElementById('daemon-log');
      el.textContent=lines.length?lines.join('\n'):'(no matching log lines)';
      el.scrollTop=el.scrollHeight;
      const countEl=document.getElementById('log-line-count');
      if(countEl) countEl.textContent=`${lines.length}/${allLogLines.length} lines`;
    }
    document.getElementById('log-filter')?.addEventListener('input',applyLogFilter);
    document.getElementById('log-level-filter')?.addEventListener('change',applyLogFilter);

    /* --- Process Tree --- */
    function renderTreeNode(node, isLast, depth) {
      if (!node) return '';
      const hasChildren = node.children && node.children.length > 0;
      const toggleCls = hasChildren ? 'proc-toggle' : 'proc-toggle leaf';
      const toggleChar = hasChildren ? '▼' : '·';
      const stateClass = (node.state || 'unknown').replace(/\s+/g, '-');
      const cmdDisplay = esc(node.command || '').length > 80
        ? esc(node.command).substring(0, 77) + '…'
        : esc(node.command || '');
      let html = `<div class="proc-node" data-pid="${node.pid}">
        <div class="proc-row">
          <span class="${toggleCls}" onclick="toggleProcChildren(this)">${toggleChar}</span>
          <span class="proc-pid">${node.pid}</span>
          <span class="proc-state ${stateClass}">${esc(node.state)}</span>
          <span class="proc-cpu">${node.cpu_pct?.toFixed(1) ?? '—'}%</span>
          <span class="proc-mem">${node.memory_mb != null ? node.memory_mb.toFixed(1) + 'M' : '—'}</span>
          <span class="proc-cmd" title="${esc(node.command)}">${cmdDisplay}</span>
        </div>`;
      if (hasChildren) {
        html += '<div class="proc-children">';
        node.children.forEach((child, i) => {
          html += renderTreeNode(child, i === node.children.length - 1, depth + 1);
        });
        html += '</div>';
      }
      html += '</div>';
      return html;
    }

    function toggleProcChildren(el) {
      const node = el.closest('.proc-node');
      const childDiv = node.querySelector(':scope > .proc-children');
      if (!childDiv) return;
      const collapsed = childDiv.classList.toggle('collapsed');
      el.textContent = collapsed ? '▶' : '▼';
    }

    async function fetchProcessTree() {
      try {
        const d=await apiFetch('/api/processes');
        const container = document.getElementById('proc-tree-container');
        const summary = document.getElementById('proc-tree-summary');
        if (!container) return;
        const procs = d.processes || [];
        if (procs.length) {
          const rootLabel = d.root_pid ? ` — OODA daemon PID ${d.root_pid}` : '';
          if (summary) summary.textContent = `${procs.length} process(es)${rootLabel} — updated ${timeAgo(d.timestamp)}`;
          // Build tree from flat list using ppid
          const byPid = {};
          procs.forEach(p => { byPid[p.pid] = { ...p, children: [] }; });
          const roots = [];
          // The OODA root's ppid won't be in our set, so it becomes a root.
          // Any other process whose ppid isn't in our set is also a root,
          // but with the descendant-walk backend this should only be the daemon.
          procs.forEach(p => {
            const node = byPid[p.pid];
            if (p.ppid && byPid[p.ppid]) {
              byPid[p.ppid].children.push(node);
            } else {
              roots.push(node);
            }
          });
          function renderNode(n, depth) {
            const indent = depth * 20;
            const hasKids = n.children.length > 0;
            const toggle = hasKids
              ? `<span class="proc-toggle" onclick="this.parentElement.parentElement.querySelector('.proc-kids').classList.toggle('collapsed');this.textContent=this.textContent==='▼'?'▶':'▼'" style="cursor:pointer;user-select:none;width:1em;display:inline-block">▼</span>`
              : `<span style="width:1em;display:inline-block;color:#484f58">·</span>`;
            const isRoot = n.is_ooda_root === true;
            const label = isRoot ? '🤖 Simard OODA Daemon' : '';
            const cmd = esc(n.full_args || n.command || '');
            const cmdShort = cmd.length > 90 ? cmd.substring(0,87)+'…' : cmd;
            const rootBadge = isRoot ? `<span style="background:#238636;color:#fff;padding:1px 6px;border-radius:4px;font-size:.75rem;margin-right:4px">${label}</span>` : '';
            let html = `<div class="proc-row" style="padding-left:${indent}px">
              ${toggle}
              <span class="proc-pid">${esc(n.pid)}</span>
              ${rootBadge}
              <span class="proc-uptime" style="color:#8b949e;font-size:.8rem;min-width:80px">${esc(n.uptime||'')}</span>
              <span class="proc-cmd" title="${cmd}" style="color:#c9d1d9">${cmdShort}</span>
            </div>`;
            if (hasKids) {
              html += '<div class="proc-kids">';
              n.children.forEach(c => { html += renderNode(c, depth+1); });
              html += '</div>';
            }
            return html;
          }
          container.innerHTML = '<div class="proc-tree">' + roots.map(r => renderNode(r, 0)).join('') + '</div>';
        } else {
          if (summary) summary.textContent = d.timestamp ? `Updated ${timeAgo(d.timestamp)}` : '';
          container.innerHTML = '<span style="color:#8b949e">No Simard-related processes found. Is the daemon running?</span>';
        }
      } catch(e) {
        const c = document.getElementById('proc-tree-container');
        if (c) c.innerHTML = '<span class="err">Failed to load process tree: ' + esc(e.toString()) + '</span>';
      }
    }

    /* --- Memory --- */
    async function fetchMemory(){
      try{
        const d=await apiFetch('/api/memory');
        let overviewHtml=`
          <div class="stat"><span class="label">Total Facts</span><span class="value">${d.total_facts}</span></div>
          <div class="stat"><span class="label">Last Consolidation</span><span class="value">${d.last_consolidation?timeAgo(d.last_consolidation)+' ('+new Date(d.last_consolidation).toLocaleString()+')':'Never'}</span></div>
          <div class="stat"><span class="label">State Root</span><span class="value" style="font-size:.8rem;word-break:break-all">${esc(d.state_root)}</span></div>`;
        if(d.native_memory){
          const nm=d.native_memory;
          overviewHtml+=`
          <h3 style="color:var(--accent);font-size:.9rem;margin-top:.75rem;border-top:1px solid var(--border);padding-top:.5rem">LadybugDB (Native Memory)</h3>
          <div class="stat"><span class="label">Sensory</span><span class="value">${nm.sensory}</span></div>
          <div class="stat"><span class="label">Working</span><span class="value">${nm.working}</span></div>
          <div class="stat"><span class="label">Episodic</span><span class="value">${nm.episodic}</span></div>
          <div class="stat"><span class="label">Semantic (Facts)</span><span class="value">${nm.semantic}</span></div>
          <div class="stat"><span class="label">Procedural</span><span class="value">${nm.procedural}</span></div>
          <div class="stat"><span class="label">Prospective</span><span class="value">${nm.prospective}</span></div>
          <div class="stat"><span class="label"><strong>Total Native</strong></span><span class="value"><strong>${nm.total}</strong></span></div>`;
        }
        document.getElementById('mem-overview').innerHTML=overviewHtml;
        const files=[
          {key:'memory_records',label:'Memory Records'},
          {key:'evidence_records',label:'Evidence Records'},
          {key:'goal_records',label:'Goal Records'},
          {key:'handoff',label:'Latest Handoff'}];
        document.getElementById('mem-files').innerHTML=files.map(f=>{
          const info=d[f.key]||{};
          const modStr=info.modified?timeAgo(info.modified):'N/A';
          return`<div class="mem-file">
            <h3>${f.label} ${info.count!==undefined?'<span class="badge">'+info.count+' records</span>':''} <span class="badge">${fmtB(info.size_bytes||0)}</span></h3>
            <div class="stat"><span class="label">Modified</span><span class="value">${modStr}</span></div>
          </div>`;}).join('');
      }catch(e){document.getElementById('mem-overview').innerHTML='<span class="err">Failed to load memory data — check state root path</span>';}
    }

    /* --- Distributed --- */
    async function fetchDistributed(){
      document.getElementById('cluster-topology').innerHTML='<span class="loading">Querying remote VMs… (this may take 10-30s)</span>';
      try{
        const d=await apiFetch('/api/distributed');
        const eb=d.event_bus;
        const emDash='\u2014';
        const fmtTs=v=>(v==null?emDash:v);
        const fmtRate=v=>(v==null?'0':(Math.round(v*100)/100).toString());
        let ebBlock='';
        if(eb){
          const topics=eb.topics||{};
          const rows=Object.keys(topics).sort().map(name=>{
            const t=topics[name]||{};
            return `<li data-testid="event-bus-topic-${esc(name)}">${esc(name)}: ${t.subscribers||0} subs, ${fmtRate(t.events_per_min)}/min, last ${esc(fmtTs(t.last_event_timestamp))}</li>`;
          }).join('');
          ebBlock=`
          <div class="event-bus-stats" style="margin-top:1rem;padding-top:.75rem;border-top:1px solid var(--border)">
            <h3 style="margin:0 0 .5rem 0;color:var(--accent);font-size:1rem">Event Bus</h3>
            <div class="stat" data-testid="event-bus-total-subscribers"><span class="label">Subscribers</span><span class="value">${eb.total_subscribers||0}</span></div>
            <div class="stat" data-testid="event-bus-events-per-min"><span class="label">Events/min</span><span class="value">${fmtRate(eb.events_per_min)}</span></div>
            <div class="stat" data-testid="event-bus-last-event"><span class="label">Last event</span><span class="value">${esc(fmtTs(eb.last_event_timestamp))}</span></div>
            <ul style="margin:.5rem 0 0 1rem;padding:0;font-size:.85rem;color:#8b949e">${rows}</ul>
          </div>`;
        }
        document.getElementById('cluster-topology').innerHTML=`
          <div class="stat"><span class="label">Topology</span><span class="value">${esc(d.topology)}</span></div>
          <div class="stat"><span class="label">Local Host</span><span class="value">${esc(d.local?.hostname||'?')}</span></div>
          <div class="stat"><span class="label">Memory Sync</span><span class="value">${esc(d.hive_mind?.protocol||'DHT+bloom gossip')}</span></div>
          <div class="stat"><span class="label">Hive Status</span><span class="value ${d.hive_mind?.status==='active'?'ok':'warn'}">${esc(d.hive_mind?.status||'standalone')}</span></div>
          ${d.hive_mind?.peers!=null?`<div class="stat"><span class="label">Peers</span><span class="value">${d.hive_mind.peers}</span></div>`:''}
          ${d.hive_mind?.facts_shared!=null?`<div class="stat"><span class="label">Facts Shared</span><span class="value">${d.hive_mind.facts_shared}</span></div>`:''}
          <div class="stat"><span class="label">Updated</span><span class="value">${timeAgo(d.timestamp)}</span></div>${ebBlock}`;
        if(d.remote_vms?.length){
          document.getElementById('remote-vms').innerHTML=d.remote_vms.map(vm=>{
            const sc=vm.status==='reachable'?'ok':(vm.status==='unreachable'?'err':'warn');
            const hasWorkloads=(vm.simard_processes||0)>0||(vm.cargo_processes||0)>0;
            return`<div style="border:1px solid var(--border);border-radius:6px;padding:1rem;margin-bottom:.75rem">
              <div style="display:flex;justify-content:space-between;align-items:center">
                <h3 style="margin:0 0 .5rem 0;color:var(--accent)">${esc(vm.vm_name)} <span class="${sc}" style="font-size:.85rem">${esc(vm.status)}</span></h3>
                <div style="display:flex;gap:.5rem">
                  ${hasWorkloads?`<button class="btn" style="font-size:.75rem;padding:2px 8px" onclick="vacateVM('${esc(vm.vm_name)}')">🚚 Vacate</button>`:''}
                  <button class="btn" style="font-size:.75rem;padding:2px 8px;color:#f85149" onclick="removeVM('${esc(vm.vm_name)}')">✕ Remove</button>
                </div>
              </div>
              ${vm.hostname?`<div class="stat"><span class="label">Hostname</span><span class="value">${esc(vm.hostname)}</span></div>`:''}
              ${vm.uptime?`<div class="stat"><span class="label">Uptime</span><span class="value">${esc(vm.uptime)}</span></div>`:''}
              ${vm.load_avg?`<div class="stat"><span class="label">Load</span><span class="value">${esc(vm.load_avg)}</span></div>`:''}
              ${vm.memory_mb?`<div class="stat"><span class="label">Memory</span><span class="value">${esc(vm.memory_mb)} MB</span></div>`:''}
              ${vm.disk_root_pct!=null?`<div class="stat"><span class="label">Root Disk</span><span class="value ${vm.disk_root_pct>90?'err':vm.disk_root_pct>70?'warn':'ok'}">${vm.disk_root_pct}%</span></div>`:''}
              ${vm.disk_data_pct!=null?`<div class="stat"><span class="label">Data Disk</span><span class="value">${vm.disk_data_pct}%</span></div>`:''}
              ${vm.disk_tmp_pct!=null?`<div class="stat"><span class="label">Tmp Disk</span><span class="value">${vm.disk_tmp_pct}%</span></div>`:''}
              ${vm.simard_processes!=null?`<div class="stat"><span class="label">Simard Processes</span><span class="value">${vm.simard_processes}</span></div>`:''}
              ${vm.cargo_processes!=null?`<div class="stat"><span class="label">Cargo Processes</span><span class="value">${vm.cargo_processes}</span></div>`:''}
              ${vm.error?`<div class="stat"><span class="label">Error</span><span class="value err">${esc(vm.error)}</span></div>`:''}
            </div>`;}).join('');
        }else{document.getElementById('remote-vms').innerHTML='<span style="color:#8b949e">No remote VMs configured. Add hosts below.</span>';}
      }catch(e){document.getElementById('cluster-topology').innerHTML='<span class="err">Failed to query distributed status — check network and azlin</span>';}
    }
    async function vacateVM(vmName){
      if(!confirm(`Vacate "${vmName}"? This will:\n1. Stop all Simard processes on the VM\n2. Export cognitive memory snapshot\n3. Transfer workloads to this host\n\nProceed?`))return;
      const el=document.getElementById('remote-vms');
      const origHtml=el.innerHTML;
      el.innerHTML=`<span class="loading">Vacating ${esc(vmName)}… stopping processes and exporting memory</span>`;
      try{
        const d=await apiFetch('/api/vm/vacate',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({vm_name:vmName})});
        if(d.status==='ok'){
          el.innerHTML=`<div class="ok" style="padding:1rem">✓ ${esc(vmName)} vacated. ${d.message||''}</div>`;
          setTimeout(fetchDistributed,3000);
        }else{
          el.innerHTML=origHtml;
          alert('Vacate failed: '+(d.error||'unknown error'));
        }
      }catch(e){el.innerHTML=origHtml;alert('Vacate error: '+e);}
    }
    async function removeVM(vmName){
      if(!confirm(`Remove "${vmName}" from the cluster? This only removes it from the dashboard — it does not deallocate the Azure VM.`))return;
      try{
        await apiFetch('/api/hosts',{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:vmName})});
        fetchDistributed();
        fetchHosts();
      }catch(e){alert('Remove error: '+e);}
    }
    async function fetchHosts(){
      try{
        const d=await apiFetch('/api/hosts');
        const el=document.getElementById('hosts-list');
        let html='';

        // Discovered VMs from azlin
        const discovered=d.discovered||[];
        const configuredNames=new Set((d.hosts||[]).map(h=>h.name));
        if(discovered.length){
          html+=`<div style="margin-bottom:.75rem"><div style="font-weight:600;font-size:.85rem;margin-bottom:.4rem;color:var(--accent)">Available VMs (${discovered.length})</div>`;
          html+=`<table class="proc-table"><tr><th>Name</th><th>Location</th><th>Resource Group</th><th>Status</th><th></th></tr>`;
          html+=discovered.map(vm=>{
            const name=esc(vm.name||vm.Name||'');
            const loc=esc(vm.location||vm.Location||'');
            const rg=esc(vm.resourceGroup||vm.resource_group||vm.ResourceGroup||'');
            const isConfigured=configuredNames.has(vm.name||vm.Name||'');
            return`<tr>
              <td><strong>${name}</strong></td>
              <td>${loc}</td>
              <td style="font-size:.8rem;color:#8b949e">${rg}</td>
              <td>${isConfigured?'<span class="ok">configured</span>':'<span style="color:#8b949e">available</span>'}${vm.is_local?' <span class="ok">joined</span>':''}</td>
              <td>${!isConfigured?`<button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="quickAddHost('${name}','${rg}')">+ Add</button>`:''}</td>
            </tr>`;
          }).join('');
          html+=`</table></div>`;
        }

        // Configured hosts
        if(d.hosts?.length){
          html+=`<div style="margin-top:.5rem"><div style="font-weight:600;font-size:.85rem;margin-bottom:.4rem">Configured Hosts (${d.hosts.length})</div>`;
          html+=d.hosts.map(h=>{
            const name=esc(h.name||'');
            return`<div style="display:flex;align-items:center;gap:0.5rem;padding:4px 0;border-bottom:1px solid var(--border)">
              <span style="flex:1"><strong>${name}</strong> <span style="color:#8b949e">(${esc(h.resource_group||'default')})</span> ${h.is_local?'<span class="ok">joined</span> ':''}<span style="color:#8b949e;font-size:.75rem">${timeAgo(h.added_at)}</span></span>
              <button class="btn" style="padding:2px 8px;font-size:.8rem" data-host="${name}">Remove</button>
            </div>`;
          }).join('');
          html+=`</div>`;
        }

        if(!html){html='<span style="color:#8b949e">No hosts discovered or configured. Ensure azlin is installed, or add a VM name below.</span>';}
        el.innerHTML=html;
        el.querySelectorAll('button[data-host]').forEach(btn=>{
          btn.addEventListener('click',()=>removeHost(btn.dataset.host));
        });
      }catch(e){document.getElementById('hosts-list').innerHTML='<span class="err">Failed to load hosts</span>';}
    }
    function quickAddHost(name,rg){
      apiFetch('/api/hosts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:name,resource_group:rg||'rysweet-linux-vm-pool'})})
        .then(d=>{if(d.status==='ok'){fetchHosts();fetchDistributed();}else alert(d.error||'Failed');}).catch(e=>alert('Error: '+e));
    }
    async function addHost(){
      const name=document.getElementById('host-name').value.trim();
      const rg=document.getElementById('host-rg').value.trim();
      if(!name){document.getElementById('host-status').textContent='Name required';return;}
      try{
        const d=await apiFetch('/api/hosts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name,resource_group:rg})});
        document.getElementById('host-status').textContent=d.status==='ok'?'Added ✓':'Error: '+(d.error||'');
        document.getElementById('host-name').value='';
        fetchHosts();
        fetchDistributed();
        setTimeout(()=>document.getElementById('host-status').textContent='',3000);
      }catch(e){document.getElementById('host-status').textContent='Network error';}
    }
    async function removeHost(name){
      if(!confirm('Remove host "'+name+'"?'))return;
      await apiFetch('/api/hosts',{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({name})});
      fetchHosts();
      fetchDistributed();
    }
    fetchHosts();

    /* --- Goals --- */
    async function fetchGoals(){
      try{"#;
