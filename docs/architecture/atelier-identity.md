---
title: Atelier Identity — Industrial & Furniture Design
description: The pluggable simard-atelier identity that designs furniture and physical products, driving Blender/FreeCAD/OpenSCAD from a product brief to an exported model, render, and fabrication package.
last_updated: 2026-07-15
owner: simard
doc_type: concept
related:
  - ../concepts/pluggable-identity.md
  - ./agent-composition.md
  - ../reference/runtime-contracts.md
---

# Atelier Identity — Industrial & Furniture Design

**Atelier** (`simard-atelier`) is a pluggable Simard identity that designs
furniture and physical products. It takes a written product brief and drives
parametric CAD tooling to a concrete, fabrication-ready result: a 3D model, a
render, and an export package (STEP/STL geometry, a cut list, and a bill of
materials).

Atelier is selectable alongside the other built-in identities
(`simard-engineer`, `simard-meeting`, `simard-gym`, `simard-goal-curator`,
`simard-improvement-curator`). It runs in a dedicated operating mode, `atelier`.

## Identity card

| Field | Value |
| --- | --- |
| Name | `simard-atelier` |
| Operating mode | `atelier` |
| Discipline | Industrial design & furniture / physical-product design |
| Base types | `local-harness`, `terminal-shell`, `rusty-clawd`, `copilot-sdk`, `claude-agent-sdk`, `ms-agent-framework` |
| Required capabilities | `prompt-assets`, `session-lifecycle`, `memory`, `evidence`, `reflection` |
| System prompt | `prompt_assets/simard/atelier_system.md` |
| CAD tooling | Blender (`bpy`), FreeCAD (`FreeCADCmd`), OpenSCAD (`openscad`) |

A shell-capable base type (`terminal-shell`) is required so the identity can
invoke the CAD tools from its session.

## Selecting Atelier

Atelier is registered as a **built-in** identity in
[`BuiltinIdentityLoader`](../reference/runtime-contracts.md#identity-and-backend-contract),
so it is selectable by name (`simard-atelier`) wherever an identity is chosen —
the same path as the engineer, meeting, gym, and curator identities.

It is **also** available as a portable [pluggable identity](../concepts/pluggable-identity.md)
card: the file `examples/atelier/identity.toml` demonstrates the file-based
form that `FileIdentityLoader` reads, so the identity can be dropped into a repo
without recompiling.

## Capabilities

1. **Brief interpretation** — extract object type, key dimensions, materials,
   joinery/assembly constraints, and ergonomic/load requirements.
2. **Parametric modeling** — express the design as named parameters so variants
   are one edit away. OpenSCAD or FreeCAD's Python API for parametric solids;
   Blender `bpy` for organic geometry, materials, lighting, and rendering.
3. **Fabrication engineering** — derive a cut list (parts, quantities, stock
   dimensions, grain) and a BOM (materials, hardware, fasteners, finishes).
4. **Export** — `STEP` (solid, for CNC/CAM and downstream CAD) and `STL` (mesh,
   for 3D-print/preview), plus a rendered image.
5. **Verification** — confirm every artifact exists, is non-empty, opens/parses,
   and matches the brief's dimensions and part counts before declaring done.

## Operating loop — inspect → act → verify → persist

Atelier follows the same operating contract as the engineer identity:

- **Inspect** — read the brief; restate object, parameters, materials, success
  criteria; pick the right tool per subtask.
- **Act** — write the parametric model source, run the CAD tool to generate
  geometry, derive the cut list + BOM, and render an image.
- **Verify** — model source is parametric; STEP/STL exist, non-empty, and open;
  render exists; cut list + BOM cover every part and agree with the model;
  dimensions and part counts match the brief.
- **Persist** — record the artifacts, the parameters, and the verification
  evidence so the exact command set reproduces the design.

## Goal-session recipes

Two goal-session recipes decompose a brief into the standard pipeline:

- [`atelier-parametric-model.yaml`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/recipes/atelier-parametric-model.yaml)
  — brief → parametric model source → STEP/STL geometry + render.
- [`atelier-fabrication-export.yaml`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/recipes/atelier-fabrication-export.yaml)
  — model → cut list + BOM + verified fabrication export package.

## Definition of done

A brief is complete when, end to end, the identity has produced and **verified**:

- parametric model source (named parameters);
- `model.step` (solid) and `model.stl` (mesh), both non-empty and openable;
- at least one render (`render.png`), non-empty;
- a cut list covering every part, consistent with the model;
- a BOM covering materials + hardware + finishes;
- an evidence note stating the dimension/part-count checks passed and the exact
  commands that regenerate every artifact.

A model alone is never sufficient: the render and the fabrication package
(STEP/STL + cut list + BOM), verified against the brief, are part of the
deliverable.
