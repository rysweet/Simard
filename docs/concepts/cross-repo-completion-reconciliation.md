---
title: "Concept: cross-repo completion reconciliation (merged-PR evidence resolves against the goal's own repo)"
description: Why the deploy-aware done-gate now resolves the merged-PR evidence check against each goal's own target repository and reads the persisted PR linkage — so a genuinely-merged PR in a non-Simard ecosystem repo satisfies the gate and the goal archives, instead of re-blocking "completion BLOCKED — missing PR not merged" every OODA cycle.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - deploy-aware-done-gate.md
  - ../reference/completion-evidence-gate-api.md
  - ../reference/cross-repo-merged-pr-evidence.md
  - ../reference/goal-target-repo-routing.md
  - ../reference/cross-repo-merge-authority.md
  - ../howto/diagnose-a-rejected-goal-completion.md
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/goal_curation/types.rs
---

# Concept: cross-repo completion reconciliation

> **Status: implemented.** The URL-aware, repo-aware merged-PR resolution
> (`parse_pr_url`, the PR-target resolver, and the reworked
> `GhCliEvidenceSource::any_pr_merged`) lives in
> [`src/goal_curation/completion_gate.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs),
> alongside the [deploy-aware done-gate](deploy-aware-done-gate.md) it extends.
> This change is **additive and non-breaking**: the [`EvidenceSource`
> trait](../reference/completion-evidence-gate-api.md#evidence-sources), the
> `WipRef` schema, and the gate's `evaluate` logic are unchanged. See the
> [cross-repo merged-PR evidence API reference](../reference/cross-repo-merged-pr-evidence.md)
> for the typed surface.
> Issue [#4375](https://github.com/rysweet/Simard/issues/4375).

> A goal marked **completed** must **reconcile deterministically**: if a genuinely
> **merged PR** exists — even in another ecosystem repo — the done-gate must see
> it, certify the goal `Complete`, and let it archive. A goal must never
> re-block "completion BLOCKED — missing PR not merged" on a PR that is, in fact,
> merged.

## The problem this solves

The [deploy-aware done-gate](deploy-aware-done-gate.md) refuses to archive a goal
without a **merged PR** (clause 1). To verify "merged," the production
[`GhCliEvidenceSource`](../reference/completion-evidence-gate-api.md#evidence-sources)
runs `gh pr view <num> --repo <slug>` and checks for `MERGED`. Two coupled
defects in that lookup made the gate un-satisfiable for goals whose work shipped
in a **non-Simard** ecosystem repo:

1. **Wrong repo (cross-repo done-gate can never pass).** The old `repo_slug`
   resolver only left the default `rysweet/Simard` in place when
   `goal.repo == None`. Goals routed to another repo via
   [target-repo routing](../reference/goal-target-repo-routing.md) — e.g. the
   eight `fix-agent-kgpacks-rs-issue-*` goals targeting
   **`rysweet/agent-kgpacks-rs`** — had their merged PRs opened in that repo, but
   if `goal.repo` was absent or not fully qualified, the gate queried the merged
   PR against **`rysweet/Simard`**. A PR number that is merged in
   `agent-kgpacks-rs` is a different PR (or no PR) in `Simard`, so the check could
   never return `MERGED`.

2. **Un-read linkage (merged PR recorded only as a URL).** `any_pr_merged`
   located a PR solely via `WipRef.ref_id` on a `wip_ref` of kind `pr`. When a
   goal's outcome persisted the merged PR as a **URL** (`WipRef.url`,
   e.g. `https://github.com/rysweet/agent-kgpacks-rs/pull/42`) rather than a bare
   numeric `ref_id`, the gate found no PR number at all, returned `Ok(false)`, and
   emitted `PrNotMerged`.

The observed symptom was a **completion-reconciliation loop**: nine goals that
`simard goal list` already showed as `completed`
(`fix-agent-kgpacks-rs-issue-12/18/19/20/21/22/23/25` and
`simard-example-identity-gastronome`) re-emitted

```text
OODA curate: completion BLOCKED for goal '<id>' — missing PR not merged
```

on **every** OODA cycle — the exact line fired 21× per goal (189 total over a
6-hour window) and kept firing. These counts are drawn from the daemon-log
evidence attached to issue [#4375](https://github.com/rysweet/Simard/issues/4375)
(the source of record for these figures). Because the gate re-blocked a goal that
was genuinely done, the goals never converged and burned cycle work indefinitely.

The guiding principle:

> **Merged-PR evidence is repo-relative. Verify "merged" against the goal's own
> target repository, using whatever linkage the outcome persisted (numeric ref
> _or_ URL) — never against the daemon's default repo by accident.**

## What changed

The fix is confined to the **merged-PR clause** of the production evidence
source. It teaches `any_pr_merged` to resolve *both* the PR number *and* the
owning `owner/repo` from every linkage the goal actually carries, then query
`gh pr view` against that resolved repo.

### 1. Read the persisted PR linkage — numeric ref *and* URL

A new pure helper, `parse_pr_url`, recovers `(owner/repo, pr_number)` from a
GitHub PR URL such as `https://github.com/rysweet/agent-kgpacks-rs/pull/42`. It
is total and panic-free: on any non-PR or malformed input it returns `None` and
logs at `debug`, never a partial or guessed value. `any_pr_merged` now consults
`WipRef.url` as an **additive fallback** whenever a bare numeric `ref_id` is
absent, recovering both the PR number and its owning repo from one seam without
any schema change.

### 2. Resolve the target repo deterministically (first match wins)

`any_pr_merged` resolves the repository to query in a fixed precedence order:

1. A **qualified** `goal.repo` (`owner/repo`, and not `Simard`).
2. The `owner/repo` **parsed from the PR `WipRef.url`**.
3. A **bare** `goal.repo` slug scoped under the default owner (existing
   behaviour, e.g. `agent-kgpacks-rs` → `rysweet/agent-kgpacks-rs`).
4. The default **`rysweet/Simard`**.

The PR **number** resolves as: numeric `WipRef.ref_id` → else the number parsed
from `WipRef.url`. Repo and number are resolved as a *pair*: when the number is
recovered from the URL, the query uses that same URL's `owner/repo`, so a
URL-derived number is never checked against a different repo (a qualified
`goal.repo` overrides the URL repo only when the number came from a numeric
`ref_id`).

This closes both root causes with a single change: a cross-repo goal whose
merged PR is recorded as a URL now resolves to the *right* repo *and* the *right*
number, and `gh pr view <num> --repo <owner/repo>` returns `MERGED`.

### Scope: only the merged-PR clause

The URL/repo recovery — the **repo-relative resolution** — applies **only** to
`any_pr_merged`. `is_deployed` and the `is_self_affecting` classifier are
unchanged. In particular, self-affecting classification still treats
`repo == None` as routing to Simard; this change does not alter which goals are
considered self-affecting. `issue_closed`'s resolution is likewise unchanged, but
it does gain the **same fail-closed slug/number validation** as `any_pr_merged`
(a non-digit issue number or unsafe `owner/repo` slug now blocks without reaching
`gh`) — a defense-in-depth parity fix, not a change to its clause semantics.

## Still fail-closed — no silent always-pass

This change **strengthens** reconciliation without weakening the gate. It never
turns the merged-PR check into an unconditional pass:

- **Truly no PR linkage** (no `pr` wip_ref, and no parseable PR URL) →
  `Ok(false)` → `PrNotMerged`. The goal stays blocked, cheaply, with no network
  call.
- **A URL that does not parse** as a valid GitHub PR falls through to the
  existing slug logic rather than fabricating a repo.
- **A `gh` query error** → propagates as an error → the gate records
  `CouldNotVerify` and the goal stays active for a re-check next cycle.

There is **no** code path where an unmerged or absent PR is treated as merged.
The determinism requirement — "a goal already marked completed must reconcile
deterministically" — is met by the pure resolver plus injected evidence source:
repeated evaluations of the same goal yield the same verdict.

## Security: argument-injection defense

The resolved repo slug and PR number flow into a `gh` subprocess. Before either
value reaches the command, the source validates them:

- PR number must be **non-empty ASCII digits** only. This applies to *every*
  number source — including a number read verbatim from `WipRef.ref_id` via
  `first_ref_of_kind`, which was previously passed to `gh` unvalidated — so the
  `ref_id` path is now hardened against argument injection, not only the new
  URL-derived path.
- Repo slug must match `^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$` — no leading `-`, no
  whitespace, no shell metacharacters.

Values that fail validation **fail closed** (block) rather than reaching `gh`.
The command is still invoked with `Command::args` (no shell, no `sh -c`, no
string interpolation), so metacharacters remain inert, and `parse_pr_url` never
URL-decodes, so it cannot reintroduce `/` or control characters into a slug. Only
read-only `gh` verbs are used (`gh pr view`); no write/merge/close verbs are
added.

## Observability

All new non-test code uses structured `tracing` + OpenTelemetry only — there are
no `print!`/`println!` calls. A miss in `parse_pr_url` and each repo/number
resolution decision are logged at `debug`, so an operator diagnosing a still-
blocked goal can see which repo and PR number the gate actually queried. See
[how to diagnose a rejected goal completion](../howto/diagnose-a-rejected-goal-completion.md#cross-repo-re-block-loop).

## How this composes

- **Target-repo routing ([#2359](https://github.com/rysweet/Simard/issues/2359)).**
  This is the completion-side complement of
  [goal target-repo routing](../reference/goal-target-repo-routing.md): routing
  put the PR in the right repo; this makes the *done-gate* look in that same repo.
- **Cross-repo merge authority.** The gate reads merge state across ecosystem
  repos consistently with
  [cross-repo merge authority](../reference/cross-repo-merge-authority.md).
- **Deploy-aware done-gate.** Clause 1 ("merged PR") is now repo-relative;
  clauses 2 and 3 are unchanged. See
  [deploy-aware-done-gate](deploy-aware-done-gate.md).

## See also

- [Cross-repo merged-PR evidence API reference](../reference/cross-repo-merged-pr-evidence.md)
- [Completion-evidence gate API reference](../reference/completion-evidence-gate-api.md)
- [deploy-aware-done-gate concept](deploy-aware-done-gate.md)
- [Goal target-repo routing API reference](../reference/goal-target-repo-routing.md)
- [How to diagnose a rejected goal completion](../howto/diagnose-a-rejected-goal-completion.md)
