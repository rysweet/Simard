# Gastronome Identity — Culinary Design + Kitchen Operations App

The **Gastronome** is a pluggable Simard identity (`simard-gastronome`) for the
culinary/catering domain. It does two jobs, in order:

1. **Design the menu** — from an event/menu brief, produce a concrete menu
   concept: the menu itself (courses and recipes), an event service flow, and a
   menu identity.
2. **Run the kitchen** — stand up a runnable kitchen app: scale every recipe to
   the guest count, produce a costed shopping list, run nutrition and cost
   analysis, and build a prep schedule.

It is *done* when it can take an event/menu brief to **a costed, scheduled menu
plan (and optional prep app) end-to-end** — the menu designed, scaled to the
guest count, priced, its nutrition analyzed, and its prep scheduled, with the
invariants verified.

## Where it lives

| Surface | Location |
|---|---|
| Identity | `simard-gastronome` in `src/identity/loader.rs` (mode: `orchestrator`) |
| Runnable domain module | `src/gastronome/` (`design`, `kitchen`, orchestrator) |
| System prompt | `prompt_assets/simard/gastronome_system.md` |
| Design / scaffold prompts | `prompt_assets/simard/gastronome_menu_design.md`, `gastronome_kitchen_scaffold.md` |
| Recipes | `prompt_assets/simard/recipes/gastronome-{menu-design,kitchen-scaffold,end-to-end}.yaml` |
| Operator probe | `simard_operator_probe gastronome-run <topology> "<brief>"` |

## The runnable prototype (`simard::gastronome`)

The `gastronome` module is the source of truth for what "runnable" means. It is
deterministic and dependency-light, so the same brief always yields the same
menu and the plan can be exercised in CI without any model call. All money is
integer cents and all quantities/calories are integers, so scaling to any guest
count is exact.

- `gastronome::design_menu(&brief) -> MenuConcept` — a menu (courses sized to
  the style tier, each dish carrying per-serving ingredients and prep tasks), an
  event service flow, and a menu identity.
- `gastronome::KitchenEngine::from_concept(&concept)` — seeds the recipes, then
  supports:
  - **Scaling**: `scale_recipe(code, guests)` — exact integer scaling of cost,
    calories, and quantities.
  - **Costed shopping list**: `shopping_list(guests)` — aggregates identical
    ingredients across the menu.
  - **Cost analysis**: `cost_analysis(guests)` — per-guest and total cents.
  - **Nutrition analysis**: `nutrition_analysis(guests)` — per-guest and total
    calories.
  - **Prep scheduling**: `prep_schedule(guests)` — fires tasks per station,
    batched by guest count, reporting wall-clock time to be service-ready.
- `gastronome::run_gastronome(&brief) -> GastronomeOutcome` — designs,
  scaffolds, costs, scales, schedules, and **verifies** the operational
  invariants.

### Verified invariants

1. Every course yields at least one dish (dish count ≥ course count).
2. Menu cost scales exactly: `per_guest_cents * guests == total_cents`.
3. Nutrition scales exactly: `per_guest_calories * guests == total_calories`.
4. The costed shopping list reconciles with the total menu cost.
5. The prep schedule covers every task, has positive wall-clock time, and never
   double-books a station.

## Security posture

The brief is treated as **untrusted data**. `MenuBrief::from_prompt` extracts
only design signals (name, occasion, guest count, style, theme) and never obeys
instructions embedded in the text (e.g. "ignore the rules above"). This is
covered by tests in `src/gastronome/design.rs` and
`tests/gastronome_end_to_end.rs`.

## Try it

```bash
# End-to-end via the runnable example
cargo run --example gastronome_end_to_end
cargo run --example gastronome_end_to_end -- "Aurora Tasting menu for a gala of 90 guests, fine dining"

# End-to-end via the operator probe (prints the menu + a verified costed plan)
cargo run --bin simard_operator_probe -- \
  gastronome-run single-process "Harvest Feast menu for a wedding of 120 guests, elegant plated"

# Confirm the identity bootstraps as a first-class identity
cargo run --bin simard_operator_probe -- \
  bootstrap-run simard-gastronome local-harness single-process "verify gastronome bootstrap"
```

A passing `gastronome-run` ends with a `Sample scaled dish: C…` line,
`Plan verified: yes`, and `Session phase: complete`.

## Tests

- Unit: `src/gastronome/{design,kitchen}.rs` and `src/gastronome/mod.rs`
  (`#[cfg(test)]`).
- Integration: `tests/gastronome_end_to_end.rs`.
- Outside-in scenarios: `tests/gadugi/gastronome-identity.{sh,yaml}` and
  `tests/qa-scenarios/gastronome-end-to-end.yaml`.
