#!/usr/bin/env python3
"""Playwright smoke test for dashboard issues #1993 and #1994.

For every dashboard route (`/`, `/#/overview`, `/#/goals`, ...) assert:

* a unique non-empty `<title>` (issue #1993)
* a unique visible `<h1 class="page-title">` (issue #1993)
* a non-empty `.page-intro` lede (issue #1994)

Logs in via ``~/.simard/.dashkey`` like ``audit_pass_01.py``.

Usage::

    .venv-audit/bin/python scripts/dashboard_audit/test_titles_and_ledes.py
    # exit 0 on pass, non-zero with a per-route diagnosis on fail

Designed to be run against a live ``simard ooda run`` daemon on
``http://localhost:8080``. Set ``DASHBOARD_URL`` to override.

Requires: ``playwright==1.59.0`` (matches ``audit_pass_01.py``).
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

try:
    from playwright.sync_api import (
        Browser,
        BrowserContext,
        Page,
        TimeoutError as PWTimeout,
        sync_playwright,
    )
except ImportError:  # pragma: no cover — installer guidance
    sys.stderr.write(
        "FATAL: playwright not installed. Run:\n"
        "  python3 -m venv .venv-audit\n"
        "  .venv-audit/bin/pip install playwright==1.59.0\n"
        "  .venv-audit/bin/python -m playwright install chromium\n"
    )
    raise

BASE_URL = os.environ.get("DASHBOARD_URL", "http://localhost:8080").rstrip("/")
DASHKEY_PATH = Path.home() / ".simard" / ".dashkey"
SETTLE_MS = 700

# Every tab the dashboard surfaces, plus `whiteboard` (an alias for
# `workboard`) — the alias must resolve to the same pane but the
# audit page enumerates both labels.
ROUTES: list[str] = [
    "overview",
    "goals",
    "traces",
    "logs",
    "processes",
    "memory",
    "costs",
    "chat",
    "workboard",
    "thinking",
    "terminal",
]
ALIASES: dict[str, str] = {"whiteboard": "workboard"}


def login(context: BrowserContext) -> None:
    if not DASHKEY_PATH.exists():
        sys.exit(f"FATAL: dashkey not found at {DASHKEY_PATH}")
    code = DASHKEY_PATH.read_text().strip()
    if not code:
        sys.exit(f"FATAL: dashkey at {DASHKEY_PATH} is empty")
    resp = context.request.post(f"{BASE_URL}/api/login", data={"code": code})
    if not resp.ok:
        sys.exit(f"FATAL: /api/login returned {resp.status}: {resp.text()}")
    body = resp.json()
    if not body.get("ok"):
        sys.exit(f"FATAL: login rejected: {body!r}")


def capture(page: Page, route: str) -> dict[str, str | int]:
    target_pane = ALIASES.get(route, route)
    page.evaluate("(h) => { window.location.hash = h; }", f"#/{route}")
    page.wait_for_timeout(SETTLE_MS)
    # Wait until the JS hash router has activated the matching pane.
    try:
        page.wait_for_function(
            "id => !!document.querySelector('#tab-' + id + '.active')",
            arg=target_pane,
            timeout=4000,
        )
    except PWTimeout:
        pass
    title = page.title().strip()
    h1 = page.evaluate(
        """(id) => {
            const pane = document.getElementById('tab-' + id);
            if (!pane) return '';
            const h = pane.querySelector('h1.page-title, [data-page-title]');
            return h ? (h.textContent || '').trim() : '';
        }""",
        target_pane,
    )
    intro = page.evaluate(
        """(id) => {
            const pane = document.getElementById('tab-' + id);
            if (!pane) return '';
            const el = pane.querySelector('.page-intro');
            return el ? (el.textContent || '').trim() : '';
        }""",
        target_pane,
    )
    return {
        "route": route,
        "title": title,
        "h1": h1,
        "intro": intro,
        "intro_chars": len(intro or ""),
    }


def check(results: list[dict[str, str | int]]) -> list[str]:
    failures: list[str] = []
    titles: dict[str, list[str]] = {}
    h1s: dict[str, list[str]] = {}

    # Track which routes are aliases (same underlying pane, so the
    # H1 / title are *expected* to be identical across them). Only
    # canonical routes are checked for uniqueness.
    canonical_only = [r for r in results if r["route"] not in ALIASES]

    for r in canonical_only:
        route = r["route"]
        title = str(r["title"])
        h1 = str(r["h1"])
        intro = str(r["intro"])
        if not title:
            failures.append(f"[{route}] empty <title>")
        if title.lower() == "simard dashboard v2":
            failures.append(f"[{route}] <title> is the legacy generic value {title!r}")
        if not h1:
            failures.append(f"[{route}] no <h1 class='page-title'> in pane")
        if h1 and h1.lower() == "🌲 simard dashboard":
            failures.append(f"[{route}] <h1> still shows the brand mark, not a page name")
        if not intro:
            failures.append(f"[{route}] no .page-intro lede in pane")
        if intro and len(intro) < 30:
            failures.append(f"[{route}] .page-intro too short to be a sentence: {intro!r}")
        titles.setdefault(title, []).append(route)
        h1s.setdefault(h1, []).append(route)

    for title, routes in titles.items():
        if len(routes) > 1:
            failures.append(f"duplicate <title> {title!r} across routes: {routes}")
    for h1, routes in h1s.items():
        if len(routes) > 1:
            failures.append(f"duplicate <h1> {h1!r} across routes: {routes}")

    # Alias routes must resolve to the same pane content.
    for alias, canonical in ALIASES.items():
        a = next((r for r in results if r["route"] == alias), None)
        c = next((r for r in results if r["route"] == canonical), None)
        if a and c and a["h1"] != c["h1"]:
            failures.append(
                f"alias {alias} resolves to {a['h1']!r} but {canonical} resolves to {c['h1']!r}"
            )

    return failures


def main() -> int:
    started = time.time()
    with sync_playwright() as p:
        browser: Browser = p.chromium.launch(headless=True)
        context = browser.new_context(
            viewport={"width": 1440, "height": 900},
            ignore_https_errors=True,
        )
        login(context)
        page = context.new_page()
        page.goto(f"{BASE_URL}/", wait_until="domcontentloaded")
        try:
            page.wait_for_load_state("networkidle", timeout=8000)
        except PWTimeout:
            pass
        page.wait_for_timeout(SETTLE_MS)

        results: list[dict[str, str | int]] = []
        for route in [*ROUTES, *ALIASES.keys()]:
            try:
                results.append(capture(page, route))
            except Exception as exc:  # pragma: no cover
                results.append({"route": route, "error": str(exc)})

        context.close()
        browser.close()

    failures = check(results)
    duration = time.time() - started

    print(json.dumps({
        "base_url": BASE_URL,
        "duration_seconds": round(duration, 2),
        "results": results,
        "failures": failures,
    }, indent=2))

    if failures:
        print(f"\nFAIL ({len(failures)} issue(s)) in {duration:.1f}s", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1
    print(
        f"\nOK — {len(results)} routes checked, "
        f"{sum(1 for r in results if r.get('h1'))} have unique H1s, "
        f"{sum(1 for r in results if r.get('intro'))} have ledes "
        f"({duration:.1f}s)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
