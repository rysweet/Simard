---
title: Overseer self-observation stability reference
description: >
  Reference for the three coupled fixes (#4128) that stop the Overseer's
  cognitive-memory loop from re-observing its own emitted observations and
  re-parking already-done goals: D1 emission-hygiene filtering of recall-derived
  `overseer-obs:*` problems at the write boundary, D2 the atomic pair of
  WHY-gate bare-blocked re-investigation (fresh CLOSED-issue / MERGED-PR
  evidence) plus a count-in-content occurrence upsert that keeps recurrence
  stable across cycles, and D3 the closing edge that keys workstream gaps
  per-signature and routes an idempotent LaunchRecipe / FileIssue terminal
  action through the shared `gate()`. Covers the data model, the recurrence
  counter contract, configuration, provenance-scoped recall aggregation, input
  sanitization, and the safety invariants.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./overseer-workstream-gap-scan.md
  - ./overseer-root-cause-why-api.md
  - ./overseer-memory-recall-api.md
  - ./no-progress-reinvestigation-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ./recipe-context-var-sanitization.md
  - ../design/overseer.md
  - ../concepts/ooda-reinvestigate-blocked-goals.md
  - ../../src/overseer/mod.rs
  - ../../src/overseer/launch.rs
  - ../../src/overseer/capabilities.rs
  - ../../src/ooda_loop/cycle.rs
---

# Overseer self-observation stability reference

> **Status: implemented.** The emission-hygiene filter and the occurrence
> upsert live in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> The WHY-gate bare-blocked re-investigation is wired in
> [`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs).
> The workstream-gap per-gap keying, the Decide-arm closing edge, and the shared
> `gate()` all live in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs);
> the recipe-launcher plumbing that edge routes into is in
> [`src/overseer/launch.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/launch.rs),
> and the shared input sanitizer (`sanitize_recalled`, `RecipeBrief`) is in
> [`src/overseer/capabilities.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/capabilities.rs).

The acting **Overseer** runs its own Observe → Orient → Decide → Act loop, writes
a one-line **observation episode** back into the shared cognitive-memory graph on
every tick that saw a problem, and on later ticks **recalls** those episodes to
raise a [`Signal::RecurringSignature`](./overseer-memory-recall-api.md) when the
same failure signature is seen `RECURRING_SIGNATURE_THRESHOLD` (= 2) or more
times. This reference specifies the fixes shipped in issue #4128 that make that
loop **stable** — it no longer amplifies its own emissions, and it no longer
leaves genuinely-completed work parked as "stuck."

For the base recall pipeline see the
[Overseer memory-recall API](./overseer-memory-recall-api.md); for the gap-scan
that produces `Signal::WorkstreamGap` see the
[workstream gap-scan reference](./overseer-workstream-gap-scan.md); for the
root-cause counter see the [root-cause (WHY) API](./overseer-root-cause-why-api.md).

## Contents

- [The problem this closes](#the-problem-this-closes)
- [D1 — Emission hygiene at the write boundary](#d1-emission-hygiene-at-the-write-boundary)
- [D2 — Bare-blocked re-investigation with a stable recurrence counter](#d2-bare-blocked-re-investigation-with-a-stable-recurrence-counter)
- [D3 — A closing edge for workstream gaps](#d3-a-closing-edge-for-workstream-gaps)
- [Input sanitization](#input-sanitization)
- [Configuration](#configuration)
- [Recurrence-counter contract](#recurrence-counter-contract)
- [Safety invariants](#safety-invariants)
- [What is unchanged](#what-is-unchanged)
- [Tests](#tests)

## The problem this closes

The canonical incident (issue #4128) surfaced as a
[`Signal::RecurringSignature`](./overseer-memory-recall-api.md) rendered:

```
recurring signature seen 2× in cognitive memory
(overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca|…)
```

Three distinct defects combined to produce it:

1. **Self-referential observation.** The Overseer's own write-back episodes carry
   a signature of the form `overseer-obs:<dedup_key>|<dedup_key>|…`. On a later
   tick, recall read those very episodes back in, so the Overseer *observed its
   own observation*. The `overseer-obs:` prefix inside the recalled signature is
   the fingerprint of this self-reference.
2. **Bare re-parking of done work.** Seven `kgpacks-rs` goals (including #17) were
   parked in a bare `[OODA-SAFEGUARD] … needs human review` block **even though**
   the underlying issues were CLOSED and PRs MERGED. The WHY-gate ("Gate A") only
   re-investigated a blocked goal when *this cycle* produced fresh
   `completion_evidence`, so a goal that was already parked with
   `completion_evidence == None` never got re-checked against the live GitHub
   state — it was re-parked forever.
3. **Never-closing gaps.** The [workstream gap-scan](./overseer-workstream-gap-scan.md)
   raised a single `workstream-gap` problem keyed on a **bare constant**, and its
   only act edge was a notify-only `NotifyOperator`. Every uncovered gap therefore
   re-collapsed onto one dedup key and never received a launch/issue that would
   actually *cover* it, so the same gap re-surfaced tick after tick.

The exact **2×** (rather than a runaway count) is expected, not a dedup bug: the
[`WhisperGate`](./overseer-memory-recall-api.md) dedup window is per-process, so a
daemon restart re-emits the observation exactly once against a static, unresolved
set. The fixes below remove the *cause* of the recurrence rather than suppressing
the honest re-observation.

The three fixes ship together and are labelled **D1**, **D2**, and **D3**.

## D1 — Emission hygiene at the write boundary

**Goal:** the Overseer must never re-observe its own emitted observations.

`observation_signature` builds the write-back signature by sorting and joining the
problem `dedup_key`s under the `overseer-obs:` prefix:

```
overseer-obs:<key_a>|<key_b>|…
```

D1 filters **recall-derived** problems whose origin is an `overseer-obs:*`
observation episode **out** of the problem set *before* it reaches
`write_back_observation`. The filter matches on the **structured origin/key** of
the recalled record — not a free-text substring of the rendered summary — so a
crafted signature that merely *contains* the literal `overseer-obs:` cannot evade
the filter, and (equally) cannot forge a self-observation to suppress a real
signal.

### Contract

- A `Signal::RecurringSignature` whose recalled `signature` begins with the
  reserved `overseer-obs:` origin prefix is **dropped** at the write boundary and
  never becomes a `Problem`, is never written back, and never counts toward the
  recurrence threshold.
- `RecurringSignature` signals derived from **non-overseer** episodes (genuine
  engineer/OODA failure signatures) are unaffected and flow through exactly as
  before.
- `write_back_observation` still writes at most one episode per qualifying tick,
  still returns `Ok(None)` on a clean tick or when the dedup slot is held, and
  still consumes the [`WhisperGate`](./overseer-memory-recall-api.md) slot only
  after a successful store.

The reserved origin prefix is a namespace, not a heuristic: the `overseer-obs:`
prefix is emitted **only** by `observation_signature`, so filtering on it is exact.

## D2 — Bare-blocked re-investigation with a stable recurrence counter

D2 is an **atomic pair**. Its two halves — D2a (open Gate A for bare-blocked
goals) and D2b (count-in-content occurrence upsert) — **must ship together**.
Opening the gate without the stable counter latches escalation forever; shipping
the counter without opening the gate leaves done goals parked. See
[Safety invariants](#safety-invariants).

### D2a — Gate A re-investigation of bare-blocked goals

Before D2a, the no-progress WHY reasoner (Gate A, in
[`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs))
only ran when the current cycle carried `completion_evidence`. D2a runs a
**bare-blocked re-investigation** whenever an evidence **source** is wired
(the production daemon always wires one), *independent* of whether this cycle
produced completion evidence.

Contract:

- For every goal parked in a bare `[OODA-SAFEGUARD] … needs human review` block
  with `completion_evidence == None`, the re-investigation **re-queries the live
  GitHub state through a fresh API call** (CLOSED issue / MERGED PR), reusing the
  [no-progress re-investigation](./no-progress-reinvestigation-api.md) reasoner
  seam and its `reinvestigated` dedupe set.
- A goal whose evidence now shows the work is **done** transitions **out** of
  `Blocked` (auto-complete / archive) instead of being re-parked.
- Re-investigation is **evidence-driven and deterministic**: it never
  auto-unblocks on recall alone. Stale or forged recall cannot clear a block; only
  fresh GitHub-API CLOSED/MERGED evidence can (**RK-2**).
- Each goal is re-investigated **at most once** per parked state via the persisted
  `reinvestigated` set — no re-query storm.

### D2b — Count-in-content occurrence upsert

`record_occurrence`
([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs))
persists a `StoredOccurrence` fact each time a root-cause is acted on. Before D2b
each occurrence was an independent fact, so the recurrence count was derived from
`recall.len()` and drifted every cycle. D2b makes the occurrence carry a
**bounded, saturating count in its serialized content** and **upserts** it, so the
recurrence value is stable across cycles.

`StoredOccurrence` (serialized as JSON into the fact content):

| Field         | Type     | Meaning                                                        |
| ------------- | -------- | ------------------------------------------------------------- |
| `signature`   | `string` | Root-cause signature (the occurrence key).                    |
| `cause_label` | `string` | Primary WHY label.                                            |
| `action`      | `string` | The remediation action taken.                                 |
| `outcome`     | `string` | `describe_outcome(...)` of the act result.                    |
| `count`       | `u32`    | Saturating occurrence count (never overflows / wraps).        |

Contract:

- On each occurrence the existing fact for `signature` is **read, incremented with
  a saturating add, and re-stored** (upsert) rather than appended — so
  `recall.len()` no longer stands in for the count.
- The count aggregates **only the Overseer's own provenance**: facts stored under
  caller-key `overseer:root-cause` **and** carrying the `overseer-root-cause` tag.
  A foreign memory writer cannot inflate the count into a forged escalation
  (**RK-1**, authz via provenance).
- The `RECURRENCE_ESCALATION_THRESHOLD` (= 3) escalation gate reads this stable
  count; it does **not** use `store_fact_with_caller_key` to overwrite (which would
  de-ratchet the count back to 1 and mask the signal — the trap explicitly avoided).
- Serialization stays `serde_json`; a serialize error is logged best-effort and the
  occurrence is skipped (recurrence tracking degrades, never panics).

### Why the pair is atomic

`RECURRING_SIGNATURE_THRESHOLD` (= 2) is unchanged. With **only** D2a, a bare
re-investigation that cannot yet clear the block re-emits the same signature every
cycle while `recall.len()` keeps climbing — the escalation latches on and never
releases. D2b's count-in-content upsert keeps the recurrence value stable so the
signal reflects the true number of *distinct* occurrences, not the number of times
the Overseer looked. Ship them together or not at all.

## D3 — A closing edge for workstream gaps

**Goal:** a flagged gap becomes **covered** and stops recurring.

Two changes, both in the Overseer act path:

1. **Per-gap Problem key.** The gap Problem is keyed **per signature** as
   `workstream-gap:<sig>` instead of the bare `workstream-gap` constant, so
   distinct gaps no longer collapse onto one dedup key and can be covered
   independently.
2. **Terminal act edge.** The Decide arm (in
   [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs))
   routes a `WorkstreamCoverage` problem to an **idempotent** `LaunchRecipe` /
   `FileIssue` terminal edge — **replacing** the prior notify-only
   `FlagWorkstreamGaps` routing that left the gap uncovered — through the shared
   `gate()` (also in `mod.rs`) so autonomy opt-in, the launch cap, the daily
   budget, a fail-closed steward-identity guard, and in-flight dedup all apply.
   The launch itself is executed by the `RecipeLauncher` plumbing in
   [`src/overseer/launch.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/launch.rs).
   The `FlagWorkstreamGaps` intervention and its notify handler still exist and
   are still admitted by the gate when constructed directly, but a coverage gap no
   longer routes to them by default.

Contract:

- One gap yields **at most one** covering launch or filed issue. The per-gap
  key + in-flight dedup guarantee idempotency: a re-observed gap already in flight
  produces **no** second launch and **no** duplicate issue (**RK-4**, no
  launch/issue spam).
- The new edge **never** bypasses `gate()`. It makes **no** direct `caps.*` call —
  every launch/issue is subject to the same blast-radius rails as any other
  cost-bearing intervention.
- When autonomy is disabled or the launch cap / budget is exhausted, the edge is
  **held** (rendered with its gate note) exactly like any other held intervention,
  and the gap is left for a later window rather than acted on.
- Without a distinct steward identity the coverage launch **fails closed**
  (an anti-recursion `OverseerError::Recursion`, isolated and counted by the tick,
  never launched) — the Overseer never acts on Simard's own backlog while
  unconfigured.
- Once a gap is covered by an in-flight workstream or open PR it enters the
  gap-scan `coverage` set and is **deduped away** (never re-flagged), closing the
  loop.

## Input sanitization

Every text that D3 can route into a **recipe-runner LLM prompt** or a **public
GitHub issue body** is passed through `sanitize_recalled`
([`src/overseer/capabilities.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/capabilities.rs)),
extending the existing recalled-text scrubbing to two new sinks:

- `RecipeBrief.task_description` built for a gap-covering `LaunchRecipe`.
- Issue briefs built from the raw `problem.summary` for a gap-covering `FileIssue`.

This blocks prompt injection from the multi-writer memory graph (**RK-3**) and
scrubs secrets from text before it reaches the LLM prompt or a public issue body
(**RK-7**). Durable structs stay minimal and `serde_json`-encoded; error text and
summaries are secret-scrubbed before they are persisted to the shared graph.

## Configuration

No **new** environment variables are introduced. The feature is governed by the
existing Overseer switches:

| Variable                     | Governs                                                                                         | Default   |
| ---------------------------- | ---------------------------------------------------------------------------------------------- | --------- |
| `SIMARD_OVERSEER_GAP_SCAN`   | The gap-scan and its D3 closing edge. When disabled, no gap is flagged, launched, or filed.     | enabled   |
| `SIMARD_OVERSEER_WHISPER`    | Whisper delivery; a disabled whisperer holds whispers (unchanged).                              | enabled   |
| `SIMARD_OVERSEER_GOAL_HEALTH`| Goal-board health, which feeds bare-blocked goals into the D2a re-investigation.                | enabled   |

The D2a re-investigation additionally honours the
[no-progress re-investigation](./no-progress-reinvestigation-api.md) enable rail
(`no_progress_investigation_enabled()`), which the production daemon sets.

Node heap for the recipe-runner subprocess is governed by the saved preference
`NODE_OPTIONS=--max-old-space-size=32768` (edit `~/.amplihack/config` to change).

### Thresholds (compile-time, unchanged)

| Constant                          | Value | Meaning                                                                     |
| --------------------------------- | ----- | --------------------------------------------------------------------------- |
| `RECURRING_SIGNATURE_THRESHOLD`   | `2`   | Min recalled episodes sharing a signature before `RecurringSignature` fires.|
| `RECURRENCE_ESCALATION_THRESHOLD` | `3`   | Stable occurrence count at/above which a recurring re-park escalates.        |

## Recurrence-counter contract

After D1 + D2, the recurrence counter obeys these guarantees:

1. **No self-amplification.** Overseer `overseer-obs:*` observations never
   contribute to their own recurrence count (D1).
2. **Stable across cycles.** The count reflects distinct *acted* occurrences via
   the count-in-content upsert, not the number of recall passes (D2b).
3. **Provenance-scoped.** Only `overseer:root-cause` caller-key +
   `overseer-root-cause` tag facts are aggregated; foreign writers are rejected.
4. **Bounded.** The count uses a saturating add and cannot overflow or wrap.
5. **Threshold-honest.** `RECURRING_SIGNATURE_THRESHOLD` stays at 2; a genuine
   cross-window re-observation of a still-unresolved signature still fires once,
   but it no longer self-amplifies.

## Safety invariants

| ID    | Invariant                                                                                                   |
| ----- | ----------------------------------------------------------------------------------------------------------- |
| RK-1  | Recurrence count aggregates **only** the Overseer's own caller-key + tag facts (authz via provenance).       |
| RK-2  | Bare-blocked auto-unblock requires **fresh GitHub-API** CLOSED/MERGED evidence — never recall alone.         |
| RK-3  | Gap launch/issue text is `sanitize_recalled`-cleaned before reaching any LLM prompt or public issue body.    |
| RK-4  | One gap → at most one launch/issue; per-gap key + in-flight dedup make the D3 edge idempotent.               |
| RK-5  | The Overseer never re-observes its own emissions (D1 origin filter).                                         |
| RK-7  | Secrets are scrubbed from error text/summaries before persistence and before issue bodies.                   |
| D2-AT | D2a and D2b are **atomic**: opening Gate A without the stable count latches escalation — ship them together. |
| GATE  | The D3 terminal edge routes through `gate()`; it makes no direct `caps.*` call (blast-radius rails apply).    |
| RK-8  | The coverage launch **fails closed** (`OverseerError::Recursion`) without a distinct steward identity — the Overseer never acts on its own backlog while unconfigured. |

The typed-OODA `SCHEMA_VERSION` stays at **1**: no API or DB surface is added.

## What is unchanged

- `RECURRING_SIGNATURE_THRESHOLD` (2) and `RECURRENCE_ESCALATION_THRESHOLD` (3).
- The [`WhisperGate`](./overseer-memory-recall-api.md) window/cap and its
  per-process, peek-then-commit semantics (a daemon restart honestly re-emits a
  still-unresolved observation once).
- The gap-scan survey inputs (goal board, open issues, live anomalies) and the
  `coverage` dedup set semantics.
- The [no-progress breaker](./no-progress-breaker-api.md) ladder and its
  `NoProgressTracker` persistence.
- The kgpacks-rs int8-PQ-embed algorithm and the other enumerated blocked goals
  (identity personas, test-coverage-to-70%, coin benchmark) — **out of scope**;
  this feature fixes the *safeguard that mis-read done as stuck*, not that work.

## Tests

| Test file                                              | Covers                                                                                     |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| `src/overseer/tests_root_cause.rs`                     | D1 focal-signature regression anchor (self-observation is no longer re-emitted); D2b upsert.|
| `src/ooda_loop/tests_no_progress_reinvestigation.rs`   | D2a bare-blocked re-query runs; recurrence stays stable; no escalation latch.               |
| `src/overseer/tests_gap_scan.rs`                       | D3 per-gap key; gap covered exactly once; idempotent; no launch/issue spam.                 |

Validate each change with its targeted test first, then run the full `overseer`
and `ooda_loop` suites (`cargo test`) to confirm no regression across the coupled
paths.
