---
title: How to tell data stories with the Cartographer identity
description: Use the pluggable Cartographer identity to take a dataset and a question end-to-end to a served interactive dashboard and a written narrative (Plotly + D3) with the `simard cartographer` CLI.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/pluggable-identity.md
  - ../howto/configure-pluggable-identity.md
  - ../howto/design-with-atelier.md
  - ../reference/simard-cli.md
---

# How to tell data stories with the Cartographer identity

**Cartographer** is a pluggable Simard identity for **data storytelling and
interactive dashboards**. It takes a *dataset* and an *analytical question* and
produces a written narrative, a set of designed charts, and a **served
interactive dashboard** — so a question can go end-to-end from raw data to a
shareable, interactive answer.

Cartographer is repo-grounded and runs in engineer mode
(`inspect → act → verify → persist`): it profiles the data, surfaces findings,
designs charts grounded in real columns, writes the story, renders a
self-contained dashboard, and serves it — writing a `manifest.json` recording
exactly what was built and verified.

## Prerequisites

- Simard binary built (`cargo build --quiet --bin simard`).
- **No external dependency for the happy path** — the dashboard is generated in
  Rust (Plotly + D3, embedded) and served by a built-in static server.
- Optional delivery targets (Cartographer degrades gracefully without them):
  - [Streamlit](https://streamlit.io/) to serve the generated `app.py`.
  - [Observable](https://observablehq.com/) to render the generated `.ojs`
    notebook.

Their absence never fails a study; the self-contained HTML dashboard is always
produced and their availability is only recorded in the manifest.

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

## Write a study brief

A brief is a small JSON document describing the dataset and the question. Save
it as `study.json`:

```json
{
  "title": "Regional Sales Storytelling",
  "question": "How does advertising spend relate to revenue across regions?",
  "dataset": { "path": "regional-sales.csv", "format": "csv" },
  "app_target": "html",
  "hints": { "x": "ad_spend", "y": "revenue", "color": "region", "time": "month" },
  "audience": "Revenue and growth stakeholders reviewing marketing efficiency."
}
```

- `dataset.path` points at a `.csv` or `.json` file (relative to the brief). You
  can instead embed small datasets inline with `dataset.csv` (header row first).
- `app_target` is one of `html` (default), `streamlit`, or `observable`.
- `hints` steer chart design toward specific columns (`x`, `y`, `color`,
  `time`); every hint must name a real column.
- `audience` is an optional note woven into the narrative.

## Build the narrative and dashboard

```bash
simard cartographer build --brief study.json --out ./pkg
```

This writes to `./pkg`:

| File             | What it is                                                   |
| ---------------- | ----------------------------------------------------------- |
| `dataset.csv`    | The profiled dataset, re-emitted as normalized CSV.         |
| `charts.json`    | The designed charts (scatter / line / bar / histogram).     |
| `narrative.md`   | The written data story, including an explicit Answer.       |
| `dashboard.html` | Self-contained interactive dashboard (Plotly + D3).         |
| `app.py`         | Streamlit source — only when `app_target = "streamlit"`.    |
| `notebook.ojs`   | Observable source — only when `app_target = "observable"`.  |
| `manifest.json`  | Build record + verification result.                         |

Example output:

```text
cartographer: Regional Sales Storytelling [html] — 12 rows × 5 cols, 5 finding(s), 4 chart(s)
  question: How does advertising spend relate to revenue across regions?
  [     ok] dataset.csv (372 bytes)
  [     ok] narrative.md (2036 bytes)
  [     ok] charts.json (929 bytes)
  [     ok] summary.json (2994 bytes)
  [     ok] dashboard.html (9901 bytes)
  verification: PASS
    ✓ dataset-loaded: 12 rows × 5 columns
    ✓ charts-designed: 4 interactive chart(s) grounded in the dataset
    ✓ narrative-present: written narrative with an explicit Answer section
    ✓ dashboard-interactive: dashboard.html embeds data and renders interactive Plotly views
```

Override the brief's delivery target with `--target html|streamlit|observable`,
and add `--strict` to make the command exit non-zero unless verification passes
— useful in CI.

## Serve the dashboard

Serve the built package over HTTP:

```bash
simard cartographer serve --out ./pkg --port 8080
# open http://127.0.0.1:8080/dashboard.html — press Ctrl-C to stop
```

For automation, `--self-check` binds an ephemeral port, issues one request for
the dashboard, prints a PASS/FAIL line, and exits non-zero on failure:

```bash
simard cartographer serve --out ./pkg --self-check
# cartographer: self-check PASS — 127.0.0.1:40121 200 (9901 bytes)
```

## Verify an existing package

`inspect` re-reads a package directory and re-runs verification without
rebuilding:

```bash
simard cartographer inspect --out ./pkg
```

Verification requires the core deliverables — the dataset loaded, at least one
chart grounded in real columns, a narrative with an explicit Answer, and an
interactive dashboard that embeds the data. A study is only *done* when the
dashboard both verifies and **serves** (the self-check returns the page).

## How degradation works

Cartographer treats the HTML dashboard as the required outcome and the other
delivery runtimes as best-effort:

- **No Streamlit** → the `app.py` source is still emitted; the manifest records
  Streamlit as unavailable. The HTML dashboard still serves the story.
- **No Observable** → the `.ojs` source is still emitted and recorded as
  unavailable. The HTML dashboard still serves the story.

Every probe result is recorded in `manifest.json`, so the package is always
self-describing and the self-contained dashboard always works offline.
