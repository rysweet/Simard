---
title: Goal Stewardship API
description: Implemented failure routing, typed issue mutation, and backlog provenance contracts.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: reference
---

# Goal Stewardship API reference

Module: `simard::stewardship`

## Failure entrypoint

The crate-internal entrypoint is:

```rust
fn process_orchestrator_run(
    run: &OrchestratorRunSummary,
    gh: &dyn StewardshipGh,
    guard: &mut MutationGuard,
) -> SimardResult<StewardshipOutcome>;
```

`OrchestratorRunSummary` contains failure facts plus typed `condition_id`,
`cycle_id`, and source `provenance`. The function:

1. validates required fields;
2. rejects typed `ObservationOnly` evidence;
3. routes the source module to a repository;
4. submits one typed create request through `MutationGuard`; and
5. leaves `GoalBoard` unchanged.

The durable mutation journal, not a stewardship-created backlog item or remote
issue marker, records completion.

## Outcomes

```rust
pub enum StewardshipOutcome {
    FiledNew { repo, issue_number, url, signature },
    MatchedExisting { repo, issue_number, url, signature },
}
```

`MatchedExisting` means completed journal replay. Remote issue text is not
trusted as idempotency or authorization state.

## GitHub interfaces

`IssueMutationTransport` and `StewardshipGh` are crate-private. The real
transport supports typed create, edit, close, and reopen, but autonomous callers
invoke it only through `MutationGuard`.

## Stable identity

`IssueMutationIdentity` is supplied by the validated semantic producer and is
authoritative. A repeated create with the same repository, identity, and source
provenance replays the completed issue even when later observation text, issue
numbers, or generated goal slugs differ.

Remote issue text is never an adoption or restart-idempotency authority.

## Related

- [Issue mutation guard reference](stewardship-mutation-guard.md)
- [Stewardship issue safety](../stewardship-safety.md)
- [Safe filing and consolidation](../howto/stewardship-filing-and-consolidation.md)
