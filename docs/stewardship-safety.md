---
title: Stewardship issue mutation safety
description: Typed recursive exclusion, durable restart idempotency, and the autonomous issue-mutation bound.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: concept
---

# Stewardship issue mutation safety

Autonomous GitHub issue creates, edits, closes, and reopens are transactions.
They consume one durable per-cycle issue-mutation budget. Pull-request
operations and explicitly invoked operator actions are outside this boundary.

The safety contract is:

1. Routine workstream-gap observation creates no GitHub issue or stewardship
   backlog item. Existing operator notifications and counters remain.
2. Typed `Stewardship` and `LegacyUnknown` provenance is rejected from goal,
   backlog, gap, and issue-mutation inputs.
3. Agentic recipes own semantic consolidation and stable condition naming.
   Rust validates identifiers, provenance, persistence, and limits; it does not
   infer semantic equivalence from prose.
4. A stable `IssueMutationIdentity` and its durable journal record are
   authoritative across restart. Remote issue text is never adopted as state.
5. Reservation `limit + 1` is persisted as cycle failure before any external
   mutation, and callers propagate the error.

## Provenance

`ArtifactProvenance` is versioned and has one of five structural origins:

| Origin | Autonomous recursive input |
|---|---:|
| `Operator` | eligible |
| `System` | eligible |
| `External` | eligible |
| `Stewardship` | rejected |
| `LegacyUnknown` | rejected |

Missing legacy fields deserialize to `LegacyUnknown`. Goal-board provenance is
stored with the board snapshot. Promotion, in-flight discovery, blocked-goal
discovery, gap detection, signal conversion, and final issue authorization all
recheck the shared eligibility rule.

## Observation-only gaps

The Overseer still detects bounded `GapItem` values, emits one consolidated
`Signal::WorkstreamGap`, updates dedicated counters, and may notify configured
operator channels. `act_flag_workstream_gaps` never calls `IssueFiler` and never
enqueues a backlog item.

## Durable issue boundary

`stewardship::mutation_guard::MutationGuard` accepts typed
`IssueMutationRequest` values. The low-level issue transport remains
crate-private, so autonomous issue mutations use guarded adapters rather than
calling GitHub directly.

The store is:

```text
$SIMARD_STATE_ROOT/state/stewardship-issue-mutations.json
```

Each transaction acquires an owner-only advisory lock, loads the journal with
no symlink following, validates schema/ownership/mode, and atomically writes via
same-directory temporary file, fsync, rename, and parent-directory fsync.
Repository, title, body, labels, assignees, and typed identifiers are bounded
before reservation.

The guard:

1. validates request and provenance;
2. returns a persisted completed result for the same identity;
3. fails closed on any unfinished reservation;
4. reserves and consumes budget atomically;
5. performs one transport operation; and
6. atomically records completion or an ambiguous failure.

An unresolved reservation never retries and remote markers never prove
completion. Issue writes replay only a persisted completion result. Unfinished
reservations require visible operator reconciliation. The journal remains
authoritative.

## Cycle bound

`SIMARD_STEWARDSHIP_ISSUE_MUTATION_LIMIT` defaults to `1` and accepts only
integers from `1` through `100`. There is no unlimited value. A reservation
counts before transport and remains consumed after failure or restart.
The broader `SIMARD_STEWARDSHIP_GITHUB_MUTATION_LIMIT` name remains a
compatibility fallback when the issue-specific variable is unset.

Starting the same typed `CycleId` reuses its persisted count. Starting another
scheduled cycle creates another durable budget record; it does not erase prior
records.

Scheduled components use `SIMARD_SCHEDULED_CYCLE_ID` as the explicit restart
token. Without it, they remain in a conservative `current` cycle indefinitely;
wall-clock changes never reset budget.

Ephemeral runners set `SIMARD_REQUIRE_EXISTING_MUTATION_JOURNAL=1`. A cache miss
then fails before reservation or GitHub access and requires restoration of a
trusted reconciled journal; it never treats missing state as permission to
write or bootstraps from forgeable remote text. After operator reconciliation,
`initialize_journal_only=true` creates and caches state in a mutation-free run;
the scan step is skipped.

## Semantic boundary

`prompt_assets/simard/recipes/issue-consolidation.yaml` classifies eligible
observations and returns typed decisions with a stable condition ID. It does not
provide provenance, authorization, mutation identity, cycle identity, budgets,
or retries. OODA Orient and Decide receive only already-admitted goals and do
not synthesize issue proposals.

## Related documentation

- [Mutation guard reference](reference/stewardship-mutation-guard.md)
- [Filing and consolidation guide](howto/stewardship-filing-and-consolidation.md)
- [Restart and mutation-bound tutorial](tutorials/stewardship-restart-and-mutation-bound.md)
- [Issue cleanup runbook](operations/stewardship-issue-cleanup.md)
