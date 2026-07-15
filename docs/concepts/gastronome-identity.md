---
title: Gastronome — the culinary / menu & event-design identity
description: How the Gastronome pluggable Simard identity turns an event or menu brief into a costed, scheduled menu plan — with nutrition and cost analysis, recipe scaling, and prep scheduling — through a pure, deterministic in-tree engine and the `simard gastronome` command tree.
last_updated: 2026-07-15
owner: simard
doc_type: concept
related:
  - ../howto/plan-a-costed-menu.md
  - ./pluggable-identity.md
  - ../reference/simard-cli.md
---

# Gastronome — the culinary / menu & event-design identity

## The problem

Simard's built-in identities (`simard-engineer`, `simard-meeting`,
`simard-gym`, the two curators) are all software-engineering personas. Some
work is not software at all: designing recipes and menus, then taking a
catering or event **brief** to a plan a kitchen can actually run — courses
chosen, quantities scaled to the guest count, cost and nutrition rolled up,
and a prep timeline that ends at service time.

**Gastronome** is a pluggable Simard identity for exactly that domain.

## The identity

`simard-gastronome` is a built-in identity (registered in
`BuiltinIdentityLoader`, see [pluggable identity](./pluggable-identity.md))
with its own system prompt, `prompt_assets/simard/gastronome_system.md`, and a
menu-design recipe, `prompt_assets/simard/recipes/gastronome-menu-design.yaml`.
It runs in engineer operating mode and, like the other identities, composes
over the same pluggable base types (`local-harness`, `rusty-clawd`,
`copilot-sdk`, `claude-agent-sdk`, `ms-agent-framework`).

The identity owns the *qualitative* design — which dishes, how they pair, how
they are presented. It delegates every *number* to a deterministic engine so a
plan is always reproducible and auditable.

## The deterministic engine

The reproducible core is the in-tree `gastronome` module (`src/gastronome/`) —
pure Rust with no I/O, clock, or network — exposed through the
`simard gastronome` command tree (the "kitchen app"). Its capabilities map
one-to-one to submodules:

| Capability | Module | What it computes |
|------------|--------|------------------|
| Menu design | `library` | A built-in pantry of ingredients, recipes, and menus, plus dietary-tag resolution (a recipe satisfies the *intersection* of its ingredients' tags). |
| Scaling | `scaling` | Scale any recipe from its base yield to the event's guest count, with resolved, costed ingredient lines. |
| Nutrition analysis | `nutrition` | Aggregate per-unit ingredient facts into per-recipe, per-batch, and per-guest macro-nutrients. |
| Cost analysis | `cost` | Per-recipe, total, and per-guest cost, with an optional per-guest budget check. |
| Prep scheduling | `scheduling` | Back-schedule stage-ordered prep tasks (prep → cook → plate) so the run finishes exactly at service time; derive the kitchen-open time. |
| End-to-end plan | `planner` | Resolve the menu, enforce dietary restrictions **fail-closed**, scale, roll up nutrition and cost, and schedule — producing one `MenuPlan`. |

Times are modelled as minutes-from-midnight (no clock dependency), and money
and nutrition are rounded only at the edges, so totals never drift.

## Design principles

- **Honest numbers.** Every cost and nutrition figure traces to ingredient
  data; the engine never invents totals.
- **Fail closed on dietary safety.** A required restriction (vegan,
  gluten-free, halal, …) that a dish cannot meet is a hard error, not a
  warning — the plan is refused rather than silently serving a violation.
- **Budget honesty.** Exceeding a per-guest budget is surfaced as a warning on
  the plan, never hidden.
- **Ruthless simplicity.** A clear, runnable, costed menu beats an elaborate
  one that cannot be scheduled.

## The "Done" contract

A Gastronome plan is *done* when a brief has become a **costed, scheduled menu
plan** end-to-end: courses chosen, recipes scaled to the guest count,
nutrition and cost rolled up (with budget warnings where relevant), and a prep
schedule that finishes at service time. The
[how-to guide](../howto/plan-a-costed-menu.md) walks through producing one.
