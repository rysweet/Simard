---
title: The Overseer escalates the PR that converges the active deploy gate
description: >
  How the Overseer's verify-and-merge escalation stopped ignoring a green, mergeable,
  non-draft PR that fixes the very deploy gate blocking every self-deploy. When
  `ObservedState.deploy_drift` is present (red-canary / DeployDrift active), the ready-PR
  ranking now surfaces a gate-converging PR (e.g. #4505) FIRST as a `VerifyAndMergePr`
  candidate via the existing opt-in merge path — a set-preserving re-ordering that never
  widens merge authority and never auto-merges unsafely.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
issue: 4505
related:
  - ../design/agentic-observe-orient-merge-queue.md
  - ./autonomous-self-merge-sensor.md
  - ./autonomous-merge-review-gate.md
  - ./reconcile-and-self-deploy.md
  - ./deploy-aware-done-gate.md
---

# The Overseer escalates the PR that converges the active deploy gate

> **Status: implemented (PR #4505, DeployDrift / red-canary context).** When a
> deploy gate is red (unit-test canary failing) and `DeployDrift` shows the
> running binary is behind merged `main`, the Overseer now **surfaces a green,
> mergeable, non-draft PR that converges that gate** as a verify-and-merge
> candidate — instead of escalating only unrelated PRs while the blocker
> persists. Primary sources:
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
> (`project_ready_prs`, `decide`, deploy-drift wiring) and
> [`src/overseer/merge_ops.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_ops.rs)
> (`survey_ready_prs`).

## The defect this fixes

PR `rysweet/Simard#4505` — *"fix(self-deploy): converge red-canary gate (env
isolation + …)"* — was **green** (checks `SUCCESS: 18`), `MERGEABLE`, non-draft,
authored by the Simard automerge author, and it **converged the self-deploy
red-canary gate that was blocking every self-deploy**. Yet from 07:55→13:14Z
every Overseer tick reported:

```text
deploy … failed — deploy_gate: red canary (unit-test exit status 101)
DeployDrift: running binary 1 commit behind merged main
```

…and the Overseer only ever escalated verify-and-merge for **#4440** and
**#4398** — never **#4505**, the PR that would *clear the very gate it kept
failing on*. The daemon was deadlocked against its own fix: the change that would
turn the canary green sat un-escalated while the red canary blocked deploys.

Root cause: the ready-PR **candidate selection did not correlate the active
deploy blocker to a PR that resolves it.** `project_ready_prs` filtered and
emitted eligible PRs, but nothing gave a *gate-converging* PR any priority — so
when the candidate window was dominated by other eligible PRs, the one PR that
mattered for the standing `DeployDrift` blocker was never surfaced for
escalation.

## The fix, in one sentence

When `DeployDrift` is active, **re-order** the already-authorized ready-PR set so
a PR that converges the active deploy gate is surfaced first as a
`VerifyAndMergePr` candidate — a *permutation*, never an expansion, of the merge
set.

## What "converges the gate" means (correlation, not authorization)

A candidate PR is treated as **gate-converging** only when it clears a concrete,
objective **minimum anchor** — never on title/branch heuristics alone:

- the PR is in the **same repo** as the active `DeployDrift` (`rysweet/Simard`);
- **it proves Simard-origin** — its `PrSnapshot.labels` carry the durable
  `simard-autonomous` marker, matched whole-string by
  [`is_engineer_pr_label`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs)
  (`SIMARD_ENGINEER_PR_LABEL`), or its `head_ref` rides an engineer-exclusive
  branch namespace (`is_engineer_branch`) — the same G3 narrowing
  `project_ready_prs` already applies;
- it is **green + `MERGEABLE` + non-draft** (already required by the objective
  gates); and
- **it carries an explicit, whole-string `converges-gate` label** — the concrete
  per-PR predicate that identifies *which* Simard-origin PR resolves the standing
  deploy gate. This label is the objective anchor for correlation: it is stamped
  by the engineer on the gate-fixing PR at `gh pr create` time and matched
  exactly (like `simard-autonomous`), so a look-alike (`converges-gate-ish`)
  never qualifies. The `DeployDrift` must also be active
  (`ObservedState.deploy_drift = Some`) for the label to have any ranking effect.

The `converges-gate` label is the **sole** correlation predicate: there is no
title/branch heuristic and no `target_commit` match in the ranking. Title text
(e.g. a `fix(self-deploy)` / red-canary hint) has **no** ranking effect — a PR
without the explicit label is never promoted.

> **Why a label and not a commit match.** The ready-PR projection
> (`ProjectionCandidate` / `PrSnapshot`) carries `labels`, `head_ref`,
> `mergeable`, checks and base — but **no PR head SHA** — so a candidate cannot be
> matched to `DeployDriftObservation.target_commit` at ranking time. The explicit
> `converges-gate` label is therefore the minimum objective, spoofing-resistant
> predicate that is actually available in the projected state.

Correlation is **advisory ordering only**. It changes *which* authorized PR is
looked at first; it never makes an unauthorized PR eligible. A PR must already
have passed every objective gate (author allowlist + `simard-autonomous`
label/engineer-branch + green + `MERGEABLE` + non-draft + base allowlist) to be
in the set that gets re-ordered.

## The ranking seam

A new **pure** helper sits beside `project_ready_prs` and is a set-preserving
permutation of its output:

```rust
/// Re-order `authorized` so any PR that converges the active deploy gate sorts
/// first, WITHOUT adding or removing any element. Pure and O(n); performs
/// no network I/O. Returns exactly the same multiset it was given.
///
/// The input `authorized` set has already passed every objective/authority gate
/// (author allowlist + Simard-origin + green + `MERGEABLE` + non-draft + base
/// allowlist) via `project_ready_prs`, so origin is NOT re-checked here. Within
/// that set, a PR is "gate-converging" iff its matching `ProjectionCandidate`
/// carries the explicit whole-string `converges-gate` label
/// ([`config::is_converges_gate_label`]). There is no PR head SHA in the
/// projection, so no `target_commit` match is attempted, and there is no
/// title/branch heuristic — the label is the sole, spoofing-resistant anchor.
///
/// `deploy_drift` is `ObservedState.deploy_drift`: `None` ⇒ identity (no
/// re-ordering).
pub fn prioritize_gate_converging_prs(
    authorized: &[PrRef],
    candidates: &[ProjectionCandidate],
    deploy_drift: Option<&DeployDriftObservation>,
) -> Vec<PrRef>;
```

The `decide()` / `project_ready_prs` call-site threads
`ObservedState.deploy_drift` into the ranking so the gate-converging PR is
surfaced/prioritised as a `VerifyAndMergePr` candidate **alongside** #4440 /
#4398 — through the *same* path they already use.

```text
observe → ObservedState { ready_prs, deploy_drift: Some(DeployDrift{..}), .. }
                              │
        project_ready_prs (author guard + engineer-PR + draft + objective gates)
                              │  ← unchanged authorized set
        prioritize_gate_converging_prs(ready_prs, candidates, deploy_drift)
                              │  ← permutation: gate-converging PR first
        decide → Intervention::VerifyAndMergePr { repo, pr: 4505 }
                              │
        allow_verify_merge gate + MergeJudge + anti-recursion author guard
                              │  (opt-in; unchanged)
        gh pr merge --squash --delete-branch   (NO --admin / NO --no-verify)
```

## Authorization is unchanged (the safety invariant)

This feature is deliberately a **re-ordering of an already-authorized set**. It
does **not**:

- widen `MergeAuthority` / `RiskClass::MergeAuthority`;
- change the opt-in `allow_verify_merge` gate (verify-and-merge still escalates
  only when the operator has enabled it);
- bypass `MergeJudge`, the objective gates (`MERGEABLE` + all checks green +
  base allowlist), the draft gate, or the anti-recursion author guard;
- use `--admin` or `--no-verify`;
- auto-merge. The gate-converging PR is surfaced through the **existing**
  `VerifyAndMergePr` path exactly as #4440 / #4398 are — an operator-gated
  verify-and-merge, not a silent self-merge.

A unit test asserts the ranking output is a **permutation of its input**
(`sorted(out) == sorted(in)`), so no PR can ever *enter* the merge set through
this seam.

> **Self-deploy recursion guard preserved.** The agent must still refuse to merge
> its own gate-fixing PR when the anti-recursion author guard would forbid it;
> this ranking never relaxes that guard. Surfacing a candidate is not merging it.

## API summary

| Symbol | Location | Role |
| --- | --- | --- |
| `prioritize_gate_converging_prs` | `src/overseer/mod.rs` | Pure, set-preserving re-ordering; gate-converging PR first when drift is active. |
| `ObservedState.deploy_drift` | `src/overseer/capabilities.rs` | The active `DeployDriftObservation`; `None` ⇒ identity ranking. |
| `DeployDriftObservation` | `src/overseer/capabilities.rs` | `{ target_commit, behind_commits }` — the standing blocker to converge on. |
| `project_ready_prs` | `src/overseer/mod.rs` | Unchanged authorized-set projection; its output is what gets re-ordered. |
| `Intervention::VerifyAndMergePr` | `src/overseer/intervention.rs` | The existing opt-in merge path the candidate is surfaced through. |

## Configuration

No new environment variables or config keys. Behavior depends only on state
already observed, plus one new **repo-level label convention** (`converges-gate`,
analogous to the existing `ooda-stuck` / `simard-autonomous` labels):

| Existing setting | Effect on this feature |
| --- | --- |
| `allow_verify_merge` (opt-in) | Must be enabled for any verify-and-merge escalation, gate-converging or not — unchanged. |
| `SIMARD_AUTOMERGE_AUTHOR` | The candidate must be authored by this login (existing `survey_ready_prs` requirement). |
| `SIMARD_AUTOMERGE_REPOS` / base allowlist | The candidate's repo must pass the existing objective base-allowlist gate. |
| DeployDrift observer wired | When `ObservedState.deploy_drift` is `Some`, gate-converging ranking activates; when `None`, ranking is the identity. |
| `converges-gate` label (on the gate-fixing PR) | The explicit whole-string label the engineer stamps on the PR that resolves the standing deploy gate; it is the objective per-PR anchor the ranking keys on. No label ⇒ no gate-converging prioritisation (ranking falls back to identity). |

## Examples

### Before — the deadlock

```text
07:55Z tick: deploy failed — deploy_gate: red canary (exit 101)
             DeployDrift: 1 commit behind merged main
             ready_prs = [#4440, #4398]      ← #4505 present but not surfaced
             escalate VerifyAndMergePr #4440
             escalate VerifyAndMergePr #4398
             (#4505 — the gate fix — never escalated)
…13:14Z: same red canary, same DeployDrift, still no #4505
```

### After — the gate-converging PR is surfaced first

```text
tick: DeployDrift active (target_commit=abc123, behind=1)
      authorized ready_prs = [#4440, #4398, #4505]
      #4505 carries labels [simard-autonomous, converges-gate]  ← objective anchor
      prioritize_gate_converging_prs → [#4505, #4440, #4398]
      decide → VerifyAndMergePr #4505   (opt-in gate + MergeJudge + author guard)
      → gh pr merge --squash --delete-branch  (no --admin / no --no-verify)
      red canary clears; DeployDrift resolves on next deploy
```

Expected structured log lines (no `print!`/`println!`):

```text
[overseer::merge] deploy-gate-converging PR prioritised for verify-and-merge repo=rysweet/Simard pr=4505 behind=1
[overseer::merge] verify-and-merge escalation repo=rysweet/Simard pr=4505
```

## Verifying the behavior

Tests in `src/overseer/tests_deploy_gate_escalation.rs` assert:

- **`deploy_gate_converging_pr_is_ranked_first_under_active_drift`** — with
  `DeployDrift` active and a green/mergeable/non-draft gate-converging PR in the
  authorized set, that PR is ranked first (ahead of #4440 / #4398) so it becomes a
  `VerifyAndMergePr` candidate.
- **`ranking_is_a_set_preserving_permutation_no_authority_widening`** —
  `prioritize_gate_converging_prs` output is a permutation of its input
  (`sorted(out) == sorted(in)`); it never adds or drops a PR.
- **`a_converging_candidate_not_in_the_authorized_set_is_never_injected`** — a
  gate-converging candidate that is not already in the authorized set is never
  promoted into it; the ranking only re-orders authorized PRs.
- **`no_drift_leaves_order_unchanged_even_with_a_converging_label`** — with
  `deploy_drift = None` the ranking is the identity even when a `converges-gate`
  label is present; ordinary escalation is unaffected.
- **`without_the_converges_gate_label_no_pr_is_promoted`** — only a PR carrying
  the explicit whole-string `converges-gate` label qualifies as gate-converging.
  A PR that merely has a `fix(self-deploy)` title but lacks the label is **not**
  ranked first; the label is the sole predicate — title/branch text has no
  ranking effect.

Quality gates that must stay green: `scan_no_stray_prints` and
`scan_no_bridge_naming`.

## Why this is safe

- **Additive / non-breaking.** A new pure helper plus a minimal `decide()`
  call-site edit; `project_ready_prs` and the merge gates are unchanged. The PRD
  is preserved.
- **Set-preserving.** The seam can only *re-order* an authorized set, proven by a
  permutation test — it can never widen who is eligible to merge.
- **Opt-in preserved.** Verify-and-merge still requires `allow_verify_merge`; a
  gate-converging PR is surfaced, not auto-merged.
- **High-confidence correlation.** Gate-converging status requires a concrete
  objective anchor — Simard-origin (the durable `simard-autonomous` label or an
  engineer branch) plus an explicit whole-string `converges-gate` label — never
  text heuristics alone, so no PR can impersonate the gate fixer with a look-alike
  title.
- **Minimal surface.** P2 touches one pure helper and one call-site to avoid
  merge conflicts with concurrent worktrees on the large `mod.rs`.

## See also

- [Design: agentic observe/orient merge-queue + issue reasoning](../design/agentic-observe-orient-merge-queue.md)
  — the `reasoned_prs → ready_prs` re-narrowing this ranking sits behind.
- [Autonomous self-merge sensor](./autonomous-self-merge-sensor.md) — the
  `survey_ready_prs` author-scoped sensor.
- [Autonomous-merge review gate](./autonomous-merge-review-gate.md) — the
  `MergeJudge` authorization this never bypasses.
- [Reconcile-and-self-deploy](./reconcile-and-self-deploy.md) and
  [Deploy-aware done-gate](./deploy-aware-done-gate.md) — the DeployDrift /
  red-canary context.
