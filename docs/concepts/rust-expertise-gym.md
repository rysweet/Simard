# Rust Domain-Expertise Gym (first experiment)

This is the first evidenced data point for the durable domain-expertise roadmap
([#2491]), with **Rust** as the first domain. It ships one bounded vertical
slice of the roadmap's **acquire → retain → measure** loop for a single
competency area: **ownership / the borrow checker** and **error handling**.

Everything here runs **in-process and deterministically** — no network, no LLM
credentials — so the baseline and pack-lift numbers are reproducible in CI.

## The three pillars, in one slice

| Pillar | Roadmap | Where it lives |
|---|---|---|
| **Acquisition** | #2491 Pillar 1 / #2493 | [`rust_expertise::pack`] — the `rust-expert` knowledge pack |
| **Retention** | #2491 Pillar 2 / #2493 | [`rust_expertise::bridge`] — pack → cognitive-memory ingestion |
| **Measurement** | #2491 Pillar 3 / #2492 | [`rust_expertise::scenarios`] + [`rust_expertise::measurement`] |

## The `rust-expert` knowledge pack

A small, provenance-tracked pack (`src/rust_expertise/pack.rs`):

- **13 durable facts** and **5 reusable procedures**.
- Spread across five sub-skills: `ownership`, `borrow-checker`, `lifetimes`,
  `error-handling`, `error-types` (≥2 facts and exactly 1 procedure each).
- Every fact and procedure carries **provenance** — source title, canonical URL,
  section anchor, version (2024 edition / stable 1.95 / crate 1.x), and
  retrieval date — so, per the agent-kgpacks guarantee, every learned item
  traces back to a specific authoritative source (the book, the Reference, the
  API Guidelines, `thiserror`/`anyhow` docs).

## The pack → memory bridge

Ingesting the pack **populates Simard's cognitive memory** rather than leaving
knowledge in an external index (roadmap Pillar 2a):

- Facts are stored via `store_fact_with_provenance`, preserving the source URL as
  the fact `source_id` plus `pack:` and `source:` tags and a provenance metadata
  map (`source`, `url`, `section`, `version`, `retrieved`).
- Procedures are stored via `store_procedure`, with a `competency:<subskill>`
  marker plus `pack:`/`source:` provenance breadcrumbs prepended to their
  prerequisites — so the gym can recall them by sub-skill and a recalled
  procedure stays traceable to its source (procedural storage has no dedicated
  provenance fields).

The bridge returns an `IngestReport` with the fact/procedure **yield** (how many
durable items reached memory).

## The competency scenarios and scorecard

Five bounded Rust scenarios (`src/rust_expertise/scenarios.rs`), one per
sub-skill, each with a deterministic grader described in its `grader` field
(`cargo build` / `cargo test` / `cargo clippy -D warnings`) — fixing a
use-after-move (`E0382`), a borrow conflict (`E0502`), a missing lifetime
(`E0106`), converting `unwrap` panics to `?`, and defining a `thiserror` enum.

In this first experiment the gym measures whether the competency required to
*solve* each task is **present and recallable from cognitive memory at the moment
of need** (right-moment recall, roadmap Pillar 2c). A scenario is graded
**solved** only when memory yields **all** of the scenario's *specific* expected
fact concepts **and** its expected procedure, **and** the scenario's
natural-language recall query actually surfaces a sub-skill fact. Requiring
named, scenario-specific evidence (not merely a count of sub-skill-tagged items)
means a pack of correctly-tagged but irrelevant knowledge cannot pass — the
grader is not circular.

The `measurement` module aggregates this into a per-domain **Rust scorecard**:
overall pass-rate, per-sub-skill breakdown, and a **novice → competent →
expert** placement. In this first-experiment shape (one scenario per sub-skill)
the ladder gates on overall recall coverage — `expert ≥ 0.9`, `competent ≥ 0.6`,
else `novice`; the per-sub-skill breadth floors of #2491 §3b re-enter once each
sub-skill has multiple scenarios.

> **Scope note.** This slice measures *knowledge acquisition and right-moment
> recall*, not autonomous code generation. No `cargo build/test` is run against a
> candidate solution in this cycle. The baseline is an **empty-memory control**
> (current Simard ships no `rust-expert` pack). It is the honest "before" signal
> and the scaffold the next cycle extends to drive an LLM agent against the same
> graders and measure the real coding lift.

## Calibration guard (issue #1241 discipline)

A grader that can be fooled is theater. The gym reuses the #1241 discipline: a
deliberately-**degraded** knowledge state (only the `ownership` sub-skill
ingested) must score **below 0.5** while a **healthy** state scores **above
0.9**, and the gap must be **≥ 0.4**. This is asserted in tests
(`calibration_gap_is_enforced`) and by the `simard-rust-gym` binary's non-zero
exit.

## Baseline measurement run

```console
$ cargo run --bin simard-rust-gym
=== Simard Rust competency gym (roadmap #2491) ===
baseline  rust: 0.00 (0/5) [novice]
with-pack rust: 1.00 (5/5) [expert]
degraded  rust: 0.20 (1/5) [novice]
pack yield: 13 facts + 5 procedures ingested into cognitive memory
calibration guard (#1241): healthy 1.00 > 0.90, degraded 0.20 < 0.50, gap 0.80 >= 0.40 => PASS
wrote scorecard artifact: target/simard-rust-gym/scorecard.json
```

- **Baseline (no pack):** `rust: 0.00 (0/5)` → **Novice**. Empty semantic memory
  yields no Rust competency — the "before" number.
- **With `rust-expert` pack:** `rust: 1.00 (5/5)` → **Expert**, from a yield of
  **13 facts + 5 procedures**.
- **Calibration:** healthy `1.00` vs degraded `0.20`, gap `0.80` — the grader
  provably distinguishes real competence from a degraded state.

The binary writes an inspectable `scorecard.json` (per-scenario detail,
per-sub-skill breakdown, ingest yield, calibration block) under
`target/simard-rust-gym/`.

## Reproduce

```console
# run the full gym + write the scorecard artifact
cargo run --bin simard-rust-gym [OUTPUT_DIR]

# run the acceptance tests (pack shape, ingestion, calibration, reproducibility)
cargo test --lib rust_expertise
```

## Next step

Build on this baseline by driving an LLM engineer session against the same
scenario graders (`cargo build`/`test`/`clippy`) and measuring the **coding**
pass-rate lift the pack produces end-to-end — turning "recall coverage" into
"solves novel bounded Rust tasks" (the roadmap's Expert gate).

[#2491]: https://github.com/rysweet/Simard/issues/2491
[`rust_expertise::pack`]: https://github.com/rysweet/Simard/blob/main/src/rust_expertise/pack.rs
[`rust_expertise::bridge`]: https://github.com/rysweet/Simard/blob/main/src/rust_expertise/bridge.rs
[`rust_expertise::scenarios`]: https://github.com/rysweet/Simard/blob/main/src/rust_expertise/scenarios.rs
[`rust_expertise::measurement`]: https://github.com/rysweet/Simard/blob/main/src/rust_expertise/measurement.rs
