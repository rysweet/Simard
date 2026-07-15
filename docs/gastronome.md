# Gastronome — Culinary, Menu & Event-Design Identity

The **Gastronome** is a pluggable Simard identity that designs recipes, menus,
and catering/event plans. It takes an event/menu brief and drives it — end to
end — to a **costed, scheduled menu plan** a kitchen can execute.

Like every Simard identity, the Gastronome works the loop
**inspect → act → verify → persist** and never invents numbers: all cost,
nutrition, scaling, and prep-scheduling figures come from a deterministic
engine, not from guesswork.

## Components

| Component | Path | Role |
|---|---|---|
| Identity persona | `prompt_assets/simard/gastronome_system.md` | The LLM-facing contract: what the Gastronome delivers and how it works. |
| Menu-design prompt | `prompt_assets/simard/gastronome_menu_design.md` | Turns a free-text brief into a machine-readable brief JSON. |
| Orchestration recipe | `prompt_assets/simard/recipes/gastronome-menu-plan.yaml` | Design → cost & schedule → verify, via the recipe runner. |
| Engine (library) | `src/gastronome/` | Deterministic cost / nutrition / scaling / scheduling / planning. |
| Kitchen app (CLI) | `gastronome-kitchen` (`src/bin/gastronome_kitchen.rs`) | Brief JSON → costed, scheduled plan. |

## The kitchen app

`gastronome-kitchen` is the reproducible numeric engine. The persona delegates
all arithmetic to it so a plan's figures are deterministic.

```text
gastronome-kitchen sample-brief                          # emit a valid sample brief JSON
gastronome-kitchen plan --brief <path|-> --format json   # brief -> costed, scheduled plan
gastronome-kitchen plan --brief <path|-> --format text   # human-readable summary
```

`plan` reads a brief from a file (or `-` for stdin), writes the plan to stdout,
and exits `0`. On any error it writes a JSON envelope `{ "error": "<msg>" }` to
stderr and exits `2`.

### Example — brief to plan

```console
$ gastronome-kitchen sample-brief | gastronome-kitchen plan --brief - --format text
Menu plan — Summer Garden Luncheon (24 guests)
Service: 2026-08-15 12:00 UTC

Courses:
  - [appetizer] Garden Salad
  - [main] Quinoa Chickpea Bowl
  - [dessert] Honey Berry Cup

Cost:
  Garden Salad                 $   16.86 total  ($0.70/guest)
  Quinoa Chickpea Bowl         $   41.76 total  ($1.74/guest)
  Honey Berry Cup              $   25.56 total  ($1.06/guest)
  EVENT TOTAL                  $   84.18 total  ($3.51/guest)
  Budget: within $12.00/guest (spent $3.51, $8.49 headroom)

Nutrition per guest:
  797 kcal | protein 26.3 g | carbs 106.0 g | fat 31.8 g

Prep schedule (2 cook(s), start 08:10 → service 12:00):
  cook1 08:10–08:30 [ahead ] Quinoa Chickpea Bowl: Rinse and simmer quinoa (20m)
  ...
  Total active prep: 408 min across 2 cook(s); makespan 230 min
```

## Brief schema

A brief is a single JSON object. Run `gastronome-kitchen sample-brief` for a
full, valid example.

| Field | Meaning |
|---|---|
| `event_name` | Human-readable event name. |
| `guest_count` | Number of guests to serve. |
| `event_start` | RFC 3339 UTC service time; **all prep finishes by this time**. |
| `budget_per_guest_usd` | Optional per-guest budget. Omit for unconstrained. |
| `dietary_constraints` | Tags every recipe must satisfy (`vegetarian`, `vegan`, `gluten_free`, `dairy_free`, `nut_free`, `halal`). |
| `cook_count` | Cooks available to prep in parallel (minimum 1). |
| `catalog` | Ingredients: `name`, `unit` (`gram`/`milliliter`/`piece`), `cost_per_unit_usd`, per-unit `nutrition`, `tags`. |
| `menu.recipes` | Each: `name`, `course`, `base_servings`, `ingredients` (catalog refs + `quantity`), `steps`. |

A **step** has `description`, `minutes` (at base yield), `make_ahead` (can be
done before service), and `scales_with_servings` (duration grows with quantity).

## How the engine computes a plan

Given a brief, `plan_event` (`src/gastronome/planner.rs`) runs this pipeline:

1. **Validate** — the menu is non-empty, every ingredient resolves against the
   catalog, and yields are positive.
2. **Dietary check** — a recipe satisfies a required tag only when *every*
   ingredient carries it; any violation is a hard error.
3. **Scale** — each recipe is scaled from its base yield to the guest count;
   ingredient quantities scale linearly, step durations scale only when
   `scales_with_servings` is set.
4. **Cost** — per-recipe and per-guest cost plus the event total, checked
   against the budget (`within_budget` / `over_budget` / `unconstrained`).
5. **Nutrition** — macro-nutrients per guest (one serving of each recipe).
6. **Schedule** — recipes are balanced across cooks (longest-processing-time
   greedy) and laid out **backward** from the service time, so make-ahead work
   is pulled earlier and at-service work finishes exactly at service.

## Running the identity via the recipe runner

```bash
amplihack recipe run gastronome-menu-plan \
  -c event_brief="Vegetarian garden luncheon for 24, noon Aug 15, ~$12/head, 2 cooks" \
  -c brief_output=/tmp/brief.json \
  -c plan_output=/tmp/plan.json \
  -c repo_path=.
```

The recipe's first step designs the menu and writes the brief JSON; the second
runs `gastronome-kitchen plan`, revises on any error or over-budget result, and
writes the final costed, scheduled plan.

## Library API

The engine is a self-contained module (`simard::gastronome`). Key entry points:

```rust
use simard::gastronome::{plan_event, sample_brief, EventBrief, MenuPlan};

let brief: EventBrief = sample_brief();
let plan: MenuPlan = plan_event(&brief).expect("valid brief");
assert!(plan.budget.is_affordable());
```

See also `recipe_cost`, `recipe_nutrition`, `scale_recipe`, and
`build_schedule` for the individual analyses.

## Testing

- Unit tests: `cargo test --lib gastronome::`
- End-to-end (CLI): `cargo test --test gastronome_end_to_end`
