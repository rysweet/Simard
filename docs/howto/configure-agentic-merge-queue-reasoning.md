---
title: Configure agentic merge-queue reasoning
description: >
  Operator runbook for Simard's agentic observe/orient merge-queue + issue
  reasoning: confirm it is default-ON over the governed roster (even with the
  autonomous-merge env vars unset), narrow the reasoning scope, explicitly
  disable it LOUDLY, verify the reasoned_prs -> ready_prs re-narrowing keeps
  merge action behind the objective + agentic gate, and check the dual-channel
  notify on every merge. No path uses --admin or --no-verify.
last_updated: 2026-07-19
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/agentic-merge-queue-reasoning.md
  - ../reference/agentic-merge-queue-reasoning-api.md
  - ../design/agentic-observe-orient-merge-queue.md
  - ../reference/cross-repo-merge-authority.md
  - ./enable-autonomous-self-merge-canary.md
  - ./triage-stale-pull-requests.md
  - ./watch-overseer-activity.md
---

# Configure agentic merge-queue reasoning

> **Goal.** Confirm Simard's observe/orient stage **reasons** about the whole
> open-PR queue and issue backlog across the governed roster every cycle
> (default-ON), narrow or explicitly disable that reasoning, and verify the merge
> **action** stays behind the unchanged objective + agentic gate with
> dual-channel notify — **never** `--admin` / `--no-verify`.

Background: [the concept](../concepts/agentic-merge-queue-reasoning.md), the
[design spec](../design/agentic-observe-orient-merge-queue.md), and the
[API reference](../reference/agentic-merge-queue-reasoning-api.md).

## Prerequisites

- The daemon binary includes the merge-queue reasoning pass (this feature).
- **`gh`** authenticated so the read-only reasoning step can list/view PRs and
  issues (`gh auth status`).
- The governed roster `prompt_assets/simard/ecosystem_repos.toml` is populated
  (install-first on a deployed daemon:
  `~/.simard/prompt_assets/simard/ecosystem_repos.toml`).

## Key change from the old allowlist sensor

The retired imperative sensor produced **zero** merge reasoning whenever
`SIMARD_AUTOMERGE_REPOS` was unset (its live-production state). Reasoning is now
**default-ON over the governed roster** and is decoupled from the
autonomous-merge env vars:

| Env var | Role now |
|---|---|
| `SIMARD_MERGE_REASONING_SCOPE` | **Reasoning** on/off/scope. Unset ⇒ ON over roster. `off` ⇒ OFF, loud. |
| `SIMARD_AUTOMERGE_REPOS` | **Action-side** narrowing only. Unset can no longer silence reasoning. |
| `SIMARD_AUTOMERGE_AUTHOR` | **Action-side** own-PR identity for the merge gate. |

## 1. Confirm reasoning is default-ON (even with merge env vars unset)

```bash
# The exact condition that produced zero reasoning before this fix:
systemctl --user show-environment | grep -E 'SIMARD_AUTOMERGE|SIMARD_MERGE_REASONING' || echo "all unset"

# Watch the reasoning pass populate the reasoned fields:
journalctl --user -u simard-ooda -f | grep 'overseer::merge_queue'
# → INFO reasoned_prs=<n> triaged_issues=<m> scope=roster status=RosterWide
```

`n`/`m` should be non-zero when there are open PRs/issues on the roster. If they
are zero *and* there are open PRs, check the roster is populated and `gh` is
authenticated.

You can also read it in `simard status`:

```bash
simard status | grep -i 'merge reasoning'
# → Merge reasoning: ACTIVE (roster, N PRs reasoned, M issues triaged)
```

## 2. Narrow the reasoning scope (optional)

To reason over a subset instead of the whole roster:

```bash
systemctl --user set-environment SIMARD_MERGE_REASONING_SCOPE="rysweet/Simard,rysweet/azlin"
systemctl --user restart simard-ooda
journalctl --user -u simard-ooda | grep 'overseer::merge_queue' | tail -1
# → scope=explicit repos=rysweet/Simard,rysweet/azlin status=Narrowed
```

A systemd-managed daemon's environment is fixed for the process lifetime, so any
change requires a **restart** to take effect.

## 3. Explicitly disable reasoning — LOUDLY

Disabling is intentional and **never silent**. Only an explicit off value
disables; unset means default-ON.

```bash
systemctl --user set-environment SIMARD_MERGE_REASONING_SCOPE=off
systemctl --user restart simard-ooda

# LOUD signal #1 — WARN log:
journalctl --user -u simard-ooda | grep 'merge reasoning DISABLED'
# → WARN merge reasoning DISABLED (SIMARD_MERGE_REASONING_SCOPE=off) — the
#   observe/orient stage will not reason about the open-PR queue or issues

# LOUD signal #2 — status field:
simard status | grep -i 'merge reasoning'
# → Merge reasoning: DISABLED (SIMARD_MERGE_REASONING_SCOPE=off)

# LOUD signal #3 — a ONE-TIME operator note arrives on email AND Signal.
```

To re-enable, unset it (default-ON) or set an explicit scope, then restart:

```bash
systemctl --user unset-environment SIMARD_MERGE_REASONING_SCOPE
systemctl --user restart simard-ooda
```

## 4. Verify the action gate is unchanged (reasoning ≠ authorization)

Broadening reasoning must **not** widen merge authorization. A PR the agent calls
`ready-for-merge` still only merges if it re-passes the objective + author +
engineer-PR gates via the `reasoned_prs → ready_prs` projection.

```bash
# Watch the projection re-narrow the agent's proposals:
journalctl --user -u simard-ooda -f | grep -E 'reasoned->ready|VerifyAndMergePr|merge_authority'
# A ready-for-merge PR that fails the author guard / engineer-PR gate / objective
# gate is logged as EXCLUDED and never reaches VerifyAndMergePr.
```

Confirm no forbidden flags exist anywhere (this is also asserted in CI):

```bash
git grep -nE -- '--admin|--no-verify' src/ && echo "UNEXPECTED" || echo "clean: no --admin/--no-verify"
```

Actual merges go through the unchanged
[cross-repo merge authority](../reference/cross-repo-merge-authority.md):
objective gates (base allowlist + `MERGEABLE` + all checks green) → `MergeJudge`
(fail-closed, six evidence sections) → `gh pr merge --squash --delete-branch`.
The autonomous-merge **action** still ships behind
`SIMARD_AUTOMERGE_REPOS` / `SIMARD_AUTOMERGE_AUTHOR`; canary-enable it per
[enable autonomous self-merge](./enable-autonomous-self-merge-canary.md).

## 5. Verify stale / duplicate handling

The reasoning pass also drives two gated, notify-first interventions on Simard's
**own** engineer PRs:

```bash
journalctl --user -u simard-ooda | grep -E 'FlagStalePr|CloseDuplicatePr'
# FlagStalePr    → gh pr comment (triage note); never merges/closes
# CloseDuplicatePr → gh pr close referencing duplicate_of
```

Both are `RiskClass::MergeAuthority` opt-in (notify-only when the autonomy gate
is off), build positional argv only, and never use `--admin`/`--no-verify`. They
never touch an operator's review PR (author guard + engineer-PR narrowing).

## 6. Verify dual-channel notify on every merge

```bash
journalctl --user -u simard-ooda | grep -E 'NotifyOperator|merge notify'
# Every merge (including autonomous) sends a concise problem + PR summary to
# rysweet on BOTH email and Signal.
```

## Run the reasoning chain by hand (debugging)

```bash
amplihack recipe run observe-merge-queue \
  -c roster_path="$PWD/prompt_assets/simard/ecosystem_repos.toml" \
  -c inflight_refs_path="/tmp/inflight.json" \
  -c merge_queue_brief_path="/tmp/merge_queue_brief.json"

cat /tmp/merge_queue_brief.json   # the bounded JSON brief the rail parses fail-closed
```

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `reasoned_prs=0` with open PRs on the roster | roster empty or `gh` not authed | populate `ecosystem_repos.toml`; `gh auth status` |
| `status=Disabled` unexpectedly | `SIMARD_MERGE_REASONING_SCOPE` set to a falsey value | `unset-environment`, restart |
| reasoning ON but nothing merges | expected — action still gated by `SIMARD_AUTOMERGE_*` + objective/agentic gate | canary-enable per [self-merge runbook](./enable-autonomous-self-merge-canary.md) |
| a `ready-for-merge` PR never merges | it failed the re-narrowing projection (author/engineer-PR/objective gate) | check the `reasoned->ready EXCLUDED` log line for the reason |
| env change not taking effect | systemd env is fixed for the process life | restart the unit |

## See also

- [Concept: agentic merge-queue reasoning](../concepts/agentic-merge-queue-reasoning.md)
- [Reference: agentic merge-queue reasoning API](../reference/agentic-merge-queue-reasoning-api.md)
- [Design: agentic observe/orient merge-queue](../design/agentic-observe-orient-merge-queue.md)
- [Enable autonomous self-merge (canary)](./enable-autonomous-self-merge-canary.md)
- [Triage stale pull requests](./triage-stale-pull-requests.md)
