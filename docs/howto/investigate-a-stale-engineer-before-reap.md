---
title: "How to investigate a stale engineer before it is reaped"
description: >
  Operator playbook for the investigate-before-reap behaviour: how to tell that a
  quiet engineer was investigated (not silently reclaimed), how to find and read
  the durable reaped-engineers/ evidence archive, how to read the verdict named
  in the reclaim log line, what happens when the investigation finds the engineer
  is still alive / blocked / recoverable / genuinely dead, and how a discovered
  Simard bug becomes a tracked self-improvement signal (issue / escalation /
  recipe) instead of a silent worktree deletion.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/investigate-stale-engineer-before-reap.md
  - ../reference/investigate-stale-engineer-api.md
  - ./diagnose-leaked-engineer-claims.md
  - ./inspect-and-clean-engineer-worktrees.md
  - ./record-an-investigation-finding.md
  - ../operations/claim-reaper-kill-switch.md
---

# How to investigate a stale engineer before it is reaped

> **Status: implemented.** Present-tense operator guide. The behaviour lives in
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs)
> and runs on the Overseer tick. See the
> [concept](../concepts/investigate-stale-engineer-before-reap.md) and the
> [API reference](../reference/investigate-stale-engineer-api.md).

Simard never reclaims a quiet/idle engineer on staleness alone. When a claim's
worktree goes idle past `SIMARD_CLAIM_REAP_STALE_SECS`, she first **archives its
evidence** and **investigates why it went quiet**, then reaps **only** if the
investigation concludes it is genuinely dead. This page shows how to observe that
loop and read what it produced.

## 1. Confirm the investigation ran (read the verdict in the log)

Every `heartbeat-stale` outcome names the investigation verdict. Grep the daemon
log (drop `--user` for a system-level install):

```bash
journalctl --user -u simard-ooda -n 500 | grep 'claim-reaper:'
```

- A **genuinely-dead** engineer is reaped **with its verdict** — the `verdict=`
  substring is stable:

  ```
  [simard] claim-reaper: reclaimed rysweet/Simard:goal-improve-tests (reason=heartbeat-stale, age=5142s, verdict=dead:panic)
  ```

- A **false positive** (still working) is **kept**, not reaped:

  ```
  [simard] claim-reaper: kept rysweet/Simard:goal-long-compile (reason=heartbeat-stale, age=5142s, verdict=still-alive — investigation says engineer still working)
  ```

- A `no-worktree` reclaim has **no verdict** — there was nothing to investigate:

  ```
  [simard] claim-reaper: reclaimed rysweet/Simard:g1 (reason=no-worktree, age=n/a)
  ```

If you see a `heartbeat-stale` line **without** a `verdict=`, that is a bug — the
new invariant is that no `HeartbeatStale` claim is reclaimed without a completed
investigation. File it.

## 2. Read the preserved evidence

The engineer's diagnostics are archived **before** its worktree is cleaned, so
the evidence survives the reap. Archives live under the state root:

```bash
# The state root is the same one the daemon runs against (see state-root docs).
ls -1dt "$SIMARD_STATE_ROOT"/reaped-engineers/*/ | head
```

Each archive is a directory named `<sanitized_claim_key>-<unix_ts>/`. Read its
`manifest.json` first — it carries the **raw** `claim_key`, the `goal_id`, the
idle age, the timestamp, and the verdict:

```bash
DIR="$SIMARD_STATE_ROOT/reaped-engineers/rysweet-Simard-goal-improve-tests-1721582400"
cat "$DIR/manifest.json"
```

Alongside the manifest you will find the worktree's newest logs / transcript, the
recipe-runner output, the captured exit status, and a narrow `journalctl` slice
for the goal. This is the evidence the investigation reasoned over — and the
evidence you would have **lost** under the old immediate-reclaim behaviour.

> Archives are overseer-owned, `0700`/`0600`, secret-scrubbed, and inherit the
> state-root lifecycle. They are a durable record, not a leak: nothing is sent
> off-host except a bounded, scrubbed excerpt to the model backend during the
> investigation itself.

## 3. Understand what the verdict means for the claim

| Verdict | Reaped? | What Simard does |
|---|---|---|
| `still-alive` | **No** | Keeps the claim; logs the false positive. The engineer is still working. |
| `blocked` | **No** | Keeps the claim; escalates the block (`EscalateBlockedGoal`) so it is worked, not reclaimed. |
| `recoverable` | **No** (this sweep) | Keeps the claim; relaunches to resume the transient-failed work. |
| `pending` | **No** (this sweep) | Investigation recipe is in flight; the reap (if any) lands on a later sweep of the same still-stale claim, once the recipe finishes. |
| `dead:<cause>` | **Yes** | Reclaims via `release_engineer_claim` + worktree cleanup, evidence already archived, verdict logged. |

`<cause>` is one of `panic`, `oom`, `e2big`, `lock-contention`, `simard-bug`,
`finished-unreported`, `unknown`.

## 4. Follow a discovered Simard bug into self-improvement

The whole point: a stalled engineer's death becomes a **self-improvement
signal**, not a silent deletion. When the investigation implicates Simard itself
(`dead:simard-bug`, or an `unknown`/`e2big`/`oom` it can root-cause), it emits
interventions that route through the Overseer's existing gated Act path — the
same one health review uses:

- **A tracked issue** is filed for the defect (`FileIssue`, the same gh-issue
  capability the no-progress breaker uses). Find it with:

  ```bash
  gh issue list --repo rysweet/Simard --search 'claim-reaper investigation in:body'
  ```

- **A fix workstream** may be dispatched (`LaunchRecipe` → `smart-orchestrator`)
  and/or the goal **escalated** (`EscalateBlockedGoal`) to the operator on email
  + Signal with a plain-English problem + next step.
- **The root cause is recorded** in cognitive memory so a recurrence of the same
  signature is recognized next time (see
  [record an investigation finding](./record-an-investigation-finding.md) and the
  [root-cause WHY API](../reference/overseer-root-cause-why-api.md)).

So even though the engineer is (correctly) reaped, the bug that killed it is
captured and worked. Reaping a dead engineer and fixing what killed it are not in
tension — Simard does both.

## 5. If a live engineer keeps being investigated as stale

The investigation is fail-closed: an ambiguous case resolves to `still-alive` and
is never reaped. But repeated `still-alive` findings for the same goal mean the
worktree legitimately goes quiet for long stretches (e.g. a long compile with no
intermediate writes). Prefer **raising the idle window** over disabling anything:

```bash
# Be more patient: 90-minute idle window instead of 30.
SIMARD_CLAIM_REAP_STALE_SECS=5400 simard daemon
```

See the [kill switch & tuning](../operations/claim-reaper-kill-switch.md) page.
Disabling the reaper (`SIMARD_CLAIM_REAP_ENABLED=off`) also disables the
investigation and the archive — it is a total no-op — so only do that to stop a
confirmed defect while you investigate.

## Related

- [Investigate-Before-Reap (concept)](../concepts/investigate-stale-engineer-before-reap.md)
- [Investigate-Before-Reap API (reference)](../reference/investigate-stale-engineer-api.md)
- [Diagnose and clear leaked engineer claims](./diagnose-leaked-engineer-claims.md)
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md)
- [Record an investigation finding](./record-an-investigation-finding.md)
- [Claim-Reaper Kill Switch & Tuning](../operations/claim-reaper-kill-switch.md)
