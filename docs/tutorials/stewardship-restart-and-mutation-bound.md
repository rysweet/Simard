---
title: Stewardship restart and GitHub mutation bound
description: Understand completed replay, unfinished fail-closed behavior, and bound exhaustion.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: tutorial
---

# Stewardship restart and GitHub mutation bound

The regression tests use an isolated journal and fake transport; they do not
touch GitHub.

## Completed replay

1. Start `cycle-a` with limit `1`.
2. Execute create request identity `condition-a`.
3. The store persists reservation 1 before transport.
4. The fake transport returns issue 101.
5. The store persists `Completed`.
6. Reconstruct `MutationGuard` from the same path.
7. Execute the identical request.

The second call returns `AlreadyCompleted` and the transport count stays one.
The persisted mutation identity, not GitHub search, is authoritative.

## Unfinished reservation

The test persists a reservation and reconstructs the guard before any completion
record. The next call returns `StewardshipUnfinishedReservation` without
consulting remote issue text or performing another mutation. An operator must
reconcile the ambiguous outcome. A forgeable remote marker cannot suppress or
authorize daemon work.

## Bound exhaustion

With limit `1`, request A reserves and mutates. Request B may be another issue
write or a push/PR/label/comment write; it causes
`StewardshipMutationBudgetExceeded` before transport. The cycle's failed state
and consumed count are durable. Reconstructing the guard and trying request C
produces the same failure and leaves the external mutation count at one.

Reservations count even when transport fails. Duplicate completed replay does
not reserve again.

## Feedback-loop regression

The end-to-end filing tests assert that a stewardship issue never enters the
goal board:

```text
stewardship issue -> durable journal -> GoalBoard unchanged
```

Even if a stewardship-provenance active goal is reconstructed directly, gap
detection rejects it:

```text
stewardship goal -> attempted gap -> rejected -> zero issue mutation
```

This proves changing issue numbers or generated goal slugs cannot revive the
old feedback loop. Typed lineage remains the exclusion authority.

Run the focused tests:

```bash
cargo test --lib stewardship::tests_safety
cargo test --lib overseer::tests_gap_scan
```

Production stores the journal under the trusted Simard state root and defaults
to one autonomous GitHub mutation per cycle.
