//! The "Report bug / Request feature" feedback widget markup (#2629).
//!
//! A SINGLE control anchored in the shared dashboard `<header>` (so it appears
//! on every tab with consistent placement) plus the modal form, styles, and the
//! client JS that captures the current page context and POSTs it to the
//! auth-gated `/api/feedback` endpoint, then polls `/api/feedback/status/<id>`
//! for the launched workstream's PR.
//!
//! Assembled into the page by [`super::index_html_string`]:
//!
//! * [`FEEDBACK_WIDGET_BUTTON`] replaces the `{{FEEDBACK_WIDGET_BUTTON}}` marker
//!   inside the `<header>` (part_00).
//! * [`FEEDBACK_WIDGET_BODY`] is injected just before `</body>`.
//!
//! Kept as flat string consts (no nested `{{`) so it survives the
//! `index_html_string` template pass and a `grep` audit. All server/user data is
//! HTML-escaped client-side before `innerHTML`; the fetch uses
//! `credentials:'same-origin'` so the dashboard auth cookie is sent.

/// The header button that opens the feedback modal. Reuses the existing
/// `glossary-toggle` button styling for visual consistency across the header.
pub(crate) const FEEDBACK_WIDGET_BUTTON: &str = r##"<button id="feedback-widget-button" class="glossary-toggle" onclick="openFeedbackModal()" title="Report bug / Request feature" style="border-color:#3fb950;color:#3fb950">💬 Feedback</button>"##;

/// The feedback modal, its styles, and the capture/submit/poll client script.
pub(crate) const FEEDBACK_WIDGET_BODY: &str = r##"<style>
    #feedback-modal{display:none;position:fixed;inset:0;background:rgba(1,4,9,0.7);z-index:2000;align-items:flex-start;justify-content:center}
    #feedback-modal .fb-card{background:var(--card);border:1px solid var(--border);border-radius:10px;padding:1.25rem;margin-top:8vh;width:min(560px,92vw);box-shadow:0 12px 32px rgba(0,0,0,0.45)}
    #feedback-modal h2{color:var(--accent);font-size:1.05rem;margin:0 0 .25rem}
    #feedback-modal .fb-sub{color:#8b949e;font-size:.8rem;margin:0 0 1rem;line-height:1.4}
    #feedback-modal label{display:block;color:#c9d1d9;font-size:.8rem;font-weight:600;margin:.6rem 0 .25rem}
    #feedback-modal select,#feedback-modal input,#feedback-modal textarea{width:100%;padding:.5rem;border:1px solid var(--border);border-radius:6px;background:#010409;color:var(--fg);font-size:.9rem;font-family:inherit}
    #feedback-modal textarea{resize:vertical;min-height:96px}
    #feedback-modal .fb-actions{display:flex;justify-content:flex-end;gap:.5rem;margin-top:1rem}
    #feedback-modal .fb-btn{padding:.5rem 1.1rem;border:none;border-radius:6px;font-weight:600;cursor:pointer;font-size:.85rem}
    #feedback-modal .fb-btn.primary{background:#3fb950;color:#0d1117}
    #feedback-modal .fb-btn.secondary{background:transparent;color:#8b949e;border:1px solid var(--border)}
    #feedback-modal .fb-btn:hover{opacity:.9}
    #feedback-result{margin-top:.85rem;font-size:.82rem;color:#9bb1c4;line-height:1.5;word-break:break-word;min-height:1.2rem}
    #feedback-result a{color:var(--accent)}
  </style>
  <div id="feedback-modal" role="dialog" aria-modal="true" aria-label="Report bug or request feature">
    <div class="fb-card">
      <h2>Report bug / Request feature</h2>
      <p class="fb-sub">Send this straight to Simard. On submit we capture the current page and its visible data, then start a new dev-orchestrator workstream from your report.</p>
      <form id="feedback-form">
        <label for="feedback-type">Type</label>
        <select id="feedback-type">
          <option value="bug">Report bug</option>
          <option value="feature">Request feature</option>
        </select>
        <label for="feedback-title">Title</label>
        <input id="feedback-title" type="text" maxlength="200" placeholder="Short summary" required>
        <label for="feedback-description">Description</label>
        <textarea id="feedback-description" maxlength="5000" placeholder="What happened, or what would you like?" required></textarea>
        <div class="fb-actions">
          <button type="button" id="feedback-cancel" class="fb-btn secondary" onclick="closeFeedbackModal()">Cancel</button>
          <button type="submit" id="feedback-submit" class="fb-btn primary">Start workstream</button>
        </div>
      </form>
      <div id="feedback-result" aria-live="polite"></div>
    </div>
  </div>
  <script>
    function fbEsc(s){
      return String(s==null?'':s)
        .replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')
        .replace(/"/g,'&quot;').replace(/'/g,'&#39;');
    }
    function openFeedbackModal(){
      var m=document.getElementById('feedback-modal');
      if(!m)return;
      m.style.display='flex';
      var r=document.getElementById('feedback-result');
      if(r)r.textContent='';
      var t=document.getElementById('feedback-title');
      if(t)t.focus();
    }
    function closeFeedbackModal(){
      var m=document.getElementById('feedback-modal');
      if(m)m.style.display='none';
    }
    function fbActivePage(){
      var el=document.querySelector('.tab-content.active');
      if(el&&el.id){return el.id.replace('tab-','');}
      return 'overview';
    }
    function fbCaptureState(){
      var el=document.querySelector('.tab-content.active');
      var txt=el?(el.innerText||el.textContent||''):'';
      return txt.slice(0,16000);
    }
    function fbPollStatus(id,out){
      if(!id||!out)return;
      var url='/api/feedback/status/'+encodeURIComponent(id);
      var tries=0;
      var timer=setInterval(function(){
        tries++;
        fetch(url,{credentials:'same-origin'})
          .then(function(r){return r.json();})
          .then(function(j){
            if(!j){return;}
            if(j.state==='pr'){
              clearInterval(timer);
              var prUrl=String(j.pr_url||'');
              if(/^https:\/\/github\.com\/[^/]+\/[^/]+\/pull\/\d+$/.test(prUrl)){
                out.innerHTML='Workstream <code>'+fbEsc(id)+'</code> opened a PR: <a href="'+fbEsc(prUrl)+'" target="_blank" rel="noopener">'+fbEsc(prUrl)+'</a>';
              }else{
                out.textContent='Workstream '+id+' opened a PR.';
              }
            }else if(j.state==='failed'){
              clearInterval(timer);
              out.textContent='Workstream '+id+' finished without a PR yet.';
            }else{
              out.textContent='Workstream '+id+' is running…';
            }
            if(tries>=60){clearInterval(timer);}
          })
          .catch(function(){});
      },5000);
    }
    (function(){
      var form=document.getElementById('feedback-form');
      if(!form)return;
      var modal=document.getElementById('feedback-modal');
      if(modal){
        modal.addEventListener('click',function(ev){if(ev.target===modal)closeFeedbackModal();});
      }
      document.addEventListener('keydown',function(ev){if(ev.key==='Escape')closeFeedbackModal();});
      form.addEventListener('submit',function(ev){
        ev.preventDefault();
        var type=(document.getElementById('feedback-type')||{}).value||'bug';
        var title=((document.getElementById('feedback-title')||{}).value||'').trim();
        var description=((document.getElementById('feedback-description')||{}).value||'').trim();
        var out=document.getElementById('feedback-result');
        if(!title||!description){if(out)out.textContent='Please fill in both a title and a description.';return;}
        var page=fbActivePage();
        var ids={page:page,url:location.pathname,hash:location.hash,doc_title:document.title};
        var report={type:type,title:title,description:description};
        var context={page:page,state:fbCaptureState(),timestamp:new Date().toISOString(),identifiers:ids};
        var btn=document.getElementById('feedback-submit');
        if(btn)btn.disabled=true;
        if(out)out.textContent='Starting workstream…';
        fetch('/api/feedback',{
          method:'POST',
          credentials:'same-origin',
          headers:{'Content-Type':'application/json'},
          body:JSON.stringify({report:report,context:context})
        })
          .then(function(r){return r.json().then(function(j){return {status:r.status,body:j};});})
          .then(function(res){
            if(btn)btn.disabled=false;
            var b=res.body||{};
            if(res.status===202&&b.ok){
              if(out)out.textContent='Workstream started ('+b.workstream_id+'). Watching for a PR…';
              document.getElementById('feedback-title').value='';
              document.getElementById('feedback-description').value='';
              fbPollStatus(b.workstream_id,out);
            }else if(res.status===429){
              if(out)out.textContent='That report was just submitted, or too many are in flight. Please wait a moment.';
            }else{
              if(out)out.textContent='Could not start a workstream: '+(b.error||('HTTP '+res.status));
            }
          })
          .catch(function(){
            if(btn)btn.disabled=false;
            if(out)out.textContent='Network error submitting feedback.';
          });
      });
    })();
  </script>
"##;
