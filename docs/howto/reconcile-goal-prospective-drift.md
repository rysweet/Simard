---
title: How to reconcile goal–prospective memory drift
description: Operator guide for detecting and fixing inconsistencies between goal records in semantic memory and their prospective memory mirror entries.
last_updated: 2026-06-12
owner: simard
doc_type: howto
related:
  - ../reference/goal-prospective-memory-mirror.md
  - ../reference/cognitive-memory-goal-store.md
  - ../reference/ooda-procedural-memory.md
  - ../howto/troubleshoot-goal-store.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# How to reconcile goal–prospective memory drift

> Shipped in issues [#2207](https://github.com/rysweet/Simard/issues/2207)
> and [#2280](https://github.com/rysweet/Simard/issues/2280).

The `CognitiveMemoryGoalStore` maintains a prospective-memory mirror for
Active goals so they surface via `check_triggers` during OODA preparation
(issue #2207). Under normal operation `put()` keeps the mirror in sync,
but transient bridge failures can leave drift: an Active goal without a
prospective trigger, or a completed goal with a stale trigger still
pending.

This guide covers how to detect drift and fix it.

## Prerequisites

- SSH access to the host running Simard.
- Knowledge of `SIMARD_STATE_ROOT` (default: `~/.simard`).

---

## 1. Symptoms of drift

| Symptom | Likely cause |
|---------|-------------|
| An Active goal is not surfacing in engineer preparation context | The `store_prospective` call failed during `put()` — the goal exists in semantic memory but has no prospective trigger |
| A completed/paused goal keeps appearing in `check_triggers` results | The `resolve_goal_prospectives` call failed during `put()` — the old prospective entry was not marked resolved |
| `put()` returned an error containing "resolve_goal_prospectives" or "store_prospective" | The mirror update failed; the fact write succeeded but the prospective mirror is inconsistent |

---

## 2. Verify drift exists

Inspect current goal records and their prospective mirrors
programmatically:

```rust
use crate::goals::CognitiveMemoryGoalStore;

let store = CognitiveMemoryGoalStore::new(state_root.clone())?;
let goals = store.list()?;
for g in &goals {
    println!("{:10} {:40} {}", g.status, g.slug, g.title);
}
```

Then compare with the prospective memory entries returned by
`check_triggers` for the `"goal:"` prefix. Every **Active** goal slug
(with dashes → spaces) should have a matching pending prospective entry.
No **Completed**, **Paused**, or **Proposed** goal should have one.

Mismatches indicate drift.

> **Note:** There are no CLI subcommands for inspecting goal or
> prospective state directly. Use the Rust API or add logging to your
> OODA daemon startup to surface the comparison.

---

## 3. Fix drift with `reconcile_prospectives()`

The `CognitiveMemoryGoalStore` exposes a `reconcile_prospectives()`
method that walks all current goal records and ensures consistency:

- Active goals get a prospective entry created if missing.
- Non-Active goals get stale prospective entries resolved.

### Via the OODA daemon

The daemon can call `reconcile_prospectives()` at cycle start as a
health check. If you are running the daemon, drift is corrected
automatically on the next cycle.

### Programmatically

```rust
use crate::goals::CognitiveMemoryGoalStore;

let store = CognitiveMemoryGoalStore::new(state_root)?;
store.reconcile_prospectives()?;
```

The method returns `SimardResult<()>` — the first error encountered
stops reconciliation, leaving subsequent goals unprocessed. Retry to
fix remaining items. See the
[partial reconciliation note](../reference/goal-prospective-memory-mirror.md#reconcile_prospectives)
for details.

---

## 4. Prevent drift

Drift occurs only when `put()` partially fails — the fact write succeeds
but the prospective mirror update fails. Since `put()` propagates these
errors (it does not swallow them), callers know when drift occurs:

- **Check `put()` return values.** Any caller that ignores `put()` errors
  risks drift. All production callers (`meeting_backend::closing`, OODA
  curate, dashboard mutation handlers) propagate or log the error.

- **Monitor logs.** Bridge failures that cause prospective-mirror errors
  appear in stderr with the prefix
  `[simard] CognitiveMemoryGoalStore::put:`. Set up log alerting for
  this prefix if running in production.

- **Run reconciliation periodically.** Even without errors, a
  `reconcile_prospectives()` call at OODA cycle start is cheap (one
  `list_via_reader` + one `check_triggers` per goal) and guarantees
  consistency.

---

## 5. Edge cases

### Database restored from backup

After restoring a cognitive memory database from backup, semantic facts
and prospective entries may be at different points in time. Run
`reconcile_prospectives()` immediately after restore.

### Multiple concurrent `put()` calls for the same slug

Two `put()` calls racing for the same slug can both succeed: the fact
store is append-only and `list()` deduplicates by slug (latest node_id
wins). However, the prospective mirror may have the trigger from the
first call resolved by the second, or vice versa. The end state is
correct because the latest `put()` writes the definitive prospective
entry — but if the *second* call's `store_prospective` fails, drift
occurs. `reconcile_prospectives()` fixes this.

---

## See also

- [Goal–prospective memory mirror reference](../reference/goal-prospective-memory-mirror.md)
  — API details, constants, error contract.
- [How to troubleshoot the goal store](./troubleshoot-goal-store.md)
  — for issues with the file-backed goal store.
- [How to unblock stuck OODA goals](./unblock-stuck-ooda-goals.md)
  — for goals that are not progressing.
