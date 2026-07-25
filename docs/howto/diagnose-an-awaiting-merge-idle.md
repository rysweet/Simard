---
title: Diagnose an awaiting-merge idle (and confirm no duplicate PR)
description: Runbook for the OODA no-progress breaker's awaiting-merge branch (issue #4441) — recognise when a goal is being idled because it already has an open, non-draft, mergeable PR, read the suppression trace and the `awaiting_merge` report field, confirm no duplicate PR was created, land the pending PR so the goal completes, and understand the fail-closed cases where the goal is still reaped.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ./diagnose-a-no-progress-block.md
  - ./unblock-stuck-ooda-goals.md
  - ./diagnose-a-rejected-goal-completion.md
  - ./run-ooda-daemon.md
  - ../concepts/no-progress-awaiting-merge-exemption.md
  - ../reference/no-progress-awaiting-merge-api.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/completion-evidence-gate-api.md
---

# Diagnose an awaiting-merge idle (and confirm no duplicate PR)

> **Status: implemented (issue #4441).** The no-progress breaker no longer reaps
> and re-dispatches an engineer whose workstream already delivered an open,
> non-draft, mergeable PR. Such a goal is *idled* (awaiting an external merge)
> rather than treated as stalled. For the exact types and the `gh` query, see
> the [awaiting-merge API reference](../reference/no-progress-awaiting-merge-api.md);
> for the rationale, the [concept](../concepts/no-progress-awaiting-merge-exemption.md).

## When you need this

Use this runbook when either:

- a goal has been "not progressing" for several cycles but you know its engineer
  already opened a PR, and you want to confirm the daemon is **correctly idling**
  it (not silently stuck); or
- you are investigating **duplicate PRs** for one workstream and want to verify
  the reaper is no longer re-dispatching finished engineers.

If a goal is parked with a `[OODA-SAFEGUARD] … needs human review` block
instead, use [Diagnose a no-progress block and read its WHY](./diagnose-a-no-progress-block.md).

## 1. Confirm the goal has an open, mergeable PR

The breaker idles a goal only when its tracked PR is simultaneously **open**,
**non-draft**, and **mergeable**. Resolve the PR from the goal's `wip_refs`
(first ref of kind `pr`) and check it exactly as the daemon does:

```console
$ gh pr view <num> --repo rysweet/Simard --json state,isDraft,mergeable
{
  "state": "OPEN",
  "isDraft": false,
  "mergeable": "MERGEABLE"
}
```

`mergeable == MERGEABLE` is the positive value of GitHub's `MergeableState` enum
(the `--json mergeable` field is `MERGEABLE` / `CONFLICTING` / `UNKNOWN` — not the
separate `mergeStateStatus` `CLEAN`/`DIRTY` field). If you instead see
`CONFLICTING`, `UNKNOWN`, `isDraft: true`, or a non-`OPEN` state, the goal is
**not** idled — it falls through to the normal reap/escalate path (see step 4).

## 2. Read the suppression trace

Every awaiting-merge idle emits exactly one structured `tracing::info!` line on
the `simard::ooda` target. Filter the daemon log for it:

```console
$ journalctl -u simard --since "10 min ago" \
    | grep 'awaiting external merge'
… simard::ooda goal=fix-4441-done-detection pr=4460 pr_open=true \
  pr_draft=false pr_mergeable=true \
  no-progress breaker: goal has an open, mergeable PR — awaiting external \
  merge; suppressing reap/re-dispatch (no duplicate PR created)
```

The line carries only the `goal`, the `pr` number, and the three decision
booleans — never a token or raw `gh` stderr. Seeing it means the breaker fired
its awaiting-merge branch and **deliberately took no action**.

## 3. Confirm no duplicate PR was created

The whole point of the fix is that no fresh engineer is spawned for a
completed-awaiting-merge goal. Verify:

```console
# There should be exactly ONE open PR for the workstream.
$ gh pr list --repo rysweet/Simard --search "head:fix-4441" --json number,state
[ { "number": 4460, "state": "OPEN" } ]
```

In the cycle report / `NoProgressBreakerReport`, the goal appears under
`awaiting_merge` and **not** under `engineer_spawned` or `escalated`. Because an
awaiting-merge idle is not a firing, `fired()` stays `false`. The compact cycle
summary still renders and now carries an `awaiting_merge=` counter, so a cycle
whose only breaker activity was this idle shows `… perpetual_idled=0 awaiting_merge=1`
with every other counter at zero. The authoritative per-goal signal is the trace
from step 2 plus the `awaiting_merge` report entry.

## 4. Land the PR to complete the goal

An awaiting-merge goal waits **indefinitely** — there is no timeout that
re-arms the reaper. It completes only when the PR is merged (by you or the merge
queue):

```console
$ gh pr merge <num> --repo rysweet/Simard --squash
```

On the next cycle the [deploy-aware done-gate](../reference/completion-evidence-gate-api.md)
sees the merged PR, certifies the goal `Complete`, and it archives normally
(`marked_done`). No manual unblock is required.

## 5. Understand the fail-closed cases (goal still reaped)

The awaiting-merge signal is **fail-closed**: any uncertainty resolves to "not
awaiting merge", so a genuinely-stalled engineer is always still reaped. The
goal is **not** idled — and you should expect a reap/escalate — in all of these
cases:

| Situation | Why not idled |
| --- | --- |
| No tracked PR on the goal | Nothing delivered yet |
| PR is a draft (`isDraft: true`) | Explicitly not ready to merge |
| PR is `CONFLICTING` | Not landable; engineer may still have work |
| PR `mergeable` is `UNKNOWN` | GitHub hasn't computed mergeability; fail-closed |
| PR is `CLOSED` without merge | Work abandoned |
| `gh` errored or returned unparseable JSON | Cannot verify; fail-closed |

If you expected an idle but the goal was reaped, run step 1 — one of the three
clauses is almost certainly failing (most often a transient `UNKNOWN`
mergeability that GitHub resolves shortly, after which the next cycle idles the
goal correctly).

## See also

- [Concept: a goal with an open, mergeable PR is awaiting merge — never reaped](../concepts/no-progress-awaiting-merge-exemption.md)
- [Awaiting-merge API reference](../reference/no-progress-awaiting-merge-api.md) — the signal, the `gh` query, the disposition/resolution, and the fail-closed table.
- [Diagnose a no-progress block and read its WHY](./diagnose-a-no-progress-block.md) — for goals that reach a `[OODA-SAFEGUARD]` block instead.
- [Diagnose a rejected goal completion](./diagnose-a-rejected-goal-completion.md) — the deploy-aware done-gate side.
- [Run the OODA daemon](./run-ooda-daemon.md) — where to find the cycle log and traces.
