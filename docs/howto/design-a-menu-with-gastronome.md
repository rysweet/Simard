# Design a Menu with the Gastronome Identity

The **Gastronome** is a pluggable Simard identity for culinary, menu, and event
design. Its headline capability is end-to-end: take a catering **brief** and
turn it into a **costed, scheduled menu plan** — per-serving nutrition, a
consolidated shopping list, per-guest cost against budget, and a backward-planned
prep schedule that has every dish ready by service time.

The numbers come from a pure, offline engine (`src/gastronome/`) exposed by the
`simard-kitchen` CLI (the small "run a kitchen" app). This guide shows the fast
path and the full file-driven path.

## Quick start: the built-in demo

Plan a complete garden-wedding menu with no input files:

```bash
cargo run --bin simard-kitchen -- demo
```

You get courses (with cost and per-serving calories), a consolidated shopping
list, total and per-guest cost with a budget verdict, per-guest nutrition, and a
prep schedule with a kitchen "call time" and per-task start/finish times. Add
`--json` for a machine-readable [`MenuPlan`](../../src/gastronome/planner.rs).

## Full path: plan from a kitchen book

A **kitchen book** is a TOML file of priced, nutrition-tagged ingredients and
recipes. It may embed a `[brief]`, or you can pass a standalone brief file.

```toml
# book.toml
[[ingredient]]
id = "flour"
name = "Bread flour"
unit = "gram"            # base family: gram | milliliter | each
price_per_base = 0.0018  # cost of ONE base unit (here, $/gram)
calories = 364           # macros are per 100 base units
protein_g = 12
carbs_g = 76
fat_g = 1.2
tags = ["vegan", "contains-gluten"]

[[ingredient]]
id = "olive_oil"
name = "Extra-virgin olive oil"
unit = "milliliter"
price_per_base = 0.012
calories = 884
fat_g = 100

[[recipe]]
id = "focaccia"
name = "Rosemary focaccia"
servings = 8
prep_minutes = 30
cook_minutes = 150
depends_on = []
  [[recipe.ingredients]]
  ingredient = "flour"
  quantity = 300
  unit = "gram"          # gram|kilogram|milliliter|liter|each; family must match
  [[recipe.ingredients]]
  ingredient = "olive_oil"
  quantity = 40
  unit = "milliliter"

[brief]
name = "Garden Wedding Dinner"
guest_count = 40
service_time = "18:30"   # 24-hour HH:MM
budget_per_guest = 18.0
  [[brief.courses]]
  recipe = "focaccia"
  portions_per_guest = 1.0
```

Cost and schedule it:

```bash
simard-kitchen plan --file book.toml            # human-readable
simard-kitchen plan --file book.toml --json     # machine-readable MenuPlan
```

Other subcommands:

```bash
simard-kitchen shopping-list --file book.toml   # consolidated list + total
simard-kitchen schedule --file book.toml        # just the prep timeline
simard-kitchen scale --file book.toml --recipe focaccia --servings 40
```

Override the embedded brief with a standalone one:

```bash
simard-kitchen plan --file book.toml --brief tasting.toml
```

## How the plan is computed

- **Scaling.** Each course is scaled to `guest_count × portions_per_guest`
  servings; every quantity is normalised to the ingredient's base unit
  (grams / millilitres / each).
- **Cost.** `base_quantity × price_per_base` per line, summed per recipe and
  across the menu, then divided by guests and checked against
  `budget_per_guest`.
- **Nutrition.** Macros stored per 100 base units are aggregated per serving and
  summed into a per-guest total.
- **Schedule.** A backward pass anchors every dish to `service_time`; a recipe's
  `depends_on` prerequisites must *finish* before it *starts*. The earliest
  required start becomes the kitchen call time. Cyclic dependencies are rejected.

## Using the identity conversationally

The Rust engine is the deterministic core; the identity's prompts wrap it for
conversational menu design:

- System prompt: `prompt_assets/simard/gastronome_system.md`
- Menu-design task prompt: `prompt_assets/simard/gastronome_menu_design.md`
- Recipe: `prompt_assets/simard/recipes/gastronome-menu-design.yaml`

The identity is registered as `simard-gastronome` in the builtin identity loader
(`src/identity/loader.rs`), so it can be selected like any other Simard identity.
It runs in engineer mode, designing recipes and menus and calling
`simard-kitchen` for ground-truth cost, nutrition, and scheduling.
