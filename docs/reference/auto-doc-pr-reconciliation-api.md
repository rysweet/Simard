---
title: Auto-doc PR reconciliation API reference
description: >
  Reference for the overseer's auto-generated documentation PR reconciliation
  pass. Specifies the composite auto-doc identity gate (title marker +
  auto-generated author + draft + label), the pure `reconcile_doc_prs` decision
  core and its `DocPrReconcileDecision` output, the canonical-PR selection, the
  supersede-and-close / stale-CONFLICTING-close actions, the executor, and the
  additive defaulted `PrGhClient::close_pr` (argv-only `gh pr close` in
  `RealPrGhClient`).
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/auto-doc-pr-reconciliation.md
  - ../concepts/durable-documentation-policy.md
  - ./cross-repo-merge-authority.md
  - ./no-point-in-time-docs-scan.md
  - ../howto/reconcile-stale-auto-doc-prs.md
  - ../../src/overseer/doc_pr_reconcile.rs
  - ../../src/overseer/mod.rs
  - ../../src/stewardship/merge_authority.rs
---

# Auto-doc PR reconciliation API reference

> **Status: implemented.** The pure reconcile core (`reconcile_doc_prs`), the
> composite identity gate, and the executor live in
> [`src/overseer/doc_pr_reconcile.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/doc_pr_reconcile.rs).
> The additive `close_pr` operation lives on `PrGhClient` in
> [`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs)
> (`RealPrGhClient` overrides it). The bounded pass is wired into the overseer
> cycle in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).

For the rationale, see
[The overseer reconciles auto-generated documentation PRs to one open at a time](../concepts/auto-doc-pr-reconciliation.md).

## Contents

- [Marker constants](#marker-constants)
- [The composite identity gate: `is_auto_doc_pr`](#the-composite-identity-gate-is_auto_doc_pr)
- [The pure decision core: `reconcile_doc_prs`](#the-pure-decision-core-reconcile_doc_prs)
- [`DocPrReconcileDecision`](#docprreconciledecision)
- [The executor: `run_doc_pr_reconcile`](#the-executor-run_doc_pr_reconcile)
- [`PrGhClient::close_pr`](#prghclientclose_pr)
- [Overseer cycle wiring](#overseer-cycle-wiring)
- [Invariants](#invariants)

## Marker constants

| Constant | Value | Meaning |
| --- | --- | --- |
| `AUTO_DOC_PR_TITLE_MARKER` | `"Update documentation with"` | Title-prefix that a candidate PR's title must start with. A durable cross-system string; changing it would silently disable reconciliation, so it is a stable contract. |
| `AUTO_DOC_PR_LABEL` | the auto-generated doc-update label (`SIMARD_ENGINEER_PR_LABEL`) | Label a candidate must carry. |

> **Author identity is resolved at runtime, not a constant.** Earlier revisions
> hard-coded the expected author to `simard-overseer[bot]`
> (`DEFAULT_OVERSEER_AUTHOR_LOGIN`). That was wrong: the auto-doc PRs are authored
> under Simard's **engineer / OODA `gh` identity**
> ([`config::automerge_author`](../reference/) — env `SIMARD_AUTOMERGE_AUTHOR`),
> which is deliberately DISTINCT from the overseer-bot login, so the constant
> matched **zero** real PRs (an inert pass). The expected author is now resolved
> at the I/O boundary in `run_doc_pr_reconcile` and threaded into the pure gate as
> the `expected_author` argument. An empty/unresolved identity (env unset) matches
> nothing, so the pass stays fail-closed.

## The composite identity gate: `is_auto_doc_pr`

```rust
/// True only when EVERY signal marks `pr` an auto-generated doc-drift PR.
/// Fails closed: any missing signal — including an empty/absent author, or an
/// empty `expected_author` (unresolved identity) — returns false, so a human PR
/// (or a mis-resolved identity) is never a reconciliation candidate.
pub fn is_auto_doc_pr(pr: &OpenPrSummary, expected_author: &str) -> bool;
```

All of the following must hold; otherwise the PR is skipped:

| Signal | Requirement |
| --- | --- |
| Expected author resolved | `expected_author` is non-empty (`config::automerge_author()` returned `Some`). Empty ⇒ `false` (fail-closed). |
| Title | `pr.title.starts_with(AUTO_DOC_PR_TITLE_MARKER)` |
| Author | `pr.author` equals `expected_author` (the runtime-resolved engineer identity). **Empty/absent author ⇒ `false` (treated as human).** Keeping this a mandatory conjunct is the security-critical guard — `pr.author.login` is GitHub-authoritative and unspoofable. |
| Draft | `pr.is_draft == Some(true)` (the field is `Option<bool>`; `None`/absent ⇒ `false`) |
| Label | `pr.labels` contains `AUTO_DOC_PR_LABEL` |

> `OpenPrSummary` is the existing listing summary from
> [`stewardship::merge_authority`](./cross-repo-merge-authority.md). The gate
> **reads its existing fields** — `title`, `author`, `labels`, and `is_draft`
> (`Option<bool>`) — all of which already exist on the struct (added for the
> autonomous-self-merge sensor, #4097). This feature adds **no** field to
> `OpenPrSummary`; the only genuinely new items are `PrGhClient::close_pr` and
> the `doc_pr_reconcile.rs` module.

## The pure decision core: `reconcile_doc_prs`

```rust
/// Pure: given the current open-PR listing for one repo and the resolved
/// auto-doc author identity, decide which single auto-doc PR to keep (canonical)
/// and which to close (with a reason). Performs NO I/O. Non-auto-doc PRs are
/// ignored entirely.
pub fn reconcile_doc_prs(open_prs: &[OpenPrSummary], expected_author: &str) -> DocPrReconcileDecision;
```

Algorithm:

1. Filter `open_prs` to auto-doc candidates via [`is_auto_doc_pr`](#the-composite-identity-gate-is_auto_doc_pr).
2. If there are **zero or one** candidates, the invariant already holds — return
   a decision with no closes (and the single candidate, if any, as canonical).
3. Otherwise select the **canonical** PR: the newest (highest number / most
   recent) candidate. It is the keeper.
4. Every other candidate is queued for close, tagged with its
   [`CloseReason`](#docprreconciledecision):
   - `SupersededDuplicate` — an older duplicate, superseded by canonical.
   - `StaleConflictingDraft` — a candidate whose `mergeable` state is
     `CONFLICTING`.

The canonical PR is **never** placed in the close set, so the decision can never
close every candidate.

## `DocPrReconcileDecision`

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocPrReconcileDecision {
    /// The PR kept open (the single-open invariant's survivor), if any candidate exists.
    pub canonical: Option<u32>,
    /// PRs to close, each with the reason it was superseded/auto-closed.
    pub to_close: Vec<DocPrClose>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocPrClose {
    pub number: u32,
    pub reason: CloseReason,
    /// Comment posted on close, e.g. "superseded by #<canonical>".
    pub comment: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    SupersededDuplicate,
    StaleConflictingDraft,
}
```

## The executor: `run_doc_pr_reconcile`

```rust
/// Apply a reconciliation to one repo: resolve the auto-doc author identity
/// (`config::automerge_author`) at this I/O boundary, list open PRs, compute the
/// pure decision, then execute the closes by NUMBER via `close_pr`. Bounded and
/// IO-guarded; returns a structured report for the overseer journal/audit.
pub fn run_doc_pr_reconcile(
    repo: &str,
    gh: &dyn PrGhClient,
) -> SimardResult<DocPrReconcileReport>;

/// Testable core: the auto-doc author identity is INJECTED so this stays free of
/// env access. `run_doc_pr_reconcile` is a thin wrapper that resolves
/// `automerge_author().unwrap_or_default()` and delegates here.
pub fn run_doc_pr_reconcile_with_author(
    repo: &str,
    gh: &dyn PrGhClient,
    expected_author: &str,
) -> SimardResult<DocPrReconcileReport>;
```

Behavior:

- Resolves the expected auto-doc author from `config::automerge_author()`
  (env `SIMARD_AUTOMERGE_AUTHOR`); unset ⇒ empty ⇒ the gate matches nothing
  (fail-closed inert pass).
- Lists open PRs via `gh.list_open_prs(repo, limit)`.
- **Fail-closed on read error:** a listing failure surfaces the error and performs
  **no** closes that cycle.
- **LOUD inert-gate surfacing:** if title-marker auto-doc PRs are present but NONE
  pass the full identity gate, emits a `warn` naming the likely cause (mis-resolved
  `SIMARD_AUTOMERGE_AUTHOR`, missing label, or non-draft) so a silently inert pass
  never lets the stale-PR churn recur unseen.
- Computes [`reconcile_doc_prs`](#the-pure-decision-core-reconcile_doc_prs) and,
  for each `DocPrClose`, calls `gh.close_pr(repo, number, &comment)` **by number**.
- Processes a **bounded** batch per cycle (never an unbounded storm of closes).
- Emits an OTel/structured-tracing event per keep/close decision — no
  `print!`/`println!`.
- Returns `DocPrReconcileReport { canonical, closed: Vec<u32>, skipped, errors }`
  for the journal.

## `PrGhClient::close_pr`

Added additively to the existing
[`PrGhClient`](./cross-repo-merge-authority.md) trait:

```rust
pub trait PrGhClient {
    // … existing methods …

    /// Close PR `number` in `repo` with an explanatory `comment`. The default
    /// is a NO-OP so every existing fake / unwired client performs no mutation;
    /// only `RealPrGhClient` overrides it. Argv-only (`gh pr close <number>
    /// --repo <repo> --comment <comment>`) — never shell-interpolated, and
    /// structurally incapable of carrying `--admin` / `--no-verify`.
    fn close_pr(&self, _repo: &str, _number: u32, _comment: &str) -> SimardResult<()> {
        Ok(()) // no-op default (fail-safe: unwired clients never mutate)
    }
}
```

`RealPrGhClient::close_pr` builds a positional argv and shells out to `gh`; the
mutation acts on a specific PR **number** only.

## Overseer cycle wiring

`run_doc_pr_reconcile` is invoked from the overseer's PR/merge reconciliation
surface in `run_cycle`, behind the same composite fail-closed gate that guards
the overseer's other IO passes. A poll/list error skips the pass for that cycle
(no closes). The pass is additive: it does not change any existing merge or
verify decision.

## Invariants

- **≤ 1 open auto-doc PR** after a successful pass (the single-open invariant).
- **Canonical never closed** — the keeper is excluded from the close set by
  construction.
- **Human PRs untouched** — the composite gate fails closed; empty author ⇒
  skipped.
- **Mutate by number only**, argv-only, no `--admin`/`--no-verify`, no `sh -c`.
- **Additive / non-breaking** — `close_pr` has a no-op default; `OpenPrSummary`
  is read as-is (no field added or removed); no `pub` item is removed; the PRD
  is preserved.
- **Bounded & fail-closed** — a read error performs no closes; a bounded batch
  per cycle.
- **OTel-only audit** — no `print!`/`println!`.
- **Pure core** — `reconcile_doc_prs` and `is_auto_doc_pr` do no I/O and are
  exhaustively unit-tested on fixture PR lists.
