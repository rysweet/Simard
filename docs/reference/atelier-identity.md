---
title: Atelier identity reference
description: Reference for the pluggable simard-atelier industrial & furniture design identity — its identity card, capabilities, the simard-atelier-build CLI, the product brief schema, and the fabrication artifacts it produces.
last_updated: 2026-07-16
owner: simard
doc_type: reference
related:
  - ../concepts/pluggable-identity.md
  - ../reference/pluggable-identity-api.md
---

# Atelier identity reference

`simard-atelier` is a pluggable Simard identity for **industrial & furniture
design**. It runs in the engineer operating mode (inspect → act → verify →
persist) and takes a **product brief** to an **exported 3D model + render** plus
**fabrication-ready outputs** (STL mesh, cut list, and bill of materials).

- Prompt asset: `prompt_assets/simard/atelier_system.md`
- Goal-session recipes: `prompt_assets/simard/recipes/atelier-parametric-modeling.yaml`,
  `prompt_assets/simard/recipes/atelier-fabrication.yaml`
- Repo-grounded tool: the `simard-atelier-build` binary
  (`src/bin/simard_atelier_build.rs`, module `simard::atelier`)

## Identity card

| Field | Value |
|---|---|
| Identity | `simard-atelier` |
| Operating mode | `engineer` |
| Base types | `local-harness`, `terminal-shell`, `rusty-clawd`, `copilot-sdk`, `claude-agent-sdk`, `ms-agent-framework` |
| Capabilities | PromptAssets, SessionLifecycle, Memory, Evidence, Reflection |
| Prompt asset | `simard/atelier_system.md` |

Because it is a built-in identity, it is selectable anywhere Simard resolves an
identity by name — including the operator probe's repo-grounded
engineer-loop-run surface:

```bash
simard_operator_probe bootstrap-run simard-atelier local-harness single-process \
  "design a small oak side table"
```

## The `simard-atelier-build` tool

```bash
simard-atelier-build --brief <brief.json> --out <dir> [--no-cad]
```

The tool is deterministic and needs no external dependency. When the `openscad`
binary is installed (and `--no-cad` is not passed) it additionally renders a
high-fidelity STL and, where a display/GL is available, a PNG. It writes:

| File | Role |
|---|---|
| `model.scad` | Parametric OpenSCAD program (CAD source of truth) |
| `model.stl` | Deterministic ASCII STL mesh (always produced) |
| `render.svg` | Orthographic front/side/top render (always produced) |
| `cutlist.csv` | Grouped parts: `part, quantity, length_mm, width_mm, thickness_mm, material` |
| `bom.csv` | Bill of materials: sheets, fasteners, glue (scaled by quantity) |
| `manifest.json` | Machine-readable summary + artifact index |
| `model.cad.stl` | High-fidelity STL from `openscad` (when available) |
| `render.png` | Raster render from `openscad` (when a display/GL is available) |

## Product brief schema

```json
{
  "name": "Studio Writing Desk",
  "product_type": "table",
  "dimensions": {
    "length_mm": 1400,
    "width_mm": 700,
    "height_mm": 740,
    "thickness_mm": 25
  },
  "material": "25mm oak-veneer plywood",
  "quantity": 1,
  "shelf_count": 3,
  "leg_section_mm": 60
}
```

| Field | Required | Default | Notes |
|---|---|---|---|
| `name` | yes | — | Non-empty; used for the STL solid name and the render title |
| `product_type` | yes | — | `panel` \| `box` \| `table` \| `shelf` |
| `dimensions.*_mm` | yes | — | Positive millimetres |
| `material` | no | `18mm plywood` | Free text |
| `quantity` | no | `1` | Scales the BOM totals |
| `shelf_count` | no | `3` | Interior shelves (`shelf` only) |
| `leg_section_mm` | no | `50` | Square leg cross-section (`table` only) |

Validation rejects non-positive dimensions, zero quantity, and (for carcass
products) a thickness that would make opposing walls overlap. A sample brief
lives at `examples/atelier/desk_brief.json`.

## Product families

- **panel** — a single flat sheet-good part.
- **box** — an open-top carcass (bottom + four walls).
- **table** — a rectangular top on four corner legs.
- **shelf** — a bookcase: two uprights, fixed top/bottom, and evenly-spaced
  interior shelves.

## End-to-end example

```bash
simard-atelier-build --brief examples/atelier/desk_brief.json --out /tmp/desk
ls /tmp/desk
# bom.csv  cutlist.csv  manifest.json  model.scad  model.stl  render.svg
```

The `tests/gadugi/atelier-identity-pipeline.sh` scenario exercises both
acceptance criteria — identity selectability and the brief → model + render
pipeline — as a hermetic outside-in test.
