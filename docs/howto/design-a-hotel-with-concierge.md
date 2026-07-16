# Design a Hotel with the Concierge Identity

**Concierge** is a pluggable Simard identity for **hospitality design and
operations software**. It does two things together: it **designs hotels**
(property layout, guest experience, brand) and it **scaffolds the software to run
them** (reservations, PMS front desk, housekeeping, and channel management).

Concierge is one identity among several that Simard can load. It runs in the
[engineer operating mode](../index.md) and follows the same
`inspect → act → verify → persist` discipline as the rest of Simard, applied to
the hospitality domain.

## The deterministic backbone

Concierge ships a deterministic, no-LLM backbone exposed through
`simard concierge`. Every command below runs offline and is fully reproducible,
which is what makes the end-to-end acceptance path testable in CI.

| Command | What it does |
|---|---|
| `simard concierge concept` | Design a hotel concept (layout, experience, brand) from a brief. |
| `simard concierge scaffold --out <dir>` | Write `concept.md` plus a runnable reservations/PMS prototype seed. |
| `simard concierge run <dir>` | Execute a scaffolded prototype end-to-end. |
| `simard concierge demo` | One-shot: design → scaffold → run, proving the whole path. |

### Design a concept

Run with the built-in demo brief, or pass your own:

```sh
simard concierge concept \
  --name "The Highline" --location "Downtown" \
  --rooms 40 --theme "industrial loft" --positioning luxury
```

The concept always contains three sections — **Property Layout** (floors, a room
mix whose counts sum to the requested room count, and tier-appropriate public
spaces), **Guest Experience** (the arrival-to-departure journey and signature
moments), and **Brand Design** (name rationale, palette, voice, tagline). Add
`--out <dir>` to also write `concept.md`, or `--json` for machine-readable
output.

### Scaffold and run the reservations/PMS prototype

```sh
simard concierge scaffold --demo --out ./my-hotel
simard concierge run ./my-hotel
```

`scaffold` writes `concept.md`, `prototype.json` (a clean PMS engine plus a
deterministic seed of booking requests), and a `README.md`. `run` books the seed
reservations, checks guests in, checks half of them out, runs a housekeeping
cycle, and pushes availability to the distribution channels — printing an
operations trace and a summary. The four operational services are all exercised:

- **Reservations** — book, hold, and cancel stays.
- **PMS front desk** — assign rooms and check guests in/out.
- **Housekeeping** — track room status and advance dirty rooms back to sellable.
- **Channel management** — push sellable availability per category.

### Prove the whole path end-to-end

```sh
simard concierge demo
```

This designs a hotel concept **and** runs its reservations/PMS prototype
end-to-end in one command — the Concierge acceptance bar.

## The agentic recipes

The deterministic backbone is the ground truth. Three agentic recipes under
`prompt_assets/simard/recipes/` compose on top of it to add market-specific
narrative and service standards — they must never contradict the backbone's
structural facts:

- `concierge-hotel-concept.yaml` — design and enrich a hotel concept.
- `concierge-pms-scaffold.yaml` — scaffold **and verify** a runnable prototype.
- `concierge-end-to-end.yaml` — deliver both the concept and a verified prototype.

The identity's system prompt lives at
`prompt_assets/simard/concierge_system.md`, with per-surface prompts for
property layout, guest experience/brand, and PMS scaffolding alongside it.

## Positioning tiers

The `--positioning` flag (`select` | `upscale` | `luxury`) drives the suite
fraction, amenity density, and brand voice. Luxury properties add a destination
bar, a full-service spa, and event space; select properties stay lean with a
grab-and-go market.
