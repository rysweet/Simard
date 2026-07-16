# Gastronome — Event Plan (end to end)

Take an event/menu brief all the way to a **costed, scheduled menu plan** the
kitchen can execute. This is the Gastronome's "done when": brief in, plan out.

## The Brief

An `EventBrief` captures the whole ask:

- `name` — event name.
- `guests` — headcount to cater (must be positive).
- `serve_time` — when food hits the table, `HH:MM` on a 24-hour clock.
- `courses` — the courses wanted, in serving order (each may pin a `recipe_id`
  and add per-course `dietary` constraints).
- `dietary_constraints` — constraints that apply to every course.

## The Plan

Running `simard-gastronome plan --brief <file> [--recipes <file>] [--json]`
produces an `EventPlan` with five parts:

1. **Menu** — one dish per course, scaled to the guest count, with whole-batch
   counts and per-dish prep time.
2. **Cost** — per-course cost, whole-event total, and cost per guest.
3. **Nutrition** — calories and macros per guest and for the whole event.
4. **Shopping list** — consolidated across the menu by ingredient and unit,
   with per-line quantities and costs.
5. **Prep schedule** — every dish back-scheduled to finish at the serve time,
   with a single "kitchen call" start time and each step marked hands-on or
   passive.

## Verify Before You Present

- Every requested course is filled; no dietary constraint is violated.
- Shopping-line costs sum to the course totals; per-guest cost × guests ≈ total.
- The schedule's last step of each dish lands at the serve time; the kitchen
  call time is the earliest start across the menu.
- Batch counts cover the headcount (`ceil(guests / base_servings)`).

## Optional: the prep app

`simard-gastronome` is the runnable kitchen app. `demo` plans a built-in brief;
`recipes` lists the recipe book; `plan --json` emits machine-readable output for
downstream tooling (printed prep sheets, station cards, procurement). A kitchen
can drive the whole service from this one command.

See `simard/gastronome_recipe_design.md` and `simard/gastronome_menu_design.md`
for the recipe- and menu-level contracts this flow composes.
