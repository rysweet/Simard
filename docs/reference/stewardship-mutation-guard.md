---
title: Stewardship issue mutation guard reference
description: Typed contracts and one durable budget for autonomous GitHub issue mutations.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: reference
---

# Stewardship issue mutation guard reference

Modules:

```text
src/stewardship/types.rs
src/stewardship/mutation_store.rs
src/stewardship/mutation_guard.rs
src/stewardship/gh_client.rs
```

## Types

| Type | Contract |
|---|---|
| `LineageId` | Bounded structural lineage identifier |
| `CycleId` | Durable scheduled-cycle identity |
| `IssueMutationIdentity` | Stable authoritative idempotency key |
| `ArtifactProvenance` | Version, structural origin, and lineage ID |
| `IssueMutationLimit` | Finite `1..=100` reservation limit |
| `IssueMutationRequest` | Repository, identity, source provenance, and typed issue operation |
| `IssueMutation` | `Create`, `Edit`, `Close`, or `Reopen` |
| `IssueMutationOutcome` | `Completed` or durable `AlreadyCompleted` |

Typed IDs accept 1 through 200 characters from `[A-Za-z0-9._:/#-]`.
`ArtifactProvenance::default()` is `LegacyUnknown` and is ineligible.
Transport fields are also bounded: repository slug, title, body, label count and
length, and assignee count and length fail validation before reservation.

## Guard

```rust
let mut guard = MutationGuard::from_default_store();
guard.begin_cycle(cycle_id.clone(), IssueMutationLimit::configured()?)?;
let outcome = guard.execute(&cycle_id, &request, transport)?;
```

`execute` and the mutation transport are crate-private. Autonomous components
use existing guarded adapters rather than invoking transport directly.

The configured environment variable is:

```text
SIMARD_STEWARDSHIP_ISSUE_MUTATION_LIMIT
```

Default: `1`. Valid range: `1..=100`.

## Journal

The version-1 journal stores:

- cycle identity, configured limit, consumed reservations, and failure reason;
- the complete typed request and stable mutation identity;
- reservation, ambiguous, rejected, or completed state;
- created/updated issue outcome; and
- typed stewardship provenance for completed issue artifacts.

Writes reuse `persistence::persist_json`. The mutation store adds an exclusive
owner-only lock and refuses symlinks, non-regular files, wrong ownership,
group/other permissions, malformed JSON, and unsupported journal versions.

## State transitions

```text
new identity -> Reserved -> Completed
                       \-> Ambiguous
ineligible identity -> Rejected
completed identity in a healthy cycle -> AlreadyCompleted
unfinished issue write -> fatal UnfinishedReservation
```

There is no automatic retry from `Reserved` or `Ambiguous`. A transport error is
persisted as ambiguous because the remote outcome may be uncertain, and the
cycle is durably failed before another identity can reserve.
Once a cycle is failed, all executions in that cycle fail, including replay of
an identity completed before the failure.

## Errors

| Error | Meaning |
|---|---|
| `StewardshipInvalidMutation` | Invalid typed field, limit, cycle, or operation |
| `StewardshipProvenanceBlocked` | Source provenance is recursive or unknown |
| `StewardshipMutationBudgetExceeded` | The cycle attempted reservation `limit + 1` |
| `StewardshipMutationCycleFailed` | A prior fatal mutation outcome poisoned the cycle |
| `StewardshipUnfinishedReservation` | Restart found an ambiguous reservation requiring operator reconciliation |
| `StewardshipMutationIdentityConflict` | One identity was reused for a different request |
| `PersistentStoreIo` | Journal or lock safety, serialization, or atomic persistence failed |
| `StewardshipGhCommandFailed` | GitHub transport failed |

Every error is fatal to the owning autonomous cycle.

## Scope

The bound covers autonomous issue create/edit/close/reopen mutations. GitHub
reads, pull-request operations, and explicitly invoked operator actions are
excluded.
