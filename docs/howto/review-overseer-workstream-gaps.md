---
title: Review the Overseer's workstream gaps
description: Read and respond to observation-only workstream-gap telemetry.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: howto
---

# Review the Overseer's workstream gaps

The Overseer reports important work that has no active workstream or open pull
request. Gap observations are operational signals, not GitHub work items.

## Where gaps appear

- The existing email and Signal operator notification, when configured.
- The dashboard Overseer tab and TUI Overseer pane.
- The `OVERSEER` section of `simard status`.
- `GET /api/overseer` counters:
  `workstream_gaps_detected` and `workstream_gaps_suppressed`.

No stewardship issue or backlog row should appear merely because a gap was
observed.

## Interpret the counter

`workstream_gaps_detected` counts fresh eligible observations surfaced during
the tick. `workstream_gaps_suppressed` counts repeated observations held by the
dedup gate. Neither count means "issues filed."

## Respond to a gap

- Assign or start a workstream for an uncovered high-priority goal.
- Start work on a high-signal external issue that has no owner or pull request.
- Investigate an unaddressed anomaly and create operator-directed work when
  appropriate.
- Correct source provenance when a legitimate legacy artifact is
  `LegacyUnknown`; do not bypass the fail-closed classification.

Once active work or an open pull request covers the source, it leaves the gap
set.

## Confirm zero GitHub mutation

For a test or diagnostic cycle, inspect the Overseer report and mutation
journal together:

```text
workstream_gaps_detected: N
issues_filed: 0
new GitHub-mutation reservations for workstream_gap: 0
new stewardship backlog items for workstream_gap: 0
```

Any `workstream_gap:*` reservation, GitHub mutation, or stewardship
backlog insertion is a boundary violation.

## Tune observation cadence

```bash
export SIMARD_OVERSEER_GAP_SCAN=off
export SIMARD_OVERSEER_GAP_SCAN_EVERY_N=4
```

The first setting disables the scan. The second runs it every fourth Overseer
tick. These settings affect observation and notification only.

See the [workstream gap-scan reference](../reference/overseer-workstream-gap-scan.md)
for data and provenance contracts.
