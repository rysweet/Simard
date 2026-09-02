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
  - ./diagnose-a-no-progress-block.md
  - ../concepts/ooda-loop-self-detection.md
  - ../concepts/perpetual-goal-no-progress-exemption.md
  - ../concepts/no-progress-root-cause-resolution.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/no-progress-root-cause-resolution-api.md
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

## Standing/perpetual goals never need unblocking (#2589)

A **standing/perpetual** goal — one described "STANDING PERPETUAL goal" /
"Standing goal", i.e. `is_perpetual()` (issue #2580) — is **exempt from the
no-progress safeguard entirely**. Such a goal is inherently *bursty*: it ships a
durable improvement, idles for a few cycles while there is nothing new to ship,
then ships again. As of #2589 the no-progress breaker recognises this:

- **Runtime:** when a standing goal produces a no-action cycle, the breaker
  resets its consecutive-no-action counter and keeps it active instead of
  climbing toward the threshold. It never sets the `[OODA-SAFEGUARD]` marker and
  never files a review issue. The idle is recorded in the cycle's
  `perpetual_idled` list and logged at `info` — normal, not a fault.
- **Load-time self-heal:** if a standing goal was already parked by an *older*
  daemon build, the daemon clears that stale `[OODA-SAFEGUARD]` block back to
  `not-started` automatically — on the next daemon start **and again at the top
  of every cycle** (`heal_stale_no_progress_blocks`, run after tombstone
  filtering). No `simard goal unblock` needed.

So a standing goal that used to appear as
`p5 [blocked: 🔒 [OODA-SAFEGUARD] …]` now stays `p5 [not-started]` across idle
cycles and self-heals if it was parked. You should not have to unblock it by
hand. If a standing goal *is* still parked, it means it was blocked by a
**different** path (operator hold, scope, dependency, or the brain-failure
safeguard) — those are out of scope for the self-heal and legitimately need the
manual steps below. See
[Standing/perpetual goals are exempt from the no-progress hard-block](../concepts/perpetual-goal-no-progress-exemption.md)
and the [no-progress breaker API](../reference/no-progress-breaker-api.md) for
details.

## No-progress blocks explain WHY and self-resolve first

> **Implemented (issue #16).** On by default; set
> `SIMARD_NO_PROGRESS_INVESTIGATE=off` to revert to the base ladder. The manual
> CLI steps below remain the escape hatch for any block you want to clear by hand.

The OODA **no-progress breaker** (3 consecutive no-action cycles) no longer parks
a goal with a bare "needs human review". Before authoring any block it classifies
**why** the goal stalled and self-resolves the machine-fixable causes:

- an **already-complete** goal (issues closed / PRs merged / deployed) is
  auto-marked `completed` — **not** blocked;
- a goal missing a **precondition** (e.g. an un-cloned governed repo) has it
  established and retries — **not** blocked;
- a goal waiting on an **upstream** goal/PR/issue is `paused` with the blocking
  ref recorded and **auto-clears** when the upstream lands — **not** blocked;
- an **unclear-criteria** / **genuinely-stuck** goal gets **one** guided engineer
  first; only if it stalls again does it block.

When a block *is* unavoidable, the reason carries the classified WHY + evidence
(it still starts with the `🔒 [OODA-SAFEGUARD]` sentinel, so the commands below
keep working):

```text
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 4 consecutive no-action cycles; why=GENUINELY-STUCK evidence=[pr #7 (OPEN)]
```

So most stalls that used to land here now resolve themselves, and the ones that
don't arrive with a diagnosis attached. For reading the WHY, the per-branch
examples (including the `kgpacks-rs` "already done" incident), and the
configuration, see
[Diagnose a no-progress block and read its WHY](./diagnose-a-no-progress-block.md).
The manual CLI steps below remain the escape hatch for a block you want to clear
by hand regardless of its cause.

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

- [Diagnose a no-progress breaker issue storm](./diagnose-a-no-progress-breaker-issue-storm.md) — when the daemon auto-files many duplicate `ooda-stuck` "goal stuck after guided retry (UNCLEAR-CRITERIA)" issues; the durable suppression marker now caps filings at one per goal.
- [Simard CLI reference: `simard goal`](../reference/simard-cli.md)
- [Re-investigate bare-blocked OODA goals](./reinvestigate-bare-blocked-goals.md) — the daemon now auto-upgrades bare `[OODA-SAFEGUARD]` blocks to a concrete WHY every cycle (#17), so manual unblocking is rarely needed.
- [Recover goal board](./recover-goal-board.md)
- [Spawn engineers from OODA daemon](./spawn-engineers-from-ooda-daemon.md)
