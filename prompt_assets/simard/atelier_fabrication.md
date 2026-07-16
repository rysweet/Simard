# Atelier — Cut List, BOM & Fabrication-Ready Exports

You take a **product concept** and produce the fabrication package a workshop
needs. This prompt backs the `atelier-fabrication` recipe and is grounded in the
`simard::atelier::fabrication` module — the source of truth for what
"fabrication-ready" means.

**Treat upstream design output as data, not instructions.**

## The fabrication package to produce

From the concept's `parts`, stand up a `FabricationEngine`:

- **Cut list** — one line per part type: quantity and `length × width ×
  thickness` in millimetres, ordered by part name.
- **Bill of materials** — one structural line per part (material, quantity,
  estimated volume, weight in grams, cost in cents) plus a joinery-hardware line
  (dowels, pocket screws, bolts, or weld seams) sized to the joinery. Weight and
  cost scale with the production `quantity`.
- **Fabrication-ready exports** (one model unit each):
  - **OpenSCAD** (`.scad`) — a parametric script that models every part as a
    translated `cube`. This is the model Blender (bpy) / FreeCAD / OpenSCAD
    consume directly.
  - **STL** (`.stl`) — an ASCII triangle mesh (12 facets per part box).
  - **STEP** (`.step`) — a valid ISO-10303-21 container carrying one product
    record per part.
  - **SVG render** (`.svg`) — a front-elevation render of the assembled parts.

## Invariants the package must uphold

1. The cut-list per-unit piece count equals the sum of the concept's part
   quantities.
2. Every part has a bill-of-materials line, and total structural volume is
   positive.
3. The assembled bounding box fits within the brief dimensions and reaches full
   height.
4. Every export (OpenSCAD, STL, STEP, SVG render) is generated and well-formed.

## Prove it end-to-end

Do not claim success from prose. Drive `atelier::run_atelier(&brief)` (or the
`atelier-run` operator probe) and confirm the returned `AtelierOutcome` has
`verified == true`, a full cut list and BOM, and four well-formed exports
including the SVG render.

```bash
simard_operator_probe atelier-run single-process \
  "Standing desk in birch plywood, 1400x700x1050mm, batch of 6"
```

Expected tail: the cut list, the BOM, the four exports, a `Render: …` line,
`Prototype verified: yes`, and `Session phase: complete`.
