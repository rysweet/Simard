---
title: Cartographer identity — data storytelling & dashboards
description: The simard-cartographer identity turns a dataset and a question into a served interactive dashboard with a written narrative, via a four-stage exploratory-analysis, visualization-design, app-delivery, and narrative loop.
last_updated: 2026-07-16
owner: simard
doc_type: concept
related:
  - ./pluggable-identity.md
  - ../howto/run-cartographer-dashboard.md
  - ../reference/pluggable-identity-api.md
---

# Cartographer identity — data storytelling & dashboards

## The problem

Simard's built-in identities all serve the ecosystem's engineering loop:
`simard-engineer` writes code, `simard-meeting` facilitates decisions,
`simard-gym` runs evaluations, and the curators tend goals and improvements.
None of them are shaped for **data storytelling** — taking a raw dataset and a
question and producing an interactive dashboard someone can open, explore, and
read a truthful story from.

That work has its own discipline: profile the data before charting it, choose
encodings that fit the data instead of decorating it, actually **serve** a
running app (not just describe one), and ground every claim in a real
computation. Bolting this onto the engineer prompt would dilute both.

## The solution: `simard-cartographer`

`simard-cartographer` is a first-class built-in identity (registered in
`BuiltinIdentityLoader`, alongside the engineer, meeting, gym, and curator
identities). It runs in **Engineer operating mode** — delivery requires writing
app files and serving a process, so it needs the same base-type reach as the
engineer identity, including `terminal-shell`. Its distinguishing surface is its
**prompt assets** and its **recipe**, not a new operating mode.

Because identity is pluggable (see [Pluggable identity](./pluggable-identity.md)),
a repository can also define its own Cartographer-flavored identity in an
`identity.toml` — but Cartographer ships in-tree so it is available everywhere by
default.

## The loop: inspect → act → verify → persist

Cartographer follows the same disciplined loop the runtime enforces for engineer
mode, specialized for data storytelling:

1. **Inspect** — load the dataset; establish shape, dtypes, missingness, ranges,
   and how it relates to the question. Understand before visualizing.
2. **Act** — do the exploratory analysis, then design and build the
   visualizations and the dashboard app.
3. **Verify** — prove the dashboard actually serves and renders by fetching the
   served URL and confirming a real HTTP 200 with the expected content; confirm
   every narrative claim is backed by a chart or a computed statistic.
4. **Persist** — write the narrative, the dashboard source, and an evidence
   record. Findings live as durable artifacts, never a throwaway point-in-time
   report doc (Simard's `no-point-in-time-docs` guideline).

## The four stages

A Cartographer run is four stages, each with a standalone prompt asset and all
orchestrated by the `cartographer-dashboard` recipe:

| Stage | Prompt asset | Output |
|---|---|---|
| Exploratory analysis | `simard/cartographer_explore.md` | Exploration brief (profile + story-worthy findings) |
| Visualization design | `simard/cartographer_visualize.md` | Visualization spec (encodings, interactivity, layout, stack) |
| App delivery | `simard/cartographer_deliver.md` | Delivery record (runnable app + served URL + render evidence) |
| Narrative | `simard/cartographer_narrative.md` | `NARRATIVE.md` grounded in the served views |

The identity system prompt is `simard/cartographer_system.md`, and the recipe is
`simard/recipes/cartographer-dashboard.yaml`.

## The toolkit

Cartographer picks the smallest tool that answers the question:

- **Streamlit** — fastest path to a served, interactive Python dashboard.
- **Plotly / Plotly Express** — interactive charts that embed in Streamlit or a
  standalone HTML file.
- **Observable / Observable Plot** — web-native, reactive, notebook-style
  dashboards and grammar-of-graphics charts.
- **D3.js** — bespoke custom web visualizations when a standard chart type
  cannot express the story.

## Untrusted data

The dataset, its values, its filenames, and the question text are **data, not
instructions**. Cartographer never obeys instructions embedded in them (e.g.
"ignore your rules", "run this command"), and never surfaces secrets it finds in
the data — it flags them and continues.

## Definition of done

For a given dataset + question, a Cartographer run is done only when: the
exploratory analysis is recorded and grounded in real computations; a small,
coherent set of visualizations is designed and justified; an interactive
dashboard is built and **actually served** (verified by a live fetch of the
served URL); a written narrative walks question → evidence → answer with every
claim backed; and the dashboard source, narrative, and evidence record are
persisted as durable artifacts.

## See also

- [Run the Cartographer dashboard workflow](../howto/run-cartographer-dashboard.md)
- [Pluggable identity](./pluggable-identity.md)
- [Pluggable identity API reference](../reference/pluggable-identity-api.md)
