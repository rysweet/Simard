#!/usr/bin/env python3
"""Dashboard audit pass 01 (issue #1662 follow-up).

One-shot Playwright-driven audit:

1. Read dashkey from ~/.simard/.dashkey and POST /api/login to obtain a session cookie.
2. Visit http://localhost:8080/, dynamically enumerate every top-level nav tab/route
   (no hard-coded tab list — walk whatever the running dashboard surfaces).
3. For each route: full-page screenshot to out/<slug>.png and visible text dump
   to out/<slug>.txt.
4. Write out/_index.json with the resolved nav -> {url, screenshot, text} mapping.

Re-run with:  python scripts/dashboard_audit/audit_pass_01.py

Requires: playwright (install via .venv-audit/bin/pip install playwright==1.59.0)
          and the system Chromium under ~/.cache/ms-playwright/chromium-* .
"""

from __future__ import annotations

import json
import re
import sys
import time
from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import (
    Browser,
    BrowserContext,
    Page,
    TimeoutError as PWTimeout,
    sync_playwright,
)

BASE_URL = "http://localhost:8080"
DASHKEY_PATH = Path.home() / ".simard" / ".dashkey"
OUT_DIR = Path(__file__).resolve().parent / "out"
NAV_WAIT_MS = 2500
PANEL_SETTLE_MS = 1500


def slugify(text: str) -> str:
    s = re.sub(r"[^a-zA-Z0-9._-]+", "-", text.strip().lower()).strip("-")
    return s or "untitled"


def login(context: BrowserContext) -> None:
    if not DASHKEY_PATH.exists():
        sys.exit(f"FATAL: dashkey not found at {DASHKEY_PATH}")
    code = DASHKEY_PATH.read_text().strip()
    if not code:
        sys.exit(f"FATAL: dashkey at {DASHKEY_PATH} is empty")
    api = context.request
    resp = api.post(f"{BASE_URL}/api/login", data={"code": code})
    if not resp.ok:
        sys.exit(f"FATAL: /api/login returned {resp.status}: {resp.text()}")
    body = resp.json()
    if not body.get("ok"):
        sys.exit(f"FATAL: login rejected: {body!r}")
    cookies = context.cookies(BASE_URL)
    if not any(c["name"] == "simard_session" for c in cookies):
        sys.exit("FATAL: login succeeded but no simard_session cookie was set")
    print(f"[auth] logged in; {len(cookies)} cookie(s) for {BASE_URL}")


def discover_nav(page: Page) -> list[dict[str, str]]:
    """Return [{label, href, slug}] for every distinct top-level nav target.

    We try multiple selectors so this works even if the SPA is restyled:
    explicit <nav> anchors first, then anything with role=tab / [data-tab],
    then any anchor under a header. Dedup by href.
    """
    page.goto(f"{BASE_URL}/", wait_until="domcontentloaded")
    try:
        page.wait_for_load_state("networkidle", timeout=8000)
    except PWTimeout:
        pass
    page.wait_for_timeout(NAV_WAIT_MS)

    js = r"""
    () => {
      const seen = new Map();
      const push = (label, href, kind) => {
        if (!label || !href) return;
        const key = href.split('#')[0] + '#' + (href.split('#')[1] || '');
        if (seen.has(key)) return;
        seen.set(key, { label: label.trim(), href, kind });
      };
      // Strategy 1: explicit <nav> anchors
      document.querySelectorAll('nav a[href], header nav a[href]').forEach(a => {
        push(a.innerText || a.getAttribute('aria-label') || a.title, a.getAttribute('href'), 'nav-anchor');
      });
      // Strategy 2: role=tab / data-tab (SPA tabs that change hash or path)
      document.querySelectorAll('[role="tab"], [data-tab], .tab, button.tab, .nav-tab').forEach(el => {
        const label = el.innerText || el.getAttribute('aria-label') || el.title || el.dataset.tab;
        const href = el.getAttribute('href') || ('#' + (el.dataset.tab || el.id || (label||'').toLowerCase().replace(/\s+/g,'-')));
        push(label, href, 'tab');
      });
      // Strategy 3: any top-of-page anchor (header region)
      document.querySelectorAll('header a[href], .header a[href], .topbar a[href]').forEach(a => {
        push(a.innerText, a.getAttribute('href'), 'header-anchor');
      });
      return Array.from(seen.values());
    }
    """
    raw = page.evaluate(js)
    nav: list[dict[str, str]] = []
    used_slugs: set[str] = set()
    for i, item in enumerate(raw):
        href = item["href"]
        if href.startswith("javascript:") or href.startswith("mailto:"):
            continue
        # Skip external links — only audit the local dashboard.
        if href.startswith("http"):
            parsed = urlparse(href)
            if parsed.netloc and parsed.netloc not in (
                urlparse(BASE_URL).netloc,
                "localhost:8080",
                "127.0.0.1:8080",
            ):
                continue
        label = item["label"] or f"tab-{i}"
        slug_base = slugify(label) or f"tab-{i}"
        slug = slug_base
        n = 2
        while slug in used_slugs:
            slug = f"{slug_base}-{n}"
            n += 1
        used_slugs.add(slug)
        nav.append({"label": label, "href": href, "slug": slug, "kind": item["kind"]})
    # Always include the root page itself so we capture it even if no nav exists.
    if not any(n["href"] in ("/", "", "#") for n in nav):
        nav.insert(0, {"label": "root", "href": "/", "slug": "00-root", "kind": "synthetic"})
    return nav


def visit(page: Page, target: dict[str, str], index: int) -> dict[str, str]:
    href = target["href"]
    slug = f"{index:02d}-{target['slug']}"
    if href.startswith("#"):
        # SPA hash route — change hash in place, then click the matching tab
        # (SPAs typically listen for hashchange OR for the click). Try both so
        # we don't depend on which event the dashboard wires up.
        page.evaluate(
            "(h) => { window.location.hash = h; }", href.lstrip("#") and href[1:]
        )
        page.evaluate(
            r"""(targetHref) => {
                const candidates = Array.from(document.querySelectorAll(
                  'a[href], [data-tab], [role="tab"]'
                ));
                for (const el of candidates) {
                  const h = el.getAttribute('href') || ('#' + (el.dataset.tab || ''));
                  if (h === targetHref) { el.click(); return true; }
                }
                return false;
            }""",
            href,
        )
    elif href.startswith("/"):
        page.goto(f"{BASE_URL}{href}", wait_until="domcontentloaded")
    else:
        page.goto(href, wait_until="domcontentloaded")
    try:
        page.wait_for_load_state("networkidle", timeout=8000)
    except PWTimeout:
        pass
    page.wait_for_timeout(PANEL_SETTLE_MS)

    png_path = OUT_DIR / f"{slug}.png"
    txt_path = OUT_DIR / f"{slug}.txt"
    try:
        page.screenshot(path=str(png_path), full_page=True)
    except Exception as exc:  # pragma: no cover — best-effort capture
        png_path.write_text(f"screenshot failed: {exc}\n")
    text = page.evaluate("() => document.body ? document.body.innerText : ''")
    txt_path.write_text(text or "")
    print(f"[visit] {slug:40s} -> {len(text or ''):6d} chars text  href={href}")
    return {
        "label": target["label"],
        "href": href,
        "slug": slug,
        "kind": target.get("kind", ""),
        "screenshot": str(png_path.relative_to(OUT_DIR.parent)),
        "text_file": str(txt_path.relative_to(OUT_DIR.parent)),
        "text_chars": len(text or ""),
    }


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    started = time.time()
    with sync_playwright() as p:
        browser: Browser = p.chromium.launch(headless=True)
        context: BrowserContext = browser.new_context(
            viewport={"width": 1440, "height": 900},
            ignore_https_errors=True,
        )
        login(context)
        page = context.new_page()
        nav = discover_nav(page)
        print(f"[nav]   discovered {len(nav)} target(s):")
        for item in nav:
            print(f"        - {item['label']!r:30s} {item['kind']:14s} {item['href']}")
        # Make sure we're on the SPA root before iterating hash routes so
        # subsequent hash changes don't trigger full reloads.
        page.goto(f"{BASE_URL}/", wait_until="domcontentloaded")
        try:
            page.wait_for_load_state("networkidle", timeout=8000)
        except PWTimeout:
            pass
        page.wait_for_timeout(NAV_WAIT_MS)
        results = []
        for i, target in enumerate(nav):
            try:
                results.append(visit(page, target, i))
            except Exception as exc:  # pragma: no cover — keep auditing
                print(f"[visit] FAILED {target!r}: {exc}")
                results.append({
                    "label": target["label"],
                    "href": target["href"],
                    "slug": target["slug"],
                    "error": str(exc),
                })
        index_path = OUT_DIR / "_index.json"
        index_path.write_text(json.dumps({
            "base_url": BASE_URL,
            "captured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "duration_seconds": round(time.time() - started, 2),
            "nav": nav,
            "results": results,
        }, indent=2))
        print(f"[done]  wrote {index_path} ({len(results)} captures, "
              f"{round(time.time()-started,1)}s)")
        context.close()
        browser.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
