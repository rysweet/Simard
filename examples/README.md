# Examples — Simard's example identity packages

`examples/` is the durable home for **example identity packages**: data-only
demonstrations of Simard's pluggable-identity framework. Everything here is a
package under [`examples/identities/`](./identities/) — no Rust, no build
targets, no part of the daemon. Behavioral **verification** for the daemon does
not live here; it lives in [`tests/`](../tests/) as outside-in integration tests
that exercise the public `simard` crate. If you are looking for harness code,
look under `tests/`, not under `examples/`.

## What is an identity

An identity is a **self-describing capability persona** in Simard's
pluggable-identity framework — a manifest plus prompts and recipes that tell the
runtime how to behave for a class of work.

Simard's **own** operating identities (`simard-engineer`, `simard-meeting`,
`simard-gym`, `simard-goal-curator`, `simard-improvement-curator`, and the
`simard-composite-engineer` composite) are compiled into the
`BuiltinIdentityLoader` in [`src/identity/loader.rs`](../src/identity/loader.rs).
That is how Simard herself operates.

**Example identities are different.** Each is a **data-only package**
(`identity.toml` + `prompts/` + `recipes/`) under
`examples/identities/<name>/`, loaded at runtime by the data-driven file loader.
An example identity adds **zero** Rust to `src/`: no `BuiltinIdentityLoader`
arm, no domain module, no `operator_cli` subcommand, no `src/bin/*` binary. It
exists to demonstrate that the framework can produce non-engineering personas
entirely from data.

For the full framework — package shape, the `identity.toml` schema, the loader
security model, and how to load or author a package — see
[`examples/identities/README.md`](./identities/README.md). This page does not
duplicate that content.

## Available example identities

The table below is **derived from each package's `identity.toml` `description`**,
so it cannot drift from the packages themselves. Each row links to the package's
own README; the authoritative, generated list of shipped packages lives in
[`examples/identities/README.md`](./identities/README.md#reference-and-shipped-example-packages).

| Package | Identity (derived from `identity.toml` description) |
| --- | --- |
| [atelier](./identities/atelier/README.md) | Parametric product brief → 3D model + render + fabrication package (STEP/STL, cut list, BOM) |
| [bursar](./identities/bursar/README.md) | Portfolio + mandate → allocation, backtest, risk, rebalancing plan + report (advisory only, never order execution) |
| [cartographer](./identities/cartographer/README.md) | Dataset + question → served dashboard + narrative |
| [concierge](./identities/concierge/README.md) | Hospitality brief → property layout + guest-experience/brand design + reservations/PMS/housekeeping/channel-management workflows |
| [gastronome](./identities/gastronome/README.md) | Menu brief + constraints → costed, nutrition-analyzed, scaled menu with a prep schedule |
| [kinema](./identities/kinema/README.md) | Story brief + shot list → rendered animation sequence + motion brief |
| [loremaster](./identities/loremaster/README.md) | Campaign brief → world lore + NPCs + XP-balanced encounters + session prep + a Foundry VTT module, then run a session end-to-end (open SRD content) |
| [maestro](./identities/maestro/README.md) | Musical brief → engraved score (LilyPond/MuseScore) + rendered audio track (MIDI + open-source synths) |
| [terra](./identities/terra/README.md) | World brief → launchable, navigable 3D scene (Godot / Blender / A-Frame WebXR) |
| [vitruvia](./identities/vitruvia/README.md) | Program/site brief → code-aware floor plan + interiors + plans/elevations + rendered walkthrough (Blender+BlenderBIM/IFC, FreeCAD) |

## Using an example identity

Example identities are loaded at runtime by the data-driven file loader, not by
`BuiltinIdentityLoader`. For the loading API, the base-directory resolution, the
`identity.toml` schema, and the security model, follow
[`examples/identities/README.md`](./identities/README.md#loading-an-example-identity).
Each package's own README describes what that identity does and which tooling
its recipes drive.

## Where the tests are

The daemon's outside-in behavioral verification lives under
[`tests/`](../tests/) and runs on every build via
`cargo test --all-features --locked --no-fail-fast` (Cargo auto-discovers every
`tests/*.rs` file, so each is a gate). These are black-box `#[test]` functions
that drive only the public `simard` crate — no private internals, no
`#[cfg(test)]`-only helpers. Fact recall and ranked cognitive-memory recall, for
instance, are verified by `tests/semantic_fact_recall_outside_in.rs` and
`tests/cognitive_memory_ranked_recall_outside_in.rs`. No verification harness
code belongs under `examples/`.
