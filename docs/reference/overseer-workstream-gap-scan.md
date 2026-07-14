---
title: Overseer workstream gap-scan reference
description: Observation-only detection, notification, and counters for uncovered work.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: reference
---

# Overseer workstream gap-scan reference

The scan surveys high-priority goals, typed high-signal issues, and live
anomalies for uncovered work. It creates no issue and no backlog item.

## Model

```rust
pub struct GapItem {
    pub provenance: ArtifactProvenance,
    pub category: GapCategory,
    pub ref_id: String,
    pub title: String,
    pub why_it_matters: String,
    pub signature: String,
}
```

Categories are `GoalUncovered`, `IssueUncovered`, and `AnomalyUnaddressed`.
Rendered fields are bounded to 120 characters and each pass returns at most 25
gaps.

Goal and issue candidates must have eligible typed provenance. Legacy goal-board
snapshots and GitHub issues without a trusted local provenance association remain
`LegacyUnknown` and are excluded. Signal conversion and Act recheck provenance,
so injected or reconstructed stewardship gaps cannot bypass the detector.

## Act behavior

`act_flag_workstream_gaps`:

1. rejects ineligible provenance;
2. applies the existing per-signature notification gate;
3. sends one consolidated operator notification when configured;
4. commits successfully notified signatures to the gate; and
5. returns `WorkstreamGapsFlagged { flagged, suppressed }`.

The current gate and gap list are runtime observation state; this page does not
claim durable gap persistence across restart. Recursive exclusion is durable
because goal-board provenance is serialized.

`workstream_gaps_detected` and `workstream_gaps_suppressed` are observation
counters. Neither contributes to `issues_filed`.

## Configuration

| Variable | Default | Meaning |
|---|---:|---|
| `SIMARD_OVERSEER_GAP_SCAN` | enabled | Enable observation and notification |
| `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` | `1` | Observe every Nth tick |

These settings never authorize GitHub mutation.

## Related

- [Review workstream gaps](../howto/review-overseer-workstream-gaps.md)
- [Stewardship issue safety](../stewardship-safety.md)
