---
title: Goal Stewardship Mode
description: Observation, agent-owned semantic decisions, recursive provenance exclusion, and bounded GitHub mutation.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: concept
---

# Goal Stewardship Mode

Goal Stewardship Mode turns eligible operational evidence into bounded,
auditable maintenance work. It does not treat every observation as a request to
write GitHub state.

The mode separates three concerns:

1. **Observation.** Sensors record typed facts with provenance. Routine
   workstream gaps remain observations and may notify the operator, but create
   no GitHub issue and no stewardship backlog item.
2. **Semantic decision.** Agentic OODA and consolidation recipes classify and
   group eligible evidence into versioned typed proposals.
3. **Safety enforcement.** Rust validates provenance, identifiers, stable
   agent-supplied condition identity, durable restart state, and the per-cycle
   autonomous GitHub-mutation limit.

Rust never decides semantic equivalence by parsing prose. Recipes provide
stable condition identities but never grant authority or choose mutation
digests.

## Feedback-loop boundary

Stewardship-created issues, board entries, gaps, and mutation records retain
typed `Stewardship` provenance. These artifacts are excluded from:

- goal and issue discovery;
- workstream-gap detection;
- backlog enqueue and promotion;
- rediscovery after restart;
- autonomous issue generation.

Legacy records without typed provenance are `LegacyUnknown` and fail closed;
they are not assumed to be safe external input.

## GitHub mutation

Autonomous GitHub issue, push, pull-request, label, review-request, and comment
writes pass through the durable mutation guard. Explicit operator actions are
outside the daemon-cycle boundary.
Reservations consume budget before GitHub is called. The guard replays
completed identities, fails closed on unfinished reservations after restart,
and fails the whole cycle before mutation `limit + 1`.

The default is one GitHub mutation per durable cycle. Restart resumes the same
cycle identity and consumed budget. Explicit operator actions outside the
daemon cycle use a separate invocation-bound path.

## Workstream gaps

The recurring workstream-gap scan answers "what important work is currently
uncovered?" It persists bounded observations, updates activity counters, and
may emit the existing consolidated operational notification. It does not file
or update an issue and does not seed the goal backlog.

This prevents the old feedback loop in which the act of observing a gap created
new work that could itself be rediscovered as a goal or another gap.

## Further reading

- [Stewardship issue safety](../stewardship-safety.md)
- [Stewardship mutation guard API](../reference/stewardship-mutation-guard.md)
- [File and consolidate stewardship issues safely](../howto/stewardship-filing-and-consolidation.md)
- [Overseer workstream gap-scan reference](../reference/overseer-workstream-gap-scan.md)
