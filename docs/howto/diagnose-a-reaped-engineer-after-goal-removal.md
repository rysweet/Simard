---
title: "How-to: diagnose a reaped engineer after goal removal"
description: >
  Confirm that removing or completing a goal reaps its already-dispatched
  in-flight engineer on the next OODA cycle instead of letting it run on and
  produce unwanted PRs. Covers reproducing the scenario, reading the
  fail-visible daemon log line, verifying the engineer_worktrees entry and its
  worktree are gone, and confirming that healthy / Blocked engineers are NOT
  reaped.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/tombstoned-goal-engineer-reaper.md
  - ../reference/tombstoned-goal-engineer-reaper-api.md
  - ./inspect-and-clean-engineer-worktrees.md
  - ./diagnose-leaked-engineer-claims.md
  - ../reference/subagent-tmux-tracking.md
---

# Diagnose a reaped engineer after goal removal

> **Status: implemented.** The per-cycle reconciliation described here ships in
> [`src/ooda_actions/advance_goal/subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs)
> (`reap_engineers_for_tombstoned_goals`) and runs from the daemon loop in
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs).
> Background:
> [Tombstoned-Goal Engineer Reaper](../concepts/tombstoned-goal-engineer-reaper.md).

## Symptom

You removed or completed a goal, but its engineer *used to* keep running for
tens of minutes afterward — burning compute and opening PRs for a goal that no
longer exists. With the reaper in place, that engineer is now terminated on the
**next OODA cycle**. This page shows how to verify that.

## Expected behaviour

| Action | What happens to the in-flight engineer |
|---|---|
| `simard goal remove <id>` | Goal tombstoned → engineer **reaped** next cycle (SIGTERM + worktree cleaned) |
| `simard goal complete <id>` | Goal tombstoned → engineer **reaped** next cycle |
| Goal still Active | Engineer **kept** (never reaped) |
| Goal Blocked but still on board | Engineer **kept** (Blocked is not a tombstone) |

The reap is **not** a wall-clock timeout — a healthy engineer whose goal is
still on the board is never killed, no matter how long it runs.

## Step 1 — Identify the in-flight engineer

List the goals with a live engineer and note the `goal_id` you intend to
remove:

```bash
simard status            # shows active goals and in-flight engineers
```

Cross-check the subagent-session registry, which maps `goal_id → {session_name,
pid}` (see [Subagent tmux tracking](../reference/subagent-tmux-tracking.md)):

```bash
cat "$SIMARD_STATE_ROOT/state/subagent_sessions.json" | jq '.sessions[] | {goal_id, session_name, pid, ended_at}'
```

You should see a row for your target `goal_id` with `ended_at: null` and a live
`pid`, plus a matching worktree directory under
`"$SIMARD_STATE_ROOT"/engineer-worktrees/`. That **live** row (not any older
`ended_at`-set retry row) is the one the reaper targets for SIGTERM.

## Step 2 — Remove (or complete) the goal

```bash
simard goal remove <goal_id>
# or
simard goal complete <goal_id>
```

Both verbs write a durable **tombstone** for `<goal_id>` (via `tombstone_goals`,
[`src/ooda_loop/curate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/curate.rs)),
which is the single signal the reaper acts on. Confirm the tombstone landed:

```bash
cat "$SIMARD_STATE_ROOT/goal_tombstones.json" | jq .   # <goal_id> should be present
```

## Step 3 — Watch the next OODA cycle reap it

The reconciliation runs each cycle, right after the board is reloaded and
tombstone-filtered. On the first cycle after the tombstone is written, the
daemon log emits one fail-visible line:

```bash
tail -f "$SIMARD_STATE_ROOT"/ooda.log | grep -i "reaped"
```

Expect:

```
[simard] OODA cycle: reaped 1 in-flight engineer(s) for tombstoned goal(s): <goal_id>
```

The engineer receives **SIGTERM** (graceful — never `kill -9`) via the existing
`kill_subordinate` primitive.

## Step 4 — Verify the engineer and its worktree are gone

```bash
# The in-memory tracking entry is dropped; its worktree dir is removed.
ls "$SIMARD_STATE_ROOT"/engineer-worktrees/ | grep <short_goal_id>   # → no output

# The process is gone (SIGTERM delivered; ESRCH is treated as already-exited).
ps -p <pid>                                                          # → no such process
```

The engineer claim for the goal is also released through the standard
`release_engineer_claim` chokepoint, so the ledger slot is freed. No new PRs are
produced for the removed goal after this point.

## Step 5 — Confirm healthy / Blocked engineers are NOT reaped

This is the important negative check — the reaper must never kill a producing
engineer whose goal still exists:

- **Active goal:** an engineer for a goal you did **not** remove keeps running;
  its `engineer_worktrees` entry and worktree survive every cycle.
- **Blocked goal:** a goal parked in Blocked state (`[OODA-SAFEGUARD]` or a
  dependency block) is **not** tombstoned, so its engineer is **kept**. Blocked
  is "temporarily off the active list but still on the board", never "gone".

If you see a Blocked or Active goal's engineer being reaped, that is a bug — the
reap predicate keys **only** on tombstone membership, never on absence from the
active list.

## Troubleshooting

| Observation | Likely cause / action |
|---|---|
| No `reaped` line after removal | Confirm the tombstone was written (Step 2). The reap fires on the **next** cycle — give it one cycle. |
| Engineer gone but worktree dir remains | Cleanup logs a warning and `Drop` runs as a safety net; check `ooda.log` for the `engineer_worktree cleanup failed` warning, then use [inspect-and-clean-engineer-worktrees](./inspect-and-clean-engineer-worktrees.md). |
| SIGTERM "skipped" in logs | The registry had no matching `goal_id` row (e.g., session GC'd). This is expected — worktree cleanup still runs authoritatively; the process, if any, is caught by the [stale-claim reaper](./diagnose-leaked-engineer-claims.md). |
| A still-present goal's engineer was reaped | Report as a bug — only tombstoned goals should reap. Capture `goal_tombstones.json` and the `ooda.log` reap line. |

## Related

- [Tombstoned-Goal Engineer Reaper (concept)](../concepts/tombstoned-goal-engineer-reaper.md)
- [Tombstoned-Goal Engineer Reaper API](../reference/tombstoned-goal-engineer-reaper-api.md)
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md)
- [Diagnose leaked engineer claims](./diagnose-leaked-engineer-claims.md)
- [Subagent tmux tracking](../reference/subagent-tmux-tracking.md)
