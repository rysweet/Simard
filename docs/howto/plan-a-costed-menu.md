---
title: Plan a costed, scheduled menu from an event brief
description: Use the `simard gastronome` kitchen app to take an event/menu brief (JSON or TOML) to a costed, scheduled menu plan — with recipe scaling, nutrition and cost analysis, prep scheduling, and fail-closed dietary enforcement.
last_updated: 2026-07-15
owner: simard
doc_type: howto
related:
  - ../concepts/gastronome-identity.md
  - ../reference/simard-cli.md
---

# Plan a costed, scheduled menu from an event brief

This guide uses the **Gastronome** identity's kitchen app,
`simard gastronome`, to turn an event/menu brief into a costed, scheduled menu
plan. Everything here is deterministic and offline — no network, no clock, no
external data — so you get the same plan every time. For the design rationale
see [the Gastronome concept](../concepts/gastronome-identity.md).

## Prerequisites

- A built `simard` binary (`cargo build --bin simard`).

## 1. See what the pantry offers

The app ships with a built-in library of ingredients, recipes, and menus:

```bash
simard gastronome recipes      # list recipes (id, course, base servings)
simard gastronome menus        # list menus and their recipes
```

## 2. Plan the built-in demo end-to-end

The fastest way to see a full plan is the demo brief:

```bash
simard gastronome demo
```

You get a report with four things a kitchen needs: the scaled menu, the cost
(total and per guest), the per-guest nutrition, and a prep schedule that
finishes at service time, for example:

```
Cost
  total:     91.67
  per guest: 3.82

Prep schedule (kitchen opens 16:03, service 18:00)
  16:03–16:18  [prep] Caprese salad — Slice tomatoes and mozzarella
  ...
  17:50–18:00  [plate] Pasta pomodoro — Toss and plate
```

## 3. Write your own brief

A brief is a JSON **or** TOML document. `service_time_min` is
minutes-from-midnight (`1080` = 18:00):

```json
{
  "event_name": "Client lunch",
  "guest_count": 16,
  "menu_id": "vegan-gf-lunch",
  "dietary_restrictions": ["vegan", "gluten-free"],
  "budget_per_guest": 8.0,
  "service_time_min": 750
}
```

Plan it — add `--json` for a machine-readable document you can pipe into `jq`:

```bash
simard gastronome plan brief.json
simard gastronome plan brief.json --json | jq '.cost'
```

## 4. Dietary restrictions fail closed

If a menu cannot satisfy a required restriction, the plan is **refused** rather
than silently serving a violating dish. For example, asking for a `vegan`
Italian dinner (which contains dairy and gluten) errors:

```bash
$ simard gastronome plan vegan-italian.json
Error: recipe 'Caprese salad' violates dietary restriction 'vegan'
```

Point the same brief at a menu that satisfies the restriction
(`vegan-gf-lunch`) and it plans successfully.

## 5. Scale a single recipe

To scale one recipe without planning a whole event:

```bash
simard gastronome scale caprese 24        # 24 servings, with costed lines
simard gastronome scale caprese 24 --json
```

## Notes

- Budgets are advisory: exceeding `budget_per_guest` adds a warning to the plan
  rather than failing it.
- Prep-step durations are per-recipe active-work estimates and do not grow with
  batch size; the schedule serialises a single kitchen line in stage order
  (prep → cook → plate) so the final task ends exactly at service time.
