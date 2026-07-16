---
title: Atelier identity — industrial & furniture design
description: The simard-atelier built-in identity turns a product brief into a parametric model, cut list, bill of materials, and — with OpenSCAD/FreeCAD — an STL/STEP export and a render.
last_updated: 2026-07-15
owner: simard
doc_type: concept
related:
  - ./pluggable-identity.md
  - ../reference/runtime-contracts.md
  - ../reference/simard-cli.md
---

# Atelier identity — industrial & furniture design

`simard-atelier` is a built-in Simard identity specialised in the design of
furniture and physical products. It takes a **product brief** and drives a
parametric CAD toolchain to fabrication-ready outputs: a parametric model, an
STL/STEP export, a render, a cut list, and a bill of materials.

It runs in Simard's **engineer** operating mode and follows the engineer loop
(**inspect → act → verify → persist**). Its persona and guardrails live in the
prompt asset `prompt_assets/simard/atelier_system.md`; its capability surface is
declared in `prompt_assets/simard/policies/atelier-capabilities.toml`.

## Selecting the identity

Because `simard-atelier` is a built-in identity, it is selectable anywhere an
identity name is accepted — for example the bootstrap probe:

```bash
simard bootstrap run simard-atelier local-harness single-process \
  "design an oak writing desk"
```

It is also driven directly by the fabrication CLI (below) and by two
goal-session recipes: `atelier-parametric-modeling` and `atelier-fabrication`.

## The fabrication engine

The identity is backed by a deterministic Rust fabrication engine
(`src/atelier/`) exposed as a CLI:

```bash
# Product brief -> shop artifacts:
simard atelier fabricate --brief brief.json --out ./out

# Or run the built-in example brief end-to-end:
simard atelier demo --out ./out
```

The engine **always** writes the deterministic artifacts and, when the external
CAD binaries are installed, also produces the tool-backed exports. A missing tool
is reported as *skipped*, never a hard failure — so the command is usable on any
machine and the pipeline is fully testable without the heavy binaries.

| Artifact | Tool | Notes |
|---|---|---|
| `<slug>.scad` | — | Parametric OpenSCAD model; variables mirror the brief |
| `cut_list.csv` | — | Every part: count, length, width, thickness, material |
| `bom.json` | — | Sheet stock, screws, dowels, glue, finish |
| `brief.json` | — | The normalised brief |
| `export_step.py` | — | FreeCAD macro (STL → STEP) |
| `manifest.json` | — | Describes every artifact and its status |
| `<slug>.stl` | OpenSCAD | Mesh export |
| `<slug>.png` | OpenSCAD | Rendered preview |
| `<slug>.step` | FreeCAD | BREP export for CAM/CNC |

For a presentation-quality render the identity may additionally drive
**Blender (`bpy`)**.

OpenSCAD needs an OpenGL context to rasterize the PNG preview. On a headless
host (no `DISPLAY`), Atelier transparently wraps the render in `xvfb-run` when
it is available, so the model **and** its render are produced end-to-end even
in CI or on a server. If neither a display nor `xvfb-run` is present, the STL
mesh is still exported and only the PNG is marked `failed` — the pipeline
degrades gracefully and never aborts.

## Product brief schema

```json
{
  "name": "Oak Writing Desk",
  "kind": "table",
  "width_mm": 1200,
  "depth_mm": 600,
  "height_mm": 740,
  "panel_thickness_mm": 18,
  "material": "oak",
  "shelves": 0,
  "quantity": 1,
  "finish": "oil"
}
```

- `kind` — one of `table`, `shelf` (bookcase), or `box` (cabinet/carcass/crate).
- `shelves` — number of interior shelves; only meaningful for `shelf`.
- `finish` — set to `"none"` to omit finish from the bill of materials.
- Dimensions are the *outer* bounding box in millimetres. Briefs with
  non-positive dimensions, or panels too thick to leave an interior, are
  rejected.

## Definition of done

An Atelier goal session is complete when a product brief has been taken to an
**exported model** (STL or STEP) **plus a render** (PNG), alongside a cut list
and bill of materials. When the CAD tools are not installed, the engine produces
the deterministic artifacts and reports the exports as skipped; installing
OpenSCAD (and FreeCAD for STEP) closes the end-to-end loop.
