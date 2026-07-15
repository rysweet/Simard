---
title: Cartographer identity — data storytelling & dashboards
description: How the Simard Cartographer identity turns a dataset plus a question into a served interactive dashboard with a written narrative, via four recipes (explore, visualize, deliver, narrate) and an end-to-end orchestrator, using Observable/Plotly/Streamlit/D3.
last_updated: 2026-07-15
owner: simard
doc_type: concept
related:
  - ./pluggable-identity.md
  - ../reference/runtime-contracts.md
  - ../architecture/agent-composition.md
---

# Cartographer identity — data storytelling & dashboards

## The problem

Simard's built-in identities cover engineering, meetings, the evaluation gym,
and curation — but none of them are shaped for **data storytelling**: taking a
dataset and a question and turning them into an interactive dashboard people can
actually explore, with a written narrative that explains what the data says.

That work has a distinct discipline. It is not "write a feature"; it is
*profile the data honestly, find the real signals, design truthful
visualizations, ship a served app, and tell the story without inventing
numbers*.

## The identity

`simard-cartographer` is a first-class built-in Simard identity (advertised by
`BuiltinIdentityLoader`, exactly like `simard-engineer`). It operates in
engineer mode — it writes real code, runs analysis, and serves apps — and it
accepts the same backends as the engineer identity, including the local
`terminal-shell` path needed to start and probe a dashboard server.

Its persona lives in `prompt_assets/simard/cartographer_system.md`. The
persona's contract:

- **Discipline:** `inspect → act → verify → persist`, the same loop the engineer
  identity runs. Inspect the data before charting; act with the smallest step;
  verify every number and confirm the app actually serves; persist source,
  narrative, and evidence.
- **Definition of done:** a dataset + question has been taken to a **served**
  interactive dashboard (a URL that responds) plus a **written narrative** whose
  numbers match the analysis — nothing fabricated.

## The four phases

The identity decomposes the work into four phases, each with a dedicated recipe
under `prompt_assets/simard/recipes/`, plus an end-to-end orchestrator that runs
all four in one continuous session:

| Phase | Recipe | Input → Output |
|-------|--------|----------------|
| Explore | `cartographer-explore.yaml` | dataset + question → evidence-backed findings (JSON) |
| Visualize | `cartographer-visualize.yaml` | findings → visualization spec (panels, encodings, tool) |
| Deliver | `cartographer-deliver.yaml` | spec + dataset → **served** interactive dashboard app |
| Narrate | `cartographer-narrative.yaml` | findings + delivery manifest → written Markdown narrative |
| End-to-end | `cartographer-dashboard.yaml` | dataset + question → served dashboard **and** narrative |

Each recipe is a single default-agent step that reads its inputs from absolute
paths (never inlined, so large datasets never overflow `ARG_MAX`) and writes its
result to a clean result file via a named `output:` channel — recipe-runner
stdout is inert, matching the pattern used by the journal and creative-ideas
recipes.

## Delivery tools

The Cartographer picks the delivery tool to fit the job:

- **Streamlit** — fastest served Python data app with widgets; the default when
  the analysis is in Python.
- **Plotly / Dash** — rich interactive charts; Dash for multi-callback apps,
  plain Plotly for a self-contained interactive HTML dashboard.
- **Observable** — JS-first, notebook-style or statically-served dashboards.
- **D3** — bespoke, fully custom interactive visualization.

## Honesty guarantees (zero-BS)

The persona and every recipe carry the same guardrails:

- **No fabrication.** Every number, trend, and claim comes from the actual data
  and is reproducible. If the data cannot answer the question, the explore phase
  writes empty findings and a populated `cannot_answer` — it never invents an
  answer.
- **Serve, then claim.** "Delivered a dashboard" means the server is up and the
  port was verified reachable (e.g. `curl -sSf http://127.0.0.1:<port>`). The
  delivery manifest's `serve_verified` flag may be `true` only when that check
  actually passed; the end-to-end recipe gates its `done` flag on it.
- **Honest degradation.** Dirty, tiny, or unanswerable inputs yield the best
  honest partial result with limits stated, not a confident fiction.

## What this is not

- **Not a new operating mode.** Cartographer reuses `OperatingMode::Engineer`;
  it is a persona + recipe package, not a new runtime scheduling mode.
- **Not a hosted analytics service.** The identity guides an agent to build and
  serve a dashboard in its working environment; Simard itself does not host
  dashboards.
- **Not a replacement for the engineer identity.** It is a sibling specialized
  for data storytelling; general engineering still uses `simard-engineer`.

## Related

- [Pluggable identity](./pluggable-identity.md) — how per-repo identities are
  loaded from `identity.toml` (the file-based counterpart to built-ins).
- [Runtime contracts](../reference/runtime-contracts.md) — the builtin
  identities advertised by the loader and their base types.
