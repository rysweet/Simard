# Dashboard usability audit — re-runnable Playwright sweep

This directory holds the diagnostic audit harness for the Simard self-serve
dashboard at `http://localhost:8080`. It is the follow-up tooling for issue
[#1662](https://github.com/rysweet/Simard/issues/1662) ("dashboard: first-pass
usability audit"). Each pass produces a fresh set of full-page screenshots and
visible-text dumps for every top-level route the dashboard surfaces, so we can
file *concrete* sub-issues (with screenshots and quoted text) rather than vague
"the dashboard is confusing" complaints.

## What the script does

`audit_pass_01.py` is a one-shot Playwright run that:

1. Reads the dashboard login code from `~/.simard/.dashkey`.
2. POSTs `/api/login` to obtain a `simard_session` cookie.
3. Loads `http://localhost:8080/` once and **dynamically enumerates** every
   top-level nav target — `<nav>` anchors, `[role=tab]`, `[data-tab]`, header
   anchors. Nothing is hard-coded, so a future tab added to the SPA is picked
   up automatically.
4. For each target it changes the SPA hash in place (no full reload, so the
   SPA's own click handlers fire), waits for network-idle + a settle delay,
   then writes:
   - `out/NN-<slug>.png` — full-page screenshot
   - `out/NN-<slug>.txt` — `document.body.innerText`
5. Writes `out/_index.json` summarising every capture (label, href, byte
   sizes, etc.).

The `out/` directory is gitignored — captures are intentionally ephemeral and
should be regenerated per audit pass against the live daemon.

## How to re-run an audit pass

Pre-requisites (one-time):

- Simard daemon running locally on `:8080` (`simard ooda run`)
- `~/.simard/.dashkey` populated (created automatically by the daemon)
- A Python virtualenv with Playwright + Chromium. The recommended layout is a
  worktree-local `.venv-audit/`:

```bash
python3 -m venv .venv-audit
.venv-audit/bin/pip install playwright==1.59.0
# Chromium is already installed under ~/.cache/ms-playwright/chromium-* on
# this host; if not, run:  .venv-audit/bin/python -m playwright install chromium
```

Run a pass:

```bash
.venv-audit/bin/python scripts/dashboard_audit/audit_pass_01.py
```

Typical wall time is ~50-60s. Output goes to `scripts/dashboard_audit/out/`.

## Regression check: per-route titles + ledes (issues #1993, #1994)

A small companion script asserts that every dashboard route exposes a
unique, plain-English `<title>` and `<h1>` plus a non-empty `.page-intro`
lede — the contract introduced by issues #1993 and #1994. Run it the same
way as the audit pass:

```bash
DASHBOARD_URL=http://localhost:8080 \
  .venv-audit/bin/python scripts/dashboard_audit/test_titles_and_ledes.py
```

The script logs in with `~/.simard/.dashkey`, visits every
`/#/<slug>` route, and prints a JSON summary. It exits non-zero if any
route is missing a title/H1/lede, has a duplicate, or is still showing
the legacy "Simard Dashboard v2" generic `<title>`. Use this as a smoke
test before shipping any change to the dashboard chrome.

## How to file findings

After the pass completes, read the `.txt` dumps (they are far easier to grep
than the PNGs) and look for:

- **Jargon** — terms a non-Simard-developer would not understand (`OODA`,
  `stewardship`, `subordinate`, `handoff`, `reflection snapshot`, `cognitive
  memory`, raw enum names like `continue_skipping`, raw struct field names,
  bare UUIDs).
- **Missing panels** — operator questions ("what is the daemon doing right
  now?", "which goals are active and why?") that cannot be answered from any
  panel.
- **Human-friendliness regressions** — ISO timestamps without a relative "X
  minutes ago", JSON blobs displayed raw, no empty-state copy, no per-panel
  explanation of purpose, data-source mismatches between two panels claiming
  to show the same thing.

For each *distinct* finding, file a focused sub-issue against `rysweet/Simard`:

```bash
gh issue create \
  --label dashboard,audit-1662-followup \
  --title "dashboard: <one-line>" \
  --body "<finding, screenshot path, proposed fix in plain prose, parent: #1662>"
```

Aim for 5-10 small sub-issues per pass — never one mega-issue.

## Future passes

The script name (`audit_pass_01.py`) is intentionally numbered. Subsequent
passes should land alongside as `audit_pass_02.py`, `audit_pass_03.py`, etc.,
optionally extending the harness (e.g. probing every documented `/api/*`
endpoint, exercising forms, simulating slow networks). Re-using the same
script per pass is fine if the harness needs no changes — only the captures
and findings change between passes.
