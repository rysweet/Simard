---
title: "Reference: Actor-Session Scope-Key API"
description: >
  The API contract for the actor-session identity/authorization-scope guard in
  the typed-OODA ledger: the ActorScopeKey value type over the six scope fields,
  ActorBinding::scope_key(), the narrowed AuthorizationScopeViolation guard in
  register_actor_session that compares scope keys (not per-cycle metadata), why
  re-leasing a stable session_id with a new cycle_id is legitimate, the
  unchanged actor_sessions upsert, and the regression test list. Fixes the
  false AuthorizationScopeViolation crash-loop on PERPETUAL/STANDING goals.
last_updated: 2026-07-19
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./stable-goal-session-identity-api.md
  - ./ooda-capability-api.md
  - ../concepts/stable-goal-session-identity.md
  - ../architecture/typed-ooda-loop.md
---

# Reference: Actor-Session Scope-Key API

> **Status: implemented (#4197 follow-up).** Present-tense description of shipped
> behaviour. Primary source:
> [`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs).
> Related identity contract:
> [Stable Goal-Session Identity API](./stable-goal-session-identity-api.md);
> conceptual overview:
> [Typed-OODA goal sessions carry a stable identity](../concepts/stable-goal-session-identity.md).

## Contents

- [Background: the perpetual-goal crash-loop](#background-the-perpetual-goal-crash-loop)
- [`ActorScopeKey`](#actorscopekey)
- [`ActorBinding::scope_key`](#actorbindingscope_key)
- [The narrowed guard in `register_actor_session`](#the-narrowed-guard-in-register_actor_session)
- [`actor_sessions` schema and upsert (unchanged)](#actor_sessions-schema-and-upsert-unchanged)
- [What is intentionally NOT relaxed](#what-is-intentionally-not-relaxed)
- [Security invariants](#security-invariants)
- [Regression tests](#regression-tests)

## Background: the perpetual-goal crash-loop

Since #4197, a typed-OODA goal session derives a **stable** session id from the
goal identity — `session_id = derive_session_id(&goal.id)` = `ooda-<sha256(goal_id)[..16]>`
([`typed_goal_session.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/typed_goal_session.rs)).
The `cycle_id` remains **per-cycle**: `cycle_id = format!("cycle-{cycle_number}-{}", goal.id)`
changes on every tick.

Actor-session leases in the typed-OODA ledger last 30 days
([`route.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/route.rs),
`ACTOR_SESSION_LEASE = Duration::from_secs(30 * 24 * 60 * 60)`).
`register_actor_session` only `DELETE`s **expired** rows, so a live lease never
clears between the ~7-minute cycle retries of a running goal.

The `AuthorizationScopeViolation` guard previously compared the **entire**
`ActorBinding`, including the per-cycle `cycle_id`. On re-lease:

1. Cycle _N_ stores a binding with `cycle_id = cycle-N-<goal>`.
2. Cycle _N+1_ derives the **same** `session_id` but computes `cycle_id = cycle-(N+1)-<goal>`.
3. The stored and new bindings differ **only** in `cycle_id` (and the 1:1
   `goal_id`), so the whole-struct equality check tripped and returned:
   `actor session is already bound to a different identity or authorization scope`.

Single-shot goals finish on cycle 1 and never re-lease, so they were unaffected.
**PERPETUAL/STANDING goals** re-lease the same stable `session_id` every cycle and
were locked out permanently — producing a long run of consecutive
`ToolFailed: actor session is already bound to a different identity or
authorization scope` failures while the goal board still showed the goal as
`not-started`. The field-report specifics behind this write-up — the affected
goal `continuously-research-and-improve-your-own-cogn-70ab8541`, the **265+
consecutive** failures, and the ~7-minute retry cadence — come from the
[#4197](https://github.com/rysweet/Simard/issues/4197) reproduction and are
background context, not code-verifiable invariants.

The fix narrows the guard to compare **only** identity/authorization scope, not
per-cycle lease metadata. Re-leasing the same session for a new cycle of the
**same** identity and scope is legitimate and is already handled by the existing
`ON CONFLICT(session_id) DO UPDATE` upsert.

## `ActorScopeKey`

`ActorScopeKey` is the single source of truth for actor-session identity +
authorization-scope equality. It derives `Eq`/`PartialEq` over **exactly** the
six scope fields and nothing else:

```rust
/// The identity + authorization-scope key of an actor session.
///
/// Equality over this type — and ONLY this type — decides whether a re-lease of
/// an existing `session_id` is the same authenticated actor at the same
/// authorization scope (allowed) or a genuine identity/scope change (rejected as
/// `AuthorizationScopeViolation`).
///
/// It deliberately EXCLUDES per-cycle lease metadata (`cycle_id`, `goal_id`) and
/// the rotating secret (`token_hash`): those are refreshed on every legitimate
/// re-lease and are not part of the actor's identity or scope.
#[derive(Debug, Eq, PartialEq)]
struct ActorScopeKey {
    actor_identity: String,
    repository_json: Vec<u8>,
    grants_json: Vec<u8>,
    engineer_permissions_json: Vec<u8>,
    working_directory_json: Option<Vec<u8>>,
    observe_only: bool,
}
```

Contract:

| Property           | Guarantee                                                                                 |
| ------------------ | ----------------------------------------------------------------------------------------- |
| Complete           | Contains **all six** authorization-scope fields; a change to any one changes the key.     |
| Minimal            | Contains **only** scope fields — never `cycle_id`, `goal_id`, or `token_hash`.            |
| Deterministic      | Byte-for-byte comparison of the same serde serializer path; stable across calls/ticks.    |
| Fail-closed        | Only an **equal** key admits a re-lease; any inequality raises `AuthorizationScopeViolation`. |

> **Serialization note.** The `*_json` fields are the exact bytes produced by the
> `ActorBinding::new` serde path (`serde_json::to_vec`). Both sides of the
> comparison are serialized identically, so byte-level `Eq` is deterministic. Do
> **not** substitute normalized/string comparison — it would widen the trust
> boundary without benefit.

## `ActorBinding::scope_key`

`ActorBinding` remains the row-carrier for a persisted actor session — it keeps
`cycle_id` and `goal_id` for `into_context` and lease resolution. It **no longer
derives `Eq`/`PartialEq`** (only `Debug`); equality is expressed solely through
its scope key:

```rust
#[derive(Debug)] // was: #[derive(Debug, Eq, PartialEq)]
struct ActorBinding {
    cycle_id: String,   // per-cycle lease metadata — NOT part of identity/scope
    goal_id: String,    // 1:1 with the stable session_id — NOT part of identity/scope
    actor_identity: String,
    repository_json: Vec<u8>,
    grants_json: Vec<u8>,
    engineer_permissions_json: Vec<u8>,
    working_directory_json: Option<Vec<u8>>,
    observe_only: bool,
}

impl ActorBinding {
    /// Project this binding onto its identity + authorization-scope key,
    /// dropping per-cycle lease metadata (`cycle_id`, `goal_id`).
    fn scope_key(&self) -> ActorScopeKey {
        ActorScopeKey {
            actor_identity: self.actor_identity.clone(),
            repository_json: self.repository_json.clone(),
            grants_json: self.grants_json.clone(),
            engineer_permissions_json: self.engineer_permissions_json.clone(),
            working_directory_json: self.working_directory_json.clone(),
            observe_only: self.observe_only,
        }
    }
}
```

`ActorBinding::new`, `into_context`, `actor_binding_from_row`, and the
`load_actor_*` helpers are **unchanged**.

## The narrowed guard in `register_actor_session`

The only behavioural change to the write path is the equality check. It compares
scope keys instead of whole bindings:

```rust
let existing_binding = load_actor_binding(&transaction, &actor.session_id)?;
if existing_binding
    .as_ref()
    .is_some_and(|existing| existing.scope_key() != binding.scope_key())
{
    return Err(CapabilityError::new(
        CapabilityErrorCode::AuthorizationScopeViolation,
        "actor session is already bound to a different identity or authorization scope",
    ));
}
```

Semantics:

| Re-lease of an existing `session_id`                                            | Result                                                                 |
| ------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| Same identity + scope, **different `cycle_id`** (perpetual goal, next cycle)    | **Allowed.** Upsert refreshes `cycle_id`/`goal_id`/`token_hash`/`expires_at`. |
| Changed `actor_identity`                                                         | Rejected — `AuthorizationScopeViolation`.                             |
| Changed `repository`                                                             | Rejected — `AuthorizationScopeViolation`.                             |
| Escalated/altered `grants`                                                       | Rejected — `AuthorizationScopeViolation`.                             |
| Changed `engineer_permissions`                                                  | Rejected — `AuthorizationScopeViolation`.                             |
| Changed `working_directory`                                                     | Rejected — `AuthorizationScopeViolation`.                             |
| Flipped `observe_only`                                                           | Rejected — `AuthorizationScopeViolation`.                             |

The error message is intentionally **generic** — it never leaks scope-field
values or identities.

> **Two distinct guards in one function.** `register_actor_session` also contains
> an earlier, unrelated `AuthorizationScopeViolation` check that validates the
> **incoming** actor context's own `bound_cycle_id`/`bound_goal_id` against the
> requested `cycle_id`/`goal_id` (self-consistency of the caller —
> `"actor registration target does not match its existing cycle and goal binding"`).
> That guard is **unchanged**. Only the comparison against the **persisted** row
> — shown above — is narrowed to a scope-key comparison. Do not conflate the two.

## `actor_sessions` schema and upsert (unchanged)

No migration is required. The table
([`schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs))
is keyed on `session_id`:

```sql
CREATE TABLE IF NOT EXISTS actor_sessions (
    session_id TEXT PRIMARY KEY,
    cycle_id TEXT NOT NULL,
    goal_id TEXT NOT NULL,
    actor_identity TEXT NOT NULL,
    repository_json BLOB NOT NULL,
    grants_json BLOB NOT NULL,
    engineer_permissions_json BLOB NOT NULL DEFAULT X'5b5d',
    working_directory_json BLOB,
    observe_only INTEGER NOT NULL,
    token_hash TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
```

The existing upsert already refreshes the mutable columns on a legitimate
re-lease — the narrowed guard simply lets that path run for a new cycle:

```sql
INSERT INTO actor_sessions(...) VALUES (...)
ON CONFLICT(session_id) DO UPDATE SET
    cycle_id=excluded.cycle_id,
    goal_id=excluded.goal_id,
    actor_identity=excluded.actor_identity,
    repository_json=excluded.repository_json,
    grants_json=excluded.grants_json,
    engineer_permissions_json=excluded.engineer_permissions_json,
    working_directory_json=excluded.working_directory_json,
    observe_only=excluded.observe_only,
    token_hash=excluded.token_hash,
    expires_at=excluded.expires_at
```

Each re-lease mints a fresh token (`Uuid::new_v4`) and overwrites `token_hash`,
so token rotation is preserved.

## What is intentionally NOT relaxed

- **`authenticate_actor_session`** remains strict on `cycle_id`, `goal_id`,
  `token_hash`, and `expires_at`. Because the upsert refreshes those columns on
  re-lease, a stale cycle's token cannot authenticate against the current row.
- **The append-only terminal guard** — `terminal_outcomes UNIQUE(session_id, cycle_id)`
  in [`schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs)
  and `ensure_cycle_open` — is untouched. See the
  [Stable Goal-Session Identity API](./stable-goal-session-identity-api.md).
- **The other `AuthorizationScopeViolation` raise sites** — six elsewhere in
  `ledger.rs` (`validate_process_execution` ×2, `validate_actor_target`,
  `authorize_engineer_scope`, `validate_action`, and the sibling
  incoming-context cycle/goal guard inside `register_actor_session` itself) plus
  one in `types.rs` (the capability-policy allowlist check) — and the
  `validate_identifier` trust boundary are untouched.
- **Request-replay idempotency** (`replay_request(request_id, "actor_session", fingerprint)`)
  is untouched; a retried `request_id` still returns the prior lease.

## Security invariants

1. **Fail-closed.** Only an equal `ActorScopeKey` admits a re-lease. Any change
   to any of the six scope fields on the same `session_id` still yields
   `AuthorizationScopeViolation`.
2. **Key completeness.** `ActorScopeKey` MUST contain all six scope fields and
   MUST NOT contain `token_hash`, `cycle_id`, or `goal_id` (identity is neither a
   secret nor lease metadata).
3. **No secret leakage.** The violation message is generic; no field values or
   identities appear in errors or logs. `session_id` remains a correlation key,
   safe to trace.
4. **Token rotation preserved.** Each legitimate re-lease overwrites `token_hash`
   with a fresh token.

## Regression tests

Added to the `typed_ooda::ledger` test module:

| Test                                                     | Asserts                                                                                                              |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `reregister_same_scope_new_cycle_succeeds`               | Two `register_actor_session` calls, **same** `session_id` + identity/scope, **different** `cycle_id` → both `Ok`; the persisted row's `cycle_id`/`goal_id` (and `token_hash`/`expires_at`) are refreshed to the second cycle. |
| `reregister_changed_identity_scope_still_violates`       | Same `session_id`, a **changed** scope field (different `actor_identity`, or `repository`, or escalated `grants`) → still `AuthorizationScopeViolation`. |
| `perpetual_goal_reentry_across_cycles_no_binding_error`  | A perpetual goal re-entering the typed goal-session across two consecutive cycles (`cycle-1-<goal>` → `cycle-2-<goal>`, same derived `session_id`) no longer fails with the identity-binding error. |

Together these lock in the fix (tests 1 and 3) and prove the security semantics
are preserved (test 2).
