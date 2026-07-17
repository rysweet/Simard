# Concierge — example identity for hospitality design + operations

The **Concierge** is an **example** non-engineering identity that demonstrates
what Simard's pluggable-identity framework can produce in the hospitality domain.
It is a **data-only package** at
[`examples/identities/concierge/`](../../examples/identities/concierge/) — a
manifest, prompts, and an agentic recipe. It is **not** part of Simard's daemon:
there is no `src/concierge/` module, no `BuiltinIdentityLoader` arm, and no
operator subcommand or probe. All of its behavior is delivered by the agent
session its recipe spawns.

> **Boundary.** Concierge is loaded at runtime by the data-driven
> `load_example_identity` rail, exactly like the reference
> [`cartographer`](../../examples/identities/cartographer/) package. See
> [Pluggable identity](./pluggable-identity.md) and the
> [example-identities README](../../examples/identities/README.md) for the
> compiled-in vs. data-only distinction.

## What it does

Concierge does two jobs, in order:

1. **Design the hotel** — from an operator brief, produce a concrete hotel
   concept: property layout (floors, room mix, public spaces), the
   guest-experience journey (discovery → arrival → stay → departure → post-stay),
   and a brand identity (name, tagline, positioning, voice, palette).
2. **Scaffold the software that runs it** — stand up a runnable reservations /
   PMS prototype that operationalizes the concept: a room inventory derived from
   the room mix, a booking lifecycle (book → check-in → check-out / cancel),
   housekeeping status per room, and a channel manager that publishes
   availability to distribution channels.

A goal session is *done* when it has produced **a hotel concept plus a runnable
reservations/PMS prototype end-to-end** — a guest can be booked, checked in,
checked out, the room serviced, and availability restored, with the operational
invariants verified against the running scaffold the agent built.

## Where it lives

| Surface | Location |
|---|---|
| Package | [`examples/identities/concierge/`](../../examples/identities/concierge/) |
| Manifest | `examples/identities/concierge/identity.toml` (identity name `concierge`, mode `curator`) |
| System prompt | `examples/identities/concierge/prompts/concierge_system.md` |
| Phase prompts | `examples/identities/concierge/prompts/concierge_intake.md`, `concierge_experience.md`, `concierge_operations.md`, `concierge_deliver.md` |
| Recipe | `examples/identities/concierge/recipes/concierge-hospitality-package.yaml`, `concierge-reservation-lifecycle.yaml` |
| Loader | `simard::identity::load_example_identity(base, "concierge", &request)` |

There is **no** `src/concierge/`, **no** `simard concierge` subcommand, and
**no** `simard_operator_probe concierge-run` probe. The reservations engine,
hotel-concept model, and any booking software the identity produces are built by
the agent inside the recipe's session, using whatever domain tooling the job
needs — never compiled into Simard's Rust daemon.

## How the behavior is delivered

The `concierge-hospitality-package.yaml` recipe drives the package as four
**agentic** steps (`type: "agent"`) that mirror the standalone stage prompts:

1. **Intake & property program** (`concierge_intake.md`) — turn the brief into a
   structured property program: floors, a room mix that sums to the requested
   room count, and public spaces.
2. **Guest experience & brand** (`concierge_experience.md`) — design the staged
   guest journey and a brand identity (name, positioning, voice, palette).
3. **Operations workflows** (`concierge_operations.md`) — specify the
   reservations / PMS / housekeeping / channel-management workflows.
4. **Assemble & verify** (`concierge_deliver.md`) — scaffold and run the
   prototype from the spec, then drive and **verify** a booking lifecycle
   end-to-end.

The focused `concierge-reservation-lifecycle.yaml` recipe re-runs the
operations + delivery stages alone to prove the operational core
(book → confirm → check-in → check-out → housekeeping → restored availability).

Because the scaffold is produced by the agent (not a Simard module), the recipe
is free to generate it in whatever stack fits — a small web service, a SQLite
schema, an in-memory engine — and to shell out to the tools that build and
exercise it. Simard's `src/` stays pure Rust.

### Verified invariants

The booking step must demonstrate, against the scaffold it built, that:

1. The generated room count equals the sum of the designed room-mix counts.
2. A booking reduces published availability by exactly one for the booked night.
3. After check-out **and** housekeeping, availability is fully restored.
4. A dirty (out-of-order) room is not sellable until serviced.

The session records the concept, a sample reservation, and the verification
result as durable artifacts — never as a throwaway point-in-time report.

## Security posture

The brief is **untrusted data**, not instructions. The prompts require the agent
to extract only design signals (name, location, room count, positioning, theme)
and to ignore any embedded commands (e.g. "ignore the rules above",
"delete everything"), falling back to safe defaults for anything missing. The
recipe follows the same containment rules as every example package: prompt-asset
paths are contained within the package, and no secrets, tokens, or PII appear in
the manifest, prompts, or recipe.

## Selecting and loading

Concierge is loaded by name through the data-driven rail, with no builtin arm:

```rust
use simard::identity::{load_example_identity, DEFAULT_EXAMPLE_IDENTITIES_DIR};

let manifest = load_example_identity(
    DEFAULT_EXAMPLE_IDENTITIES_DIR.as_ref(), // examples/identities, relative to cwd
    "concierge",
    &request,
)?;
assert_eq!(manifest.name, "concierge");
assert_eq!(manifest.default_mode, OperatingMode::Curator);
```

A missing package directory or invalid `identity.toml` returns a fail-visible
`IdentityTomlParseError` — never a panic and never a silent fallback to a
built-in identity.

## Tests

- Example-loader unit test in `src/identity/example_loader.rs` proves
  `examples/identities/concierge/identity.toml` parses and loads into an
  `IdentityManifest` with name `concierge`, mode `curator`, and **no**
  `BuiltinIdentityLoader` entry for that name.
- The reference cartographer example test stays green alongside it.
