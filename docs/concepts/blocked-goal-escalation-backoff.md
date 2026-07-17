---
title: Blocked-goal escalations back off exponentially instead of re-firing every tick
description: Why the Overseer no longer re-escalates the same blocked goal on every OODA tick; how the WhisperGate gained per-signature exponential backoff (base 900s, doubling per re-hit, capped ~4h) keyed on the existing per-goal signature (escalate:{goal_id} / unblock:{goal_id}), so a repeat goal is escalated once then suppressed for a growing window and re-fires after the cooldown; and why the same backoff applies to the shared self-heal (unblock) path.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./stable-goal-session-identity.md
  - ../reference/whisper-gate-backoff-api.md
  - ./overseer-root-cause-why.md
---

# Blocked-goal escalations back off exponentially

> **Status: implemented (issue #4255).** The blocked-goal path
> (`EscalateBlockedGoal` and the self-heal `UnblockGoal`) now runs through a
> `WhisperGate` configured with **per-signature exponential backoff**, so an
> already-handled goal is not re-escalated (or re-unblocked) on every tick.
> Primary sources:
> [`src/overseer/guardrails.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/guardrails.rs)
> (`WhisperGate::with_backoff`) and
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
> (`blocked_goal_gate` wiring, `act_escalate_blocked_goal`, `act_unblock_goal`).
> API details: [WhisperGate backoff reference](../reference/whisper-gate-backoff-api.md).

## The defect this fixes

The Overseer's OODA loop emits `recurring_blocked_goal` and, via
`Intervention::EscalateBlockedGoal`, surfaces a blocked goal to the human for
review. Both that path and the self-heal `Intervention::UnblockGoal` path share
one dedup gate — `blocked_goal_gate` — which was a `WhisperGate` with a **fixed**
15-minute (`900s`) window and no growth:

```rust
blocked_goal_gate: WhisperGate::new(900, 20),   // BEFORE (#4255)
```

The gate is keyed **per goal**, not per cluster. Each path builds a one-goal
signature:

```rust
let signature = format!("escalate:{goal_id}"); // act_escalate_blocked_goal
let signature = format!("unblock:{goal_id}");  // act_unblock_goal
```

A fixed window re-admits the **same goal** every 15 minutes. In practice a
handful of perpetually-blocked goals each re-escalated on ~4+ consecutive ticks
(observed 06:14–07:18Z, 3× in cognitive memory), wasting cycles and burying the
operator in duplicate escalations — especially while the underlying root cause
(#4197) kept those goals blocked. Because there are several goals, the operator
saw several independent 15-minute repeats, not one "cluster" repeat.

## The fix: per-signature exponential backoff

The blocked-goal gate is now built with `with_backoff`:

```rust
blocked_goal_gate: WhisperGate::with_backoff(900, 14_400, 20),
//                                            base  cap    cap_per_hour
```

Behaviour, per **signature** (i.e. per `escalate:{goal_id}` /
`unblock:{goal_id}` — one entry per goal per path):

- **First escalation** of a goal is delivered immediately.
- Each re-hit **within** the current suppression window doubles the next window:
  `window = base * 2^strikes`, i.e. `900s → 1800s → 3600s → 7200s → 14400s`,
  **capped at ~4h** (`14_400s`).
- After the window elapses, the goal **re-fires once** (a heartbeat); a delivery
  alone does not reset the strike counter.

The net effect: the old "re-admit every 15 min forever, per goal" becomes a
rapidly growing per-goal suppression window, while a periodic ~4h heartbeat still
reaches the operator so a genuinely-stuck goal is never silenced forever.

## The signature: what counts as "the same escalation"

The signature is the **existing per-goal string**, unchanged by this fix:

```
escalate:{goal_id}   // human escalation path
unblock:{goal_id}    // self-heal path
```

- Each distinct `goal_id` has its own independent backoff window — the "same four
  goals" case is **four independent per-goal windows**, not one shared cluster
  window.
- The `escalate:` and `unblock:` prefixes keep the two paths' backoff separate
  even for the same goal, so escalating a goal never suppresses a later
  self-heal attempt on it (and vice versa).
- No hashing or set-membership logic is introduced; the gate continues to key on
  the literal signature string.

## The shared self-heal path

`blocked_goal_gate` is used by **both** `act_escalate_blocked_goal` and
`act_unblock_goal`. Switching it to backoff therefore also changes the
**self-heal cadence**: a goal that keeps re-blocking is auto-unblocked, then its
`unblock:{goal_id}` signature backs off the same way (900s → … → 14400s). This
is intended — it stops the Overseer from thrashing an auto-unblock/re-block loop
on a goal that is not actually recoverable, and lets the growing window give
in-flight work time to land before the next self-heal attempt.

## Why this is safe

- **Suppression can hide a real problem** → mitigated by the ~4h cap (each goal
  re-surfaces at least every 4h).
- **Unbounded strike map** → the per-signature strike/last-delivery map is bounded
  with eviction/TTL (as the existing rolling-hour budget already prunes) so a
  churning set of goal ids cannot grow memory without limit.
- The change is **additive and non-breaking**: `WhisperGate::new` keeps its
  original fixed-window semantics; `with_backoff` is a new constructor, and only
  `blocked_goal_gate` adopts it. The other gates (`whisper_gate`,
  `write_back_gate`, `gap_gate`) are unchanged.

## Verifying the behaviour

Unit tests in `guardrails.rs` and the goal-health/root-cause suites assert:

- **Escalate once, then suppress** — the same signature is delivered once and
  suppressed on subsequent ticks within the window.
- **Window growth** — successive re-hits produce `900 → 1800 → 3600 → …` windows,
  capped at `14_400s`.
- **Re-fire after cooldown** — once the window elapses, the signature re-fires.
- **Per-signature isolation** — backoff for one `goal_id` (or one path prefix)
  does not affect another.
- **Fixed-window unchanged** — `WhisperGate::new` retains its original constant
  window (regression guard for the other gates).

See the [reference doc](../reference/whisper-gate-backoff-api.md) for the exact
signatures and configuration.
