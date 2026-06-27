#!/usr/bin/env python3
"""Capture before/after evidence that the dashboard no longer contradicts itself.

Drives a running Simard dashboard with Playwright (Chromium) and, for the
Workboard and Overview tabs, records:

  * the "Cycle #N" header (Workboard) / OODA loop header (Overview)   — #1680
  * the "Active Engineers" panel                                      — #1678
  * the "Working Memory" panel slot count                            — #1679

A full-page screenshot per tab is written to ``out/<label>-<tab>.png`` and a
one-line text summary of the three contested values is appended to
``out/<label>-summary.txt`` so the values can be diffed across runs.

Usage::

    python scripts/dashboard_audit/contradiction_evidence.py \\
        --url http://localhost:8080 --label before
    python scripts/dashboard_audit/contradiction_evidence.py \\
        --url http://localhost:18901 --label after

The dashkey is read from ``$SIMARD_DASHKEY`` or ``~/.simard/.dashkey``.

Tracks issues #1678, #1679, #1680 (epic #1992).
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys

from playwright.sync_api import sync_playwright

OUT_DIR = pathlib.Path(__file__).resolve().parent / "out"


def read_dashkey() -> str:
    key = os.environ.get("SIMARD_DASHKEY", "").strip()
    if key:
        return key
    keyfile = pathlib.Path.home() / ".simard" / ".dashkey"
    return keyfile.read_text(encoding="utf-8").strip()


def find_chromium() -> str | None:
    """Locate an installed Chromium, tolerating Playwright build-number skew."""
    env = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE")
    if env and pathlib.Path(env).exists():
        return env
    cache = pathlib.Path.home() / ".cache" / "ms-playwright"
    for pattern in ("chromium-*/chrome-linux*/chrome",
                    "chromium_headless_shell-*/chrome-headless-shell-linux*/chrome-headless-shell"):
        matches = sorted(cache.glob(pattern))
        if matches:
            return str(matches[-1])
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://localhost:8080")
    parser.add_argument("--label", default="run", help="filename prefix, e.g. before/after")
    args = parser.parse_args()

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    key = read_dashkey()
    chromium_exe = find_chromium()

    with sync_playwright() as pw:
        launch_kwargs = {}
        if chromium_exe:
            launch_kwargs["executable_path"] = chromium_exe
        browser = pw.chromium.launch(**launch_kwargs)
        ctx = browser.new_context(viewport={"width": 1400, "height": 2200})
        page = ctx.new_page()

        # Authenticate the API context, then mirror the session cookie.
        resp = ctx.request.post(f"{args.url}/api/login", data={"code": key})
        if resp.status != 200:
            print(f"login failed: HTTP {resp.status}", file=sys.stderr)
            return 1

        # Pull the live API values so the summary captures exact numbers.
        wb = ctx.request.get(f"{args.url}/api/workboard").json()
        sub = ctx.request.get(f"{args.url}/api/subagent-sessions").json()
        cycle = (wb.get("cycle") or {}).get("number")
        engineers = len(wb.get("spawned_engineers") or [])
        live_sessions = len(sub.get("live") or [])
        working_count = (wb.get("cognitive_statistics") or {}).get("working_count")
        mem = ctx.request.get(f"{args.url}/api/memory").json()
        memory_working = (mem.get("native_memory") or {}).get("working")
        action_cycles = [a.get("cycle", 0) for a in (wb.get("recent_actions") or [])]
        max_action_cycle = max(action_cycles, default=0)

        summary = {
            "label": args.label,
            "url": args.url,
            "cycle_header": cycle,
            "recent_actions_max_cycle": max_action_cycle,
            "active_engineers": engineers,
            "terminal_live_sessions": live_sessions,
            "workboard_working_count": working_count,
            "memory_tab_working_count": memory_working,
            "cycle_consistent": cycle is not None and cycle >= max_action_cycle,
            "engineers_consistent": engineers == live_sessions,
            "working_memory_consistent": working_count == memory_working,
        }
        (OUT_DIR / f"{args.label}-summary.txt").write_text(
            json.dumps(summary, indent=2) + "\n", encoding="utf-8"
        )
        print(json.dumps(summary, indent=2))

        for tab in ("workboard", "overview"):
            page.goto(args.url)
            page.locator(f'.tab[data-tab="{tab}"]').click()
            page.wait_for_timeout(1500)
            page.screenshot(path=str(OUT_DIR / f"{args.label}-{tab}.png"), full_page=True)

        browser.close()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
