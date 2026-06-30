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
  - ../concepts/ooda-loop-self-detection.md
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

## A different stuck mode: spinning at a high completion-% (no lockout)

Not every stuck daemon is locked out. A second failure mode looks *healthy* from
the brain's perspective but ships nothing:

- The board shows one goal `in-progress` parked at a high percent (e.g. 99%)
  with an empty backlog.
- The brain confidently emits `advance_goal` every cycle.
- Each cycle re-triages the same PRs, finds nothing to merge, re-records the same
  percent, and repeats. `~/.simard/cycle_reports/` shows actions "succeeding"
  but no new commits, PRs, or merges accumulate.

This is the **open-ended-goal loop**: a goal like "increase test coverage across
the ecosystem" has no reachable 100%, so it never completes, never archives, and
parks forever while real work stalls. As of issue #2403 the prompt assets make
Simard reason about this herself — see [OODA loop self-detection,
reflectiveness, and proactivity](../concepts/ooda-loop-self-detection.md) for the
full design. The goal-action brain now:

1. checks, before triaging, whether the last few cycles produced **real
   progress** (new commit SHAs, opened/merged PRs, closed issues) versus mere
   re-triage;
2. on detecting a loop, **changes strategy** — decomposing the open-ended goal
   into a concrete, completable sub-goal and executing it, completing/retiring
   the goal, or proposing fresh work from Simard's own open issues; and
3. lets the progress-assessment gate **reject** a re-asserted high percent that
   has only re-triage behind it, nudging decompose/complete/demote.

### What an operator can do

Because the prompt content hot-reloads, no rebuild is needed — syncing
`prompt_assets/simard/` to `~/.simard/prompt_assets/simard/` is enough. If a goal
is still parked after the next few cycles, an operator can give it a concrete,
bounded shape directly:

```bash
# Inspect the parked goal and its recorded percent.
simard goal list

# Demote the open-ended goal off the active board (it moves to the backlog),
# or remove it entirely if no bounded progress remains.
simard goal demote <goal-id>
simard goal remove <goal-id>

# Add a concrete, completable replacement at a chosen priority (1-7) so the
# board stays at its active cap rather than idling on one stalled item.
simard goal add <priority> "module X line coverage >= 80%, PR merged"
```

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
