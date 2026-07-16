---
title: How to tell data stories and build dashboards with the Cartographer identity
description: Use the pluggable Cartographer identity to take a dataset and a question end-to-end to an exploratory analysis, an interactive Plotly dashboard, and a written narrative, served over HTTP with the `simard cartographer` CLI.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/pluggable-identity.md
  - ../howto/configure-pluggable-identity.md
  - ../reference/simard-cli.md
---

# How to tell data stories and build dashboards with the Cartographer identity

**Cartographer** is a pluggable Simard identity for data storytelling &
dashboards. It takes a *dataset* and a *question* and produces an exploratory
analysis, an **interactive dashboard** (a self-contained Plotly page), and a
written **narrative** — so a raw table can go end-to-end from a question to a
served, shareable story.

Cartographer is repo-grounded and runs in engineer mode
(`inspect → act → verify → persist`): it profiles the data, designs charts,
renders the dashboard and narrative, verifies the deliverables against the
brief, and writes a `manifest.json` recording exactly what was built.

## Prerequisites

- Simard binary built (`cargo build --quiet --bin simard`).
- A dataset as CSV or a JSON array of row objects.
- No external tool is required for the core deliverables: the analysis, the
  interactive dashboard, the narrative, and the built-in HTTP server are all
  pure Rust. Plotly.js is loaded in the browser from a CDN.
- Optional, for the delivery variants (Cartographer degrades gracefully without
  them): `python3` and `streamlit` to run the generated `app.py`; `node` for
  JavaScript tooling.

Check what is available:

```bash
simard cartographer inspect --out /tmp/does-not-exist   # prints a tool report
```

## Select the Cartographer identity

Cartographer ships as a built-in identity (`simard-cartographer`) and as a
pluggable identity card under
`prompt_assets/simard/identities/cartographer/identity.toml`. Select it for a
session with the identity environment variable:

```bash
export SIMARD_IDENTITY=simard-cartographer
```

See [Configure Pluggable Identity](configure-pluggable-identity.md) for how
identity cards are discovered and loaded.

## Two ways to point Cartographer at data

### A dataset + a question (ad-hoc)

The fastest path is to pass a dataset and a question directly:

```bash
simard cartographer build \
  --dataset sales.csv \
  --question "Which region and product drive the most revenue over time?" \
  --out ./dashboard
```

### A brief file (repeatable)

For a repeatable build, write a small JSON *brief* and save it as
`brief.json`:

```json
{
  "name": "regional-sales",
  "title": "Regional Sales Dashboard",
  "question": "Which region and product drive the most revenue over time?",
  "dataset": "sales.csv",
  "dataset_format": "csv",
  "max_rows": 100000,
  "max_points": 5000
}
```

The `dataset` path is resolved relative to the brief file. `dataset_format` is
`csv` or `json` (inferred from the extension when omitted). `title`, `max_rows`,
and `max_points` are optional. Then build from the brief:

```bash
simard cartographer build --brief brief.json --out ./dashboard
```

## Build the analysis, dashboard, and narrative

```bash
simard cartographer build --brief brief.json --out ./dashboard
```

This writes to `./dashboard`:

| File             | What it is                                                     |
| ---------------- | -------------------------------------------------------------- |
| `analysis.json`  | Machine-readable dataset profile, column types, and chart specs. |
| `dashboard.html` | Self-contained interactive Plotly dashboard (opens in a browser). |
| `narrative.md`   | Written data story: overview, key findings, and chart callouts. |
| `app.py`         | A Streamlit app that renders the same dashboard (skipped with `--no-streamlit`). |
| `manifest.json`  | Build record + verification result.                            |

Example output:

```text
cartographer: Regional Sales Dashboard — 24 rows x 4 columns, 4 chart(s)
  question: Which region and product drive the most revenue over time?
  [     ok] analysis.json
  [     ok] dashboard.html
  [     ok] narrative.md
  [     ok] app.py
  verification: PASS
  manifest: ./dashboard/manifest.json
```

Add `--strict` to make the command exit non-zero unless verification passes.
Use `--no-streamlit` to skip generating `app.py`.

## Serve the dashboard over HTTP

Build and serve in one step:

```bash
simard cartographer build --brief brief.json --out ./dashboard --serve --port 8787
```

Or serve an already-built package:

```bash
simard cartographer serve --out ./dashboard --host 127.0.0.1 --port 8787
# open http://127.0.0.1:8787/
```

The built-in server is a small, path-traversal-safe static file server: `/`
returns the interactive `dashboard.html`, and the other package files
(`analysis.json`, `narrative.md`) are served by name. The default port is
`8787`.

## Verify an existing package

`inspect` re-reads a package directory and re-runs verification without
rebuilding:

```bash
simard cartographer inspect --out ./dashboard
```

Verification always requires the core deliverables — a loaded dataset, at least
one chart, the interactive dashboard, the narrative, and the analysis document.
Because the core pipeline is pure Rust, `build`, `inspect`, and `serve` all work
on any host, with no display and no external analytics stack.

## How degradation works

Cartographer keeps the core deliverables dependency-free and treats the delivery
variants as best-effort:

- **No `python3` / `streamlit`** → the interactive `dashboard.html` and the
  narrative are still produced and serveable; only running the generated
  `app.py` requires Streamlit.
- **No `node`** → unaffected; JavaScript tooling is only used for optional
  front-end workflows.

Every optional tool is reported by `inspect`, so the package is always
self-describing.
