---
title: How to plan menus and events with the Gastronome identity
description: Use the pluggable Gastronome identity to take an event/menu brief end-to-end to a costed, scheduled menu plan (menu card, shopping list, nutrition, prep schedule, optional kitchen app) with the `simard gastronome` CLI.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/pluggable-identity.md
  - ../howto/configure-pluggable-identity.md
  - ../reference/simard-cli.md
---

# How to plan menus and events with the Gastronome identity

**Gastronome** is a pluggable Simard identity for culinary, menu &amp; event
design. It takes a structured *menu brief* and produces a **costed, scheduled
menu plan** — a menu card, a consolidated shopping list, a nutrition breakdown,
and a back-timed prep schedule — so an event brief can go end-to-end from an
idea to a plan the kitchen can execute.

Gastronome is repo-grounded and runs in engineer mode
(`inspect → act → verify → persist`): it scales recipes, rolls up cost and
nutrition, schedules prep, verifies the result against the brief, and writes a
`manifest.json` recording exactly what was planned.

## Prerequisites

- Simard binary built (`cargo build --quiet --bin simard`).
- Nothing else. Gastronome is self-contained — every stage is deterministic and
  pure-Rust, so there is no external engine to install.

## Select the Gastronome identity

Gastronome ships as a built-in identity (`simard-gastronome`) and as a pluggable
identity card under
`prompt_assets/simard/identities/gastronome/identity.toml`. Select it for a
session with the identity environment variable:

```bash
export SIMARD_IDENTITY=simard-gastronome
```

See [Configure Pluggable Identity](configure-pluggable-identity.md) for how
identity cards are discovered and loaded.

## Write a menu brief

A brief is a JSON document describing the event and its dishes. Recipes are
expressed **per serving**; Gastronome scales them to the guest count. Save it as
`brief.json`:

```json
{
  "event": "Autumn tasting dinner",
  "guests": 24,
  "currency": "USD",
  "service_time": "19:00",
  "dishes": [
    {
      "name": "Roasted squash soup",
      "course": "starter",
      "tags": ["vegetarian", "gluten-free"],
      "ingredients": [
        { "name": "Butternut squash", "qty_per_serving": 180, "unit": "g", "cost_per_unit": 0.004,
          "nutrition": { "kcal": 0.45, "protein_g": 0.011, "carbs_g": 0.12, "fat_g": 0.001 } }
      ],
      "prep": [ { "task": "Roast squash", "minutes": 45, "station": "oven" } ]
    }
  ],
  "budget": 400.0
}
```

Supported `course` values include `starter`, `main`, `side`, `dessert`, and
`drink` (unknown courses fall back to `other`). Quantities and nutrition are
**per unit** of the ingredient's `unit`. `budget` and `service_time` are
optional; when a budget is set, Gastronome flags an over-budget plan as an
advisory, and when a service time is set the prep schedule carries wall-clock
start times.

## Build the menu plan

```bash
simard gastronome build --brief brief.json --out ./plan --prep-app
```

This writes to `./plan`:

| File                 | What it is                                                        |
| -------------------- | ----------------------------------------------------------------- |
| `menu.md`            | Menu card grouped by course, with per-guest nutrition &amp; cost. |
| `shopping_list.csv`  | Ingredients aggregated across dishes, scaled, with a cost roll-up.|
| `nutrition.csv`      | Per-guest, whole-event, and per-dish energy and macronutrients.   |
| `prep_schedule.csv`  | Prep tasks back-timed from the service time.                      |
| `prep_app.html`      | Self-contained kitchen prep checklist app — only with `--prep-app`.|
| `manifest.json`      | Plan record + totals + verification result.                       |

Example output:

```text
gastronome: Autumn tasting dinner — 24 guests, 3 dish(es) across 3 course(s), 72 serving(s)
  estimated cost: 164.64 USD, 6.86/guest
  per-guest nutrition: 866 kcal, 44.98 g protein, 49.5 g carbs, 54.38 g fat
  prep: 295 min critical path (service 19:00)
  [     ok] menu.md
  [     ok] shopping_list.csv
  [     ok] nutrition.csv
  [     ok] prep_schedule.csv
  [     ok] prep_app.html — 5 prep task(s)
  verification: PASS
```

Add `--strict` to make the command exit non-zero unless verification passes —
useful in CI or a goal-session where the plan must be complete.

## Run the kitchen prep app

`prep_app.html` is a single self-contained file: open it in any browser (no
network, no build step). It shows every prep task in execution order with its
start time, a station tag, and a checkbox, plus a progress bar — a runnable
kitchen checklist for the line during service.

## Verify an existing plan

`inspect` re-reads a plan directory and re-runs verification without rebuilding:

```bash
simard gastronome inspect --out ./plan
```

Verification always requires the core deliverables — a valid menu, a shopping
list, a nutrition breakdown, and an internally consistent prep schedule. The
budget check is advisory, so Gastronome still produces a usable plan for an
over-budget menu (it just flags it).

## How the pipeline works

1. **Scale.** Each recipe is multiplied to the guest count, rounding servings up
   to whole portions.
2. **Aggregate.** Ingredients are consolidated across dishes by `(name, unit)`
   into one shopping list with a cost roll-up.
3. **Analyse.** Cost and nutrition are rolled up per guest and for the whole
   event.
4. **Schedule.** Prep tasks are ordered by station and back-timed from the
   service time, yielding a single-cook critical path.
5. **Present &amp; persist.** A menu card and (optionally) a kitchen prep app are
   rendered, and everything is recorded in a verified `manifest.json`.
