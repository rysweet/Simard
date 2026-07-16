# Gastronome — Menu Design

Compose a coherent multi-course menu from a recipe book, honouring the brief's
dietary constraints, then hand it to the planner for costing, nutrition, and
scheduling.

## Inputs

- A recipe book (JSON array of `Recipe`, or `{ "recipes": [...] }`).
- The desired courses, in serving order (e.g. `starter`, `main`, `side`,
  `dessert`).
- Brief-wide dietary constraints, plus any per-course constraints.

## Selection Rules

- **Fill every course.** For each course, choose a recipe whose `course` matches
  and which carries **every** required dietary tag (brief-wide ∪ per-course).
- **Deterministic default.** When multiple recipes qualify, pick the
  **cheapest per serving**, breaking ties by `id`. Pin a specific dish with
  `recipe_id` when a client asks for it by name.
- **No silent relaxation.** If no recipe fills a course under the constraints,
  report the course and the constraints that excluded every candidate, and
  propose either a new recipe (see `simard/gastronome_recipe_design.md`) or the
  closest alternative. Never serve a dish that violates a constraint.
- **Balance.** Prefer contrast across courses (light → substantial → sweet) and
  avoid repeating a hero ingredient across every course when alternatives exist.

## Output

A menu is realised as an `EventBrief` fed to the planner:

```json
{
  "name": "Summer Dinner",
  "guests": 24,
  "serve_time": "19:30",
  "dietary_constraints": ["vegetarian"],
  "courses": [
    { "course": "starter" },
    { "course": "main", "dietary": ["gluten-free"] },
    { "course": "dessert", "recipe_id": "fruit-crumble" }
  ]
}
```

Run `simard-gastronome plan --brief <file> [--recipes <file>]` to produce the
costed, nutritionally analysed, prep-scheduled plan. Verify every course is
filled and the per-guest cost and nutrition look sane before presenting it. See
`simard/gastronome_event_plan.md` for the end-to-end event flow.
