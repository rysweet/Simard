# Example identities — data-only identity packages

This directory is the durable home for **example non-engineering identities** —
demonstrations of what Simard's pluggable-identity framework can produce. They
are **not** part of Simard's own daemon. Each example is a self-describing,
data-only package under `examples/identities/<name>/`; the index of shipped
examples below is **derived from those package directories**, so adding one is a
pure data change (see [Reference and shipped example packages](#reference-and-shipped-example-packages)).

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

Each example identity is a **self-describing package**: its descriptive blurb
lives in its own `examples/identities/<name>/README.md`, not in this shared file.
[`cartographer/`](./cartographer/README.md) is the **reference package** — copy
its shape when authoring a new example.

The index below is **generated**: it is derived from the package directories
under `examples/identities/`, in alphabetical order, with one linked entry per
package. **Do not hand-edit the block between the markers** — adding, renaming, or
removing an example is a pure data change (create or delete its package
directory), and the index updates itself. The
`tests/example_identities_index_valid.rs` staleness gate asserts the committed
block is byte-for-byte what `render_identity_index` derives; regenerate it with:

```bash
UPDATE_EXPECT=1 cargo test --test example_identities_index_valid
```

<!-- BEGIN GENERATED IDENTITY INDEX -->
- [atelier](./atelier/README.md)
- [bursar](./bursar/README.md)
- [cartographer](./cartographer/README.md)
- [concierge](./concierge/README.md)
- [gastronome](./gastronome/README.md)
- [kinema](./kinema/README.md)
- [loremaster](./loremaster/README.md)
- [maestro](./maestro/README.md)
- [terra](./terra/README.md)
- [vitruvia](./vitruvia/README.md)
<!-- END GENERATED IDENTITY INDEX -->

> **All domain tooling lives in the recipes.** Every example's domain tooling —
> CAD engines, booking / PMS / channel-management workflows, rules engines and
> seeded dice rollers, game engines, notation and audio toolchains — runs inside
> the agent sessions the recipes spawn, never in Simard's Rust daemon. Simard's
> `src/` stays pure Rust (no Python, no `kuzu`, no CAD engine, no PMS module, no
> dice engine, no game engine). Each package's own README describes what that
> identity does and which tooling its recipes drive.
