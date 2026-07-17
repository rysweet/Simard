---
title: Typed-OODA goal sessions carry a stable identity so a completed goal reads back as done
description: Why a typed-OODA goal session now derives a deterministic, stable session_id from the goal identity instead of minting a fresh UUID per tick; how a stable, re-derivable session_id plus a new session-scoped terminal read (terminal_for_session) lets a later tick recognise that a goal-session already recorded a terminal and mark the goal done instead of re-surfacing it as blocked; why cycle_id stays per-cycle; and how the existing request-replay + UNIQUE(session_id, cycle_id) idempotency is preserved.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./blocked-goal-escalation-backoff.md
  - ../reference/stable-goal-session-identity-api.md
  - ./closed-loop-outcome-verification.md
---

# Typed-OODA goal sessions carry a stable identity

> **Status: implemented (issue #4197).** The typed-OODA goal-session route now
> derives a **deterministic `session_id` from the goal identity** instead of a
> fresh `Uuid::now_v7()` per tick. Combined with a new **session-scoped terminal
> read** (`terminal_for_session`), a later tick can recognise that the goal's
> session already recorded a terminal and mark the goal `done`. Primary sources:
> [`src/ooda_actions/advance_goal/typed_goal_session.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/typed_goal_session.rs)
> and
> [`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs).
> The `UNIQUE(session_id, cycle_id)` schema and the request-replay dedup are
> unchanged.

## The defect this fixes

Every tick, `advance_goal::typed_goal_session::run` mints a session id and a
cycle id for the goal-session invocation:

```rust
// BEFORE (#4197)
let cycle_number = lock_state(state).cycle_count;          // increments every cycle
let cycle_id   = format!("cycle-{cycle_number}-{}", goal.id);
let session_id = format!("ooda-{}", uuid::Uuid::now_v7()); // fresh, non-re-derivable
```

The session recipe writes its terminal outcome keyed by `(session_id, cycle_id)`,
and the within-invocation reader `terminal_for_cycle(session_id, cycle_id)`
confirms the recipe produced a durable terminal (see
[`executor.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/executor.rs)
and [`route.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/route.rs)).

That in-invocation check works. The problem is **cross-tick recognition**: because
`session_id` was a fresh UUID every tick, nothing on a *later* tick could
reconstruct the identity a previous tick's session ran under. There was no stable
handle to ask "has this goal's session already reached a terminal?", so a goal
that had effectively completed kept being re-planned and re-surfaced as
**blocked** — feeding the perpetual `recurring_blocked_goal` signal the Overseer
kept re-escalating (see
[Blocked-goal escalation backoff](./blocked-goal-escalation-backoff.md), #4255).

## The fix: a stable session id + a session-scoped read

Two coordinated changes:

**1. Derive `session_id` from the goal identity** so any tick for the same goal
reconstructs the same session handle:

```rust
// AFTER (#4197)
let session_id = derive_session_id(&goal.id);   // e.g. "ooda-goal-<goal.id>", stable + re-derivable
let cycle_id   = format!("cycle-{cycle_number}-{}", goal.id); // UNCHANGED — still per-cycle
```

**2. Add a session-scoped terminal read** on the ledger:

```rust
pub fn terminal_for_session(&self, session_id: &str)
    -> CapabilityResult<Option<TerminalOutcome>>;   // latest terminal for the session, any cycle
```

`cycle_id` **intentionally stays per-cycle**. Each tick must still record its own
terminal under a *fresh* `(session_id, cycle_id)` pair, because
`commit_terminal` calls `ensure_cycle_open`, which rejects a second terminal for
an already-recorded `(session_id, cycle_id)` with `TerminalAlreadyRecorded`.
Reusing one cycle id across ticks would break that invariant. Keeping `cycle_id`
per-cycle preserves it; making `session_id` stable is what makes the *session*
re-discoverable.

With a stable session id and a session-scoped read, cross-tick recognition
closes:

1. A goal session reaches a terminal state and records it under
   `(derive_session_id(goal.id), cycle_id)`.
2. On a later tick the executor re-derives the **same** `session_id` and calls
   `terminal_for_session(session_id)`. It gets the recorded terminal and marks
   the goal `done`.
3. The done goal is no longer emitted as `blocked`, so the Overseer stops
   re-escalating it.

The operator CLI benefits too: `simard ooda terminal status --session-id … `
now takes a **re-derivable** id, so an operator can check a goal-session's
terminal state without scraping a random UUID out of the logs.

## Idempotency is preserved (not newly added)

The terminal write is **already idempotent** — this fix does not add an
`ON CONFLICT` clause. `commit_terminal`
([`ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs))
guards writes at two existing layers:

- **Request replay** — `replay_request(request_id, "terminal", fingerprint)`
  short-circuits and returns the prior outcome if the same `request_id` is
  retried, so a retried terminal is a no-op. `request_id` is the
  `terminal_outcomes` PRIMARY KEY.
- **Cycle guard** — `ensure_cycle_open` enforces `UNIQUE(session_id, cycle_id)`,
  raising `TerminalAlreadyRecorded` if a *different* request tries to write a
  second terminal for the same cycle.

Making `session_id` stable does not weaken either guard: every tick still uses a
distinct `cycle_id` (so the UNIQUE key never collides across ticks), and retries
within a tick still dedup on `request_id`. The interaction is: **stable
`session_id` makes the session correlatable across ticks; the existing
`request_id`/`(session_id, cycle_id)` keys keep each write idempotent.**

> **Note on mutation budget.** Because the session id is now stable per
> goal-session rather than per tick, mutation-scope budget counters keyed off
> `session_id` are now scoped **per goal-session**, not per tick. This is the
> intended behaviour: a goal session's mutation budget spans its lifetime.

## Verifying the behaviour

Regression coverage lives beside the changed code:

- **Determinism** — `derive_session_id(goal.id)` returns the same value on
  repeated calls and for equal goal ids; distinct goals yield distinct ids; the
  result passes `validate_identifier` (including adversarial goal-id input).
- **Session-scoped read-back** — a session that records a terminal under
  `(derive_session_id(goal.id), cycle_id)` is found by
  `terminal_for_session(derive_session_id(goal.id))` on a later tick with a
  *different* `cycle_id`, and the goal is marked `done`.
- **No re-surfacing** — a goal recognised as terminal is no longer emitted as
  `blocked`.
- **Idempotency unchanged** — a retried terminal (same `request_id`) is a no-op;
  a conflicting second terminal for a cycle still raises `TerminalAlreadyRecorded`.

See the [reference doc](../reference/stable-goal-session-identity-api.md) for the
API surface and the full test list.
