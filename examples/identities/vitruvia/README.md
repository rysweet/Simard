# vitruvia — example identity

`vitruvia` is an **architecture & interior design** identity (named for
Vitruvius) that turns a program/site brief into a designed building — a code-aware
BIM floor plan, an interior layout, technical drawings (plans and elevations),
and a rendered walkthrough. Its two goal-session recipes are
[`vitruvia-massing-plan.yaml`](./recipes/vitruvia-massing-plan.yaml)
(program → massing → code-aware IFC floor plan, verifying space areas, egress, and
accessibility) and
[`vitruvia-drawings-walkthrough.yaml`](./recipes/vitruvia-drawings-walkthrough.yaml)
(interiors → plans/elevations → rendered walkthrough → persist the package with a
design narrative). Its recipes drive real BIM/CAD tooling — Blender + the
BlenderBIM / IfcOpenShell IFC toolkit and FreeCAD's Arch/BIM + TechDraw
workbenches — directly from their agent sessions, with zero `src/` changes.
`tests/vitruvia_example_identity_valid.rs` — run by the
`tests/qa-scenarios/vitruvia-example-identity.yaml` scenario — proves the package
loads through the data-driven loader and its recipes drive the full pipeline.

See [`../README.md`](../README.md) for the data-only example-identity boundary
and the `identity.toml` schema.
