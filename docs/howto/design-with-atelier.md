---
title: How to design furniture and products with the Atelier identity
description: Use the pluggable Atelier identity to take a product brief end-to-end to a 3D model, render, and fabrication package (STL, cut list, BOM) with the `simard atelier` CLI.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/pluggable-identity.md
  - ../howto/configure-pluggable-identity.md
  - ../reference/simard-cli.md
---

# How to design furniture and products with the Atelier identity

**Atelier** is a pluggable Simard identity for industrial &amp; furniture
design. It takes a structured *product brief* and produces a parametric 3D
model, a render, and a **fabrication package** — a cut list and bill of
materials (BOM) — so a brief can go end-to-end from an idea to shop-ready
outputs.

Atelier is repo-grounded and runs in engineer mode
(`inspect → act → verify → persist`): it generates geometry, drives CAD tools
to export artifacts, verifies the result against the brief, and writes a
`manifest.json` recording exactly what was built.

## Prerequisites

- Simard binary built (`cargo build --quiet --bin simard`).
- [OpenSCAD](https://openscad.org/) installed and on `PATH` — the only hard
  dependency for the model + STL export.
- Optional, for extra artifacts (Atelier degrades gracefully without them):
  - `xvfb-run` (Linux headless) so OpenSCAD can render a PNG with no display.
  - `freecadcmd` (FreeCAD) to additionally export a STEP solid.
  - `blender` for a photoreal render.

Check what is available:

```bash
simard atelier inspect --out /tmp/does-not-exist   # prints a tool report
```

## Select the Atelier identity

Atelier ships as a built-in identity (`simard-atelier`) and as a pluggable
identity card under
`prompt_assets/simard/identities/atelier/identity.toml`. Select it for a
session with the identity environment variable:

```bash
export SIMARD_IDENTITY=simard-atelier
```

See [Configure Pluggable Identity](configure-pluggable-identity.md) for how
identity cards are discovered and loaded.

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

Supported `kind` values include `bookcase`, `table`, `stool`, and `carcass`
(a generic cabinet box). Dimensions are in millimetres. `budget` is optional;
when set, Atelier flags an over-budget design as an advisory.

## Build the model and fabrication package

```bash
simard atelier build --brief brief.json --out ./pkg --fabrication
```

This writes to `./pkg`:

| File            | What it is                                              |
| --------------- | ------------------------------------------------------- |
| `model.scad`    | Parametric OpenSCAD source (the geometry model).        |
| `model.stl`     | Mesh export for 3D printing / CAM.                      |
| `render.png`    | Preview render (via OpenSCAD, headless with `xvfb-run`).|
| `cutlist.csv`   | Panel cut list: part, qty, length, width, thickness.    |
| `bom.csv`       | Bill of materials with cost roll-up.                    |
| `model.step`    | STEP solid — only when FreeCAD is installed.             |
| `manifest.json` | Build record + verification result.                     |

Example output:

```text
atelier: Two-shelf bookcase (bookcase) — 4 parts / 7 instances, 1 sheet(s)
  estimated material cost: 58.60
  [     ok] model.scad
  [     ok] model.stl
  [     ok] render.png — openscad via xvfb-run
  [     ok] cutlist.csv
  [     ok] bom.csv
  [skipped] model.step — freecadcmd not installed
  verification: PASS (render: yes)
```

Add `--strict` to make the command exit non-zero unless *every* advisory check
(including the render) passes — useful in CI where the render must be present.

## Verify an existing package

`inspect` re-reads a package directory and re-runs verification without
rebuilding:

```bash
simard atelier inspect --out ./pkg --fabrication
```

Verification always requires the core deliverables — valid geometry, an STL,
a cut list, a BOM, and that every part fits stock sheet stock. The render and
budget checks are advisory unless `--strict` is set, so Atelier still produces
a usable fabrication package on hosts without a display or without FreeCAD.

## How degradation works

Atelier treats OpenSCAD as required and everything else as best-effort:

- **No `xvfb-run` / display** → the STL, cut list, and BOM are still produced;
  the render is skipped (advisory).
- **No FreeCAD** → STEP export is skipped; the STL still covers CAM/printing.
- **No Blender** → the OpenSCAD render is used instead of a photoreal one.

Every skip is recorded in `manifest.json` with a reason, so the package is
always self-describing.
