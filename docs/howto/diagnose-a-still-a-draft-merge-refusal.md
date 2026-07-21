---
title: Diagnose a "still a draft" merge refusal
description: >
  Operator playbook for the self-merge stall in which a GREEN, non-draft,
  MERGEABLE PR is re-escalated every Overseer tick with a "Pull Request is still a
  draft" abort (#4344 / #4145). How to confirm the PR's real draft state, read the
  draft gate's verdict, and tell a genuine draft from the fixed stale-draft race.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: how-to
status: reference
related:
  - ../concepts/merge-draft-state-revalidation.md
  - ../reference/merge-draft-gate-api.md
  - ../reference/cross-repo-merge-authority.md
  - ./triage-stale-pull-requests.md
  - ./watch-overseer-activity.md
---

# Diagnose a "still a draft" merge refusal

**Symptom.** The Overseer keeps escalating the *same* PR to you every tick, and
the merge attempt logs `Pull Request is still a draft` — even though the PR looks
GREEN, non-draft, and mergeable on GitHub. This is the #4344 / #4145 self-merge
stall.

With the [merge draft gate](../reference/merge-draft-gate-api.md) in place, a
genuinely-draft PR is a **quiet `Refused`** (no escalation) and a non-draft
mergeable PR **merges**. If you are still seeing an escalation loop, use the steps
below to tell which case you are in.

## 1. Confirm the PR's real draft state

```bash
gh pr view <PR> --repo rysweet/Simard \
  --json number,isDraft,mergeable,statusCheckRollup \
  --jq '{number, isDraft, mergeable,
         checks: [.statusCheckRollup[].state] | unique}'
```

- `isDraft: true` → the PR **is** a draft. The gate is correct to refuse it; mark
  it ready and re-run (step 3).
- `isDraft: false`, `mergeable: "MERGEABLE"`, checks all `SUCCESS`/`NEUTRAL`/
  `SKIPPED` → the PR is genuinely merge-ready. Proceed to step 2.

## 2. Read the draft gate's verdict directly

Run the gated merge path against the live PR and read the outcome:

```bash
simard merge-pr <PR> --repo rysweet/Simard
```

Expected outcomes after the fix:

| Output                                            | Meaning                                     |
| ------------------------------------------------- | ------------------------------------------- |
| `merged: PR #<n> in rysweet/Simard`               | The stall is resolved — it merged.          |
| `refused: PR #<n> … PR is still a draft`           | The PR really is a draft (see step 3).      |
| `refused: PR #<n> … mergeable status is '<X>'`     | Not a draft problem — a conflict/queue state.|

Because the gate decides against the single snapshot the merge path fetches
immediately before merging, a non-draft/mergeable PR merges here rather than
aborting downstream. A `refused`
is an **expected, quiet** result — it does not re-escalate to you.

## 3. If it genuinely is a draft, mark it ready

```bash
gh pr ready <PR> --repo rysweet/Simard
simard merge-pr <PR> --repo rysweet/Simard   # re-run the gated merge
```

## 4. Verify the loop cleared on the activity feed

```bash
simard status
# or watch the live feed:
```

See [watch Overseer activity](./watch-overseer-activity.md). A resolved stall
shows the PR moving to `Merged`, or a single quiet `Refused` with an actionable
reason — **not** a repeating operator escalation for an unchanged PR.

## Why the loop happened before the fix

The merge snapshot never requested `isDraft`, so the objective gates couldn't
check draft state; the gates passed and `gh pr merge` aborted on a stale
server-side draft flag, surfacing as an `Err` (not a `Refused`), which the
Overseer escalates. Nothing changed tick-to-tick, so the same PR escalated ~13
times over ~5 h. The [draft gate](../reference/merge-draft-gate-api.md) makes
draft a first-class fact and evaluates it against a fresh pre-merge snapshot, so a
mergeable PR merges and only a genuine draft is (quietly) refused. Full rationale:
[merge draft-state re-validation](../concepts/merge-draft-state-revalidation.md).
