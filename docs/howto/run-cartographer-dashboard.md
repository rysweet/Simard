---
title: Run the Cartographer dashboard workflow
description: How to use the simard-cartographer identity and the cartographer-dashboard recipe to take a dataset and a question to a served interactive dashboard with a written narrative.
last_updated: 2026-07-16
owner: simard
doc_type: howto
related:
  - ../concepts/cartographer-identity.md
  - ../concepts/pluggable-identity.md
---

# Run the Cartographer dashboard workflow

This guide shows how to take a **dataset and a question** to a **served
interactive dashboard with a written narrative** using the `simard-cartographer`
identity and the `cartographer-dashboard` recipe.

For the design and rationale, see
[Cartographer identity — data storytelling & dashboards](../concepts/cartographer-identity.md).

## Prerequisites

- A dataset file (CSV/Parquet/JSON/…).
- A concrete question you want the dashboard to answer.
- The delivery stack you plan to serve with available on the machine
  (for the default path: Python with `streamlit` and `plotly`).

## 1. Confirm the identity is available

Cartographer is a built-in identity. Bootstrap it through the operator probe to
confirm it resolves and runs end-to-end (inspect → act → verify → persist):

```bash
cargo run --quiet --bin simard_operator_probe -- \
  bootstrap-run simard-cartographer local-harness single-process \
  "verify cartographer identity bootstrap"
```

The probe output reports `Identity: simard-cartographer`, the selected base
type, the topology, and a completed session phase.

## 2. Run the four-stage recipe

The `cartographer-dashboard` recipe orchestrates the four stages — exploratory
analysis, visualization design, app delivery, and narrative — passing each
stage's output to the next. Run it through the recipe runner with your context
variables:

```bash
amplihack recipe run \
  prompt_assets/simard/recipes/cartographer-dashboard.yaml \
  -c dataset_path=/path/to/data.csv \
  -c question="Which regions drove the Q3 revenue change and why?" \
  -c output_dir=/tmp/cartographer-run \
  -c serve_port=8501
```

Context variables:

| Variable | Meaning |
|---|---|
| `dataset_path` | Path to the dataset to analyze. |
| `question` | The question the dashboard must answer. |
| `output_dir` | Directory for the runnable app and `NARRATIVE.md`. |
| `serve_port` | Port the dashboard is served on (default `8501`). |

## 3. What each stage produces

1. **Exploratory analysis** (`cartographer_explore.md`) — profiles the dataset,
   tests hypotheses the question implies, and shortlists the story-worthy
   findings, each backed by a computed number.
2. **Visualization design** (`cartographer_visualize.md`) — maps each finding to
   a fitting chart type and interactivity, composes a coherent layout, and picks
   the delivery stack.
3. **App delivery** (`cartographer_deliver.md`) — builds a reproducible,
   file-based app under `output_dir`, **serves** it on `serve_port`, and
   **verifies it renders** by fetching the served URL.
4. **Narrative** (`cartographer_narrative.md`) — writes `NARRATIVE.md` walking
   question → evidence → answer, with every claim backed by a served view or a
   statistic.

## 4. Verify the served dashboard

App delivery only counts as done when the served URL actually renders. Confirm it
yourself:

```bash
curl -fsS http://127.0.0.1:8501/ | head -c 400
```

A real HTTP 200 with the expected dashboard content (title, app root, or a chart
element) is the evidence that the dashboard is served — not merely that a process
started.

## 5. Collect the artifacts

The run persists durable artifacts under `output_dir`:

- the runnable dashboard source (e.g. `app.py` or `index.html` + data),
- `NARRATIVE.md`, the written data story,
- an evidence record: the served URL and what was verified.

These artifacts — not a throwaway point-in-time report doc — are the deliverable
(Simard's `no-point-in-time-docs` guideline).

## Running a single stage

Each stage has a standalone prompt asset (`simard/cartographer_explore.md`,
`simard/cartographer_visualize.md`, `simard/cartographer_deliver.md`,
`simard/cartographer_narrative.md`), so you can invoke one stage directly when
you only need, say, a fresh visualization design over an existing exploration
brief.
