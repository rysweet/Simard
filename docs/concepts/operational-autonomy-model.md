---
title: Operational autonomy model
description: How Simard self-promotes goals and self-validates / self-merges clean, green, merge-ready work autonomously for most operations — without waiting on a human approver — while a small, named set of HIGH-RISK operations still surfaces to the operator for sign-off, and every quality and safety gate stays fully intact.
last_updated: 2026-06-29
owner: simard
doc_type: concept
status: reference
related:
  - ./stewardship-mode.md
  - ./deploy-aware-done-gate.md
  - ../reference/cross-repo-merge-authority.md
  - ../reference/pr-finalization-pipeline.md
  - ../reference/goal-decomposition.md
  - ../reference/ado-acl-self-escalation-guard.md
---

# Operational autonomy model

> **Operator directive (Ryan Sweet):** *"For most operations she should not need
> outside-party validation."*

Simard governs the amplihack ecosystem as its engineering steward. For the vast
majority of routine operations she now acts **autonomously**: she **self-promotes**
well-scoped goals onto the active board, and she **self-validates** and
**self-merges** clean, green, merge-ready work — **without waiting for operator
approval or any outside-party validation**.

Autonomy here has a precise, bounded meaning:

> **Autonomy = no waiting on a HUMAN approver. It is NOT a license to skip
> quality or safety gates.**

Every objective gate (CI-green, base-branch allowlist, merge-judge verdict, the
rysweet-author priority gate) and every destructive-operation safety floor
(`git_guardrails`, `ado_acl_guard`) stays **fully intact**. Autonomy removes the
*human rubber-stamp wait* from the routine path; it removes nothing from the
evidence or safety contract.

A small, explicitly named set of **HIGH-RISK** operations remains gated: for
those, Simard does **not** auto-execute — she surfaces the action to the operator
and waits for sign-off. See [HIGH-RISK boundary](#high-risk-boundary).

## What changed

Before this model, several prompt surfaces told Simard to *propose and wait*:
goals required operator approval before promotion, improvements required operator
approval before becoming active goals, and engineers were instructed to treat the
mere absence of a human approver as a blocker. In practice this throttled a
fully-governed system on a human step that, for the repos Simard owns, has **no
required human reviewer** behind it.

The autonomy model replaces *propose-and-wait* with *act-and-record* for routine
operations:

| Surface | Before (propose-and-wait) | After (autonomous) |
|---------|---------------------------|--------------------|
| **Goal curation** | "You propose; he decides. You do not unilaterally promote goals to active status without operator approval." | Simard **self-promotes** well-scoped goals onto the active board. Ryan can reprioritize or defer **asynchronously**; routine promotion does not block on his sign-off. The top-N discipline and the evidence requirements are unchanged. |
| **Improvement curation** | "Improvements require operator approval before promotion to active goals." | Simard **self-promotes** well-evidenced improvements. Operator review is **asynchronous**, not a gate. Every proposal still cites concrete evidence. |
| **Engineering / merge** | "Get operator approval, and ship." / "Blocked on a required human review/approval." | Simard **self-validates against the merge-ready gates and ships** autonomously. "Required approvals satisfied" is met, for a governed repo with no required human reviewer, once the **objective gates + merge-judge pass**. |

The directive applies broadly across Simard's modes; the same act-and-record
principle is reflected in the goal-curator, improvement-curator, engineer, and
goal-session prompts (and, for consistency, the gym self-curation prompt).

## The merge-ready contract is unchanged

Self-merge does **not** mean merge-without-evidence. The
[merge-ready contract](../reference/pr-finalization-pipeline.md) holds in full:
the merge-judge's six evidence headings — QA-team, Documentation, Quality-audit,
CI, Scope, and the explicit Verdict — are all still required, and the
merge-readiness skill (`merge-ready`, the source of truth the judge defers to)
is unchanged.

Autonomy reinterprets **exactly one** criterion, and it is **not** one of the
judge's six headings. The `merge-ready` skill carries a separate
**required-reviews/approvals criterion** ("Required reviews/approvals are
satisfied, with no outstanding requested changes") and treats a PR "blocked by
external approval" as not-ready. Autonomy clarifies *that* criterion — by name,
renumbering nothing — for the repos Simard governs:

> **The required-reviews/approvals criterion, clarified.** For a repo Simard
> governs that has **no required human reviewers** and **no
> branch-protection-required approval** Simard cannot satisfy, there is no
> outstanding *required* approval to wait on — so the `merge-ready` skill's
> required-reviews/approvals criterion is met **once the objective gates and the
> merge-judge verdict pass**. Simard does **not** block waiting for an external
> approver who does not exist.
>
> A *genuinely required* human reviewer — a branch-protection rule that mandates
> an approval Simard cannot provide — **is still a real blocker**: she records it
> as a specific blocker and stops. The mere **absence** of a human rubber-stamp
> is **not** a blocker.

This distinction is the heart of the model: Simard waits for *gates that exist*,
never for *approvers that do not*.

## HIGH-RISK boundary

Autonomy is bounded. The following operations are **HIGH-RISK** and are the
explicit exception to autonomous execution. For each, Simard **surfaces the
action to the operator and waits for sign-off** — she does **not** auto-execute
it under the autonomy model:

1. **Git history rewrite / force-push** — `push --force` / `push -f`, history
   rewrites, or `reset --hard` on a shared branch.
2. **Deleting repositories or branches** — repository deletion, or `branch -D`
   on a protected branch.
3. **Public / breaking API changes** — any change to a published interface's
   compatibility or a stable public surface.
4. **Security- or credential-affecting changes** — secrets, auth, tokens, or
   any permission / ACL escalation.
5. **Any write to the operator's protected local repos under `~/src`**
   — the paths listed in `SIMARD_GIT_PROTECTED_REPOS`.

This list is stated verbatim in the engineer and goal-session prompts so the
agent reads it on every cycle. It is **not** advisory-only: it is backed by
code-level safety floors that already **hard-block** the most destructive members
of the list regardless of what any prompt says.

> **Durability anchor — the boundary is enforced, not just described.** The
> HIGH-RISK list is layered on top of pre-existing, always-on guardrails:
>
> - [`src/git_guardrails.rs`](https://github.com/rysweet/Simard/blob/main/src/git_guardrails.rs)
>   hard-blocks force-push, `reset --hard`, `branch -D` on `main`/`master`/`release`,
>   `clean -fdx`, reflog/gc, and **every write under `SIMARD_GIT_PROTECTED_REPOS`**.
> - [`ado_acl_guard`](../reference/ado-acl-self-escalation-guard.md) forbids
>   self-escalation of a repository's security ACLs (issue #809); privileged ACL
>   remediation is permitted only behind an explicit operator opt-in
>   (`SIMARD_ALLOW_ADO_ACL_ESCALATION=1`) and is crash-safe / idempotent.
>
> The HIGH-RISK list is therefore a **prompt-level surfacing rule** over enforced
> floors — no new enforcement code is required, and the two stay discoverable
> together.

## Preserved gates (autonomy weakens none of these)

The following gates and bounds are **preserved exactly** — autonomy removes the
human-wait, never these:

| Invariant | Why it stays | Where |
|-----------|--------------|-------|
| **AIMD engineer concurrency cap** + 4× scaler ceiling + 429 / load backoff | Bounds how many engineers run *concurrently*. Raising the goal-board cap does **not** raise actual parallelism. | `src/ooda_loop/types.rs`, `adaptive_scaling.rs` — see [adaptive scaling](./adaptive-scaling.md) |
| **Merge base-branch allowlist** (`SIMARD_MERGE_BASE_ALLOWLIST`, default `["main"]`) | First objective gate; refuses a PR targeting a stale or wrong base. | `merge_authority` — see [cross-repo merge authority](../reference/cross-repo-merge-authority.md) |
| **All required CI checks green** before merge | Self-merge never means merge-with-red-CI. | `merge_authority` objective gates (`statusCheckRollup`) |
| **Merge-judge verdict gate** | A prompt-driven judge must independently confirm the evidence criteria are satisfied. | `merge_judge`, `prompt_assets/simard/merge_readiness_judge.md` |
| **rysweet-author priority gate** | Author-priority ordering in the OODA decide path is unchanged. | OODA decide path |
| **Destructive-op safety floor** | Force-push, hard reset, and protected-repo writes stay hard-blocked. | `git_guardrails`, `ado_acl_guard` |

## Relationship to the goal-board cap

Autonomy lets Simard *fan out* a large umbrella goal into many distinct per-repo
work items. To stop the goal board from becoming the binding constraint on that
fan-out, the active-goal cap (`MAX_ACTIVE_GOALS`) is **20** (raised from 7).

> **The two caps are different concerns — do not conflate them.**
>
> - `MAX_ACTIVE_GOALS = 20` bounds **how many distinct goals may exist** on the
>   active board at once.
> - The **AIMD engineer concurrency cap** bounds **how many engineers run
>   concurrently**.
>
> Raising the goal-board cap lets more parallel work items *exist* so they are
> not throttled at the board; it does **not** raise real parallelism. Actual
> concurrent execution is still bounded by the AIMD cap and its 429 / load
> backoff. A 15-repo supply-chain umbrella can now spawn its full set of per-repo
> goals instead of stalling once the board fills at 7.

See [goal decomposition](../reference/goal-decomposition.md) and
[maximum safe parallelism](../reference/maximum-safe-parallelism.md).

## Cross-repo self-merge

Because Simard governs more than one repository, the gated self-merge authority
is **repo-parameterized**: the same objective-gates + merge-judge pipeline that
lands a `rysweet/Simard` PR can squash-merge a merge-ready PR in **any repo
Simard governs** (for example supply-chain-hardening PRs in `rysweet/azlin` or
`rysweet/gadugi-agentic-test`). Engineers reach it through
`simard merge-pr <PR> --repo <owner/repo>` (default `rysweet/Simard`) instead of
a bare `gh pr merge`, so cross-repo merges run through the **gated** authority,
not an ungated shortcut. See the
[cross-repo merge authority reference](../reference/cross-repo-merge-authority.md).

> **Frozen-pin build-dependency repos have one extra step.** The repos Simard
> pins by exact git rev in her own `Cargo.toml` — `rysweet/amplihack-rs`,
> `rysweet/amplihack-memory-lib`, `rysweet/RustyClawd` — merge cross-repo through
> the same gated authority, but landing the upstream PR is **not** the finish
> line: the engineer must then **bump the matching pin** and re-verify
> `cargo build`. The examples above use non-pinned governed repos so the
> merge-and-done path stays unambiguous; see the
> [cross-repo merge authority reference](../reference/cross-repo-merge-authority.md)
> for the pin-bump caveat.

## Invariants

- **No human wait for routine work.** Goal promotion, improvement promotion, and
  merge of clean, green, merge-ready PRs proceed without an operator sign-off
  step.
- **Evidence gates intact.** CI-green, scope, tests/QA, docs-links, base-branch
  allowlist, and the merge-judge verdict are all still required before merge.
- **HIGH-RISK still gated.** The five HIGH-RISK operations surface to the
  operator and are never auto-executed under autonomy.
- **Real blockers still block.** A genuinely required human reviewer (a
  branch-protection-mandated approval Simard cannot satisfy) is still recorded as
  a specific blocker; only the *absence* of an approver is not.
- **Concurrency unchanged.** The AIMD cap and its backoff still bound real
  parallelism; the goal-board cap governs how many goals may *exist*, not how
  many engineers run at once.

## Related reading

- [Goal stewardship mode](./stewardship-mode.md) — the durable backlog and
  top-N goal discipline the autonomous curator maintains.
- [Cross-repo merge authority](../reference/cross-repo-merge-authority.md) — the
  repo-parameterized gated squash-merge and its `--repo` flag.
- [PR-finalization review pipeline](../reference/pr-finalization-pipeline.md) —
  the bounded review→merge pipeline whose merge-ready gate this model leaves
  intact while reinterpreting only the `merge-ready` skill's
  required-reviews/approvals criterion.
- [Deploy-aware done-gate](./deploy-aware-done-gate.md) — why a goal is done only
  with a merged PR and a closed issue; autonomy speeds the path to that gate, it
  does not remove the gate.
- [ADO ACL self-escalation guard](../reference/ado-acl-self-escalation-guard.md)
  — the security floor behind HIGH-RISK item #4.
- [Goal decomposition](../reference/goal-decomposition.md) and
  [maximum safe parallelism](../reference/maximum-safe-parallelism.md) — how the
  raised goal-board cap and the AIMD concurrency cap interact.
