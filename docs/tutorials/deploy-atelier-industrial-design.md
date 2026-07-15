---
title: "Tutorial: Deploy Atelier, an industrial & furniture design identity"
description: End-to-end walkthrough of the Atelier identity — a pluggable Simard persona that takes a furniture / physical-product brief and drives Blender (bpy), FreeCAD, and OpenSCAD headlessly to produce a parametric 3D model, a render, and fabrication-ready exports (STEP/STL) with a cut list and a bill of materials.
last_updated: 2026-07-15
owner: simard
doc_type: tutorial
related:
  - ../howto/configure-pluggable-identity.md
  - ../concepts/pluggable-identity.md
  - ../concepts/identity-scoped-cognition.md
  - ../reference/pluggable-identity-api.md
---

# Tutorial: Deploy Atelier, an industrial & furniture design identity

By the end of this tutorial you will be able to select **Atelier**, a Simard
identity that designs furniture and physical products, and carry a **product
brief** all the way to an **exported, fabrication-ready model and a render** —
end to end.

Atelier is a maker's studio in software. It reasons about form, structure,
materials, joinery, and manufacturability, then produces the concrete geometry
and documents a workshop needs: a **parametric 3D model**, a **render**, and
**fabrication-ready exports** (`STEP`/`STL`) with a **cut list** and a **bill of
materials (BOM)**. It runs in Simard's **engineer** operating mode and obeys the
same inspect → act → verify → persist loop as every engineer identity — an
object it cannot open, measure, and export is not done.

The behavior is almost entirely **configuration**: an identity card, a system
prompt, and two goal-session recipes. The geometry and rendering are produced by
driving three headless tools.

## Prerequisites

- Simard binary built (`cargo build --quiet`).
- At least one of the modeling tools installed and runnable headlessly:
  - **OpenSCAD** — parametric solids via CLI (`openscad -o out.stl -D 'w=600;' model.scad`).
  - **FreeCAD** — parametric solids, assemblies, and **STEP** export via `freecadcmd`.
  - **Blender** — assembly, materials, and **renders** via `blender -b -P script.py`.

    Atelier detects which tools are present (`command -v openscad freecadcmd blender`)
    and falls back per its tool matrix, recording any substitution.

## Step 1 — Select the Atelier identity

`simard-atelier` is a **built-in** identity, so it is selectable with no extra
configuration. Point `SIMARD_IDENTITY` at it:

```bash
export SIMARD_IDENTITY=simard-atelier
```

Selecting it loads Atelier's system prompt
(`prompt_assets/simard/atelier_system.md`) — the identity card and capabilities
— and puts the session in engineer mode.

### Identity card

| Field | Value |
|---|---|
| Identity name | `simard-atelier` |
| Operating mode | `engineer` (inspect → act → verify → persist) |
| Domain | Industrial design, furniture, physical product design |
| Primary tools | Blender (`bpy`), FreeCAD, OpenSCAD |
| Deliverables | 3D models, renders, exports (STEP/STL), cut lists, BOMs |
| Done means | A product brief has been carried to an exported model **and** a render |

## Step 2 (optional) — Ship Atelier as a pluggable identity

If you would rather ship Atelier as a **pluggable** identity (per repository,
without recompiling Simard), declare it in an `identity.toml` instead of using
the built-in. This mirrors the built-in definition:

```toml
[package]
name = "atelier-identity"
version = "0.1.0"
description = "Industrial & furniture design identity"

[[identities]]
name = "simard-atelier"
default_mode = "engineer"
supported_base_types = ["local-harness", "terminal-shell", "rusty-clawd"]
required_capabilities = ["prompt-assets", "session-lifecycle", "memory", "evidence", "reflection"]

[[identities.prompt_assets]]
id = "atelier-system"
path = "atelier_system.md"

[identities.memory_policy]
allow_project_writes = false
summary_scope = "session-summary"
```

Place `atelier_system.md` alongside `identity.toml` and point Simard at the
directory with `SIMARD_IDENTITY_PATH`. See
[How to configure pluggable identities](../howto/configure-pluggable-identity.md)
for the full schema, path-security rules, and fallback behavior.

## Step 3 — Give Atelier a product brief

A **brief** is a short description of an object: its purpose, overall
dimensions, materials, and constraints. For example:

> A low, three-legged oak side table. Round top 450 mm diameter, 22 mm thick.
> Overall height 500 mm. Tapered legs, 40 mm square at the top. For 3D-print
> prototyping first, then CNC in solid oak.

Atelier runs two goal-session recipes in order:

1. **`prompt_assets/simard/recipes/atelier-parametric-modeling.yaml`** — brief →
   parametric script → 3D model → render. It parses the brief into a parameter
   table (width, depth, height, thickness, clearances), authors a parametric
   script (OpenSCAD/FreeCAD) as the canonical geometry, then a Blender script for
   materials and a render.
2. **`prompt_assets/simard/recipes/atelier-fabrication-export.yaml`** — model →
   verified `STEP`/`STL` exports → cut list + BOM. It regenerates geometry from
   the script, exports, and **verifies** every export before declaring it done.

## Step 4 — Verify the deliverables (inspect → act → verify → persist)

Atelier does not hand-wave a partial result. Before it reports success it:

- Confirms each export file exists and is non-empty.
- Loads each export back and asserts its **bounding box** matches the brief's
  overall dimensions within tolerance.
- For `STL`, asserts the mesh is **watertight / manifold**.
- Confirms the render PNG exists and the cut list and BOM are present and
  internally consistent.

All artifacts are persisted to a predictable directory, `out/<slug>/`:

```
out/oak-side-table/
├── README.md          # brief, resolved parameters, tool substitutions, verification
├── table.scad         # (or table.py) — parametric script, source of truth
├── model.step         # solid interchange export (FreeCAD)
├── model.stl          # watertight mesh export (OpenSCAD/Blender/FreeCAD)
├── render.png         # render
├── cut_list.csv       # part, quantity, material, finished dimensions, notes
└── bom.csv            # parts, fasteners, hardware, finish — with quantities
```

## Definition of done

- The brief's key dimensions are driven by **named parameters** in a script that
  regenerates the model deterministically.
- A **render** (`render.png`) exists and shows the object.
- At least one **fabrication export** (`model.step` and/or `model.stl`) exists,
  loads without error, and matches the brief's dimensions within tolerance (STL
  additionally watertight/manifold).
- A **cut list** and a **BOM** exist and are consistent with the model.
- Everything is persisted under `out/<slug>/` with a `README.md` recording the
  brief, resolved parameters, tool substitutions, and verification results.

If a tool needed for a deliverable is unavailable and no fallback can produce it,
Atelier **says so explicitly** and persists everything it could produce — it
never silently claims a missing export succeeded.

## See also

- [How to configure pluggable identities](../howto/configure-pluggable-identity.md)
- [Pluggable identity](../concepts/pluggable-identity.md)
- [Identity-scoped cognition](../concepts/identity-scoped-cognition.md)
- [Deploy Crocutus (read-only observer)](deploy-crocutus-read-only-observer.md) — another shipped identity, config-not-code
