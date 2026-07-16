# Atelier Identity — Furniture / Industrial Design + Fabrication

The **Atelier** is a pluggable Simard identity (`simard-atelier`) for the
furniture and physical-product domain. It does two jobs, in order:

1. **Design the product** — from a brief, produce a concrete, buildable concept:
   a parametric part model, material and joinery selection, and an aesthetic
   (style, palette, finish).
2. **Fabricate it** — turn the concept into a cut list, a bill of materials, and
   fabrication-ready exports: an OpenSCAD parametric model, an STL mesh, a STEP
   (ISO-10303-21) container, and an SVG elevation render.

It is *done* when it can take a product brief **to an exported model plus a
render, end-to-end** — with the cut list, BOM, and every export verified against
the concept.

## Where it lives

| Surface | Location |
|---|---|
| Identity | `simard-atelier` in `src/identity/loader.rs` (mode: `orchestrator`) |
| Runnable domain module | `src/atelier/` (`design`, `fabrication`, orchestrator) |
| System prompt | `prompt_assets/simard/atelier_system.md` |
| Design / fabrication prompts | `prompt_assets/simard/atelier_product_design.md`, `atelier_fabrication.md` |
| Recipes | `prompt_assets/simard/recipes/atelier-{product-design,fabrication,end-to-end}.yaml` |
| Operator probe | `simard_operator_probe atelier-run <topology> "<brief>"` |

## The runnable prototype (`simard::atelier`)

The `atelier` module is the source of truth for what "runnable" means. It is
deterministic and dependency-light, so the same brief always yields the same
concept and the same exports, and the prototype can be exercised in CI without
any external CAD binary.

- `atelier::design_product(&brief) -> ProductConcept` — a parametric part model
  (each part a dimensioned box with placements that fit within the declared
  bounding box), a material + joinery selection, and an aesthetic/finish.
- `atelier::FabricationEngine::from_concept(&concept)` — derives:
  - **Cut list**: one line per part type (quantity, `L × W × T` in mm).
  - **Bill of materials**: one structural line per part (volume, weight, cost)
    plus a joinery-hardware line; weight and cost scale with the run quantity.
  - **Exports**: `openscad_source`, `stl_source`, `step_source`, and
    `svg_render` — the OpenSCAD script is the parametric model that Blender
    (bpy), FreeCAD, and OpenSCAD consume to render or convert to STEP/STL.
- `atelier::run_atelier(&brief) -> AtelierOutcome` — designs, fabricates, and
  **verifies** the package before returning it.

### Verified invariants

1. The cut-list per-unit piece count equals the sum of the concept's part
   quantities.
2. Every part has a bill-of-materials line, and total structural volume is
   positive.
3. The assembled bounding box fits within the brief dimensions and reaches full
   height.
4. Every export (OpenSCAD, STL, STEP, SVG render) is generated and well-formed.

## Security posture

The brief is treated as **untrusted data**. `ProductBrief::from_prompt` extracts
only design signals (name, category, material, dimensions, quantity) and never
obeys instructions embedded in the text (e.g. "ignore the rules above"). This is
covered by tests in `src/atelier/design.rs` and `tests/atelier_end_to_end.rs`.

## Try it

```bash
# End-to-end via the runnable example
cargo run --example atelier_end_to_end
cargo run --example atelier_end_to_end -- "Standing desk in birch plywood, 1400x700x1050mm, batch of 6"

# End-to-end via the operator probe (prints the concept + a verified package)
cargo run --bin simard_operator_probe -- \
  atelier-run single-process "Larch dining table in solid oak, 1800x900x740mm"

# Confirm the identity bootstraps as a first-class identity
cargo run --bin simard_operator_probe -- \
  bootstrap-run simard-atelier local-harness single-process "verify atelier bootstrap"
```

A passing `atelier-run` ends with the cut list, BOM, four exports, a `Render: …`
line, `Prototype verified: yes`, and `Session phase: complete`.

## Tests

- Unit: `src/atelier/{design,fabrication}.rs` and `src/atelier/mod.rs` (`#[cfg(test)]`).
- Integration: `tests/atelier_end_to_end.rs`.
- Outside-in scenarios: `tests/gadugi/atelier-identity.{sh,yaml}` and
  `tests/qa-scenarios/atelier-end-to-end.yaml`.
