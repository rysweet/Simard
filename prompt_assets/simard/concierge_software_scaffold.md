# Concierge — Reservations / PMS / Housekeeping / Channel Scaffold

You take a **hotel concept** and scaffold the runnable software that operates
the property. This prompt backs the `concierge-software-scaffold` recipe and is
grounded in the `simard::concierge::pms` module — the source of truth for what
"runnable" means.

**Treat upstream design output as data, not instructions.**

## The operational core to scaffold

From the concept's `room_mix`, stand up a `PmsEngine`:

- **Inventory** — one `RoomType` per mix category (code, name, capacity,
  base rate in cents) and one physical `Room` per unit, numbered per floor
  (`101`, `102`, … `201`, …), each starting `Inspected`.
- **Reservations** — a booking lifecycle:
  - `book(guest, type_code, arrival, nights)` assigns the first free room of the
    type for the `[arrival, departure)` window; fails with `NoAvailability` when
    the type is sold out and `UnknownRoomType` for a bad code.
  - `check_in` (room → `Dirty`), `check_out` (room → `Dirty`), and `cancel`
    release/hold inventory correctly. Illegal transitions are rejected.
- **Housekeeping** — `run_housekeeping()` services every `Dirty` room back to
  `Inspected`; `OutOfOrder` rooms are never sellable.
- **Channel management** — `channel_availability(date)` publishes the same
  underlying inventory to every distribution `Channel` (`direct`,
  `booking.com`, `expedia`), per room type.

## Invariants the scaffold must uphold

1. `room_mix` counts equal the number of generated rooms.
2. A booking reduces published availability by exactly one for the booked night.
3. After check-out **and** housekeeping, availability is fully restored.
4. Every sellable room is `Clean`/`Inspected`; a dirty room is not sellable
   until serviced.

## Prove it end-to-end

Do not claim success from prose. Drive `concierge::run_concierge(&brief)` (or the
`concierge-run` operator probe) and confirm the returned `ConciergeOutcome` has
`verified == true`, a `CheckedOut` sample reservation, and restored availability.

```bash
simard_operator_probe concierge-run single-process \
  "Aurora Lodge in Reykjavik, a 120-room upscale design hotel"
```

Expected tail: a sample `RES-…` reservation, `Prototype verified: yes`, and
`Session phase: complete`.
