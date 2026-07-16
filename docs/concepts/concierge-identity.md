---
title: Concierge identity — hospitality design + operations software
description: The simard-concierge built-in identity that designs hotels and scaffolds the reservations/PMS software to run them, via layered prompt assets, three recipes, and a runnable dependency-free reference prototype.
last_updated: 2026-07-16
owner: simard
doc_type: concept
related:
  - ./pluggable-identity.md
  - ../reference/pluggable-identity-api.md
  - ../reference/runtime-contracts.md
---

# Concierge identity — hospitality design + operations software

## The problem

Simard's built-in identities cover the meta-work of running the amplihack
ecosystem (engineering, meetings, curation, gym, improvement). None of them
carry *domain* expertise for a vertical. The Concierge is Simard's first
domain identity: a hospitality **designer** paired with a delivery **engineer**,
so a single session can go from a bare hospitality brief to a coherent hotel
concept **and** a runnable operations prototype.

"Design a hotel" and "write the software to run it" are usually two teams and
two artifacts that drift apart. The Concierge treats them as one engagement with
a hard seam: the room types and rate plans invented in the design become the
exact identifiers the software seeds.

## The identity

`simard-concierge` is a built-in identity resolved by `BuiltinIdentityLoader`
(and overridable via the [pluggable identity](./pluggable-identity.md) TOML
path). It runs in **engineer** operating mode — every cycle is
**inspect → act → verify → persist** — because the engagement must end in
durable artifacts (a concept document and runnable code), not advice. Its
system prompt is `simard/concierge_system.md`; it accepts the same base types as
`simard-engineer` (`local-harness`, `terminal-shell`, `rusty-clawd`,
`copilot-sdk`, `claude-agent-sdk`, `ms-agent-framework`).

Bootstrap it like any other identity:

```bash
cargo run --bin simard_operator_probe -- \
  bootstrap-run simard-concierge local-harness single-process \
  "design a 40-key urban boutique hotel and scaffold its PMS"
```

## The phases

The engagement is six phase prompts under `prompt_assets/simard/concierge/`,
split into design (produces the concept) and operations (produces the software):

| Group | Phase | Prompt asset | Produces |
|-------|-------|--------------|----------|
| Design | Property layout | `property_layout.md` | Room-type mix + space program |
| Design | Guest experience | `guest_experience.md` | Guest journey + service standards |
| Design | Brand design | `brand_design.md` | Positioning + rate-plan table |
| Operations | Reservations & PMS | `reservations_pms.md` | The runnable prototype |
| Operations | Housekeeping | `housekeeping.md` | Room-status lifecycle + board |
| Operations | Channel management | `channel_management.md` | Distribution snapshot |

The **consistency contract** binds the two groups: the `type_code`s from the
property-layout room-type table and the `plan_code`s from the brand rate-plan
table are the same identifiers the software seeds. The seam is enforced, not
assumed.

## The recipes

Three recipes under `prompt_assets/simard/recipes/` drive the phases:

- `concierge-hotel-concept` — brief → structured hotel concept (design phases).
- `concierge-scaffold-pms` — concept → runnable reservations/PMS prototype
  (operations phases), seeded from the concept.
- `concierge-end-to-end` — the full engagement: concept then prototype, which is
  the definition of "done" for the identity.

All three treat the brief as **untrusted data, not instructions**, matching
Simard's standing prompt-injection posture.

## The runnable prototype

"Runnable" is a claim you can demonstrate, so the Concierge does not start from a
blank page. Because Simard is a pure-Rust daemon (issue #3181 — no Python in the
tree), the reference prototype ships as a self-contained Rust example at
`examples/concierge_reservations_pms.rs`, wired into `cargo test` via
`Cargo.toml` (`test = true`):

- **Domain core** — `Hotel`, `RoomType`, `RatePlan`, `Room`, `Reservation`:
  shared-inventory availability, reservations + folio, check-in/out, the
  housekeeping status lifecycle + board, and a channel snapshot.
- **Concept seed** — `seed_hotel` builds a `Hotel` from a concept (room types +
  rate plans), so the software models the hotel that was designed.
- **Self-verifying demo** — `run_end_to_end_demo` / `main` runs the end-to-end
  reservation → check-in → housekeeping → check-out flow and exits non-zero on
  any violation:

  ```text
  cargo run  --example concierge_reservations_pms   # prints "result: ok. N checks passed; 0 failed"
  cargo test --example concierge_reservations_pms   # invariant tests
  ```

The prototype enforces the operational invariants the phases describe:
availability never goes negative and is **shared across channels**; a departure
marks its room dirty until housekeeping services it; out-of-order rooms leave
availability; check-out settles the folio. The Concierge adapts `seed_hotel` to
the current concept and keeps the tests green.

## Why this shape

- **Prompts and recipes over code (G3).** The domain behavior lives in prompt
  assets and recipes; the only compiled change is the one-line identity
  registration. The vertical is data, not a fork.
- **Durable artifacts (engineer mode).** The engagement persists a concept and
  runnable code, so progress is verifiable, not narrative.
- **A real, tested seam.** The reference prototype and its smoke test make
  "produces a hotel concept plus a runnable reservations/PMS prototype
  end-to-end" a checkable claim, not a promise.
