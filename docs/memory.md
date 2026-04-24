---
title: Memory architecture
description: Top-level overview of Simard's six-type cognitive memory, consolidation flow, and on-disk layout. Cross-links to the canonical architecture page.
last_updated: 2026-04-24
owner: simard
doc_type: concept
---

# Memory architecture

Simard's memory is not a flat key-value store. She uses **six distinct memory types** modeled after cognitive psychology, implemented natively in Rust via `NativeCognitiveMemory` backed by LadybugDB (the `lbug` crate). There is no Python bridge — memory operations are direct LadybugDB calls.

For the full canonical specification (schema, consolidation rules, hive event bus contract) see [Cognitive Memory Architecture](architecture/cognitive-memory.md). This page is the operator-level summary.

## The six memory types

| Type | Lifetime | What it holds |
|------|----------|---------------|
| **Sensory** | TTL ~300 s (configurable) | Raw observations: PTY output, error messages, objective text. Auto-expires unless promoted. |
| **Working** | Task-scoped (cleared at task end) | The 20-slot active task context: goal, constraints, plan steps, current execution state. |
| **Episodic** | Persistent, autobiographical | "What happened this session" — every cycle, every action, every observation. |
| **Semantic** | Persistent, deduplicated | Facts and learned concepts promoted from episodic memory ("the test harness uses CARGO_TARGET_DIR"). |
| **Procedural** | Persistent, indexed by trigger | Learned how-to: action sequences that worked for a given situation. |
| **Prospective** | Persistent, time/event-indexed | Future intentions: "when CI is green for #1209, post a follow-up comment." |

## Consolidation flow

```
Sensory   ──(attention)──▶  Episodic
Working   ──(task end)───▶  Episodic
Episodic  ──(consolidate)─▶ Semantic
Episodic  ──(repeated success)──▶ Procedural
```

The OODA daemon dispatches a `consolidate-memory` action whenever working-memory pressure or recent-episode density crosses a threshold. Consolidation is idempotent and runs without spawning an engineer subprocess.

## Cross-session recall

Semantic, procedural, and prospective memory survive process restarts and are queried at the start of every engineer dispatch. When the daemon spawns a new engineer for a goal it seeds the engineer's working memory with the most relevant prior episodes for that goal-id, so engineers continue where the previous attempt left off.

## On-disk layout

```
~/.simard/memory/
  ├── lbug/                  # LadybugDB persistent store (semantic, procedural, prospective, episodic)
  ├── working/               # Per-task working-memory snapshots
  └── sensory/               # Short-lived sensory ring buffer
```

Inspect with the dashboard's **Memory** tab ([Dashboard](dashboard.md)) — the graph view supports per-type filters and full-text search across the persistent layers.

![Memory tab](assets/dashboard-memory.png)

## Hive event bus (multi-agent knowledge sharing)

When multiple agents (engineer subprocesses, meeting facilitators, gym runs) operate concurrently, they share knowledge through the **hive event bus** (`src/hive_event_bus.rs`). Each agent emits memory events that other agents can subscribe to, enabling cross-agent learning without a central coordinator.

For multi-host coordination see [Distributed operations](distributed-operations.md).

## Code entry points

- `src/cognitive_memory/mod.rs` — `NativeCognitiveMemory` runtime
- `src/cognitive_memory/schema.rs` — LadybugDB schema
- `src/hive_event_bus.rs` — multi-agent event bus

## Related

- [Cognitive Memory Architecture](architecture/cognitive-memory.md) (canonical, full detail)
- [Dashboard](dashboard.md) — Memory tab
- [Daemon mode](daemon-mode.md) — when consolidation runs
