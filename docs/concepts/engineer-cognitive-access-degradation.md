---
title: "Concept: engineer cognitive-access degradation (no artifact-less exits on a lost open-lock race)"
description: >
  Why concurrent OODA engineers must never starve on — and hard-exit at — the
  single-writer cognitive open-lock. Each engineer routes its cognitive access
  through the daemon memory-IPC client (shared read + serialized short-held
  write) and — on a genuinely lost open-lock race — degrades to deferred /
  read-only cognition and STILL produces its commit/PR instead of dying
  artifact-less. (An isolated per-worktree state root for standalone runs is a
  designed, deferred follow-up.) The corruption guard for a genuine second
  concurrent writer is preserved unchanged; a degrade is always a loud WARN +
  OTel counter, never a silent fallback.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ../reference/engineer-cognitive-access-degradation-api.md
  - ../reference/cognitive-memory-open-serialization.md
  - ../howto/diagnose-a-degraded-engineer-cognitive-access.md
  - ./enrichment-observability.md
  - ../reference/engineer-worktree-isolation.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ../reference/telemetry-metrics.md
  - ../../src/cognitive_memory/open_guard.rs
  - ../../src/ooda_loop/client_factory.rs
---

# Concept: engineer cognitive-access degradation

> **Status: implemented.** An OODA-spawned engineer that loses the race for the
> cross-process cognitive **open-lock** no longer shuts down its `MeterProvider`
> and exits artifact-less. It **prefers the daemon memory-IPC client** (shared
> read, serialized short-held write); when a direct open would genuinely contend
> it **degrades to deferred / read-only cognition** and still produces its
> commit/PR. Every degrade is a loud `WARN` on a `simard::*` target plus a
> `simard.enrichment.degraded{reason="cognitive_open_lock"}` counter — never a
> silent fallback. The fail-loud corruption guard for a genuine second concurrent
> **writer** of a non-isolated store is unchanged. (Giving a *standalone*
> engineer — one with no daemon socket — its own **isolated per-worktree state
> root** for a fully live, uncontended store is a designed, deferred follow-up;
> see §"How an engineer resolves its cognitive access".)

## The failure this closes

The [cognitive open-serialization guard](../reference/cognitive-memory-open-serialization.md)
converts an lbug lock-conflict-as-corruption event — which would otherwise
**wipe** the cognitive store — into a *loud, recoverable* failure. That guard is
correct and stays. But it exposed a second, systemic defect one layer up:

When the daemon runs a real workload, **multiple** OODA engineers run
concurrently — each in its own [git worktree](../reference/engineer-worktree-isolation.md).
Historically each engineer opened its **own second direct
`LibraryCognitiveMemory` handle** on the shared daemon state root instead of
routing through the daemon's memory-IPC client. The single-writer exclusive
`flock(LOCK_EX|LOCK_NB)` on `cognitive.open.lock` admits exactly one holder, so
with `in_flight_engineer_count = 7` competing for one cognitive store, the losers
spun for the full `DEFAULT_BUDGET` (15 000 ms) and then hit:

```
cognitive store is held open by another process … after waiting 15000ms
```

The engineer treated that as fatal: it shut down its `MeterProvider` and **exited
without producing any commit, PR, or artifact**. The daemon's own brain correctly
diagnosed this as *"a systemic contention/corruption-risk signal, not a per-goal
glitch"* — the same goal (`move-the-governed-repo-roster`) tripped the
no-progress breaker three times in six hours. Losing an advisory-cognition race
must never cost a whole unit of engineering work.

> This is **distinct from** — and must not be conflated with — the benign
> memory-ipc broken-pipe reconnect (`memory_errors = 0`, tracked in
> [#2860](https://github.com/rysweet/Simard/issues/2860)). That is an already
> handled transient; this concept is about the *hard exit on a lost open-lock
> race*. The two share no code path.

## The principle: cognition is advisory; artifacts are not optional

Cognitive recall is *advisory* input to a decision — it makes an engineer smarter,
but its absence must never abort the engineer. Artifact production (the commit /
PR the goal exists to produce) is the **non-negotiable** output. The fix follows
one rule:

> **A lost open-lock race degrades cognition; it never degrades artifact
> production. The engineer always finishes.**

This mirrors the [enrichment-observability](./enrichment-observability.md)
contract already proven in the daemon's own OODA turn: a memory reader that fails
to launch degrades to `None` so the turn still dispatches — loudly (`WARN` +
counter), never silently. Engineer cognitive access now degrades the same way.

## How an engineer resolves its cognitive access

Resolution is a strict, ordered preference — the first tier that succeeds wins.
An engineer never opens a **second exclusive** handle on the shared store and
then hard-exits on the lost race: a contended open resolves to tier 3 (degrade)
instead of the fatal 15 s error the open-guard message names.

| Tier | Condition | Behaviour | Cognition |
|---|---|---|---|
| **1 — Shared read via daemon IPC** | A daemon socket exists at the shared state root | Route through the [`RemoteCognitiveMemory`](../reference/cognitive-memory-client-helpers.md) IPC client. Reads are shared; writes are serialized and short-held by the daemon | **Live** (full read + write) |
| **2 — Uncontended direct open** | No daemon socket, and the open-lock is **not** contended (classified `Acquired`) | Open a direct `LibraryCognitiveMemory` on the state root — safe because nothing else holds the lock | **Live** (full read + write) |
| **3 — Deferred / read-only** | No IPC *and* a direct open on the shared root would contend (classified `Contended`, not budget-exhausted) | Serve reads from an empty snapshot; **defer** writes (drop-with-metric via a bounded counter — nothing buffered); emit `WARN` + `degraded{reason="cognitive_open_lock"}` | **Deferred** (read-only writes) |

In every tier the engineer **proceeds to produce its commit/PR**. Tier 3 is the
graceful-degradation path: the engineer runs with reduced cognition rather than
dying.

> **Deferred follow-up — isolated per-worktree root.** A fourth tier is designed
> but not implemented in this change: for a **standalone** engineer with no
> daemon socket, allocate an isolated `<worktree>/cognitive-state` root so its
> open is *live and uncontended* rather than degraded. It is deferred because the
> engineer's `--state-root` serves both cognition and the OODA ledger, and the
> engineer is launched across a recipe/agent boundary — isolating only cognition
> needs a new dedicated cognitive-root parameter threaded through the launch
> path. Until then, a standalone engineer that hits contention takes tier 3
> (degrade), which already keeps it artifact-producing.

### Why not just raise the budget?

Raising `DEFAULT_BUDGET` was explicitly rejected as the fix. A larger timeout only
makes 7 engineers wait *longer* for a lock that admits one holder; it does not let
them make progress, and it hides the contention instead of resolving it. The
budget stays at 15 000 ms. The fix removes the *need* to win the lock at all
(tiers 1–2) and makes losing it *survivable* (tier 3).

## The distinction that keeps the corruption guard intact

The open-guard's fail-loud behaviour for a genuine second concurrent writer is
**preserved exactly**. The change is **additive** and driven by **caller role /
intent**, not by lock state:

- **`CallerRole::Daemon`** (and any true exclusive-writer caller) keeps the
  current fail-loud path. A genuine second writer of a non-isolated store still
  errors with `PersistentStoreIo` — the corruption guard is unchanged.
- **`CallerRole::Engineer`** passes an explicit *may-degrade* intent. A contended
  open resolves to **deferred / read-only** cognition instead of a hard exit.

The open-guard exposes a *distinguishable* "contended" outcome
(`OpenLockOutcome::Contended`) so the **caller** chooses degrade-vs-fail; the
guard's `acquire()` semantics are byte-for-byte unchanged. `CallerRole` is an
**internal capability token** — it is never derived from an env var, CLI flag, or
file input, so a degraded write can never be induced from outside the process.

## Anti-hollow-success: a deferred write is never claimed as persisted

The tension this feature had to resolve: engineers must finish, **but** a silent
no-op write that *claims* success is exactly the hollow-success failure
[#2896](https://github.com/rysweet/Simard/issues/2896) /
[#1590](https://github.com/rysweet/Simard/issues/1590) forbid. The resolution:

- A **deferred** write is **observable** — counted on the degradation metric,
  logged — and **never reported to the caller as persisted**.
- The daemon / true-writer path stays **fail-loud**: `launch_writer_client` never
  silently no-ops a write it could not persist.

So cognition can degrade without ever telling a lie about what was stored.

## What this is and is not

- **Additive & non-breaking.** No change to recall, ranking, rendering, or the
  open-guard's corruption semantics. Engineers gain a graceful-degradation path;
  everything else behaves as before.
- **Fail-loud, never fail-silent.** A degrade is a `WARN` + an OTel counter
  increment, never a hidden fallback. Reusing the existing
  `simard.enrichment.degraded` counter (new bounded `reason="cognitive_open_lock"`)
  keeps telemetry cardinality flat — no new counter surface.
- **No legacy "bridge"-style naming, no stray `print!`/`println!`.** New/changed paths use
  structured `tracing` + OTel only (grep-verified; `tests/no_bridge_naming.rs`
  passes).
- **Out of scope: the #2860 broken-pipe reconnect.** That transient is untouched.

## See also

- [Engineer cognitive-access degradation — API reference](../reference/engineer-cognitive-access-degradation-api.md)
  — the `OpenLockOutcome` classifier, `CallerRole`, `CognitiveAccess`, `WriteMode`,
  the reused degrade counter, configuration, and security invariants.
- [Cognitive-Memory Open Serialization](../reference/cognitive-memory-open-serialization.md)
  — the corruption safety-net this builds on and preserves.
- [How to diagnose a degraded engineer cognitive access](../howto/diagnose-a-degraded-engineer-cognitive-access.md)
  — the operator playbook.
- [Concept: enrichment observability](./enrichment-observability.md) — the
  fail-loud degrade contract this replicates.
- [Per-Engineer Worktree Isolation](../reference/engineer-worktree-isolation.md)
  — the filesystem-diff isolation an isolated cognitive root (deferred follow-up)
  would sit alongside.
