---
title: "How-to: diagnose duplicate done-gate PRs for one goal"
description: >
  Confirm whether Simard has opened more than one done-gate pull request for a
  single goal, understand why the older in-flight guards missed it, verify the
  goal-PR emission ledger is suppressing further duplicates, and clean up
  already-filed duplicates. Covers reading the goal-emission ledger rows,
  matching PRs by their Simard-Goal-Key trailer, interpreting the
  ooda::done_gate log lines, and the best-effort supersede sweep.
last_updated: 2026-07-18
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/idempotent-done-gate-pr-emission.md
  - ../reference/goal-pr-emission-ledger-api.md
  - ./triage-stale-pull-requests.md
  - ./diagnose-leaked-engineer-claims.md
  - ../concepts/engineer-claim-liveness-lease.md
---

# Diagnose duplicate done-gate PRs for one goal

> **Status: implemented.** The per-goal emission guard described here ships in
> [`dispatch_spawn_engineer`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs)
> backed by the `goal_pr_emissions` ledger
> ([`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)).
> Background:
> [idempotent done-gate PR emission](../concepts/idempotent-done-gate-pr-emission.md);
> typed surface:
> [goal-PR emission ledger API](../reference/goal-pr-emission-ledger-api.md).

## Symptom

The target repo shows **several open PRs that clearly pursue the same goal** —
same title, same intent, filed cycles apart, most `CONFLICTING` or stale. The
motivating incident (2026-07-18, `rysweet/Simard`): 3× `coin-benchmark`
(#4326/#4329/#4332) and 4× `kgpacks-parity` (#4324/#4328/#4330/#4333), inflating
the repo to ~31 open PRs.

## Why it happened (root cause)

`dispatch_spawn_engineer` had two idempotency guards — board `assigned_to` and
live on-disk worktree — both keyed on *in-flight engineer state*. When an
engineer finishes and exits, its board assignment clears and its worktree is
reaped (its `engineer_claims` row is `DELETE`d — see
[engineer-claim liveness lease](../concepts/engineer-claim-liveness-lease.md)),
but its PR stays **open and unmerged**. The next cycle sees "no live engineer"
and dispatches a fresh one, which opens a second PR. Neither guard was keyed on
an already-open PR. The fix adds a third guard keyed on **durable goal identity**.

## 1. Confirm it is the same goal (not distinct goals)

Match PRs on the `Simard-Goal-Key:` body trailer, **not** the title:

```bash
gh pr list --repo rysweet/Simard --state open --label simard-autonomous \
  --json number,title,headRefName,body \
  --jq '.[] | {number, headRefName,
        key: (.body | capture("Simard-Goal-Key: (?<k>[0-9a-f]{16})").k // "none")}'
```

PRs sharing the same 16-hex `key` are duplicates of one goal. A `key: "none"`
PR predates the feature (or was authored elsewhere) — reconciliation adopts it
by head-branch (`engineer/{key}-...`) instead.

## 2. Inspect the emission ledger

The ledger is the authoritative record. Locate the typed-OODA store and read the
open emissions:

```bash
# The store lives under the instance state root. Simard resolves it as
# $SIMARD_STATE_ROOT, else $SIMARD_HOME, else ~/.simard (see
# typed_ooda_state_root() in spawn.rs).
STATE_ROOT="${SIMARD_STATE_ROOT:-${SIMARD_HOME:-$HOME/.simard}}"
sqlite3 "$STATE_ROOT/typed-ooda/outcomes.sqlite3" \
  "SELECT goal_key, goal_id, repo, pr_number, state, updated_at
     FROM goal_pr_emissions
    WHERE state = 'open'
    ORDER BY updated_at DESC;"
```

Healthy state: **exactly one** `state='open'` row per `goal_key`. The
`UNIQUE(repo, pr_number)` constraint and the `goal_key` primary key make more
than one open row per goal impossible once the guard is active.

If a duplicate slipped in **before** the fix shipped, you will see one ledger row
but several live PRs — the extras are the pre-existing duplicates to clean up in
step 4.

## 3. Verify the guard is suppressing new duplicates

Watch the OODA log for the guard firing on the target `ooda::done_gate`:

```bash
journalctl -u simard --since "1 hour ago" | grep 'ooda::done_gate'
```

Expected lines:

- **Ledger hit (primary):**
  `spawn skipped: open done-gate PR already tracked for goal` with
  `goal_id`, `pr`, and the 16-hex `key`.
- **Reconciliation self-heal (secondary):** the same skip after adopting a
  pre-existing PR into the ledger.
- **Fail-open (advisory lister errored):**
  `open-PR reconciliation failed; proceeding on ledger guard only` — a `WARN`,
  not an error. The ledger guard still holds; this is expected on a transient
  `gh` hiccup and never wedges the loop.

If you instead see a **new** PR opened for a goal that already has an open one,
the ledger row is missing — capture the store and the logs and file a bug
referencing #4166/#4189.

## 4. Clean up already-filed duplicates (best-effort)

The fix prevents *future* duplication; it does not retroactively close the
existing extras. Supersede them manually, keeping the newest/most-complete PR per
goal:

```bash
# for each duplicate PR to retire (keep one per goal-key):
gh pr close <number> --repo rysweet/Simard \
  --comment "Superseded: duplicate done-gate PR for the same goal (Simard-Goal-Key match). See #4166."
```

Then consolidate the tracking issues #4166 and #4189 if the fix resolves them.
See the [triage-stale-pull-requests runbook](./triage-stale-pull-requests.md)
for the broader stale-PR sweep.

## 5. Force a re-check (optional)

Emission suppression is never permanent: once the kept PR **merges or closes**,
its ledger row transitions out of `open`, so a genuinely new occurrence of that
goal may emit again. To adopt a PR the ledger missed, simply let the next OODA
cycle run — reconciliation lists open PRs once per cycle and records any match.

## See also

- [Concept: idempotent done-gate PR emission](../concepts/idempotent-done-gate-pr-emission.md)
- [Goal-PR emission ledger API reference](../reference/goal-pr-emission-ledger-api.md)
- [How to triage stale pull requests](./triage-stale-pull-requests.md)
- [How to diagnose and clear leaked engineer claims](./diagnose-leaked-engineer-claims.md)
