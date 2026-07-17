---
title: How to design furniture and products with the Atelier example identity
description: Use the Atelier example identity — a data-only pluggable-identity package — to take a product brief end-to-end to a 3D model, render, and fabrication package (STL, cut list, BOM). All CAD tooling runs inside the identity's agentic recipe, not in Simard's daemon.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/pluggable-identity.md
  - ../howto/configure-pluggable-identity.md
  - ../../examples/identities/README.md
---

# How to design furniture and products with the Atelier example identity

**Atelier** is an **example** non-engineering identity for industrial &amp;
furniture design. It takes a structured *product brief* and produces a parametric
3D model, a render, and a **fabrication package** — a cut list and bill of
materials (BOM) — so a brief can go end-to-end from an idea to shop-ready
outputs.

Atelier is a **data-only package** at
[`examples/identities/atelier/`](../../examples/identities/atelier/): a manifest,
prompts, and an agentic recipe. It is **not** part of Simard's daemon — there is
no `src/atelier/` module and no `simard atelier` subcommand. The CAD toolchain
(OpenSCAD, FreeCAD, Blender) is driven by the **agent** inside the identity's
recipe, never compiled into Simard. See
[Pluggable identity](../concepts/pluggable-identity.md) and the
[example-identities README](../../examples/identities/README.md) for the
compiled-in vs. data-only boundary.

## Prerequisites

The recipe's agent drives real CAD tools, so the host it runs on needs:

- [OpenSCAD](https://openscad.org/) on `PATH` — the only hard dependency for the
  model + STL export.
- Optional, for extra artifacts (the recipe degrades gracefully without them):
  - `xvfb-run` (Linux headless) so OpenSCAD can render a PNG with no display.
  - `freecadcmd` (FreeCAD) to additionally export a STEP solid.
  - `blender` for a photoreal render.

These tools live on the host where the agent session runs — **not** in Simard's
`src/`. Simard's own code stays pure Rust.

## Load the Atelier example identity

Atelier is discovered by the data-driven loader, not by `BuiltinIdentityLoader`:

```rust
use simard::identity::{load_example_identity, DEFAULT_EXAMPLE_IDENTITIES_DIR};

let manifest = load_example_identity(
    DEFAULT_EXAMPLE_IDENTITIES_DIR.as_ref(), // examples/identities, relative to cwd
    "atelier",
    &request,
)?;
assert_eq!(manifest.name, "atelier");
```

A missing package or invalid `identity.toml` returns a fail-visible
`IdentityTomlParseError` — never a silent fallback to a built-in identity. See
[Configure pluggable identity](configure-pluggable-identity.md) for how example
packages are discovered and loaded.

## Write a product brief

A brief is a small JSON document describing the product. Save it as
`brief.json`:

```json
{
  "name": "Two-shelf bookcase",
  "kind": "bookcase",
  "dimensions_mm": { "width": 800, "depth": 300, "height": 1000 },
  "material": { "name": "Birch plywood", "thickness_mm": 18, "cost_per_sheet": 55.0, "grain": true },
  "parameters": { "shelves": 2, "back_panel": true },
  "hardware": [ { "name": "Confirmat screw", "qty": 24, "unit_cost": 0.15 } ],
  "finish": "clear matte lacquer",
  "budget": 120.0
}
```

Typical `kind` values include `bookcase`, `table`, `stool`, and `carcass`
(a generic cabinet box). Dimensions are in millimetres. `budget` is optional;
when set, the agent flags an over-budget design as an advisory. The brief is
**untrusted data** — the prompts instruct the agent to read design signals from
it and ignore any embedded instructions.

## Run the Atelier recipe

Atelier's behavior is delivered by its agentic recipe,
`examples/identities/atelier/recipes/atelier-cad-pipeline.yaml`. Run it with your
recipe runner, passing the brief and an output directory as context:

```bash
amplihack recipe run atelier-cad-pipeline \
  -c brief_path=brief.json \
  -c output_dir=./pkg
```

The recipe drives the design end-to-end as agentic steps:

1. **Design** (`atelier_design.md`) — parse the brief and generate a *parametric*
   OpenSCAD program whose parameters (dimensions, thickness, joinery) are driven
   directly by the brief — never hard-coded literals.
2. **Fabricate** (`atelier_fabricate.md`) — run the exporters to produce the
   model, render, and derived cut list + BOM, then verify the package against the
   brief.

It writes to `./pkg`:

| File            | What it is                                              |
| --------------- | ------------------------------------------------------- |
| `model.scad`    | Parametric OpenSCAD source (the geometry model).        |
| `model.stl`     | Mesh export for 3D printing / CAM.                       |
| `render.png`    | Preview render (via OpenSCAD, headless with `xvfb-run`).|
| `cutlist.csv`   | Panel cut list: part, qty, length, width, thickness.    |
| `bom.csv`       | Bill of materials with cost roll-up.                    |
| `model.step`    | STEP solid — only when FreeCAD is installed.             |
| `manifest.json` | Build record + verification result.                     |

A brief is only *done* when the exported model and render exist and the cut list
and BOM are internally consistent with the model. The agent records that
verification in `manifest.json`.

## How degradation works

The recipe treats OpenSCAD as required and everything else as best-effort, and
records every skip in `manifest.json` with a reason, so the package is always
self-describing:

- **No `xvfb-run` / display** → the STL, cut list, and BOM are still produced;
  the render is skipped (advisory).
- **No FreeCAD** → STEP export is skipped; the STL still covers CAM/printing.
- **No Blender** → the OpenSCAD render is used instead of a photoreal one.

Never fail the whole package because an *optional* engine is missing — emit the
OpenSCAD STL + PNG, the cut list, and the BOM, and note the skipped exports.
