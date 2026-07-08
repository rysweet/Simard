---
title: Re-investigate bare-blocked OODA goals
description: Operator runbook for the OODA no-progress re-investigation pass (#17) — how to spot goals stranded with a bare "[OODA-SAFEGUARD] … needs human review" block, how the daemon automatically re-investigates and re-classifies them each cycle (completing, dropping, healing, deferring, or spawning a fixer), how to verify the re-classification, and how to confirm idempotency (no duplicate fixers).
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/ooda-reinvestigate-blocked-goals.md
  - ../reference/no-progress-reinvestigation-api.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/simard-cli.md
  - ./unblock-stuck-ooda-goals.md
  - ./spawn-engineers-from-ooda-daemon.md
  - ./run-ooda-daemon.md
---

# Re-investigate bare-blocked OODA goals

## Symptom

`simard goal list` shows one or more goals parked with the **bare** no-progress
safeguard marker — the `[OODA-SAFEGUARD]` sentinel with *no* explanation of why:

```
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 3 consecutive no-action cycles; needs human review
```

There is no named upstream dependency, no fixer, and no path forward — just "needs
human review." This is the exact state the live incident on deploy #41 left five
kgpacks-rs goals in (`advance-rysweet-agent-kgpacks-rs-to-full-parity`,
`fix-agent-kgpacks-rs-issue-18`, `-issue-21`, `-issue-22`, `-issue-23`).

## Automatic recovery (no operator action needed)

As of **issue #17** you do **not** need to unblock these by hand. Every OODA cycle,
after the on-transition breaker runs, the daemon runs a **re-investigation pass**
(`reinvestigate_bare_blocked_goals`) that:

1. scans the active board for goals in a **bare** no-progress block
   (`is_bare_no_progress_block`) that are not [standing/perpetual](../concepts/perpetual-goal-no-progress-exemption.md);
2. runs each through the same agentic WHY reasoner the on-transition path uses;
3. **rewrites** the block to embed a concrete WHY (class + cited evidence) so it is
   no longer bare; and
4. **resolves** it along the shared ladder — complete, drop, heal, defer behind the
   named upstream, or spawn exactly one guided fixer.

So a goal that appears today as a bare `[OODA-SAFEGUARD] … needs human review` will,
within the next cycle or two, become one of:

| Outcome | What you'll see in `simard goal list` |
| --- | --- |
| **WHY-stated block** | `blocked: 🔒 [OODA-SAFEGUARD] … why=UPSTREAM-DEPENDENCY evidence=[… #16 …]` |
| **Completed** | the goal leaves the active board as done (work was already merged) |
| **Dropped** | the goal is removed (obsolete) |
| **Un-blocked + fixer** | `not-started`, with a new `engineer-*.log` under `~/.simard/agent_logs/` |
| **Deferred** | `paused`, with a `[no-progress-defer]` ref naming the upstream it waits on |

No goal is left as a bare "needs human review." Operator intervention is only
needed when the daemon is **offline** (so the pass never runs) or when you want an
immediate manual override — see [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md).

## Verify the re-classification

### 1. List the board and find bare blocks

```bash
simard goal list
```

A goal still bare will show `[OODA-SAFEGUARD] …` ending in `needs human review`
with **no** `why=` segment. A re-classified goal carries a `why=<CLASS>` segment
(the class token is upper-case and bracketless, e.g. `why=UPSTREAM-DEPENDENCY`):

```text
# before (bare — pre-#17 / offline)
advance-rysweet-agent-kgpacks-rs-to-full-parity   p3   blocked: 🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 3 consecutive no-action cycles; needs human review

# after re-investigation (WHY-stated)
fix-agent-kgpacks-rs-issue-18                      p4   blocked: 🔒 [OODA-SAFEGUARD] … 3 consecutive no-action cycles; why=UPSTREAM-DEPENDENCY evidence=[eval baseline #16 OPEN, no landed PR — unmeasurable until #16 lands]
```

### 2. Watch a cycle re-investigate

The cycle report and logs record what the pass did. The compact cycle-log line now
carries a `reinvestigated={n}` field:

```bash
# Tail the daemon and look for the re-investigation summary + per-goal traces.
journalctl --user -u simard-ooda.service -f | grep -E "reinvestigat|OODA-SAFEGUARD|why="

# Or inspect the latest cycle report.
cat "$(ls -t ~/.simard/cycle_reports/*.json | head -1)" | jq '.no_progress'
```

You should see, per re-investigated goal, either a status rewrite (now carrying a
`why=<CLASS>` segment), a completion/drop, a `paused` defer, or a spawned engineer.

### 3. Confirm a spawned fixer

When the WHY is actionable and the goal has not spent its one guided retry, the pass
spawns exactly one fixer through the normal
[engineer-dispatch path](./spawn-engineers-from-ooda-daemon.md):

```bash
ls -t ~/.simard/agent_logs/ | head -5           # new engineer-*.log
ls ~/.simard/engineer-worktrees/                # its isolated worktree
```

## Confirm idempotency (no duplicate fixers)

Re-investigation runs every cycle, but it is **idempotent** — a goal is taken to a
terminal action (spawn / complete / drop / defer) **at most once per WHY class**.
Two guards enforce this (see the
[API reference](../reference/no-progress-reinvestigation-api.md#persisted-data-contract)):

1. the rewrite makes the block non-bare, so the next scan skips it; and
2. a persisted dedupe set `reinvestigated: {(goal_id, class_token)}` in the tracker
   short-circuits any terminal action already taken — even across a daemon restart.

To confirm no second fixer was spawned for the same goal:

```bash
# Count engineer logs referencing a given goal id — expect at most one active fixer
# per (goal, WHY class).
grep -l "advance-rysweet-agent-kgpacks-rs-to-full-parity" ~/.simard/agent_logs/engineer-*.log | wc -l

# Inspect the persisted dedupe set (the goal board is the source of truth).
jq '.no_progress.reinvestigated' ~/.simard/state/goal_board.json
```

A restart between the board rewrite and the tracker persist does **not** double-spawn:
the `(goal, class)` pair is only inserted on success and is honored on reload.

## Fail-closed behavior (what a reasoner error looks like)

If the WHY reasoner errors for a goal, the pass **keeps the bare marker**, takes no
action, and retries next cycle. You will see:

- the goal *still* bare in `simard goal list` (no `why=` segment yet), and
- an `investigation_errors` entry in the cycle report / an `error`-level trace.

There are **no wall-clock timeouts** on the reasoner — a slow-but-live investigation
is not killed; only a genuine error or idle-death defers it to the next cycle. A
persistently-bare goal after several cycles means the reasoner keeps erroring for
it; check `journalctl` for the recorded error and treat it as a normal daemon fault
(not a stuck goal). You may still force a manual override with
[`simard goal unblock <goal-id>`](./unblock-stuck-ooda-goals.md#clear-a-single-goal-unconditionally).

## Worked example — the deploy #41 kgpacks-rs incident

After #17 shipped and the daemon ran a few cycles, the five stranded goals resolved
as follows (this is the R9 validation evidence recorded in the PR body):

| Goal | Bare before | After re-investigation |
| --- | --- | --- |
| `fix-agent-kgpacks-rs-issue-18` (WS3) | ✅ | `why=UPSTREAM-DEPENDENCY` — deferred behind the named eval-baseline upstream |
| `fix-agent-kgpacks-rs-issue-21` (WS6) | ✅ | `why=UPSTREAM-DEPENDENCY` — deferred, upstream named |
| `fix-agent-kgpacks-rs-issue-22` (WS7) | ✅ | `why=UPSTREAM-DEPENDENCY` — deferred, upstream named |
| `fix-agent-kgpacks-rs-issue-23` (WS8) | ✅ | `why=UPSTREAM-DEPENDENCY` — deferred, upstream named |
| `advance-rysweet-agent-kgpacks-rs-to-full-parity` | ✅ | WHY-stated block or guided fixer, depending on the live evidence at re-investigation time |

`fix-agent-kgpacks-rs-issue-17` is the **reference format** — it already carried a
proper `UPSTREAM-DEPENDENCY` WHY naming the open eval-baseline issue `#16`
("#17's done-criterion is gated on eval recall parity, which depends on #16's eval
baseline; #16 is still OPEN with no landed baseline, so #17's gate is unmeasurable —
a genuine hard upstream dependency"). Every re-investigated goal now reads like that,
never like a bare "needs human review."

## Related

- [Concept: Re-investigating already-blocked OODA goals](../concepts/ooda-reinvestigate-blocked-goals.md)
- [No-progress re-investigation API reference](../reference/no-progress-reinvestigation-api.md)
- [No-progress breaker API reference](../reference/no-progress-breaker-api.md)
- [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md) — manual override for the offline / immediate cases.
- [Spawn engineers from the OODA daemon](./spawn-engineers-from-ooda-daemon.md)
- [Simard CLI reference: `simard goal`](../reference/simard-cli.md)
