---
title: Unblock OODA goals stuck after a brain-failure lockout
description: Runbook for clearing goals marked Blocked by the deterministic brain-failure safeguard, plus the auto-recovery behaviour introduced in issue #1911.
last_updated: 2026-05-18
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/simard-cli.md
  - ./recover-goal-board.md
  - ./run-ooda-daemon.md
  - ./spawn-engineers-from-ooda-daemon.md
---

# Unblock OODA goals stuck after a brain-failure lockout

## Symptom

The OODA daemon stops dispatching engineers even though `journalctl
--user -u simard-ooda.service` shows the brain producing confident
`advance_goal` decisions. Every cycle reports `0/N` successful actions.
Goals on the board show `status = Blocked` with a reason text that
starts with `🔒 [OODA-SAFEGUARD] OODA brain failing for N consecutive
cycles; needs human review` (the deterministic safeguard's marker).

This is the production lockout fixed by issue #1911. The brain itself
is healthy; the dispatcher was reading the persisted marker and short-
circuiting before consulting the brain.

## Automatic recovery (no operator action needed)

As of #1911, `dispatch_advance_goal` includes an auto-recovery branch:
when the persisted `Blocked` reason matches the brain-failure marker
(`is_brain_failure_marker`), the dispatcher

1. clears the goal's failure counter,
2. restores `GoalProgress::NotStarted`, and
3. falls through to normal session-based dispatch.

The next healthy cycle that touches a marker-blocked goal will heal it.
Operator intervention is only required when the daemon is offline (so
the auto-recovery branch never runs) or when an operator wants
immediate manual override.

> **Scope**: only the safeguard's sentinel-bearing marker triggers
> auto-recovery. Operator-set, scope-blocked, dependency-blocked, and
> subordinate-blocked goals continue to short-circuit dispatch — they
> are explicitly out of scope so the system never overrides intentional
> operator holds.

## Manual recovery via the CLI

### List the board

```bash
simard goal list
```

Tab-separated, one row per active goal. Inspect the `STATUS` column for
`blocked: 🔒 [OODA-SAFEGUARD] …` entries.

### Bulk-clear safeguard markers (preferred)

```bash
simard goal unblock-all
```

Scoped narrowly to the brain-failure marker. Operator-set Blocked
goals are left untouched, so the command is safe to rerun whenever you
suspect a recurrence. The stderr summary reports the number of cleared
markers vs. the number of non-marker Blocked goals it skipped.

### Clear a single goal unconditionally

```bash
simard goal unblock <goal-id>
```

The single-id form is an unconditional override — it clears `Blocked`
to `NotStarted` regardless of the reason text. Use this when an
operator has decided a specific goal (including operator-set holds) is
unstuck.

## Production recovery sequence (full)

When you arrive at a stuck daemon, run:

```bash
# 1. Inspect (no mutation).
simard goal list

# 2. Bulk-clear safeguard markers. Idempotent.
simard goal unblock-all

# 3. (Re)start the daemon so it reloads the cleared snapshot.
systemctl --user restart simard-ooda.service

# 4. Wait one cycle and verify engineers spawn.
ls -t ~/.simard/cycle_reports/ | head -1
ls -t ~/.simard/agent_logs/ | head -5
ls ~/.simard/engineer-worktrees/
```

The next cycle report under `~/.simard/cycle_reports/` should show
non-zero successful actions and at least one new `engineer-*.log` file
under `~/.simard/agent_logs/`.

## Related

- [Simard CLI reference: `simard goal`](../reference/simard-cli.md)
- [Recover goal board](./recover-goal-board.md)
- [Spawn engineers from OODA daemon](./spawn-engineers-from-ooda-daemon.md)
