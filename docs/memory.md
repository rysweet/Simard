---
title: Memory architecture
description: Top-level overview of Simard's six-type cognitive memory, consolidation flow, and on-disk layout. Cross-links to the canonical architecture page.
last_updated: 2026-06-19
owner: simard
doc_type: concept
---

# Memory architecture

Simard's memory is not a flat key-value store. She uses **six distinct memory types** modeled after cognitive psychology. They are provided by the upstream [`amplihack-memory-lib`](https://github.com/rysweet/amplihack-memory-lib) crate (persistent, LadybugDB/`lbug`-backed) and reached through the `LibraryCognitiveMemory` adapter, which implements the `CognitiveMemoryOps` trait. This library backend is the sole on-disk cognitive-memory backend — there is no Python bridge and no native fork.

For the full canonical specification (schema, consolidation rules, hive event bus contract) see [Cognitive Memory Architecture](architecture/cognitive-memory.md). This page is the operator-level summary.

## The six memory types

| Type | Lifetime | What it holds |
|------|----------|---------------|
| **Sensory** | TTL ~300 s (configurable) | Raw observations: PTY output, error messages, objective text. Auto-expires unless promoted. |
| **Working** | Task-scoped (cleared at task end) | The 20-slot active task context: goal, constraints, plan steps, current execution state. |
| **Episodic** | Persistent, autobiographical | "What happened this session" — every cycle, every action, every observation. |
| **Semantic** | Persistent, deduplicated | Facts and learned concepts promoted from episodic memory ("the test harness uses CARGO_TARGET_DIR"). |
| **Procedural** | Persistent, indexed by trigger, deduplicated by name | Learned how-to: action sequences that worked for a given situation. Written by the OODA Act phase for successful outcomes. Storing an identically-named procedure is idempotent (#2298). See [OODA procedural memory](reference/ooda-procedural-memory.md) and [Procedural-memory store idempotency](reference/cognitive-memory-procedural-idempotency.md). |
| **Prospective** | Persistent, time/event-indexed | Future intentions: Active goals as trigger-action pairs, meeting action items. See [Goal–prospective memory mirror](reference/goal-prospective-memory-mirror.md). |

## Consolidation flow

```
Sensory   ──(attention)──▶  Episodic
Working   ──(task end)───▶  Episodic
Episodic  ──(consolidate)─▶ Semantic
OODA Act  ──(success)────▶  Procedural    (#2280)
Goal put  ──(Active)─────▶  Prospective   (#2207/#2280)
```

The OODA daemon dispatches a `consolidate-memory` action whenever working-memory pressure or recent-episode density crosses a threshold. Consolidation is idempotent and runs without spawning an engineer subprocess. Procedural memories are written inline during the OODA Act phase (not during consolidation) — each successful `ActionOutcome` produces an `ooda:{kind}` procedure. Prospective memories are written by `CognitiveMemoryGoalStore::put()` whenever a goal transitions to Active.

## Cross-session recall

Semantic, procedural, and prospective memory survive process restarts and are queried at the start of every engineer dispatch. When the daemon spawns a new engineer for a goal it seeds the engineer's working memory with the most relevant prior episodes for that goal-id, so engineers continue where the previous attempt left off.

## On-disk layout

The library backend persists at `state_root/cognitive` (a LadybugDB `GraphStore`). In production `state_root` is `~/.simard`:

```
~/.simard/
  └── cognitive/             # library CognitiveMemory store (LadybugDB):
                             #   sensory, working, episodic, semantic,
                             #   procedural, prospective
```

The library owns its own durability (WAL + CHECKPOINT). The old native store at `~/.simard/cognitive_memory.ladybug` is abandoned by Phase 2b — it is never read or migrated, and the memory store rebuilds from scratch in `cognitive/`.

Inspect with the dashboard's **Memory** tab ([Dashboard](dashboard.md)) — the graph view supports per-type filters and full-text search across the persistent layers.

![Memory tab](assets/dashboard-memory.png)

## Hive event bus (multi-agent knowledge sharing)

When multiple agents (engineer subprocesses, meeting facilitators, gym runs) operate concurrently, they share knowledge through the **hive event bus** (`src/hive_event_bus.rs`). Each agent emits memory events that other agents can subscribe to, enabling cross-agent learning without a central coordinator.

For multi-host coordination see [Distributed operations](distributed-operations.md).

## Code entry points

- `src/cognitive_memory/mod.rs` — `CognitiveMemoryOps` trait + DTOs
- `src/cognitive_memory/library_adapter.rs` — `LibraryCognitiveMemory` (the sole backend)
- `src/hive_event_bus.rs` — multi-agent event bus

## Related

- [Cognitive Memory Architecture](architecture/cognitive-memory.md) (canonical, full detail)
- [Library-backed Cognitive Memory](architecture/cognitive-memory-library-adapter.md) — the `amplihack-memory-lib` backend, now the sole on-disk store (de-fork Phase 2b)
- [OODA procedural memory](reference/ooda-procedural-memory.md) — how successful OODA outcomes become procedures
- [Procedural-memory store idempotency](reference/cognitive-memory-procedural-idempotency.md) — exact-name dedup that stops repeated cycles re-storing identical procedures (#2298)
- [Goal–prospective memory mirror](reference/goal-prospective-memory-mirror.md) — how Active goals become prospective triggers
- [Prospective-trigger firing](reference/prospective-trigger-firing.md) — how the OODA objective probe and case-insensitive match make stored triggers fire
- [Dashboard](dashboard.md) — Memory tab
- [Daemon mode](daemon-mode.md) — when consolidation runs
