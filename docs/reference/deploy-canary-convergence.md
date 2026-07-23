---
title: Deploy-canary convergence reference
description: >
  Reference for the self-deploy canary convergence path — why a red canary
  blocks a deploy, how a genuinely-stale-but-sound binary is driven to converge
  without weakening the deploy gate, the fail-closed drift-read contract, and the
  gate-preservation regression test that guards the RedCanary trust boundary.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: proposed
related:
  - ./self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/converge-a-stuck-deploy-canary.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../../src/self_relaunch/canary.rs
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_deploy/drift.rs
  - ../../src/overseer/deploy.rs
---

# Deploy-canary convergence reference

> **Status: proposed (design spec).** The convergence driver in
> [`src/self_relaunch/canary.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/canary.rs)
> and the transient-failure classifier are
> **not yet implemented** — this document is the specification they must satisfy.
> The gate-preservation regression test **is** implemented and shipped by this
> change (see [Gate-preservation regression test](#gate-preservation-regression-test) below).
> The pieces they build on already exist and ship today: the fail-closed drift
> read in
> [`src/self_deploy/drift.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/drift.rs)
> (`try_detect`/`origin_strict`), the canary primitives
> (`build_self_deploy_candidate`, `verify_canary`, `all_gates_passed`), and the
> self-deploy pipeline documented in [self-deploy-api](./self-deploy-api.md). The
> deploy gate ([`evaluate_deploy_gate`](../../src/overseer/deploy.rs)) is **frozen**
> — its signature, thresholds, and refusal variants must not change.

This reference specifies how the self-deploy pipeline **converges** when the
running daemon is behind merged `main` but the deploy is stuck at a red canary.
It exists because a deploy can wedge in one of two shapes that look identical
from the overseer's perspective — a *sound* binary the canary keeps failing to
promote, and a *genuinely broken* binary the canary correctly refuses — and the
correct remediation for each is opposite. The rule that reconciles them is a
single invariant:

> **The deploy gate is immutable. Convergence re-triggers a valid canary build;
> it never relaxes, bypasses, or overrides the gate.**

## Contents

- [The stuck-canary problem](#the-stuck-canary-problem)
- [Convergence model](#convergence-model)
- [`evaluate_deploy_gate` is frozen](#evaluate_deploy_gate-is-frozen)
- [Fail-closed drift reads](#fail-closed-drift-reads)
- [Convergence driver API](#convergence-driver-api)
- [Gate-preservation regression test](#gate-preservation-regression-test)
- [Tracing and observability](#tracing-and-observability)
- [Security prerequisites](#security-prerequisites)

## The stuck-canary problem

The overseer emits a `DeployDrift` signal when the running binary is behind
merged `main`:

```text
DeployDrift — running binary is 1 commit(s) behind merged main — self-deploy required
```

The self-deploy path then builds a **canary** of the target commit, runs the
canary gates, and only promotes it if every gate passes. A deploy is *stuck*
when the same target commit fails the `deploy_gate` red-canary check across
consecutive overseer cycles while `needs_deploy` stays `true` — the drift never
self-heals because the canary never goes green.

Two root causes produce the identical symptom:

| Root cause | Correct remediation |
|------------|---------------------|
| The canary **build/verify** transiently failed (runner env, stripped git vars, warm target dir stale) even though the commit builds green in CI. | **Converge:** re-trigger a valid canary build for the same target commit. |
| The target commit is a **genuine regression** — the canary build or a gate legitimately fails. | **Do not deploy.** The red canary is correct; the fix is a new commit, not a gate change. |
| The drift read is **malformed/absent** (git probe error). | **Fail closed:** report "unknown", block the deploy — never treat unknown as "no drift". |

Convergence only ever applies to the **first** row. It is a no-op for a truly
red canary and for an unknown drift state.

## Convergence model

Convergence is the act of *re-attempting a valid canary build* for a target
commit whose gate refusal was `RedCanary` and whose failure evidence points at a
transient build/verify fault rather than a code regression. It is driven from
the self-deploy orchestrator path and reuses the existing canary primitives
verbatim:

- [`build_self_deploy_candidate`](../../src/self_relaunch/canary.rs) — builds the
  already-checked-out target commit into the warm target dir, neutralizing
  ambient `GIT_DIR`/`GIT_WORK_TREE` so `SIMARD_GIT_HASH` is derived from the
  package's own checkout.
- [`verify_canary`](../../src/self_relaunch/gates.rs) — runs the configured
  gates against the freshly-built binary.
- [`all_gates_passed`](../../src/self_relaunch/gates.rs) — the boolean that feeds
  `DeployContext.canary_passed`.

The convergence decision is:

```text
if drift.needs_deploy
   && last_refusal == RedCanary
   && canary_failure_is_transient(evidence)
   && target_commit is a trusted merged commit:
       re-build canary for target_commit
       re-run verify_canary
       feed all_gates_passed(...) into evaluate_deploy_gate
else:
       leave the deploy blocked; surface the refusal to the operator
```

`canary_failure_is_transient` classifies the surfaced `SimardError` from the
prior build/verify attempt: a build **spawn/IO** failure or a missing-artifact
error is treated as transient (retriable); a build that completed with a
non-zero compiler exit, or a failing test/health gate, is treated as a genuine
regression (**not** retriable). When in doubt, the classification is
**not-transient** — convergence never masks a real failure.

Convergence is bounded by the existing self-deploy min-interval anti-thrash
guard (see [self-deploy-api](./self-deploy-api.md)); a target commit that keeps
failing a re-triggered build stops being retried and stays surfaced to the
operator rather than looping.

## `evaluate_deploy_gate` is frozen

The deploy gate is a security trust boundary. Convergence changes **nothing**
about it. For the avoidance of doubt, this is the frozen contract:

```rust
pub fn evaluate_deploy_gate(ctx: &DeployContext) -> Result<(), DeployRefusal> {
    if commits_equivalent(&ctx.running_commit, &ctx.target_commit) {
        return Err(DeployRefusal::NoOp);
    }
    if ctx.target_is_ancestor_of_running {
        return Err(DeployRefusal::Rollback);
    }
    if !ctx.canary_passed {
        return Err(DeployRefusal::RedCanary);   // ← immutable
    }
    if ctx.recent_restart_churn >= CRASH_LOOP_CHURN_THRESHOLD {
        return Err(DeployRefusal::CrashLoop { churn: ctx.recent_restart_churn });
    }
    Ok(())
}
```

Frozen means:

- `DeployContext`'s fields and their meanings are unchanged.
- `DeployRefusal`'s variants (`NoOp`, `Rollback`, `RedCanary`, `CrashLoop`) are
  unchanged.
- `CRASH_LOOP_CHURN_THRESHOLD` is unchanged.
- There is **no** override flag, environment variable, or "force" path that lets
  a caller promote a binary with `canary_passed == false`.

Convergence influences the gate **only** by legitimately flipping
`canary_passed` from `false` to `true` via a *real* passing build. It cannot
reach the gate any other way.

## Fail-closed drift reads

A convergence decision keys off `DeployDrift`, so a *wrong* drift read is as
dangerous as a wrong gate. Two distinct mechanisms keep the autonomous path safe,
and it is worth being precise about which does what:

- **`origin_strict()` / `head_fallback: false` — the current autonomous-trigger
  guarantee.** The autonomous deploy trigger
  ([`src/overseer/deploy_trigger.rs`](../../src/overseer/deploy_trigger.rs))
  builds its [`GitDeploySource`](../../src/self_deploy/drift.rs) with
  `origin_strict()`, so an unresolvable `origin/main` **errors** instead of
  falling back to a local `HEAD`. It then calls `ReconcileDetector::detect`,
  whose fail-**safe** contract turns that error into `needs_deploy: false` — i.e.
  the daemon **refuses to deploy an unverified `HEAD`**. Safe for *triggering*,
  because "do nothing" is the safe default there.
- **`try_detect` — the explicit-unknown API the convergence decision will use.**
  [`ReconcileDetector::try_detect`](../../src/self_deploy/drift.rs) returns the
  git/source error as `Err` — an explicit "unknown" — instead of collapsing it to
  `needs_deploy: false`. Convergence needs this distinction: it must tell "drift
  unknown" apart from "confirmed current" so an unknown state **blocks the
  convergence attempt** rather than silently reading as "no drift." (Today
  `try_detect` is used by the outcome-verify Rail-3, #2751; the convergence driver
  adopts the same explicit-unknown read.)
- `behind_commits` is parsed defensively: a malformed or absent count is an
  error, not a silent `0`. An unknown drift state blocks convergence (and thus
  the deploy) rather than fabricating a "current" or "stale" verdict.

The governing rule: **on any drift/canary/gate error path, fail closed.** An
unknown state must never promote a binary and must never silently suppress a
needed deploy — it surfaces to the operator.

## Convergence driver API

The convergence driver reuses the self-deploy orchestrator's existing entry
points; it introduces no new public deploy verb and no new CLI subcommand. An
operator interacts with it through the existing surface:

| Action | Command |
|--------|---------|
| Inspect current drift (running-vs-merged) | `simard self-deploy --check` |
| Drive one deploy/convergence attempt for the running host | `simard self-deploy` |
| Verify the running commit advanced | `simard self-deploy --check` (see [verify-and-roll-back](../howto/verify-and-roll-back-a-self-deploy.md)) |

`simard self-deploy` performs exactly one bounded deploy attempt and **ends** (it
is not a loop): read drift → build the canary → `verify_canary` →
`evaluate_deploy_gate` → promote iff the gate passes. This **operator-triggered**
command uses the fail-**safe** drift read (`ReconcileDetector::detect`), matching
`simard self-deploy --check`; it is the *autonomous overseer convergence path*
(not this manual command) that reads drift fail-**closed** via `try_detect`, so an
unknown drift state blocks an unattended deploy. `simard self-deploy --check` is
the read-only companion that reports the same drift a deploy would act on without
changing anything. The `RedCanary` refusal itself is surfaced to the
operator through the daemon's `deploy-refused` notification and logs
([`overseer::notify`](../../src/overseer/notify.rs)), not as a `self-deploy
--check` field. **Proposed:** a `--canary-evidence` surface exposing the last
canary failure and its transient/regression classification (see the runbook) is
part of this spec and does not exist yet.

## Gate-preservation regression test

A regression test in
[`src/self_relaunch/canary.rs`](../../src/self_relaunch/canary.rs) locks the
invariant so no future convergence change can silently weaken the gate:

```text
test deploy_gate_red_canary_is_immutable_for_forward_deploys
  GIVEN a DeployContext for a genuine forward deploy
        (target_commit != running_commit, not a rollback)
    AND canary_passed = false
  WHEN evaluate_deploy_gate(&ctx) is called for every churn level
  THEN it returns Err(DeployRefusal::RedCanary) in every case
```

The test asserts the gate returns `RedCanary` for *every* `canary_passed ==
false` forward-deploy context regardless of restart churn, and that convergence
exposes no code path that promotes a binary while `canary_passed` is `false`. Its
companions `deploy_gate_still_allows_clean_forward_deploy` (a passing canary is
not over-tightened into a refusal) and `crash_loop_threshold_is_frozen` (the
`CRASH_LOOP_CHURN_THRESHOLD` safety constant is pinned) round out the guard. If a
change makes the gate promote an un-passed canary, this test fails.

## Tracing and observability

All new convergence diagnostics use structured `tracing` spans and OTel — no
`print!`/`println!` in new code. Each convergence attempt emits a span with:

- `target_commit` (SHA, id only)
- `refusal` (the `DeployRefusal` variant that triggered the attempt)
- `transient` (the `canary_failure_is_transient` verdict)
- `outcome` (`converged` / `still_red` / `skipped_nontransient` / `skipped_unknown_drift`)

Spans record **lengths and identifiers only** — never prompt bodies, build
output, or any `GITHUB_TOKEN`/canary environment values. The pre-existing
cost-write-failure `eprintln!` elsewhere in the tree is intentionally left
untouched; the "no `print!`" rule applies to new code.

## Security prerequisites

- **Gate immutability.** Weakening `RedCanary`/`CrashLoop` to force a promotion
  is treated as privilege escalation of unverified code and is disallowed.
- **Fail closed everywhere.** Every drift/canary/gate error path blocks the
  deploy; unknown is never promoted.
- **Trusted refs only.** Convergence rebuilds only an in-repo, merged, trusted
  commit. It never builds or promotes an unpinned, detached, or
  attacker-influenceable ref (mirrors the `head_fallback: false` requirement on
  the autonomous drift source).
- **Argument-vector subprocesses.** Any git/cargo invocation on the convergence
  path passes refs/SHAs as `Command::arg` vectors, never interpolated into a
  shell string; commit-ishes are validated (`is_hex_commitish`) before use.
- **Least privilege.** Convergence reuses the existing `gh`/`GITHUB_TOKEN`
  scopes — no new PATs, secrets, or cross-repo write.

## See also

- [How to converge a stuck deploy canary](../howto/converge-a-stuck-deploy-canary.md)
- [Self-deploy API reference](./self-deploy-api.md)
- [Reconcile and self-deploy](../concepts/reconcile-and-self-deploy.md)
