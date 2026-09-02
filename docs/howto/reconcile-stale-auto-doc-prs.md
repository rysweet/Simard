---
title: Reconcile stale auto-generated documentation PRs
description: >
  Runbook for the overseer's auto-doc PR reconciliation pass. Explains how to
  recognise the single-open invariant is holding, how to clear a pre-existing
  backlog of stale `Update documentation with N changed files` drafts, and how
  to confirm human PRs are never affected.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/auto-doc-pr-reconciliation.md
  - ../concepts/durable-documentation-policy.md
  - ../reference/auto-doc-pr-reconciliation-api.md
  - ../reference/simard-cli.md
  - ../design/overseer.md
---

# Reconcile stale auto-generated documentation PRs

## Symptom

The repository has a growing pile of draft PRs, all titled like:

```
Update documentation with N changed files
```

Many are in `CONFLICTING` mergeable state and were opened days apart, never
rebased or closed. They clutter the PR list and confuse merge-readiness sensors.

As of the overseer's `doc_pr_reconcile` pass, the daemon keeps **at most one**
such PR open at a time — superseding-and-closing older duplicates and
auto-closing stale `CONFLICTING` auto-doc drafts. This runbook confirms the
invariant and clears any pre-existing backlog.

## Confirm the invariant is holding

Count the currently-open auto-doc PRs. After the pass has run a cycle, this
should be `0` or `1`:

```bash
gh pr list --repo rysweet/Simard --state open --draft \
  --search "Update documentation with in:title" --json number,title,mergeable
```

Check the overseer journal / logs for the reconciliation audit events (OTel /
structured tracing, one per keep/close decision):

```bash
# The pass records a canonical keeper and the numbers it closed each cycle.
grep -i "doc_pr_reconcile\|auto-doc" ~/.simard/ooda.log | tail -20
```

A healthy cycle logs one `canonical` (the keeper) and, when a backlog exists, a
bounded set of `closed` numbers each tagged `SupersededDuplicate` or
`StaleConflictingDraft`.

## Clear a pre-existing backlog

The pass processes a **bounded batch per cycle**, so a large pre-existing backlog
drains over several cycles rather than all at once (this is intentional — it
avoids a storm of `gh pr close` mutations). Simply let the daemon run; the count
converges to one.

To watch it drain:

```bash
watch -n 60 'gh pr list --repo rysweet/Simard --state open --draft \
  --search "Update documentation with in:title" --json number | jq length'
```

If the daemon is offline, the pass never runs. Either start it, or close the
backlog by hand — but note the pass will re-establish the single-open invariant
automatically once running, so manual cleanup is rarely needed:

```bash
# Manual fallback ONLY (the daemon does this automatically): keep the newest,
# close the rest with a supersede comment.
CANON=$(gh pr list --repo rysweet/Simard --state open --draft \
  --search "Update documentation with in:title" --json number --jq 'max_by(.number).number')
gh pr list --repo rysweet/Simard --state open --draft \
  --search "Update documentation with in:title" --json number --jq '.[].number' \
  | grep -v "^$CANON$" \
  | xargs -I{} gh pr close {} --repo rysweet/Simard --comment "superseded by #$CANON (auto-doc reconciliation)"
```

## Confirm human PRs are never affected

The pass uses a composite fail-closed identity gate (title marker +
auto-generated author + draft + label), and treats an **empty/absent author as
human**, so a human PR — even one whose title coincidentally starts with
"Update documentation with" — is never a reconciliation candidate.

Verify by listing non-draft or human-authored PRs and confirming none were
closed by the pass:

```bash
# Human/non-auto PRs should never appear in the pass's closed set.
gh pr list --repo rysweet/Simard --state closed \
  --search "Update documentation with in:title" --json number,author,isDraft \
  --jq '.[] | select(.isDraft == false)'
```

This should return nothing closed by reconciliation — the canonical keeper stays
open and only auto-generated drafts are closed.

## Related

- [The overseer reconciles auto-generated documentation PRs to one open at a time](../concepts/auto-doc-pr-reconciliation.md) — the design and rationale.
- [Auto-doc PR reconciliation API reference](../reference/auto-doc-pr-reconciliation-api.md) — the identity gate, decision core, and `close_pr`.
- [Durable-Documentation Policy (G4)](../concepts/durable-documentation-policy.md) — the merge-gate counterpart that governs what may land.
- [Simard CLI reference](../reference/simard-cli.md)
