# Dashboard usability audit — Rust-native headless-Chrome sweep

This directory holds the diagnostic audit harness for the Simard self-serve
dashboard at `http://localhost:8080`. It is the follow-up tooling for issue
[#1662](https://github.com/rysweet/Simard/issues/1662) ("dashboard: first-pass
usability audit"), migrated from Python/Playwright to Rust as part of
[#2156](https://github.com/rysweet/Simard/issues/2156). Each pass produces a
fresh set of full-page screenshots and visible-text dumps for every top-level
route the dashboard surfaces, so we can file *concrete* sub-issues (with
screenshots and quoted text) rather than vague "the dashboard is confusing"
complaints.

## Binaries

The audit tools are compiled as Rust binaries gated behind the `dashboard-audit`
Cargo feature. Source lives in `src/bin/`:

| Binary                    | Source                               | Former Python          |
| ------------------------- | ------------------------------------ | ---------------------- |
| `simard-audit-pass01`     | `src/bin/simard_audit_pass01.rs`     | `audit_pass_01.py`     |
| `simard-audit-dashboard`  | `src/bin/simard_audit_dashboard.rs`  | `audit_dashboard.py`   |

## What `simard-audit-pass01` does

A one-shot headless-Chrome audit that:

1. Reads the dashboard login code from `~/.simard/.dashkey`.
2. Authenticates via an in-browser `fetch()` POST to `/api/login`.
3. Loads `http://localhost:8080/` once and **dynamically enumerates** every
   top-level nav target — `<nav>` anchors, `[role=tab]`, `[data-tab]`, header
   anchors. Nothing is hard-coded, so a future tab added to the SPA is picked
   up automatically.
4. For each target it changes the SPA hash in place (no full reload, so the
   SPA's own click handlers fire), waits for a settle delay, then writes:
   - `out/NN-<slug>.png` — full-page screenshot
   - `out/NN-<slug>.txt` — `document.body.innerText`
5. Writes `out/_index.json` summarising every capture (label, href, byte
   sizes, etc.).

## What `simard-audit-dashboard` does

A sibling tool purpose-built for the **self-serve-dashboard-improvement**
initiative (parent issue [#1990](https://github.com/rysweet/Simard/issues/1990)).
Where `simard-audit-pass01` produces raw screenshots + text dumps for human
review, `simard-audit-dashboard` additionally:

- Captures per-page **HTTP errors** (fetch/XHR responses with status ≥ 400)
  and **browser console errors** via JS instrumentation.
- Runs a **jargon scan** across every captured DOM dump and produces an
  inverted index (term → list of pages where it appears).
- Emits a consolidated **`out/REPORT.md`** — the document that gets embedded
  into the parent epic issue and decomposed into 3–5 child issues.

## When to use which binary

| You want…                                          | Use                       |
| -------------------------------------------------- | ------------------------- |
| Raw PNG + TXT dumps to grep, no opinion            | `simard-audit-pass01`     |
| A structured REPORT.md ready to paste into an epic | `simard-audit-dashboard`  |
| Per-page HTTP + console error inventory            | `simard-audit-dashboard`  |
| Numbered, ordered captures for diffing across runs | `simard-audit-pass01`     |
| Jargon inverted index                              | `simard-audit-dashboard`  |

Both binaries write to the same `out/` directory under **disjoint filename
conventions** that cannot collide:

- `simard-audit-pass01` writes `NN-<slug>.{png,txt}` and `out/_index.json`.
- `simard-audit-dashboard` writes `<slug>.{png,txt,errors.json}`,
  `out/REPORT.md`, and `out/_audit_dashboard_index.json`.

The `out/` directory is gitignored — captures are intentionally ephemeral and
should be regenerated per audit pass against the live daemon.

## How to build

```bash
cargo build --features dashboard-audit \
  --bin simard-audit-pass01 \
  --bin simard-audit-dashboard
```

The `dashboard-audit` feature gates the `headless_chrome`, `regex`, and `url`
dependencies so normal builds are unaffected.

## How to re-run an audit pass

Pre-requisites:

- Simard daemon running locally on `:8080` (`simard ooda run`)
- `~/.simard/.dashkey` populated (created automatically by the daemon)
- Chrome or Chromium installed and findable on `$PATH`, or set `CHROME_PATH`
  to the binary location

Run a pass:

```bash
# raw captures
cargo run --features dashboard-audit --bin simard-audit-pass01

# structured audit with REPORT.md
cargo run --features dashboard-audit --bin simard-audit-dashboard
```

Or use the pre-built binaries:

```bash
target/debug/simard-audit-pass01
target/debug/simard-audit-dashboard
```

Typical wall time is ~45-75s depending on route count. Output goes to
`scripts/dashboard_audit/out/`.

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

## Configuration

All knobs are constants at the top of each binary's source file:

| Constant       | Default                       | Binary             | Purpose                                |
| -------------- | ----------------------------- | ------------------ | -------------------------------------- |
| `BASE_URL`     | `http://localhost:8080`       | both               | Dashboard origin                       |
| `NAV_WAIT_MS`  | `2500`                        | pass01             | Settle time after initial load         |
| `PANEL_SETTLE_MS` | `1500`                     | pass01             | Settle time per page capture           |
| `MAX_ROUTES`   | `50`                          | dashboard          | Hard upper bound on routes audited     |
| `TIMEOUT_MS`   | `8000`                        | dashboard          | Per-page navigation timeout            |
| `FALLBACK_TABS`| `[overview, goals, …]`        | dashboard          | Routes union'd if DOM discovery is thin|
| `JARGON_TERMS` | seed list (14 terms)          | dashboard          | Terms scanned per page                 |

Set `CHROME_PATH` to override Chrome/Chromium binary discovery.

There are no CLI flags by design — the binaries are invoked the same way every
time, so output is diff-able across runs.

## Output layout

```
scripts/dashboard_audit/out/
├── REPORT.md                         ← consolidated, embedded into epic issue
├── overview.png
├── overview.txt
├── overview.errors.json
├── goals.png
├── goals.txt
├── goals.errors.json
├── …
├── _index.json                       ← pass01 manifest
└── _audit_dashboard_index.json       ← dashboard manifest (disjoint from _index.json)
```

`out/` is gitignored both by the root `.gitignore` and a local
`scripts/dashboard_audit/.gitignore`, so a careless `git add -A` cannot leak
session data or screenshots.

## REPORT.md anatomy

`out/REPORT.md` is a deterministic Markdown render with exactly five sections:

1. **Pages found** — slug, label, href, text-length, http-errors, console-errors.
2. **What each page conveys** — per slug: `<h1>` text and a ≤240-char excerpt.
3. **Jargon inventory** — inverted index `term → page1, page2, …`.
4. **Missing context** — heuristic flags (text-length < 200, no `<h1>`).
5. **Top-5 highest-impact usability fixes** — heuristic-ranked, engineer curates.

REPORT.md is restricted to aggregate counts, titles, jargon terms, and short
page excerpts. **Re-read REPORT.md end-to-end before `gh issue create`** and
hand-edit any line that looks sensitive.

## Hard safety constraints (enforced in code)

- **Forbidden write paths.** `simard-audit-dashboard` asserts at startup that
  `OUT_DIR` does not fall under `~/.simard/prompt_assets/` or
  `$SIMARD_PROMPT_ASSETS_DIR`.
- **Slug whitelist.** Output filenames are derived through `[a-z0-9_-]+` regex
  with collision suffixes. Path traversal is structurally impossible.
- **Same-origin only.** Routes pointing outside `http://localhost:8080` are
  rejected during discovery.
- **Route cap.** `MAX_ROUTES = 50` bounds the work.
- **No dashkey in logs.** Login errors surface only HTTP status — never the key.
- **No external network.** Only `http://localhost:8080` is contacted.

## Plain-English acceptance criteria — the house style

Every child issue derived from `REPORT.md` MUST express its acceptance criteria
in the form:

> A human visiting `<page>` can `<observable outcome>` without knowing the term
> `<jargon>`.

## Safety & privacy posture

- The dashkey is read once, held in memory, used once for the login POST. It
  is never written to disk, never logged, never echoed in error messages.
- `out/` artefacts can contain operator-visible session data. They are
  **gitignored** and should never be committed.
- `REPORT.md` is restricted by construction to counts, titles, jargon terms,
  and short excerpts. Review before publishing.

## Relationship to issues #1662 and #2156

The original Python scripts (`audit_pass_01.py` and `audit_dashboard.py`) were
filed under [#1662](https://github.com/rysweet/Simard/issues/1662). They have
been rewritten in Rust as part of
[#2156](https://github.com/rysweet/Simard/issues/2156) (child of the
Rust-migration epic [#2155](https://github.com/rysweet/Simard/issues/2155)).
The Rust binaries preserve the same auth model, discovery strategy, output
format, jargon scanning, and report generation — with the Python + Playwright
dependency eliminated.
