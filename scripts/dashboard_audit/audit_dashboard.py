#!/usr/bin/env python3
"""First-pass dashboard audit + REPORT.md generator (issue #1990) — sibling to
audit_pass_01.py: additionally captures per-page HTTP/console errors, scans every
DOM dump for jargon, emits out/REPORT.md + out/_audit_dashboard_index.json (the
latter deliberately disjoint from audit_pass_01.py's _index.json)."""
from __future__ import annotations
import json, os, re, sys, time
from pathlib import Path
from urllib.parse import urlparse
from playwright.sync_api import sync_playwright, TimeoutError as PWTimeout  # noqa: F401
BASE_URL = "http://localhost:8080"
DASHKEY_PATH = Path.home() / ".simard" / ".dashkey"
OUT_DIR = Path(__file__).resolve().parent / "out"
MAX_ROUTES, TIMEOUT_MS = 50, 8000
FALLBACK_TABS = ("overview", "goals", "traces", "logs", "processes", "memory", "costs", "chat", "whiteboard", "thinking", "terminal")
JARGON_TERMS = ("OODA", "OODA loop", "cognitive memory", "handoff bundle", "facilitator", "recipe runner", "consolidation", "episodic", "semantic memory", "procedural memory", "LadybugDB", "spawn_engineer", "workboard", "whiteboard")
_fb = os.environ.get("SIMARD_PROMPT_ASSETS_DIR", "").strip()
if _fb:
    assert (_f := Path(_fb).resolve()) != (_o := OUT_DIR.resolve()) and _f not in _o.parents, "OUT_DIR under SIMARD_PROMPT_ASSETS_DIR"
assert "/prompt_assets/" not in str(OUT_DIR.resolve()), "OUT_DIR under prompt_assets"
def _slug(s): return re.sub(r"[^a-z0-9_-]+", "-", (s or "").lower().strip()).strip("-") or "page"
def _norm(h):
    if h.startswith("#"): return "#" + h.lstrip("#/").split("?")[0].strip("/")
    if h.startswith("/"): return "/" + h.lstrip("/").split("?")[0].split("#")[0].strip("/")
    return h
def _t(fn, d=None):
    try: return fn()
    except Exception: return d
def load_key():
    if not DASHKEY_PATH.exists(): raise SystemExit(f"FATAL: dashkey not found at {DASHKEY_PATH}")
    k = DASHKEY_PATH.read_text().strip()
    if not (1 <= len(k) <= 256): raise SystemExit(f"FATAL: dashkey at {DASHKEY_PATH} fails 1<=len<=256 sanity envelope")
    return k
def authenticate(context, key):
    r = context.request.post(f"{BASE_URL}/api/login", data={"code": key})
    if not getattr(r, "ok", False): raise SystemExit(f"FATAL: /api/login status={getattr(r,'status','?')}")
    try: cookies = context.cookies(BASE_URL)
    except TypeError: cookies = context.cookies()
    if any((c or {}).get("name") == "simard_session" for c in (cookies or [])): return
    m = re.search(r"simard_session=([^;]+)", (getattr(r, "headers", {}) or {}).get("set-cookie", "") or "")
    if m: context.add_cookies([{"name": "simard_session", "value": m.group(1), "url": BASE_URL}])
def discover_routes(page):
    _t(lambda: page.goto(f"{BASE_URL}/", wait_until="domcontentloaded"))
    _t(lambda: page.wait_for_load_state("networkidle", timeout=TIMEOUT_MS))
    _t(lambda: page.wait_for_timeout(1200))
    raw = _t(lambda: page.evaluate(r"""() => { const out=[],seen=new Set(),P=(l,h)=>{if(!h)return;const k=(l||'').trim()+'|'+h;if(seen.has(k))return;seen.add(k);out.push({label:(l||'').trim(),href:h});};
      document.querySelectorAll('a[href]').forEach(a=>P(a.innerText||a.title,a.getAttribute('href')));
      document.querySelectorAll('[role="tab"],[data-tab],.tab,.nav-tab').forEach(el=>{const l=el.innerText||el.getAttribute('aria-label')||el.dataset.tab||el.id||'';P(l,el.getAttribute('href')||('#/'+(el.dataset.tab||l.toLowerCase().replace(/\s+/g,'-'))));});
      return out;}"""), []) or []
    base_host = urlparse(BASE_URL).hostname
    seen, routes = set(), []
    for it in raw:
        h, lab = (it.get("href") or "").strip(), (it.get("label") or "").strip()
        if not h or h.startswith(("javascript:", "mailto:", "data:", "file:", "tel:")): continue
        if h.startswith("http"):
            pu = urlparse(h)
            if pu.hostname and pu.hostname != base_host: continue
            h = (pu.path or "/") + (("#" + pu.fragment) if pu.fragment else "")
        n = _norm(h)
        if n in seen: continue
        seen.add(n); routes.append({"label": lab or n, "href": h, "slug": _slug(lab or n)})
    for tab in FALLBACK_TABS:
        n = _norm("#/" + tab)
        if n in seen: continue
        seen.add(n); routes.append({"label": tab.title(), "href": "#/" + tab, "slug": _slug(tab)})
    if not any(_norm(r["href"]) in ("/", "#") for r in routes):
        routes.insert(0, {"label": "root", "href": "/", "slug": "root"})
    counts = {}
    for r in routes:
        c = counts[r["slug"]] = counts.get(r["slug"], 0) + 1
        if c > 1: r["slug"] = f"{r['slug']}_{c}"
    return routes[:MAX_ROUTES]
def _tab_slug_from_href(href):
    """Extract the data-tab slug from a hash href like '#/overview' or '#overview'."""
    return href.lstrip("#").lstrip("/").split("?")[0].split("/")[0].strip()

def capture_page(page, route):
    http_errs, console_errs = [], []
    on_r = lambda r: (getattr(r, "status", 0) >= 400) and http_errs.append({"url": getattr(r, "url", "?"), "status": r.status})
    on_c = lambda m: (getattr(m, "type", "") == "error") and console_errs.append({"text": getattr(m, "text", "")})
    page.on("response", on_r); page.on("console", on_c)
    try:
        href, slug = route["href"], route["slug"]
        if href.startswith("#"):
            tab_slug = _tab_slug_from_href(href)
            # Click the actual .tab element — the dashboard doesn't listen for
            # hashchange, it only switches tabs via click handlers on .tab[data-tab].
            clicked = _t(lambda: page.evaluate(r"""(slug) => {
                const tab = document.querySelector('.tab[data-tab="' + slug + '"]');
                if (tab) { tab.click(); return true; }
                // Fallback: try clicking any tab whose text matches the slug
                for (const el of document.querySelectorAll('.tab')) {
                    if ((el.textContent || '').trim().toLowerCase().replace(/[^a-z0-9]/g, '') ===
                        slug.toLowerCase().replace(/[^a-z0-9]/g, '')) {
                        el.click(); return true;
                    }
                }
                return false;
            }""", tab_slug), False)
            if not clicked:
                print(f"[capture] {slug}: no matching tab element for '{tab_slug}'")
            # Wait for the tab panel to become active
            try:
                page.wait_for_selector(f"#tab-{tab_slug}.active", timeout=5000)
            except Exception:
                pass  # not all tabs follow #tab-{slug} convention
        elif href.startswith("/"):
            _t(lambda: page.goto(f"{BASE_URL}{href}", wait_until="domcontentloaded"))
        _t(lambda: page.wait_for_load_state("networkidle", timeout=TIMEOUT_MS))
        _t(lambda: page.wait_for_timeout(1500))
        _t(lambda: page.screenshot(path=str(OUT_DIR / f"{slug}.png"), full_page=True))
        # Capture text from the active tab panel to get tab-specific content,
        # falling back to full body if no active panel is found.
        tab_slug = _tab_slug_from_href(href) if href.startswith("#") else ""
        text = _t(lambda: page.evaluate(r"""(slug) => {
            // Try the active tab-content panel first
            const panel = document.querySelector('.tab-content.active');
            if (panel && panel.innerText && panel.innerText.trim().length > 0) {
                return panel.innerText;
            }
            // Try by #tab-{slug}
            if (slug) {
                const byId = document.getElementById('tab-' + slug);
                if (byId && byId.innerText) return byId.innerText;
            }
            return document.body ? document.body.innerText : '';
        }""", tab_slug), "") or ""
        if not isinstance(text, str): text = str(text)
        title = _t(lambda: page.evaluate("()=>document.title"), "") or ""
        h1 = _t(lambda: page.evaluate(r"""(slug) => {
            // Look for h1 in the active tab panel first
            const panel = document.querySelector('.tab-content.active');
            if (panel) {
                const h = panel.querySelector('h1, .page-h1');
                if (h) return h.innerText.trim();
            }
            if (slug) {
                const byId = document.getElementById('tab-' + slug);
                if (byId) {
                    const h = byId.querySelector('h1, .page-h1');
                    if (h) return h.innerText.trim();
                }
            }
            const h = document.querySelector('h1');
            return h ? h.innerText.trim() : null;
        }""", tab_slug))
        (OUT_DIR / f"{slug}.txt").write_text(text)
        (OUT_DIR / f"{slug}.errors.json").write_text(json.dumps({"http": http_errs, "console": console_errs}, indent=2))
        return {"slug": slug, "href": href, "url": href, "label": route.get("label", slug),
                "title": title, "h1": h1 or None, "excerpt": text[:400], "text_chars": len(text),
                "http_errors": len(http_errs), "console_errors": len(console_errs)}
    finally:
        page.remove_listener("response", on_r); page.remove_listener("console", on_c)
def scan_jargon(text_dumps):
    return {term: hits for term in JARGON_TERMS
            for hits in [sorted({s for s, b in (text_dumps or {}).items() if term.lower() in (b or "").lower()})]
            if hits}
def _why(p):
    return (([f"only {p.get('text_chars',0)} chars of body text"] if (p.get("text_chars") or 0) < 200 else [])
            + (["no <h1>"] if not p.get("h1") else []))
def _score(p, j):
    return (3 * p.get("http_errors", 0) + 2 * p.get("console_errors", 0)
            + (2 if not p.get("h1") else 0) + (2 if (p.get("text_chars") or 0) < 200 else 0)
            + sum(1 for h in j.values() if p["slug"] in h))
def write_report(pages, jargon):
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    L = [f"# Dashboard audit — first pass (issue #1990)\n\n_Generated {time.strftime('%Y-%m-%d %H:%M UTC', time.gmtime())}; base {BASE_URL}_\n", "## 1. Pages found\n"]
    L += [f"- `{p['slug']}` — {p.get('label','?')} ({p.get('href','?')}) [{p.get('text_chars',0)} chars; "
          f"http_errors={p.get('http_errors',0)}, console_errors={p.get('console_errors',0)}]" for p in pages]
    L += ["\n## 2. What each page appears to convey\n"]
    L += [ln for p in pages for ln in (f"### `{p['slug']}` — {p.get('title') or p.get('label','?')}",
        f"- H1: {p.get('h1') or '(no <h1>)'}",
        f"- Excerpt: {((p.get('excerpt') or '').replace(chr(10),' ').strip()[:240])!r}\n")]
    L += ["## 3. Jargon inventory (terms that read as jargon to a non-Simard-developer)\n"]
    L += ([f"- **{t}** → {', '.join(h)}" for t, h in sorted(jargon.items(), key=lambda kv: (-len(kv[1]), kv[0].lower()))]
          if jargon else ["_No flagged jargon terms detected._"])
    L += ["\n## 4. Missing context — pages a human would struggle to interpret\n"]
    short = [p for p in pages if _why(p)]
    L += [f"- `{p['slug']}` — {', '.join(_why(p))}" for p in short] if short else ["_All pages cleared the basic-context heuristics._"]
    L += ["\n## 5. Top-5 highest-impact usability fixes (heuristic ranking — engineer curates)\n"]
    for i, p in enumerate(sorted(pages, key=lambda q: -_score(q, jargon))[:5], 1):
        L.append(f"{i}. **`{p['slug']}`** (heuristic score {_score(p, jargon)}) — add plain-English H1, "
                 f"de-jargon labels, surface 'what this is for', fix errors. Acceptance: a "
                 f"first-time visitor can describe in one sentence what `{p['slug']}` is for.")
    (OUT_DIR / "REPORT.md").write_text("\n".join(L) + "\n")
def main():
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={"width": 1440, "height": 900})
        authenticate(context, load_key())
        page = context.new_page()
        routes = discover_routes(page)
        print(f"[discover] {len(routes)} route(s)")
        results, dumps = [], {}
        for r in routes:
            res = _t(lambda r=r: capture_page(page, r))
            if res is None: print(f"[capture] {r['slug']}: FAILED"); continue
            results.append(res); dumps[res["slug"]] = (OUT_DIR / f"{res['slug']}.txt").read_text()
            print(f"[capture] {res['slug']:20s} {res['text_chars']:6d} chars")
        jargon = scan_jargon(dumps)
        write_report(results, jargon)
        (OUT_DIR / "_audit_dashboard_index.json").write_text(json.dumps(
            {"base": BASE_URL, "routes": routes, "pages": results, "jargon": jargon}, indent=2))
        context.close(); browser.close()
    return 0 if results else 2
if __name__ == "__main__":
    sys.exit(main())
