---
title: Gastronome identity — culinary, menu & event design
description: A pluggable Simard identity that designs recipes and menus and turns an event/menu brief into a costed, nutritionally analysed, prep-scheduled plan via the simard-gastronome app.
last_updated: 2026-07-16
owner: simard
doc_type: concept
related:
  - pluggable-identity.md
  - ../reference/pluggable-identity-api.md
---

# Gastronome identity — culinary, menu & event design

**Gastronome** is a pluggable Simard identity for the kitchen. Where the
engineering identities (engineer, meeting, gym, curator) reason about code,
Gastronome reasons about food: it designs recipes and menus, then runs the
deterministic planning math a kitchen needs — cost, nutrition, scaling, and prep
scheduling — to execute them.

It is a worked example of the [pluggable identity](pluggable-identity.md)
mechanism: a first-class identity (`simard-gastronome`) with its own system
prompt and operating posture, backed by a self-contained deterministic library
and a runnable CLI.

## What it delivers

Given an **event/menu brief** and a **recipe book**, Gastronome produces a
complete, self-consistent plan:

1. **Menu** — one dish per requested course, chosen to honour every dietary
   constraint, scaled to the guest count with whole-batch counts.
2. **Cost analysis** — per-course cost, whole-event total, and cost per guest.
3. **Nutrition analysis** — calories and macros per guest and for the event.
4. **Shopping list** — consolidated across the menu by ingredient and unit.
5. **Prep schedule** — every dish back-scheduled to land at the serve time, with
   a single "kitchen call" start time and each step marked hands-on or passive.

This is the identity's "done when": a brief in, a costed and scheduled menu plan
out, deterministically.

## The `simard-gastronome` app

The identity is backed by the `simard-gastronome` binary (the "prep app"):

```text
simard-gastronome demo                         # plan a built-in demo brief
simard-gastronome recipes                       # list the recipe book
simard-gastronome plan --brief b.json --recipes r.json [--json]
```

- `--brief <path>` — a JSON `EventBrief` (name, guests, `serve_time` as `HH:MM`,
  courses, and dietary constraints). Omitted, the app plans a built-in demo.
- `--recipes <path>` — a JSON recipe book (a bare array or `{ "recipes": [...] }`).
  Omitted, it uses the built-in sample book.
- `--json` — emit the machine-readable `EventPlan` instead of the text report.

Runnable examples live under
[`examples/gastronome/`](https://github.com/rysweet/Simard/tree/main/examples/gastronome):
an `event_brief.json`, a `recipes.json`, and an `identity.toml` showing the
file-based pluggable identity path.

## How planning works

- **Recipes are portioned data.** Ingredient quantities, cost, and nutrition are
  authored *per serving*; scaling to `N` guests is a linear multiply, while prep
  happens in whole batches (`ceil(guests / base_servings)`).
- **Constraints are hard.** A dish satisfies a constraint only if it carries the
  required dietary tag. A `vegan` brief means every course is vegan; if a course
  cannot be filled, the planner reports which course and why rather than serving
  a non-compliant dish.
- **Selection is deterministic.** When several recipes qualify for a course, the
  cheapest-per-serving wins, ties broken by recipe id, so the same brief and book
  always yield the same plan. A course may pin a specific recipe by id.
- **Scheduling back-plans from the serve time.** Each dish's last step ends at
  the serve time; the kitchen call time is the earliest start across the menu.

## Identity registration

`simard-gastronome` is registered as a built-in identity (operating in the
`orchestrator` mode — it orchestrates a kitchen) with the
`prompt_assets/simard/gastronome_system.md` system prompt. The same identity can
also be shipped as data through an `identity.toml`
([example](https://github.com/rysweet/Simard/blob/main/examples/gastronome/identity.toml)),
loaded by the file-based loader exactly like any other pluggable identity.

The task-specific prompt contracts are:

- `prompt_assets/simard/gastronome_recipe_design.md` — design one recipe.
- `prompt_assets/simard/gastronome_menu_design.md` — compose a menu.
- `prompt_assets/simard/gastronome_event_plan.md` — the end-to-end event flow.

See [Pluggable identity](pluggable-identity.md) for the underlying mechanism and
the [Pluggable Identity API](../reference/pluggable-identity-api.md) for the
manifest surface.
