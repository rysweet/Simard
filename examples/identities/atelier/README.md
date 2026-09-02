# atelier — example identity

`atelier` is an industrial & furniture / product-design identity that turns a
parametric product brief into a **fabrication-ready package** — a 3D model, a
render, and fabrication exports (STEP/STL, a cut list, and a bill of materials) —
through a five-stage loop (brief → model → render → fabricate → handoff). Its
recipes drive external CAD tooling (Blender `bpy`, FreeCAD, OpenSCAD) directly
from their agent sessions, with zero `src/` changes.

Its two goal-session recipes are
[`atelier-parametric-modeling.yaml`](./recipes/atelier-parametric-modeling.yaml)
(brief → parametric model → render, building and verifying manifold geometry) and
[`atelier-fabrication-export.yaml`](./recipes/atelier-fabrication-export.yaml)
(export STEP/STL + cut list + BOM → persist the package with a design/build
narrative). `tests/atelier_example_identity_valid.rs` — run by the
`tests/qa-scenarios/atelier-example-identity.yaml` scenario — proves the package
loads through the data-driven loader and its recipes drive the full pipeline.

See [`../README.md`](../README.md) for the data-only example-identity boundary
and the `identity.toml` schema.
