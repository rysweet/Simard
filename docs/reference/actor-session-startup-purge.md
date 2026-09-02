---
title: "Reference: Actor-session startup purge"
description: Contract for clearing transient typed-OODA actor-session leases at daemon startup while preserving durable ledger records and live scope enforcement.
last_updated: 2026-07-30
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./actor-session-scope-key-api.md
  - ./stable-goal-session-identity-api.md
  - ./ooda-capability-api.md
  - ../howto/run-ooda-daemon.md
  - ../daemon-mode.md
---

# Reference: Actor-session startup purge

The OODA daemon deletes persisted `actor_sessions` rows once during startup.
Actor-session leases represent actors that are active in the current daemon
process. No actor session remains in flight after that process exits, so carrying
these leases into a new process is invalid even when their `expires_at` value is
still in the future.

The purge lets a stable per-goal `session_id` bind to the identity and
authorization scope of the new daemon process after a restart, host migration,
state-root migration, or observe-only posture change. It does not relax the
runtime scope guard: changing the immutable scope of a session that is already
registered in the current process still returns
`AuthorizationScopeViolation`.

## Startup contract

`run_ooda_daemon()` must perform actor-session cleanup in this order:

1. Resolve the daemon state root.
2. Create the state-root directory.
3. Resolve the ledger with `typed_ooda::ledger_path(state_root)`.
4. Ensure the canonical ledger parent directory exists.
5. Open the typed-OODA ledger.
6. Call `CapabilityHandler::purge_actor_sessions()`.
7. Continue with the remaining daemon initialization and goal-cycle work.

The canonical ledger path is:

```text
<state-root>/typed-ooda/outcomes.sqlite3
```

The purge must run exactly once per `run_ooda_daemon()` invocation. It must not
run:

- in `CapabilityHandler::open()`;
- during schema initialization or migration;
- when an operator opens the ledger through another command;
- before or after each goal cycle;
- during actor-session registration.

Opening and purging the ledger are part of daemon startup. If either
operation fails, `run_ooda_daemon()` must return the error and must not begin
goal-cycle work. The daemon must never continue with stale actor-session state
after a failed purge.

## Internal ledger operation

The purge is crate-visible for daemon startup use. It is not part of the
public `CapabilityHandler` API for downstream consumers.

```rust
impl CapabilityHandler {
    /// Delete every transient actor-session lease.
    ///
    /// This operation is intended for authoritative daemon startup, when no
    /// actor session can still be in flight. It is idempotent and returns
    /// persistence failures through `CapabilityResult`.
    pub(crate) fn purge_actor_sessions(&self) -> CapabilityResult<()>;
}
```

The operation must execute one hard-coded SQL statement:

```sql
DELETE FROM actor_sessions
```

Deleting zero rows must succeed. SQLite open, lock, and execution errors must use
the existing typed-OODA persistence error conversion and be returned to the
caller. The method must not accept SQL, a table name, a state root, or a filter
from the caller.

The daemon owns the lifecycle decision. `CapabilityHandler` exposes the
fallible deletion primitive but does not call it automatically.

## Data lifecycle

`actor_sessions` is transient authentication state. Its rows contain the
current token hash, expiry, cycle metadata, actor identity, repository binding,
grants, engineer permissions, working-directory binding, and observe-only
posture for an in-flight actor.

All other typed-OODA ledger data must remain durable. The startup purge must not
delete or rewrite:

- `terminal_outcomes`;
- `progress_records`;
- `mutation_requests`;
- `mutation_scope_counters`;
- `effect_jobs`;
- `engineer_claims`;
- approvals, process executions, or authorization records;
- schema metadata.

Request replay records must be preserved. A fresh actor registration after
restart must therefore use a fresh `request_id`, as every new registration
already does.

The existing expiry sweep must remain in place:

```sql
DELETE FROM actor_sessions WHERE expires_at < ?1
```

That sweep removes expired rows during schema creation or migration and
ordinary session registration, not on every ledger initialization. The
startup purge has a different purpose: it removes all leases inherited from a
process that no longer exists, including future-dated leases.

## Scope enforcement after startup

The immutable actor-session scope key continues to contain:

| Field | Runtime behavior |
| --- | --- |
| Actor identity | A change on the same live `session_id` is rejected. |
| Repository | A change on the same live `session_id` is rejected. |
| Capability grants | A change on the same live `session_id` is rejected. |
| Engineer permissions | A change on the same live `session_id` is rejected. |
| Working directory | A change on the same live `session_id` is rejected. |
| Observe-only posture | A change on the same live `session_id` is rejected. |

`cycle_id`, `goal_id`, and the rotating token hash must remain outside the
immutable scope key. See the
[actor-session scope-key API](./actor-session-scope-key-api.md) for the complete
registration contract.

The target lifecycle boundary must produce these results:

| Situation | Result |
| --- | --- |
| Same process, same stable session ID, same immutable scope | Registration may refresh the lease. |
| Same process, same stable session ID, changed immutable scope | `AuthorizationScopeViolation`. |
| New daemon process, stale persisted lease, changed immutable scope | Startup deletes the stale row; the first new registration succeeds. |
| Startup cannot open or purge the ledger | Daemon startup fails before goal-cycle work. |

## Configuration

The purge must have no enable/disable switch, retention period, row limit, or
separate database setting.

| Input | Effect |
| --- | --- |
| Daemon state root | Selects the canonical ledger at `typed-ooda/outcomes.sqlite3`. |
| `SIMARD_STATE_ROOT` | Overrides the runtime state root when supported by the invoking command or service. |
| `SIMARD_HOME` | Supplies the installed service state root unless a deliberate state-root override is used. |
| `SIMARD_OBSERVE_ONLY` | Contributes to each new actor's immutable scope; after implementation, changing it across a restart is safe because startup clears prior-process leases. |
| Identity, repository, and working directory | Contribute to the new actor scope but do not change purge behavior. |

`drain.conf` and identity posture can affect whether the new daemon runs
observe-only, but neither may change the cleanup rule. Startup must always
remove prior-process actor sessions.

The lifecycle assumes one authoritative daemon process per state root.
Concurrent daemons sharing one state root are unsupported because one daemon's
startup could invalidate another daemon's live actor sessions.

## Usage examples

### Restart after changing observe-only posture

Set the intended service environment, then restart the daemon:

```bash
systemctl --user restart simard-ooda.service
journalctl --user -u simard-ooda.service -n 100 --no-pager
```

The new process clears actor sessions from its canonical typed-OODA ledger
before it starts a goal cycle. No manual SQLite deletion is required.

For a bounded foreground check against an explicit state root:

```bash
SIMARD_OBSERVE_ONLY=1 \
  simard ooda run --cycles=1 "$HOME/.simard"
```

The same startup cleanup must run before the bounded cycle.

### Move a state root to another host

After transferring the durable state root, start the daemon normally on the new
host:

```bash
simard ooda run --cycles=1 "$HOME/.simard"
```

Future-dated actor leases copied with `typed-ooda/outcomes.sqlite3` must be
discarded at startup. Durable outcomes, effect records, claims, and request
history must remain available.

Do not add an operator script that runs
`DELETE FROM actor_sessions`. The daemon startup lifecycle owns this cleanup and
surfaces failures through its normal startup error path.

## Regression contract

The persisted-SQLite regression test must use the real canonical ledger path
and prove all three required transitions:

1. Register a stable session ID with a future-dated lease under one immutable
   scope, then close that handler so the row is persisted.
2. Run the daemon startup purge and register the same session ID under a
   different scope, including a changed `observe_only` value. Registration
   succeeds because the prior-process row is gone.
3. Reuse the same running handler and attempt another scope change with a new
   request ID. Registration fails with
   `CapabilityErrorCode::AuthorizationScopeViolation`.

Unique request IDs are required at each registration so request replay cannot
mask the lifecycle behavior. The test uses a future expiry so the ordinary
expiration sweep cannot produce a false positive.

## Security properties

- **Startup-only authority:** production code must invoke the purge only from
  `run_ooda_daemon()` before any goal-cycle work.
- **Fail-visible startup:** ledger open and purge failures must abort startup.
- **Narrow deletion:** one fixed statement must delete only `actor_sessions`.
- **Live protection preserved:** the runtime scope-key comparison and
  `AuthorizationScopeViolation` must remain unchanged.
- **No secret logging:** startup must not log actor identities, token hashes,
  scope keys, or row contents.
- **Durable history preserved:** outcomes, requests, effects, claims, and other
  authorization history must survive daemon restarts.

## Related

- [Actor-session scope-key API](./actor-session-scope-key-api.md)
- [Stable goal-session identity API](./stable-goal-session-identity-api.md)
- [OODA capability API](./ooda-capability-api.md)
- [How to run the OODA daemon](../howto/run-ooda-daemon.md)
- [Daemon mode](../daemon-mode.md)
