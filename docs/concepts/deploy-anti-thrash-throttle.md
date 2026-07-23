---
title: "Concept: deploy anti-thrash throttle (stop re-deploying a red-canary commit)"
description: >
  Why Simard's autonomous self-deploy needs a restart-durable, fail-closed
  anti-thrash throttle — how the process-global min-interval guard alone let a
  single red-canary commit thrash every overseer tick, and how the durable
  per-commit `DeployAttemptLedger` (#4390) makes the loop converge by
  remembering known-bad commits across restarts and backing off instead of
  churning.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ../reference/overseer-deploy-throttle-api.md
  - ../howto/configure-overseer-deploy-throttle.md
  - reconcile-and-self-deploy.md
  - deploy-aware-done-gate.md
  - gap-scan-backoff-dedup.md
  - ../reference/self-deploy-api.md
  - ../reference/overseer-backoff-gate-api.md
  - ../safe-self-update.md
  - ../../src/overseer/deploy_throttle.rs
  - ../../src/overseer/deploy_trigger.rs
  - ../../src/overseer/mod.rs
---

# Concept: deploy anti-thrash throttle

> **Status: implemented ([#4390](https://github.com/rysweet/Simard/issues/4390)).**
> The durable `DeployAttemptLedger` lives in
> [`src/overseer/deploy_throttle.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy_throttle.rs)
> and is wired into the Overseer OBSERVE/ACT rails in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> For the typed surface and config knobs see the
> [deploy-throttle API reference](../reference/overseer-deploy-throttle-api.md).

## The thrash

Simard closes the "merged-but-not-running" gap by
[reconciling-and-self-deploying](reconcile-and-self-deploy.md): each OODA cycle
the Overseer OBSERVEs whether the running binary is behind merged `main`, and if
so it plans a guarded `Deploy` that builds a canary, verifies it, swaps, and
rolls back on a failed health check.

That loop assumes the canary usually goes **green**. When a merged commit's
canary is **red**, the guarded deployer correctly refuses the swap — but nothing
stopped the *next* tick from observing the same drift and re-attempting the same
doomed deploy.

That is exactly what happened in
[#4390](https://github.com/rysweet/Simard/issues/4390). Commit `56b10bef5057`
failed the canary `deploy_gate` on **five consecutive** overseer ticks
(15:10Z–17:38Z). Every tick re-observed:

```text
DeployDrift — running binary is 1 commit behind merged main — self-deploy required
```

so the system never converged: it kept re-attempting an identical failing
deploy, re-sending the operator notice, and burning a canary build each cycle.

## Why the old guard wasn't enough

The only anti-thrash guard on that path was the process-global min-interval
clock, `global_deploy_throttle_allow`
([self-deploy API](../reference/self-deploy-api.md)). It has three properties
that, together, let the thrash through:

1. **Commit-agnostic.** It throttles *any* attempt inside a 15-minute window,
   but forgets *which* commit failed. Once the window elapses it happily
   re-admits the same known-bad SHA.
2. **Restart-resetting.** It is a process `static` (`AtomicU64`). A self-deploy
   attempt can restart the daemon — and a restart resets the clock to "never
   attempted", so the very next tick re-attempts immediately. The guard that was
   supposed to suppress the retry is erased by the thing it was suppressing.
3. **Fail-open.** Missing/empty state defaults to "allow".

A throttle that resets on restart and forgets which commit is bad cannot make a
red-canary loop converge.

## The durable fix

The fix adds a **second layer**: a durable, per-commit `DeployAttemptLedger`
that remembers, for each target SHA, how many times it has failed and until when
it should be suppressed — persisted to disk so it **survives a restart**, and
**fail-closed per-SHA** so a known-bad commit is refused rather than re-tried
when the ledger can't be trusted.

The two guards compose (both must admit before a deploy fires):

- **Layer 1 — fast rate cap** (`global_deploy_throttle_allow`): cheap, per-tick,
  commit-agnostic; unchanged, still fail-open. Caps *how often* any attempt can
  happen.
- **Layer 2 — durable per-commit memory** (`DeployAttemptLedger`): remembers
  *which* commit is bad, across restarts; fail-closed per-SHA. Stops a *specific*
  red-canary SHA from re-firing every tick.

### Three principles

**Restart-durable.** The ledger is written to
`~/.simard/state/deploy-attempt-ledger.json` (atomic tmp+rename, `0600`),
mirroring the [safe-update state](../safe-self-update.md) rail. A restarted
overseer `load`s it and immediately knows `56b10bef5057` failed 5× and is still
inside its backoff — so it does **not** re-deploy.

**Fail-closed, per-SHA.** At OBSERVE the Overseer only knows *there is drift* and
*which SHA* — it does **not** have a live "the canary is red" signal (that is
only learned after an ACT attempt returns an error). The ledger *is* that durable
memory: a past red canary is recorded as `last_deploy_result=failed` with a
`backoff_until`. So when a SHA's record is present but corrupt or ambiguous
(result unset), the throttle returns `FailClosed` and the deploy is refused.
Crucially this is scoped to the *single candidate SHA* in flight this tick — a
never-seen SHA (no record) still deploys, and a *missing* ledger loads empty and
allows. Blocking *all* deploys on any read error would break normal convergence
and deadlock the first-ever deploy; "refuse to re-attempt a commit whose record
we can't trust" is a per-commit statement, so the guard is too.

**Backoff, not hard-stop.** Each failure grows an exponential backoff window
(base = the deploy min-interval, 15 min; capped at 6 h). A `record_success`
clears it. So a commit whose canary later goes green becomes eligible again
without any manual reset — the throttle backs off churn but never becomes a
permanent lockout that an operator has to clear by hand.

## Surfacing, not silence

Convergence must be *observable*. On every suppressed tick the Overseer emits a
single structured `tracing::warn!` with `deploy_throttle.stuck=true`, the
`target_sha`, `failure_count`, `backoff_until`, and the suppression `reason`
(plus the matching OTel span attribute). Instead of a stream of identical
red-canary deploy attempts, the operator sees one throttled, explained warning
per tick — the "surface the stuck state instead of silently looping" the issue
asked for. No `print!`/`println!` is introduced; this reuses the existing
tracing + OTel path.

## What did **not** change

- The guarded deploy SAFETY judgment (canary build+verify, red-canary/rollback/
  crash-loop refusals, operator notification) stays in the `GuardedDeployer` and
  the high-risk `AutonomyGate`. The throttle never swaps a binary; it only
  decides whether an attempt is *admitted*.
- The origin/`main` root-of-trust, the full-hex-SHA target validation, and the
  opt-out (`SIMARD_OVERSEER_AUTONOMOUS_DEPLOY=0`) are unchanged.
- The change is additive and non-breaking: green-path deploys behave exactly as
  before; only a *repeatedly failing* SHA is now backed off.

## See also

- [Overseer durable deploy anti-thrash throttle API](../reference/overseer-deploy-throttle-api.md)
- [Configure the Overseer deploy throttle](../howto/configure-overseer-deploy-throttle.md)
- [reconcile-and-self-deploy](reconcile-and-self-deploy.md)
- [Gap-scan dedup & backoff](gap-scan-backoff-dedup.md) — the sibling backoff rail for coverage gaps
