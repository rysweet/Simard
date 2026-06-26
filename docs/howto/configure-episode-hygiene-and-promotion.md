---
title: Configure episode hygiene and promotion
description: Operator guide for Simard's episode ingestion policy and automatic distillation scheduler — tuning the promotion thresholds, reading the per-cycle intake and promotion log lines, verifying that noise is dropped and meaningful episodes are stored, and confirming that distillation runs automatically and writes provenance-linked facts and procedures.
last_updated: 2026-06-20
owner: simard
doc_type: howto
related:
  - ../architecture/episode-ingestion-policy.md
  - ../reference/episode-ingestion-classifier.md
  - ../reference/automatic-distillation-scheduler.md
  - ../architecture/episode-distillation.md
  - ../reference/simard-memory-cli.md
  - ../memory.md
---

# Configure episode hygiene and promotion

> Applies to issue [#2327](https://github.com/rysweet/Simard/issues/2327).

Simard keeps episodic memory clean and self-promoting through two
cooperating mechanisms:

- An **ingestion policy** that drops or down-scopes operational-noise
  episodes before they are stored.
- An **automatic promotion scheduler** that distills recurring episodes
  into semantic facts and procedures at the end of every OODA cycle.

This guide shows how to observe both, tune the promotion thresholds, and
verify the behaviour end-to-end. Neither mechanism needs configuration to
work — the defaults are production-ready. Tune only when you have a reason.

## When to use this

- You want to confirm noise is being dropped and meaningful events stored.
- You want distillation to run more (or less) eagerly.
- The undistilled backlog looks stuck and you want to know why.
- You are auditing what became a fact or procedure, and from which
  episodes.

## Observe the ingestion policy

Every OODA cycle emits one aggregated intake counter line. The daemon writes
its `[simard]` operational lines to **stderr** and mirrors the
dashboard-visible ones into `<state_root>/ooda.log` (default `~/.simard`,
relocated by `SIMARD_STATE_ROOT`), so you can grep either:

```bash
grep "episode-intake" ~/.simard/ooda.log | tail -5
```

Typical output:

```
[simard] episode-intake dropped=7 stored=3 downscoped=2
```

- `dropped` — known-noise episodes (session start/complete/persist,
  `flushing working memory`, `continue_skipping` / `no decision keyword`)
  with no failure signal. **Not stored.**
- `stored` — durable events: action failures, completed actions, handoffs,
  goal archival/promotion, user decisions, recipe failures.
- `downscoped` — unrecognised content, stored at `importance = 0.1` with
  `is_operational = true` so it ranks below real signal.

A healthy busy cycle drops more than it stores — that is the point. If
`dropped` is `0` across many cycles, the noise markers may have changed
upstream; check `src/memory_consolidation/classifier.rs` against the
current intake content.

### Verify a noise episode is dropped and a meaningful one is stored

The fastest check is the counter line above. For a deterministic unit-level
check, the classifier is pure and directly testable:

```bash
cargo test memory_consolidation::classifier
```

The shipped tests assert that `Session … started with objective …` →
`Drop`, that an `ActionCompleted` event → `Store` with all five metadata
keys, and that a noisy line containing `error` / `panic` is **kept** via
the failure override.

## Observe the promotion scheduler

The scheduler logs once whenever it fires:

```bash
grep "promotion:" ~/.simard/ooda.log | tail -5
```

```
[simard] promotion: backlog=27 threshold=25 cycles_since=8 → 27 episodes, 4 facts, 1 procedure, 27 marked
```

Read it as: the undistilled backlog reached 27 (≥ the threshold of 25), so
distillation ran, produced 4 facts and 1 procedure, and marked all 27
input episodes distilled. When the scheduler does not fire it logs nothing
(the underlying distillation pass still logs its own `distill:` lines when
invoked by `ConsolidateMemory`).

The scheduler fires when **either**:

- the undistilled backlog reaches `distill_min_episodes`, **or**
- `distill_interval_cycles` cycles have elapsed since the last pass.

The interval trigger guarantees a quiet run still promotes its trickle of
episodes at least once every `distill_interval_cycles` cycles.

## Tune the promotion thresholds

Two environment variables drive the scheduler. Set them before starting
the daemon:

```bash
export SIMARD_DISTILL_MIN_EPISODES=25      # backlog size that triggers promotion (default 25)
export SIMARD_DISTILL_INTERVAL_CYCLES=50   # cycle interval that guarantees promotion (default 50)
simard ooda run        # start the OODA loop daemon
```

| Variable | Default | Effect of lowering | Effect of raising |
|---|---:|---|---|
| `SIMARD_DISTILL_MIN_EPISODES` | 25 | Promote sooner, more frequent (smaller, costlier) LLM passes | Larger batches, fewer LLM calls, higher recall latency |
| `SIMARD_DISTILL_INTERVAL_CYCLES` | 50 | Guarantee promotion more often on quiet runs | Tolerate longer gaps before forced promotion |

Guidance:

- For a **busy** improvement run, the defaults promote on the backlog
  trigger long before the interval matters.
- For a **quiet** curation/meeting run, lower `SIMARD_DISTILL_INTERVAL_CYCLES`
  if you want facts to surface faster.
- Avoid setting `SIMARD_DISTILL_MIN_EPISODES` very low (< 10): distillation
  is a many-to-few operation and tiny batches waste LLM calls for little
  quality gain. The pass also carries an internal hard floor of 20
  episodes inside `distill_recent_episodes_with_runner`.

## Verify provenance-linked facts and procedures

After a promotion pass, confirm distilled facts and procedures landed and
trace them back to their source episodes:

```bash
simard memory stats     # semantic (facts) and procedural counts should grow after a pass
simard memory dump      # sample rows, including concept labels and procedure names
```

Provenance edges (`DERIVES_FROM` for facts, `PROCEDURE_DERIVES_FROM` for
procedures) are written automatically — every distilled fact and procedure
links back to the `source_episode_ids` it was derived from. See
[Cognitive-memory provenance](../reference/cognitive-memory-provenance.md)
for how to recall the source episodes of a fact, and the **Memory** tab of
the [dashboard](../dashboard.md) for the graph view.

## Edge cases

- **Reflection transcript still stored despite noise.** The reflection
  transcript episode is *sanitized*, not dropped, when facts are derived
  this cycle — its id is needed to anchor fact provenance. A
  pure-noise transcript with no derived facts is dropped. This is expected.
- **Distillation logged an error.** Promotion swallows distillation errors
  at the cycle boundary so the OODA cycle never aborts. The batch is **not**
  marked distilled on error, so it retries on the next interval. Grep for
  `recipe error` in the `distill:` lines to diagnose.
- **Backlog never reaches the threshold.** On very quiet runs the interval
  trigger is what fires promotion — confirm `SIMARD_DISTILL_INTERVAL_CYCLES`
  is not set absurdly high.
- **A procedure re-appears each pass.** Procedure storage is
  upsert-by-name with reinforcement, so re-derivation reinforces an
  existing procedure rather than duplicating it.

## Related

- [Episode ingestion policy & automatic promotion](../architecture/episode-ingestion-policy.md) —
  design rationale
- [Episode ingestion classifier API](../reference/episode-ingestion-classifier.md) —
  the classifier surface
- [Automatic distillation scheduler API](../reference/automatic-distillation-scheduler.md) —
  the scheduler surface and config fields
- [Episode distillation](../architecture/episode-distillation.md) — the
  fact-extraction pipeline
- [Memory introspection CLI](../reference/simard-memory-cli.md) —
  `simard memory stats` / `simard memory dump`
