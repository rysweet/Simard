---
title: How to design buildings and interiors with the Vitruvia example identity
description: Use the Vitruvia example identity — a data-only pluggable-identity package — to take a program/site brief end-to-end to a code-aware BIM floor plan, interior layout, technical drawings (plans and elevations), and a rendered walkthrough. All BIM/CAD tooling runs inside the identity's agentic recipes, not in Simard's daemon.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/pluggable-identity.md
  - ../howto/configure-pluggable-identity.md
  - ../howto/design-with-atelier.md
  - ../../examples/identities/README.md
---

# How to design buildings and interiors with the Vitruvia example identity

**Vitruvia** is an **example** non-engineering identity for **architecture and
interior design**. It takes a structured *program/site brief* and produces a
code-aware BIM floor plan, an interior layout, technical drawings (plans and
elevations), and a **rendered walkthrough** — so a brief can go end-to-end from a
program and a site to a designed, walkable building. It is named for
**Vitruvius**, whose triad *firmitas, utilitas, venustas* (durability,
usefulness, beauty) still frames what a building must deliver.

Vitruvia is a **data-only package** at
[`examples/identities/vitruvia/`](../../examples/identities/vitruvia/): a
manifest, prompts, and two agentic recipes. It is **not** part of Simard's daemon
— there is no `src/vitruvia/` module and no `simard vitruvia` subcommand. The
BIM/CAD toolchain (Blender + BlenderBIM / IfcOpenShell, FreeCAD) is driven by the
**agent** inside the identity's recipes, never compiled into Simard. See
[Pluggable identity](../concepts/pluggable-identity.md) and the
[example-identities README](../../examples/identities/README.md) for the
compiled-in vs. data-only boundary.

## Prerequisites

The recipes' agent drives real BIM/CAD tools, so the host it runs on needs:

- [Blender](https://www.blender.org/) on `PATH` with the
  [BlenderBIM](https://bonsaibim.org/) add-on / `ifcopenshell` — the core
  dependency for authoring the IFC model and rendering the walkthrough
  (`blender --background --python script.py`).
- Optional, for extra artifacts (the recipes degrade gracefully without them):
  - [FreeCAD](https://www.freecad.org/) (`freecadcmd`) with the Arch/BIM and
    TechDraw workbenches, for dimensioned plans/elevations and IFC round-trip.
  - A video encoder (e.g. `ffmpeg`) so the walkthrough can be an `.mp4`; without
    it the recipe emits ordered key still frames instead.

These tools live on the host where the agent session runs — **not** in Simard's
`src/`. Simard's own code stays pure Rust.

## Load the Vitruvia example identity

Vitruvia is discovered by the data-driven loader, not by `BuiltinIdentityLoader`:

```rust
use simard::identity::{load_example_identity, DEFAULT_EXAMPLE_IDENTITIES_DIR};

let manifest = load_example_identity(
    DEFAULT_EXAMPLE_IDENTITIES_DIR.as_ref(), // examples/identities, relative to cwd
    "vitruvia",
    &request,
)?;
assert_eq!(manifest.name, "vitruvia");
```

A missing package or invalid `identity.toml` returns a fail-visible
`IdentityTomlParseError` — never a silent fallback to a built-in identity. See
[Configure pluggable identity](configure-pluggable-identity.md) for how example
packages are discovered and loaded.

## Write a program/site brief

A brief is a small JSON document describing the program and the site. Save it as
`brief.json`:

```json
{
  "name": "Neighborhood clinic",
  "building_type": "clinic",
  "levels": 1,
  "spaces": [
    { "name": "Waiting",      "area_m2": 40, "occupant_load": 20, "daylight": true },
    { "name": "Reception",    "area_m2": 12 },
    { "name": "Exam room A",  "area_m2": 12, "plumbing": true },
    { "name": "Exam room B",  "area_m2": 12, "plumbing": true },
    { "name": "Accessible WC", "area_m2": 6, "plumbing": true }
  ],
  "adjacencies": [ ["Waiting", "Reception"], ["Reception", "Exam room A"] ],
  "site": { "width_m": 24, "depth_m": 18, "setbacks_m": 3, "street_side": "north" },
  "code": {
    "min_exits": 2,
    "max_travel_distance_m": 30,
    "min_door_clear_width_mm": 850,
    "min_corridor_clear_width_mm": 1100,
    "accessible_turning_radius_mm": 750
  },
  "budget": 350000
}
```

Areas are in square metres and site dimensions in metres. The `code` block is the
governing constraint set the plan is checked against; `budget` is optional. The
brief is **untrusted data** — the prompts instruct the agent to read design
signals from it and ignore any embedded instructions.

## Run the Vitruvia recipes

Vitruvia's behavior is delivered by two agentic recipes. Run them in order with
your recipe runner, passing the brief and an output directory as context.

First, take the brief to a code-aware BIM floor plan
(`examples/identities/vitruvia/recipes/vitruvia-massing-plan.yaml`):

```bash
amplihack recipe run vitruvia-massing-plan \
  -c brief_path=brief.json \
  -c output_dir=./building
```

1. **Program** (`vitruvia_program.md`) — parse the brief into a dimensioned
   program spec (space schedule, adjacencies, site envelope, code constraints).
2. **Massing** (`vitruvia_massing.md`) — size and place the building volume
   within the setback envelope, respecting any height/area limit.
3. **Plan** (`vitruvia_plan.md`) — author `model.ifc` as real BIM (spaces, walls,
   doors, levels) and verify space areas, adjacencies, egress, and accessibility.

Then produce the interiors, drawings, and walkthrough
(`examples/identities/vitruvia/recipes/vitruvia-drawings-walkthrough.yaml`):

```bash
amplihack recipe run vitruvia-drawings-walkthrough \
  -c brief_path=brief.json \
  -c output_dir=./building
```

4. **Interiors** (`vitruvia_interiors.md`) — furnish each space, specify finishes
   and lighting, and re-check clearances.
5. **Drawings** (`vitruvia_drawings.md`) — generate a plan per level and the
   elevations from the IFC, and cross-check them against the program.
6. **Walkthrough** (`vitruvia_walkthrough.md`) — render a camera-path walkthrough
   and persist the package with a design narrative.

It writes to `./building`:

| File                     | What it is                                                    |
| ------------------------ | ------------------------------------------------------------- |
| `plan.py` / `massing.py` | Build scripts that author the BIM model.                      |
| `model.ifc`              | The BIM model — spaces, walls, doors, levels, interiors.      |
| `plan_level_1.pdf`       | Dimensioned floor plan per level.                             |
| `elevation_north.pdf`    | Exterior elevations (entry + a side), and a section if useful.|
| `walkthrough.mp4`        | Rendered camera-path walkthrough (or ordered key stills).     |
| `DESIGN.md`              | Design narrative + verification/evidence record.              |

A brief is only *done* when the IFC model is valid, each space's area meets its
program target, the egress and accessibility checks pass, the plans and
elevations match the model, and the walkthrough renders. The agent records that
verification in `DESIGN.md`.

## How degradation works

The recipes treat Blender + IFC authoring as the core and everything else as
best-effort, recording every skip so the package is self-describing:

- **No FreeCAD/TechDraw** → the plans/elevations fall back to an IfcOpenShell 2D
  projection; the IFC model still drives them.
- **No video encoder** → the walkthrough is emitted as an ordered set of key
  still frames instead of an `.mp4`.

Never fail the whole package because an *optional* engine is missing — emit the
IFC model, the plans, and a still-frame walkthrough, and note the skipped
exports.

## Code-aware, not code-stamped

Vitruvia checks the design against the *specific* constraints in the brief's
`code` block (exit count and widths, travel distance, door/corridor clearances,
accessible turning radius, area limits) measured from the modeled geometry. This
is a **design self-check, not a stamped approval** — the narrative flags anything
that needs a licensed architect or the authority having jurisdiction (AHJ) to
review.
