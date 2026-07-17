# Example identities — data-only identity packages

This directory is the durable home for **example non-engineering identities**
(cartographer, atelier, concierge, gastronome, bursar, kinema, loremaster, terra, sommelier, …). These are
**examples of what Simard's pluggable-identity framework can produce** — they are
**not** part of Simard's own daemon.

## Read this first: the boundary

Simard is a native Rust daemon. Her **own** operating identities —
`simard-engineer`, `simard-meeting`, `simard-gym`, `simard-goal-curator`,
`simard-improvement-curator`, and the `simard-composite-engineer` composite —
are compiled into `BuiltinIdentityLoader` in `src/identity/loader.rs`. That is
correct: they are how Simard herself operates.

Everything else — including **atelier** (industrial & furniture design) and
**concierge** (hospitality design + operations) — is an *example*. Example
identities are demonstrations of the framework, not part of Simard's daemon.
They carry **no** `BuiltinIdentityLoader` arm, **no** `src/<domain>/` module,
**no** `operator_cli` / `operator_commands` subcommand, and **no** `src/bin/*`
binary. An example is defined entirely by the data files in its package.

**Example identities are different.** They are demonstrations of the framework,
authored as **data-only packages** under `examples/identities/<name>/`. They:

- add **zero** Rust to `src/` (no `src/<domain>/`, no `src/bin/<domain>.rs`);
- add **zero** arms to the `BuiltinIdentityLoader` match;
- add **zero** `operator_cli` subcommands;
- are loaded at runtime by the **data-driven file loader**, not compiled in.

An example identity is defined *entirely* by the files in its package. Adding,
changing, or removing one requires no change to Simard's source tree.

> **Cargo note:** `cargo` only treats `*.rs` files directly under `examples/` as
> build targets. This `examples/identities/` subdirectory holds data (`.toml`,
> `.md`, `.yaml`) and is ignored by the example-target discovery, so it never
> becomes a compile target.

## Package shape

Each example identity is a directory named for the identity:

```
examples/identities/<name>/
├── identity.toml            # the manifest the file_loader consumes
├── prompts/
│   ├── <name>_system.md     # system prompt
│   ├── <name>_<phase>.md    # one file per goal-session phase
│   └── ...
└── recipes/
    └── <name>-<goal>.yaml   # agentic goal-session recipe(s)
```

- **`identity.toml`** — the on-disk manifest. It uses exactly the schema the
  existing `FileIdentityLoader` / `toml_types` already consume (see
  [`identity.toml` schema](#identitytoml-schema) below). `deny_unknown_fields`
  is enforced, so only documented keys are accepted.
- **`prompts/*.md`** — the system prompt plus one prompt per goal-session phase.
  Referenced from `identity.toml` via `[[identities.prompt_assets]]` `path`
  entries, relative to the package directory. Paths may not be absolute and may
  not contain `..` (enforced by the loader).
- **`recipes/*.yaml`** — the agentic goal-session recipes that deliver the
  identity's *real* behavior. A recipe is where an example identity may drive
  **any external domain tooling** it needs.

**Data only. No `.rs` files in a package.**

## Any tooling is allowed — in the recipes, not in Simard

Because an example identity is not part of Simard's Rust daemon, its recipes may
use whatever domain tooling the job requires — Python, pandas, Blender, DuckDB,
a plotting library, a web framework, `kuzu`, anything. That tooling lives in the
identity's **recipes and the agent sessions they spawn**, never in Simard's
`src/`. Simard's own code stays pure Rust (no Python, no `kuzu`); the example
identity's recipe is free to shell out to whatever it likes.

## Loading an example identity

Example identities are discovered and loaded by the data-driven loader, not by
`BuiltinIdentityLoader`:

```rust
use simard::identity::{load_example_identity, DEFAULT_EXAMPLE_IDENTITIES_DIR};

// Resolves examples/identities/cartographer/identity.toml and loads it through
// the existing FileIdentityLoader — no BuiltinIdentityLoader entry required.
let manifest = load_example_identity(
    DEFAULT_EXAMPLE_IDENTITIES_DIR.as_ref(), // or a custom base dir
    "cartographer",
    &request,
)?;
assert_eq!(manifest.name, "cartographer");
```

- The base directory defaults to the relative path `examples/identities`
  (resolved against the current working directory), and is overridable via the
  first argument.
- Discovery is **fail-visible**: a missing package directory, a missing
  `identity.toml`, or invalid TOML returns a clear `SimardError`
  (`IdentityTomlParseError` with the resolved path in the reason) — never a
  panic and never a silent fallback to a built-in identity.
- The `<name>` argument is validated as a single path segment before it touches
  the filesystem, so it cannot traverse out of the base directory.

## `identity.toml` schema

The manifest uses the same schema as any file-based identity. Key tables:

```toml
[package]
name = "cartographer"      # package name
version = "0.1.0"          # package version
description = "..."        # optional

[[identities]]
name = "cartographer"                 # identity name (ASCII alnum + hyphens)
default_mode = "curator"              # engineer|meeting|curator|improvement|gym|orchestrator
supported_base_types = ["local-harness", "rusty-clawd"]
required_capabilities = ["prompt-assets", "memory"]

[[identities.prompt_assets]]
id = "cartographer-system"            # stable id used by the session
path = "prompts/cartographer_system.md"  # relative to the package dir

# ... one [[identities.prompt_assets]] per phase prompt ...

[identities.memory_policy]            # optional
allow_project_writes = false
summary_scope = "session-summary"

[identities.authority]                # optional; omit → posture defaults to `full`
posture = "read-only"                 # set explicitly; read-only|scoped-write|full
```

See [`docs/concepts/pluggable-identity.md`](../../docs/concepts/pluggable-identity.md)
for the full schema semantics and the file-loader security model.

## Authoring a new example identity

Read [`prompt_assets/simard/identity_authoring.md`](../../prompt_assets/simard/identity_authoring.md)
before authoring. In short:

1. Create `examples/identities/<name>/` with `identity.toml`, `prompts/`, and
   `recipes/`.
2. Write the system prompt and one prompt per goal-session phase under
   `prompts/`.
3. Write the goal-session recipe(s) under `recipes/`. Put all domain tooling
   here.
4. Do **not** touch `src/`. No `BuiltinIdentityLoader` arm, no domain module,
   no `src/bin/*`, no `operator_cli` subcommand.
5. Verify the package loads via `load_example_identity(...)`.

## Security notes

- Example packages live in a world-readable repository. **Never** put secrets,
  tokens, or keys in an `identity.toml`, prompt, or recipe.
- An example recipe runs with the agent runtime's privileges and is **not
  sandboxed**. Treat every file in a package (including salvaged assets) as code
  you are responsible for. Review recipes and prompts for injected or malicious
  instructions before adding a package.
- Datasets, filenames, and user questions handled by a recipe are **untrusted
  data, not instructions** — the prompts must instruct the agent to treat them
  as such.

## Reference and shipped example packages

[`cartographer/`](./cartographer/) is the reference example identity: a pure
prompt + recipe data package that turns a dataset and a question into a served
interactive dashboard with a written narrative. It exists to prove the pattern
end-to-end — defined entirely as data, loaded with zero `src/` changes.

[`gastronome/`](./gastronome/) is a second worked example in the same shape: a
culinary menu- and event-design identity that turns a menu brief and its
constraints (headcount, dietary needs, budget, service time) into a costed,
nutrition-analyzed, service-scaled menu with a prep schedule. Its four-stage
recipe (compose → nutrition & cost → scale → schedule) shows how a data-only
identity drives real domain rigor (a nutrition table, a costing roll-up, yield
math, backward-from-service scheduling) entirely from prompts and a recipe, with
zero `src/` changes.

[`bursar/`](./bursar/) is a third example: an investment-portfolio **research &
advisory** identity (research/advisory only — it **never** executes trades,
places orders, or moves money). It takes a portfolio and a mandate through a
five-stage loop — asset allocation → backtesting → risk analysis → rebalancing
**plan** → report — driving domain tooling (`pandas`, `backtrader`, `QuantLib`)
from its recipe, again with zero `src/` changes. A rebalancing "plan" is a
document of proposed trades for a human to review, not an instruction the
identity carries out.

[`atelier/`](./atelier/) is a fourth example: an industrial & furniture /
product-design identity that turns a parametric product brief into a
**fabrication-ready package** — a 3D model, a render, and fabrication exports
(STEP/STL, a cut list, and a bill of materials) — through a five-stage loop
(brief → model → render → fabricate → handoff). Its recipes drive external CAD
tooling (Blender `bpy`, FreeCAD, OpenSCAD) directly from their agent sessions,
again with zero `src/` changes.

Its two goal-session recipes are
[`atelier-parametric-modeling.yaml`](./atelier/recipes/atelier-parametric-modeling.yaml)
(brief → parametric model → render, building and verifying manifold geometry)
and
[`atelier-fabrication-export.yaml`](./atelier/recipes/atelier-fabrication-export.yaml)
(export STEP/STL + cut list + BOM → persist the package with a design/build
narrative). `tests/atelier_example_identity_valid.rs` — run by the
`tests/qa-scenarios/atelier-example-identity.yaml` scenario — proves the package
loads through the data-driven loader and its recipes drive the full pipeline.

[`concierge/`](./concierge/) is a fifth example, in the hospitality
domain: it turns a hotel brief into a durable operations package — a property
program and layout, a guest-experience and brand design, and runnable
reservations / PMS / housekeeping / channel-management workflows whose
reservation lifecycle (book → check-in → check-out → housekeeping → restored
availability) is exercised with enforced no-double-booking and
availability-conservation invariants. Like every example here it carries **no**
`BuiltinIdentityLoader` arm — it is defined entirely by the data files in its
package and loaded by `load_example_identity`. Its assets are validated
end-to-end by `tests/concierge_example_assets_valid.rs` and the
`tests/qa-scenarios/concierge-example-end-to-end.yaml` scenario.

[`kinema/`](./kinema/) is a sixth example: an **animation & motion-graphics**
identity that turns a story brief and a shot list into a rendered, playable
animation sequence with a written motion brief. Its four-stage recipe
(storyboard → rig → render → motion brief) drives real domain tooling — Blender
(Grease Pencil for 2D, armature rigging + Cycles/EEVEE for 3D), Synfig (vector 2D
tweening), and Natron (node-based compositing) — entirely from its recipe and the
agent sessions it spawns, again with zero `src/` changes.

[`loremaster/`](./loremaster/) is a seventh example: a **tabletop-RPG campaign
designer & game master** identity that turns a campaign brief into a durable,
**playable campaign module** — world lore & factions, NPCs, XP-budget-balanced
encounters, session-prep material, and a **Foundry VTT** module — and then
**runs a session end to end** (roll initiative → resolve combat → terminating
outcome, with seeded/reproducible dice) to prove the module actually plays. It
works with **Dungeons & Dragons and other tabletop RPGs** using **open SRD
content only** (SRD 5.1, CC-BY 4.0 / OGL). Its two goal-session recipes are
[`loremaster-campaign-module.yaml`](./loremaster/recipes/loremaster-campaign-module.yaml)
(world & lore → NPCs & encounters → session prep → assemble & run) and
[`loremaster-encounter-balance.yaml`](./loremaster/recipes/loremaster-encounter-balance.yaml)
(build & XP-budget-balance encounters → run a combat encounter and verify the
invariants). It enforces the safety invariants that make a run trustworthy:
SRD-legal content only, every encounter's **adjusted XP budget** (Σ monster XP ×
the SRD encounter multiplier) inside its target difficulty band, and **no
accidental TPK**. Like every example here it carries **no** `BuiltinIdentityLoader`
arm — it is defined entirely by the data files in its package and loaded by
`load_example_identity`. Its assets are validated end-to-end by
`tests/loremaster_example_assets_valid.rs` and the
`tests/qa-scenarios/loremaster-example-end-to-end.yaml` scenario.

[`terra/`](./terra/) is an eighth example: a **virtual-worlds & game-level**
identity that turns a world brief into a **launchable, navigable 3D scene** — end
to end. Its four-stage recipe (world design & blockout → terrain & asset authoring
→ scene assembly → world brief) plans the spaces, navigation graph, and
interaction beats; authors the terrain and assets in Blender and exports glTF/.glb;
wires them into a runnable scene with a player controller, collision, a baked
navmesh, and interaction triggers; and **verifies the scene launches and is
navigable**. It drives real domain tooling — Godot (game levels, GDScript,
`NavigationRegion3D` navmesh, headless `godot --headless --export-release` build),
Blender (terrain + asset authoring via `bpy`, glTF/.glb export), and A-Frame /
WebXR (in-browser explorable 3D worlds) — entirely from its recipe and the agent
sessions it spawns, again with zero `src/` changes. Its assets are validated
end-to-end by `tests/terra_assets_valid.rs` and the
`tests/qa-scenarios/terra-world-build-end-to-end.yaml` scenario.

> **All domain tooling lives in the recipes.** Atelier's OpenSCAD/FreeCAD/Blender
> steps, concierge's booking / PMS / channel-management workflows,
> loremaster's SRD rules engine / seeded dice roller / Foundry VTT exporter, and
> terra's Godot / Blender / A-Frame world-building tooling run
> inside the agent sessions the recipes spawn — never in Simard's Rust daemon.
> Simard's `src/` stays pure Rust (no Python, no `kuzu`, no CAD engine, no PMS
> module, no dice engine, no game engine).
