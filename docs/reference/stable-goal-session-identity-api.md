---
title: "Reference: Stable Goal-Session Identity API"
description: >
  The API contract for deterministic typed-OODA goal-session identity: the
  derive_session_id(goal_id) helper, the per-cycle cycle_id (unchanged), the
  session-scoped terminal_for_session(session_id) read, the existing
  request-replay + UNIQUE(session_id, cycle_id) idempotency guards, the real
  terminal_outcomes columns, and the regression test list.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/stable-goal-session-identity.md
  - ./whisper-gate-backoff-api.md
---

# Reference: Stable Goal-Session Identity API

> **Status: implemented (#4197).** Present-tense description of shipped
> behaviour. Primary sources:
> [`src/ooda_actions/advance_goal/typed_goal_session.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/typed_goal_session.rs),
> [`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs),
> [`src/typed_ooda/schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs).
> Conceptual overview:
> [Typed-OODA goal sessions carry a stable identity](../concepts/stable-goal-session-identity.md).

## `derive_session_id`

```rust
/// Derive a deterministic, re-derivable session id for a typed-OODA goal
/// session from the goal identity. Stable across ticks and restarts; distinct
/// across distinct goals; passes `validate_identifier`.
fn derive_session_id(goal_id: &str) -> String;
```

Contract:

| Property        | Guarantee                                                        |
| --------------- | --------------------------------------------------------------- |
| Deterministic   | `derive_session_id(x) == derive_session_id(x)` for all `x`.     |
| Injective       | `a != b` ⟹ `derive_session_id(a) != derive_session_id(b)`.      |
| Valid           | Result satisfies `validate_identifier("session id", …)`.        |
| Not a secret    | It is a **correlation/idempotency key**; safe to log via trace. |

The goal-session route uses it in place of the former per-tick UUID. `cycle_id`
is **unchanged** — it stays per-cycle so each tick records under a fresh
`(session_id, cycle_id)` pair:

```rust
let cycle_number = lock_state(state).cycle_count;
let cycle_id   = format!("cycle-{cycle_number}-{}", goal.id); // unchanged, per-cycle
let session_id = derive_session_id(&goal.id);                 // was: format!("ooda-{}", Uuid::now_v7())
```

## `terminal_outcomes` schema (unchanged)

No migration is required. The existing table
([`schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs)):

```sql
CREATE TABLE IF NOT EXISTS terminal_outcomes (
    request_id   TEXT PRIMARY KEY,
    request_hash TEXT NOT NULL,
    session_id   TEXT NOT NULL,
    cycle_id     TEXT NOT NULL,
    outcome_id   TEXT NOT NULL UNIQUE,
    outcome_json BLOB NOT NULL,
    UNIQUE(session_id, cycle_id)
);
```

There is **no `goal_id` column**; the goal is carried inside `outcome_json` and
by the derived `session_id`. The writer is the free fn `insert_terminal`, called
from `commit_terminal`.

## Session-scoped read

```rust
/// Latest terminal outcome recorded under `session_id`, across any cycle.
/// Enables cross-tick recognition once `session_id` is re-derivable.
pub fn terminal_for_session(&self, session_id: &str)
    -> CapabilityResult<Option<TerminalOutcome>>;
```

Called with the **re-derived** `session_id`, this lets the done-gate mark the
goal `done` on a later tick without knowing the exact `cycle_id` the terminal was
recorded under. The existing per-cycle and per-request reads are retained:

```rust
pub fn terminal_for_cycle(&self, session_id: &str, cycle_id: &str)
    -> CapabilityResult<Option<TerminalOutcome>>;   // within-invocation durability check
pub fn terminal_for_request(&self, request_id: &str)
    -> CapabilityResult<Option<TerminalOutcome>>;
```

## Idempotency guards (existing, unchanged)

The terminal write is idempotent through two existing layers in `commit_terminal`
— **no new `ON CONFLICT` clause is added**:

- **Request replay** — `replay_request(request_id, "terminal", fingerprint)`
  returns the prior outcome for a retried `request_id` (PRIMARY KEY), so a retry
  is a no-op.
- **Cycle guard** — `ensure_cycle_open` enforces `UNIQUE(session_id, cycle_id)`
  and raises `TerminalAlreadyRecorded` if a different request tries to write a
  second terminal for the same cycle.

Because `session_id` is stable but `cycle_id` remains per-cycle, the
`(session_id, cycle_id)` key never collides across ticks; stability only makes
the session correlatable, it does not change the write path.

## Regression tests

| Test                                            | Asserts                                                       |
| ----------------------------------------------- | ------------------------------------------------------------ |
| `derive_session_id_is_deterministic`            | Same goal id ⟹ same session id across calls.                 |
| `derive_session_id_is_injective`                | Distinct goal ids ⟹ distinct session ids.                    |
| `derive_session_id_passes_validate_identifier`  | Result is a valid identifier, incl. adversarial goal input.  |
| `terminal_for_session_finds_prior_tick`         | A terminal recorded one tick is found by `terminal_for_session` on a later tick with a different `cycle_id`. |
| `terminal_reads_back_marks_goal_done`           | A goal whose session recorded a terminal is marked `done`.   |
| `terminal_not_resurfaced_as_blocked`            | A done goal is no longer emitted as `blocked`.               |
| `terminal_retry_same_request_id_is_noop`        | Retrying a terminal (same `request_id`) returns the prior outcome. |
| `second_terminal_same_cycle_is_rejected`        | A conflicting terminal for a recorded cycle raises `TerminalAlreadyRecorded`. |
