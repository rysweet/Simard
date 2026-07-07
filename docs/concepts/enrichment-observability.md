---
title: "Concept: enrichment observability (is recalled memory reaching decisions?)"
description: Why recalling memory is not the same as USING it — the observability gap where each OODA turn computes a top-10 facts / top-5 procedures recall, renders it into the prompt preamble, and dispatches the decision, yet whether the cognitive-memory bridge actually ATTACHED or silently DEGRADED to None was never logged or metered. The fail-loud instrumentation at the enrich_turn_input / EnrichmentSource::resolve seam, the live dashboard attach-rate surface, and the recall-on-vs-recall-off ablation eval that turns "recalled memory influences decisions" into a reproducible yes/no (#2942), feeding the hybrid self-measurement (#2644).
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ../reference/enrichment-observability-api.md
  - ../howto/verify-recall-reaches-decisions.md
  - ./hybrid-cognition-measurement.md
  - ../reference/recall-precision-hybrid-api.md
  - ../reference/telemetry-metrics.md
  - ../reference/base-type-adapters.md
  - ../reference/cognitive-memory-fact-recall.md
  - ../reference/cognitive-memory-ranked-recall.md
  - ../../src/base_type_turn.rs
  - ../../src/enrichment_observability/mod.rs
---

# Concept: enrichment observability

> **Status: implemented.** Every OODA decision the daemon dispatches now emits,
> at the [`enrich_turn_input`](https://github.com/rysweet/Simard/blob/main/src/base_type_turn.rs)
> seam, an `INFO` `simard::enrichment` line **and** the `simard.enrichment.*`
> telemetry metrics recording whether the cognitive-memory bridge **attached**
> and exactly how much recall was **injected** into the prompt. A degrade is a
> loud `WARN` with a concrete reason — never a silent `None`.

Simard already *recalls* memory on every turn: the preparation phase builds a
`PreparedContext` (the top-10 ranked facts and top-5 objective-scoped
procedures), the session builder wires production enrichment via
`with_enrichment(default_state_root())`
([#1664](https://github.com/rysweet/Simard/issues/1664)), and `enrich_turn_input`
renders `## Relevant Memory Facts` + `## Known Procedures` into the turn's
`prompt_preamble`. The rendering is unit-tested. The wiring is real.

**But recalling memory is not the same as *using* it, and until #2942 the
difference was invisible.** This page is the durable rationale for closing that
gap: making it *provable, live, and per-decision* that recalled memory actually
reaches Simard's decisions — and making any failure to do so **loud**.

## The gap: a silent `None`

`EnrichmentSource::resolve` launches the native cognitive-memory and knowledge
bridges via `launch_enrichment_bridges`. Honouring the honest-degradation
contract, a bridge that fails to launch degrades to `None` so the turn still
dispatches instead of aborting. That degradation is *correct behaviour* — but it
was **unobservable**:

- `launch_enrichment_bridges` degraded on failure through a bare `eprintln!`
  that no structured log, metric, or dashboard ever surfaced.
- `enrich_turn_input` emitted **nothing** about whether memory attached or how
  many facts/procedures it actually rendered into the prompt.

So the two states below were **indistinguishable** from the outside:

| State | Prompt preamble | Was it visible? |
|---|---|---|
| **Attached** — memory bridge is `Some`, store has facts/procedures, preamble carries them | `## Relevant Memory Facts` + `## Known Procedures` populated | No signal either way |
| **Degraded** — bridge launch failed (e.g. a live `memory-ipc` **Broken pipe**), memory is `None`, preamble carries no recall | Only `## Objective` | No signal either way |

A daemon could run for hours making decisions with **zero** recalled memory
because the memory-IPC socket broke once at launch, and nothing on the dashboard,
in the logs, or in the metrics would say so. Recall precision could be perfect on
the benchmark rail while, live, recall was reaching *none* of the decisions. That
is exactly the kind of silent degradation the project philosophy
(PHILOSOPHY.md) forbids: **fail loud, never hide a
degrade.**

## The principle: prove it, per decision, fail loud

Enrichment observability rests on one rule — *a decision that did not receive its
recalled memory must be as visible as one that did.* Three surfaces make that
concrete, from cheapest to strongest proof.

### 1. Per-decision instrumentation (the live evidence)

At the `enrich_turn_input` seam, after the enrichment block is rendered, every
turn emits both a structured log line and telemetry:

- **`attached`** — did the memory bridge resolve to `Some` (recall reached this
  decision) versus degrade to `None`?
- **`preamble_bytes`** — the size of the rendered enrichment block actually
  injected into `prompt_preamble`.
- **`facts_injected` / `procedures_injected`** — the counts of facts and
  procedures actually *rendered into the prompt* (read post-render, so they
  reflect what the model saw, not what the candidate set held).

The turn's objective/slug rides the `INFO` line as a truncated, control-stripped
field so an operator can correlate a decision with its recall — never as a metric
attribute (that would poison metric cardinality).

On **degrade**, the bare `eprintln!`s become loud `WARN`s carrying the concrete
reason — a memory-IPC error (`memory_ipc`) or a knowledge-bridge launch failure
(`knowledge_launch`) — plus a `simard.enrichment.degraded{reason}` counter. A
degradation is now *impossible to miss*.

### 2. A live dashboard surface (the at-a-glance answer)

Per-decision lines are proof but not a *dashboard*. The Memory tab renders a
small **"Recall reaching decisions"** panel — recent **attach-rate** and the
**average facts / procedures / preamble bytes injected per decision** — read from
the live store (consistent with the goal-board-live-read direction), so an
operator can answer "is recall reaching decisions right now?" at a glance, and
sees the degrade reason the moment attach-rate drops below 100%.

### 3. The ablation eval (the hard proof)

Instrumentation proves recall was *injected*; it does not prove recall
*mattered*. The hard proof is a reproducible **ablation**: run a representative
decision **with recall injected** versus **with recall suppressed** and measure
the delta. A non-zero delta is a reproducible **yes** on "recalled memory
influences decisions"; a zero delta is an honest **no** that says the recall is
inert. The eval reuses the existing gym harness and feeds its delta into the
[hybrid self-measurement](./hybrid-cognition-measurement.md)
([#2644](https://github.com/rysweet/Simard/issues/2644)) so the claim is tracked
over time, not asserted once.

## Why this belongs next to hybrid cognition measurement

[Hybrid cognition measurement](./hybrid-cognition-measurement.md) (G1) says a
cognition metric is trusted only when it improves on a **fixed benchmark** *and*
trends the same way in **live** production. `recall_precision_at_k` measures how
*good* the ranked recall is. Enrichment observability measures something more
basic and upstream: whether that recall is *plumbed through to the decision at
all*. A brilliant `recall_precision_at_k` is worthless if the bridge silently
degraded and no decision ever saw the facts. The ablation delta is the live rail
for the claim "recall reaches — and moves — decisions", exactly the shape of the
hybrid measurement it feeds.

## What this is and is not

- **Additive.** No recall, ranking, rendering, or dispatch behaviour changes.
  This feature only *observes* — it instruments the existing seam, surfaces the
  numbers, and adds a reproducible ablation.
- **Fail-loud, never fail-silent.** A degrade is a `WARN` + a counter, never a
  hidden `None`.
- **Scoped to the daemon's own in-process OODA decisions.** Enrichment inside an
  engineer subprocess reaches OTLP through the subprocess's own telemetry but not
  this in-process registry; that boundary is documented as a known limitation in
  the [API reference](../reference/enrichment-observability-api.md#guarantees-and-non-guarantees).
- **Configured ≠ degraded.** `enrich_turn_input` is a *shared* seam: some turns
  (non-enriching adapters, CLIs, tests) never wire a bridge at all. Those are a
  benign `attached=false`, logged at `INFO` (`expected=false`) and excluded from
  the attach-rate; only a bridge that was *expected but degraded* raises a `WARN`.
  This keeps "any `WARN` = a real degrade" true and stops unrelated non-enriching
  work from dragging the attach-rate below 100%. The seam learns whether
  enrichment was expected from a provenance bit set on `EnrichmentClients` at
  `EnrichmentSource::resolve` time (`Native` ⇒ expected, `Disabled` ⇒ not), so a
  fully-degraded bridge is still counted as expected rather than mistaken for
  "unconfigured" — see the
  [API reference](../reference/enrichment-observability-api.md#threading-expected).
- **Content-free.** The instrumentation records *counts and byte sizes*, never
  the fact or procedure *text*.

## See also

- [Enrichment observability API reference](../reference/enrichment-observability-api.md)
  — the seam, the metric catalog, the `/api/enrichment` endpoint, the ablation eval.
- [How to verify recall is reaching decisions](../howto/verify-recall-reaches-decisions.md)
  — the operator playbook.
- [Concept: hybrid cognition measurement](./hybrid-cognition-measurement.md) —
  the benchmark+live rail this feeds.
- [Telemetry metrics reference](../reference/telemetry-metrics.md) — the facade
  and `metrics_snapshot.json` plumbing.
- [Base-type adapters reference](../reference/base-type-adapters.md) — the shared
  enrichment seam this instruments.
