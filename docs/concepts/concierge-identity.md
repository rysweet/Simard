# Concierge Identity — Hospitality Design + Operations Software

The **Concierge** is a pluggable Simard identity (`simard-concierge`) for the
hospitality domain. It does two jobs, in order:

1. **Design the hotel** — from a brief, produce a concrete hotel concept:
   property layout, guest-experience journey, and brand identity.
2. **Scaffold the software that runs it** — stand up a runnable reservations /
   PMS prototype: room inventory, a booking lifecycle, housekeeping, and channel
   management.

It is *done* when it can produce **a hotel concept plus a runnable
reservations/PMS prototype end-to-end** — a guest booked, checked in, checked
out, the room serviced, and availability restored, with invariants verified.

## Where it lives

| Surface | Location |
|---|---|
| Identity | `simard-concierge` in `src/identity/loader.rs` (mode: `orchestrator`) |
| Runnable domain module | `src/concierge/` (`design`, `pms`, orchestrator) |
| System prompt | `prompt_assets/simard/concierge_system.md` |
| Design / scaffold prompts | `prompt_assets/simard/concierge_property_design.md`, `concierge_software_scaffold.md` |
| Recipes | `prompt_assets/simard/recipes/concierge-{hotel-design,software-scaffold,end-to-end}.yaml` |
| Operator probe | `simard_operator_probe concierge-run <topology> "<brief>"` |

## The runnable prototype (`simard::concierge`)

The `concierge` module is the source of truth for what "runnable" means. It is
deterministic and dependency-light, so the same brief always yields the same
concept and the prototype can be exercised in CI without any model call.

- `concierge::design_hotel(&brief) -> HotelConcept` — property layout (floors,
  room mix summing exactly to the room count, public spaces), a staged
  guest-experience journey, and a brand identity.
- `concierge::PmsEngine::from_concept(&concept)` — seeds room types and numbered
  rooms, then supports:
  - **Reservations**: `book`, `check_in`, `check_out`, `cancel` with correct
    inventory holds and rejected illegal transitions.
  - **Housekeeping**: `run_housekeeping()` services dirty rooms; out-of-order
    rooms are never sellable.
  - **Channel management**: `channel_availability(date)` publishes inventory to
    `direct`, `booking.com`, and `expedia`.
- `concierge::run_concierge(&brief) -> ConciergeOutcome` — designs, scaffolds,
  drives a booking lifecycle, and **verifies** operational invariants (a booking
  reduces availability by one; after check-out and housekeeping availability is
  fully restored; the reservation reaches checked-out).

### Verified invariants

1. Generated room count equals the sum of the designed `room_mix` counts.
2. A booking reduces published availability by exactly one for the booked night.
3. After check-out **and** housekeeping, availability is fully restored.
4. A dirty room is not sellable until serviced.

## Security posture

The brief is treated as **untrusted data**. `HotelBrief::from_prompt` extracts
only design signals (name, location, room count, positioning, theme) and never
obeys instructions embedded in the text (e.g. "ignore the rules above"). This is
covered by tests in `src/concierge/design.rs` and `tests/concierge_end_to_end.rs`.

## Try it

```bash
# End-to-end via the runnable example
cargo run --example concierge_end_to_end
cargo run --example concierge_end_to_end -- "Aurora Lodge in Reykjavik, 90-room luxury spa resort"

# End-to-end via the operator probe (prints the concept + a verified booking)
cargo run --bin simard_operator_probe -- \
  concierge-run single-process "Harbor Light in Lisbon, a 120-room boutique waterfront hotel"

# Confirm the identity bootstraps as a first-class identity
cargo run --bin simard_operator_probe -- \
  bootstrap-run simard-concierge local-harness single-process "verify concierge bootstrap"
```

A passing `concierge-run` ends with a `RES-…` sample reservation,
`Prototype verified: yes`, and `Session phase: complete`.

## Tests

- Unit: `src/concierge/{design,pms}.rs` and `src/concierge/mod.rs` (`#[cfg(test)]`).
- Integration: `tests/concierge_end_to_end.rs`.
- Outside-in scenarios: `tests/gadugi/concierge-identity.{sh,yaml}` and
  `tests/qa-scenarios/concierge-end-to-end.yaml`.
