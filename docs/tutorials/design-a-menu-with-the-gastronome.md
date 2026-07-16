# Design a Menu with the Gastronome

The **Gastronome** is a pluggable Simard identity (`simard-gastronome`) for
culinary, menu, and catering/event design. It takes an event or menu brief all
the way to a **costed, scheduled menu plan** — and, when you want it, a prep
sheet the kitchen runs on the day.

This tutorial takes you from a brief to a finished plan using the
`simard-gastronome` kitchen app, the deterministic engine behind the identity.

## Prerequisites

- A build of Simard (`cargo build --bin simard-gastronome`).

## 1. See a plan end-to-end

The app ships with a built-in sample so you can see the whole flow with no input
files:

```bash
simard-gastronome --demo
```

You get a menu with per-item cost, a budget verdict, per-guest nutrition, and a
prep schedule that is **back-scheduled from the service time** so every task
finishes exactly when the food is served:

```text
## Prep schedule (kitchen starts 17:45, 105 min lead)
- 17:45–18:00  Herb roast chicken · Season and truss chicken
- 18:00–19:15  Herb roast chicken · Roast until cooked through
- ...
```

Add `--format json` for a machine-readable `MenuPlan`.

## 2. Write a kitchen brief bundle

A **kitchen brief bundle** is a single JSON object with four parts:

```json
{ "ingredients": [ ... ], "recipes": [ ... ], "menu": { ... }, "brief": { ... } }
```

- **ingredients** — pantry items. Cost and nutrition are given **per unit**
  (`gram`, `milliliter`, or `piece`) so the engine aggregates by multiply-and-sum.
  Each ingredient also declares its `allergens`, `vegetarian`, and `vegan` flags.
- **recipes** — each has a base `servings` count, ingredient lines (a `quantity`
  in the ingredient's unit), and ordered **prep steps** with `duration_minutes`
  and `depends_on` (a per-recipe dependency graph).
- **menu** — the items to serve, each a `recipe_id` served as a `course`
  (`appetizer | main | side | dessert | beverage`).
- **brief** — `guests`, `service_time_minutes` (minutes since midnight, so 19:30
  is `1170`), an optional `budget_total`, and optional `dietary_restrictions`
  (`vegetarian | vegan`) and `excluded_allergens`.

A complete, runnable example lives at
[`docs/examples/gastronome-harvest-dinner.json`](../examples/gastronome-harvest-dinner.json).

## 3. Plan your brief

```bash
simard-gastronome plan docs/examples/gastronome-harvest-dinner.json
```

The engine, for `guests` diners each eating one serving of every item:

1. **scales** each recipe to the guest count,
2. **costs** it (total, per guest, and against the budget),
3. **analyses nutrition** per guest,
4. **screens** every ingredient against the brief's dietary restrictions and
   excluded allergens, and
5. **schedules** all prep backwards from the service time along each recipe's
   dependency graph, reporting when the kitchen must start.

## 4. Enforce the constraints in CI

Use `--strict` to make the app exit non-zero when the plan is over budget or not
dietary compliant — handy in a pipeline or a `qa-team` scenario:

```bash
simard-gastronome plan brief.json --strict
# exit 0 = clean, 3 = over budget or dietary violation, 1 = usage/parse error
```

## How the identity uses the app

The `simard-gastronome` identity never hand-calculates numbers. It assembles the
bundle from a brief (designing or adapting recipes as needed), runs the app, and
iterates — swapping ingredients, resizing portions, or re-timing prep — until the
plan is within budget and compliant. The
`gastronome-menu-design` recipe
(`prompt_assets/simard/recipes/gastronome-menu-design.yaml`) drives that loop.

A menu design is done only when the plan is verified end-to-end: brief in, a
costed and scheduled plan out, dietary constraints satisfied or their violations
explicitly surfaced.
