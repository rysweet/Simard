---
title: The overseer reconciles auto-generated documentation PRs to one open at a time
description: >
  Why the ~30 stale, CONFLICTING, draft auto-generated `Update documentation
  with N changed files` PRs no longer accumulate. An additive overseer
  reconciliation pass (`overseer::doc_pr_reconcile`) enforces a single-open
  invariant — at most one open auto-doc PR at a time — by superseding-and-closing
  older duplicates and auto-closing stale CONFLICTING auto-doc drafts, matched on
  a composite fail-closed identity gate (title marker `Update documentation
  with` + auto-generated author + draft + label). The keeper (canonical) PR is
  never closed; every mutation is by PR number through an argv-only `gh pr close`
  behind a defaulted, non-breaking `PrGhClient::close_pr`. The reconcile core is
  a pure function; the executor is a bounded, IO-guarded overseer pass. Empty
  author = treated as human = skipped.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./durable-documentation-policy.md
  - ./stewardship-mode.md
  - ./gap-scan-backoff-dedup.md
  - ./autonomous-merge-review-gate.md
  - ./draft-pr-merge-exclusion.md
  - ../reference/auto-doc-pr-reconciliation-api.md
  - ../reference/cross-repo-merge-authority.md
  - ../howto/reconcile-stale-auto-doc-prs.md
  - ../design/overseer.md
---

# The overseer reconciles auto-generated documentation PRs to one open at a time

> **Status: implemented.** The pure reconcile core and its executor live in
> `src/overseer/doc_pr_reconcile.rs`. The additive, defaulted `close_pr`
> operation lives on `PrGhClient` in `src/stewardship/merge_authority.rs`
> (`RealPrGhClient` overrides it to shell out to `gh pr close` argv-only). The
> bounded pass is wired into the overseer cycle in `src/overseer/mod.rs`. For the
> exact types and functions see the
> [auto-doc PR reconciliation API reference](../reference/auto-doc-pr-reconciliation-api.md).

## The defect

Over roughly a week the repository accumulated **~30 stale, CONFLICTING, draft**
pull requests, all sharing one title shape:

```
Update documentation with N changed files
```

(the oldest dating to 2026-07-22). Each was opened by an automated doc-drift
event and then never rebased, never closed, and never superseded — so they rotted
into `CONFLICTING` drafts that clutter the PR list, confuse merge-readiness
sensors, and bury the real PRs.

Root cause: the doc-update automation opens a **fresh** PR per doc-drift event
with no deduplication. There is no single generating workflow/script to patch —
codebase analysis confirmed the literal title string
`"Update documentation with"` is not emitted by any committed workflow; these PRs
are agent/prompt-driven via `gh pr create`. So the fix cannot live in a
generator that does not exist. It must live where PR state is already
**reconciled**: the overseer.

## The fix: an additive overseer reconciliation pass

The overseer already reconciles PR/merge state each cycle. This feature adds a
small, additive pass — `overseer::doc_pr_reconcile` — that enforces a
**single-open invariant** for auto-doc PRs:

> **At most one open auto-doc PR exists at a time.** All older duplicates are
> superseded-and-closed; stale `CONFLICTING` auto-doc drafts are auto-closed.

Because there is no generator to make idempotent, reconciliation is done
**after the fact**, keyed on a **stable doc-PR marker**, not on a generator's
internal state.

### Identifying an auto-doc PR — a composite, fail-closed gate

A PR is treated as an auto-doc PR only when **all** of a composite identity gate
match. The gate is deliberately conservative and **fails closed** (skips) on any
doubt, so a human PR can never be misclassified and closed:

| Signal | Requirement |
| --- | --- |
| **Title** | Title-prefix match on the marker `"Update documentation with"`. |
| **Author** | Author is the known auto-generation identity. **An empty/absent author is treated as human and skipped** — never reconciled. |
| **Draft** | The PR is a draft (auto-doc PRs are opened as drafts). |
| **Label** | Carries the auto-generated doc-update label. |

Only a PR matching every signal is a reconciliation candidate. Anything else —
including a human-authored PR that happens to start with the same words — is left
completely untouched.

### Choosing the canonical (keeper) PR

Among the matched candidates the pass selects exactly **one canonical** PR to
keep open — the newest / most-current one (the one most likely to reflect the
latest doc drift). The canonical PR is **never closed**. Every other matched
candidate is a *superseded duplicate*.

### The two reconciliation actions

```text
matched auto-doc PRs (title + author + draft + label)
        │
        ├─ pick canonical (newest) ──────────────► keep open (never closed)
        │
        └─ for each non-canonical candidate:
                 ├─ superseded duplicate ─────────► close (supersede)
                 └─ stale CONFLICTING draft ──────► close (auto-close)
```

1. **Supersede-and-close older duplicates.** Every non-canonical matched PR is
   closed with a comment pointing at the canonical PR, collapsing the population
   to the single-open invariant.
2. **Auto-close stale CONFLICTING drafts.** A matched draft in `CONFLICTING`
   mergeable state is closed rather than left to rot. (A future enhancement may
   *rebase* instead of close where safe; the shipped behavior is close, because a
   fresh auto-doc PR will be re-opened on the next real doc drift anyway.)

The pure core returns the **decision** (which PR is canonical, which numbers to
close and why); the executor applies it. This keeps the whole classification and
selection surface testable on fixture PR lists with zero network.

## Safety — this pass mutates PRs, so it is bounded and fail-closed

Closing PRs is a new destructive mutation for the overseer, so every safeguard is
conservative:

- **Composite identity gate.** All four signals must match; an empty author is
  treated as human and skipped. A human PR is structurally unclosable by this
  pass.
- **Canonical never closed.** The keeper is excluded from the close set by
  construction, so the invariant can never close *every* PR.
- **Mutate by number only.** `close_pr` acts on a specific PR **number**; the
  argv is built positionally and is structurally incapable of carrying `--admin`
  / `--no-verify`. No shell interpolation (`gh` argv-only, never `sh -c`).
- **Additive, defaulted trait method.** `PrGhClient::close_pr` has a **no-op
  default**, so every existing fake and unwired client performs no mutation;
  only `RealPrGhClient` shells out. Adding it breaks no caller.
- **Bounded, IO-guarded pass.** The pass runs behind the same composite
  fail-closed gate as the overseer's other IO passes: a read/list failure surfaces
  the error and performs **no** closes that cycle (better to skip than to
  mis-close on a transient error). It processes a bounded batch per cycle.
- **OTel-only audit.** Every classify/keep/close decision is emitted as a
  structured tracing / OTel event — no `print!`/`println!`.

## Relationship to the durable-documentation policy (G4)

This feature is the **cleanup** counterpart to
[G4 durable-documentation policy](./durable-documentation-policy.md). G4's
pr-verify scan blocks *point-in-time report docs* from being **merged**; this
pass stops *auto-doc-drift PRs* from **accumulating**. They are complementary:
G4 governs what may land, `doc_pr_reconcile` governs how many auto-doc PRs may be
open at once. Both live inside the overseer's PR/merge reconciliation surface.

## Why reconcile after the fact instead of fixing a generator?

- **There is no generator to fix.** The title string is not emitted by any
  committed workflow/script; the PRs are agent/prompt-driven `gh pr create`
  calls. A patch to a nonexistent emitter would be fiction.
- **State reconciliation is the right home.** The single-open invariant and the
  stale-draft auto-close are properties of the *live PR population*, which the
  overseer already reconciles each cycle. Enforcing them there is additive and
  needs no change to how any PR is opened.
- **It is a self-amplifying-loop fix.** Like the
  [gap-scan backoff/dedup](./gap-scan-backoff-dedup.md), the defect is a
  safeguard/automation observing a condition and re-emitting instead of
  converging. Reconciliation converges the population to one.

## What an operator sees now

- At most **one** open `Update documentation with …` PR at a time; older
  duplicates are closed with a supersede comment linking the canonical one.
- Stale `CONFLICTING` auto-doc drafts are auto-closed instead of rotting.
- Human PRs — including any that coincidentally start with the same words — are
  never touched.

See [Reconcile stale auto-doc PRs](../howto/reconcile-stale-auto-doc-prs.md) for
confirming the invariant and clearing a pre-existing backlog.

## See also

- [Auto-doc PR reconciliation API reference](../reference/auto-doc-pr-reconciliation-api.md) — the reconcile core, the identity gate, and `close_pr`.
- [Durable-Documentation Policy (G4)](./durable-documentation-policy.md) — the merge-gate counterpart.
- [Gap-scan dedup & exponential backoff](./gap-scan-backoff-dedup.md) — the overseer's sibling self-amplifying-loop fix.
- [Draft-PR merge exclusion](./draft-pr-merge-exclusion.md) — why auto-doc drafts are never merge candidates.
- [Overseer — operator/observer co-process (design)](../design/overseer.md) — the cycle this pass runs inside.
