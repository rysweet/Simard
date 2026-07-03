---
title: Brain introspection + memory hygiene
description: Design rationale for Simard's periodic brain self-examination and memory-hygiene pass (#2419) — why a daemon interval task (not a standing goal or per-cycle hook), the bridge-reachability finding that drives the safe-first increment split, the cadence/knobs, and the safety model for bounded reversible pruning.
last_updated: 2026-06-27
review_schedule: as-needed
owner: simard
doc_type: explanation
related:
  - ../reference/brain-introspection-api.md
  - ../howto/configure-brain-introspection.md
  - ./episode-ingestion-policy.md
  - ../reference/automatic-distillation-scheduler.md
---

# Brain introspection + memory hygiene

> Shipped in issue [#2419](https://github.com/rysweet/Simard/issues/2419).

Simard runs a **standing brain self-examination + memory-hygiene** pass on its
own periodic cadence (default daily). The pass examines the OODA brain's
decision quality, mines recent activity for recurring patterns, safely trims a
bounded amount of low-value memory, consolidates episodic memory into
semantic/procedural memory, and writes actionable findings to a GitHub issue.

This page explains *why* the feature is built the way it is. For the executable
contract (APIs, structs, markers, config) see the
[Brain introspection API](../reference/brain-introspection-api.md). For operator
tasks see [Configure brain introspection](../howto/configure-brain-introspection.md).

## What problem this solves

Simard accumulates memory and emits decision metrics continuously, but nothing
periodically *steps back* to ask: is the brain's decision quality drifting? Are
the same blockers recurring? Is memory filling with stale, superseded, or
duplicate entries? Per-cycle distillation (issue
[#2327](https://github.com/rysweet/Simard/issues/2327)) handles incremental
promotion, but it is **not** an introspection layer — it does not analyze brain
health, mine cross-episode patterns, or prune by value.

The brain-introspection pass is that higher-level, lower-frequency layer. It
*uses* the existing infra (distillation, statistics, sensory prune) rather than
duplicating it.

## Why a periodic daemon task (the cadence decision)

The goal says "runs REGULARLY." Three mechanisms were considered:

| Option | Verdict | Why |
| --- | --- | --- |
| **(a) Periodic daemon task on its own interval** | **Chosen** | Mirrors the proven, tested `SIMARD_DISK_HEALTH_INTERVAL_SECS` / worktree-sweep pattern in `operator_commands_ooda/daemon/mod.rs`. Deterministic cadence, minimal/testable Rust, fail-open. |
| (b) Standing low-priority OODA goal | Rejected as the *cadence* source | A low-priority goal can starve indefinitely behind higher-value goals — the wrong fit for a hygiene cadence that must run on schedule. |
| (c) A recipe the brain invokes ad hoc | Rejected as the *cadence* source | No schedule guarantee; the brain may never choose it. |

Options (b) and (c) are the *content* path — the daemon task **dispatches** the
`brain-introspection` recipe as a `recipe-runner-rs` subprocess for the agentic
reasoning. The daemon owns the clock; the recipe owns the judgment. This is
exactly the disk-health split (deterministic Rust scheduler + agentic recipe).

## Split of labor

```
┌─────────────────────────────┐        ┌──────────────────────────────┐
│ Rust hook (daemon-side)      │        │ Recipe (recipe-runner-rs)    │
│ src/brain_introspection.rs   │        │ brain-introspection.yaml     │
├─────────────────────────────┤        ├──────────────────────────────┤
│ • get_statistics()          │ stats  │ • brain-health analysis      │
│ • prune_expired_sensory()   │──────▶ │ • pattern mining             │
│   (non-discretionary;       │ cap    │ • prune-CANDIDATE recommend  │
│    NOT capped)              │──────▶ │   (≤ max_prune cap)          │
│ • consolidate_episodes()    │        │ • create/update gh issue     │
│ • measure consolidated Δ    │        │                              │
│ • record_metric() × N       │ ◀──────│   emits text markers          │
│ • parse markers             │ markers│                              │
└─────────────────────────────┘        └──────────────────────────────┘
```

Recipe agents cannot call `CognitiveMemoryOps` trait methods, so **all real
memory operations run in the hook**. The recipe reasons over the real numbers
the hook already measured (passed as `-c stats=<json>`) and *recommends* prunes;
it never deletes.

## The bridge-reachability finding (drives the increment split)

The single most important design fact: the daemon's `adapters.memory` is a
`CognitiveMemoryAdapter` — a **JSON-RPC IPC client** — not the in-process
`LibraryCognitiveMemory`. Over that bridge:

- `prune_superseded()` falls through to the **default trait impl `Ok(0)` — a
  no-op**. Only `LibraryCognitiveMemory` reclaims superseded rows.
- `graph_stats()` returns the **empty default**.
- `backup_memory()` needs a `&dyn MemoryStore`; daemon-side there is **no
  store** — it lives in the bridge *server* process.

So the naïve design — "bounded destructive superseded-prune + backup in the
daemon Rust hook" — would **silently delete nothing while reporting success**: a
hollow-success / silent-degradation bug the codebase explicitly warns against.

### Resolution: a safe-first increment

| | First increment (shipped) | Follow-up |
| --- | --- | --- |
| Sensory prune (transient) | ✅ `prune_expired_sensory`, RPC-backed, non-discretionary TTL cleanup (exempt from cap) | — |
| Consolidation | ✅ `consolidate_episodes`, RPC-backed, additive; count measured via stats delta | — |
| Superseded/low-value prune | 📝 **recommended** as `PRUNE_CANDIDATE` (≤ cap) in the issue | 🔜 backed-up destructive prune via new server RPCs (cap-bounded) |
| Backup before destructive ops | n/a (no destructive ops daemon-side) | 🔜 `memory.backup` server RPC |

The follow-up (filed as an issue) adds `memory.prune_superseded` +
`memory.backup` RPCs **on the bridge server**, where the store actually lives,
to enable backed-up, bounded, reversible deletion. Until then, the pass is
read-mostly and the only daemon-side deletion is transient sensory rows.

## Safety model

The SAFETY constraints from issue #2419 are honored as follows:

- **Bounded recommendations.** `enforce_prune_cap(requested, cap)` clamps the
  number of **value-bearing prune candidates** the recipe may recommend to
  `SIMARD_BRAIN_INTROSPECTION_MAX_PRUNE` (default 25, absolute). It is passed to
  the recipe as `-c max_prune=<cap>` and clamps the returned count. `cap = 0` ⇒
  zero recommendations. The cap does **not** throttle `prune_expired_sensory`,
  which removes only already-expired transient rows (non-discretionary TTL
  cleanup). When destructive value-bearing prune lands as a follow-up, it too
  will be clamped by this cap.
- **Never deletes high-value / provenance-bearing memory.** The only daemon-side
  deletion is `prune_expired_sensory` (already-expired transient rows). Valuable
  memory is never touched; superseded candidates are surfaced for review (capped
  at `max_prune`), not deleted.
- **Additive consolidation.** `consolidate_episodes` distills episodic →
  semantic/procedural; it stores, it does not lose. The hook measures the result
  as the post−pre delta of (semantic + procedural) counts, since
  `consolidate_episodes` itself returns `Option<String>`, not a count.
- **Reversible (follow-up).** When destructive superseded prune lands, it backs
  up affected rows first (via the server `memory.backup` RPC) and aborts the
  prune (non-fatal) if backup fails.
- **Off by explicit `0`; conservative defaults.** Daily cadence, cap 25.

## Output: issue, not snapshot doc

Per the no-point-in-time-docs rule, each run writes findings to a **GitHub
issue** on `rysweet/Simard` (label `brain-introspection`) and to
`self_metrics`. The issue uses a **stable title** so repeated runs *update*
rather than spam. The only durable repo document is this reference page and the
[API reference](../reference/brain-introspection-api.md) — both describe the
pass and its knobs, not a point-in-time finding.

## Baseline and regression detection

Each run records `brain_introspection_*` metrics to `metrics.jsonl`. The
brain-health step compares the current run against the rolling window of the
previous `SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS` runs (default 7) and emits
`REGRESSION:` lines when a signal worsens materially. The **first run** finds no
prior entries and reports `BRAIN_HEALTH: no prior baseline`, establishing the
baseline for subsequent runs.

## Relationship to per-cycle distillation

This pass **does not duplicate** the per-cycle distillation scheduler
([#2327](https://github.com/rysweet/Simard/issues/2327)). That scheduler runs
every OODA cycle and handles incremental episode→fact/procedure promotion. The
introspection pass is a higher-level, lower-frequency layer that *invokes the
same* `consolidate_episodes` entry as one of its steps and adds the analysis +
hygiene that the per-cycle path intentionally omits.

## Configuration knobs

| Knob | Env var | Default |
| --- | --- | ---: |
| Cadence | `SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS` | `86400` (24h; `0` = disabled) |
| Safe-prune cap | `SIMARD_BRAIN_INTROSPECTION_MAX_PRUNE` | `25` (absolute) |
| Baseline window | `SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS` | `7` runs |

See [Configure brain introspection](../howto/configure-brain-introspection.md)
for tuning and observability.

## Related

- [Brain introspection API](../reference/brain-introspection-api.md) — the executable contract
- [Configure brain introspection](../howto/configure-brain-introspection.md) — operator guide
- [Automatic distillation scheduler API](../reference/automatic-distillation-scheduler.md) — the per-cycle consolidation reused here
- [Episode ingestion policy & promotion](./episode-ingestion-policy.md) — the memory hygiene/promotion model this pass sits above
