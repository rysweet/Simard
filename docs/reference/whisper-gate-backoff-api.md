---
title: "Reference: WhisperGate Exponential-Backoff API"
description: >
  The API contract for per-signature exponential backoff on the Overseer
  WhisperGate: the with_backoff(base_secs, cap_secs, cap_per_hour) constructor,
  the strike-counted window formula window = base * 2^strikes capped at cap_secs,
  the existing per-goal signatures (escalate:{goal_id} / unblock:{goal_id}),
  peek/commit/admit semantics under backoff, bounded strike-map eviction, the
  shared blocked_goal_gate wiring in overseer/mod.rs, and the regression test list.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/blocked-goal-escalation-backoff.md
  - ./stable-goal-session-identity-api.md
---

# Reference: WhisperGate Exponential-Backoff API

> **Status: implemented (#4255).** Present-tense description of shipped
> behaviour. Primary sources:
> [`src/overseer/guardrails.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/guardrails.rs),
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> Conceptual overview:
> [Blocked-goal escalations back off exponentially](../concepts/blocked-goal-escalation-backoff.md).

## Constructors

```rust
impl WhisperGate {
    /// Fixed dedup window (unchanged, non-breaking).
    pub fn new(window_secs: i64, cap_per_hour: usize) -> Self;

    /// Per-signature EXPONENTIAL backoff. The effective suppression window for a
    /// signature grows `base_secs * 2^strikes`, capped at `cap_secs`. Each
    /// re-hit within the current window increments the signature's strike count.
    /// `cap_per_hour` is the rolling-hour delivery budget, as in `new`.
    pub fn with_backoff(base_secs: i64, cap_secs: i64, cap_per_hour: usize) -> Self;
}
```

`new(w, c)` is equivalent to a backoff gate whose window never grows
(`base == cap == w`); existing call sites are unaffected.

## Window formula

For a signature with `strikes` prior re-hits inside its window:

```
window(strikes) = min(base_secs * 2^strikes, cap_secs)
```

With `with_backoff(900, 14_400, 20)`: `900, 1800, 3600, 7200, 14400, 14400, …`
(seconds). The cap guarantees the signature re-surfaces at least every `cap_secs`.

## Signatures

The gate keys on the **existing literal per-goal signature strings** — this fix
does **not** change how signatures are formed, only how the gate ages them:

```rust
// src/overseer/mod.rs
let signature = format!("escalate:{goal_id}"); // act_escalate_blocked_goal
let signature = format!("unblock:{goal_id}");  // act_unblock_goal
```

Each `goal_id` (and each path prefix) has an independent backoff window. There is
no cluster/set hashing: distinct goals never share a window, and the `escalate:`
and `unblock:` prefixes keep the two paths' backoff independent for the same goal.

## Decide / commit semantics

`peek` / `commit` / `admit` keep their existing shapes and return
`WhisperDecision` (`Deliver`, `SuppressDuplicate`, `SuppressCapReached`). Under
backoff:

- `peek(signature, now)` returns `Deliver` when `now - last_delivered >=
  window(strikes)` **or** the signature is new; otherwise `SuppressDuplicate`
  (or `SuppressCapReached` if the rolling-hour budget is exhausted).
- `commit(signature, now)` records the delivery, and — if the previous delivery
  was within the prior window — increments `strikes` for that signature.
- A signature not seen for `> cap_secs` is eligible for eviction from the strike
  map (bounded memory), consistent with the gate's existing stale-entry pruning.

## Wiring

```rust
// src/overseer/mod.rs (constructor)
blocked_goal_gate: WhisperGate::with_backoff(900, 14_400, 20),  // was new(900, 20)
```

`blocked_goal_gate` is **shared by both** `act_escalate_blocked_goal`
(`escalate:{goal_id}`) and `act_unblock_goal` (`unblock:{goal_id}`), so backoff
applies to escalation and self-heal alike. The other gates — `whisper_gate`,
`write_back_gate`, and `gap_gate` — keep their fixed-window `new(…)` construction.

## Regression tests

| Test                                          | Asserts                                                    |
| --------------------------------------------- | ---------------------------------------------------------- |
| `backoff_delivers_once_then_suppresses`       | First hit delivers; subsequent in-window hits suppress.    |
| `backoff_window_doubles_per_strike`           | Windows follow `900 → 1800 → 3600 → …`.                    |
| `backoff_window_is_capped`                    | Window never exceeds `cap_secs` (14400).                   |
| `backoff_refires_after_cooldown`              | Signature re-fires once the window elapses.                |
| `backoff_is_per_signature_isolated`           | One goal's (or path's) backoff does not affect another.    |
| `escalate_and_unblock_prefixes_are_isolated`  | `escalate:{g}` and `unblock:{g}` back off independently.   |
| `new_gate_window_is_constant`                 | `WhisperGate::new` keeps a non-growing fixed window.       |
| `strike_map_is_bounded`                       | Stale signatures are evicted; memory stays bounded.        |
