---
title: How to converge a stuck deploy canary
description: >
  Operator runbook for a self-deploy that is wedged at a red canary — confirm the
  binary is genuinely behind merged main, decide whether the red canary is a
  transient build fault or a real regression, drive convergence with one bounded
  attempt, and verify the running commit advanced. Never weaken the deploy gate.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: howto
status: proposed
related:
  - ../reference/deploy-canary-convergence.md
  - ../reference/self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ./verify-and-roll-back-a-self-deploy.md
  - ./run-self-deploy-from-any-directory.md
---

# How to converge a stuck deploy canary

> **Status: proposed (design spec).** `simard self-deploy` and `simard
> self-deploy --check` exist today; the transient/regression **classification**
> and the `--canary-evidence` surface described in Step 2 are **proposed** and not
> yet implemented. Convergence reuses the existing self-deploy orchestrator and
> the canary primitives in
> [`src/self_relaunch/`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/canary.rs).
> The deploy gate is frozen — this runbook never weakens it. For the design,
> see [deploy-canary-convergence](../reference/deploy-canary-convergence.md).

This guide is for an operator when the overseer keeps reporting

```text
DeployDrift — running binary is 1 commit(s) behind merged main — self-deploy required
```

across consecutive cycles and the self-deploy never lands because the canary
stays red. You will decide **which** of the two stuck shapes you have and apply
the matching remediation.

## Prerequisites

- The `simard` daemon is installed at `~/.simard/bin/simard` and managed by
  systemd (`simard-ooda` user unit).
- You can read `~/.simard/state/` and run `simard` on the host.
- The target commit is a **merged** commit on `main` (branch-protected, signed
  merge). Convergence never builds an un-merged or detached ref.

## Step 1 — Confirm the binary is genuinely behind

Ask the daemon for its drift with the read-only drift check:

```console
$ simard self-deploy --check
simard self-deploy --check:
  running commit : 56b10bef5057
  merged head    : 9f3c1a2b8de4
  behind commits : 1
  drifted pins   : (none)
  needs deploy   : YES
```

- `needs deploy: YES` with `behind commits >= 1` confirms real drift.
- The `RedCanary` refusal that is keeping the deploy stuck is surfaced separately,
  through the daemon's `deploy-refused` operator notification and logs
  ([`overseer::notify`](../../src/overseer/notify.rs)) — it is not a
  `--check` field.

Note that `self-deploy --check` is the fail-**safe** operator report: it uses
the local-`HEAD` fallback and warns loudly on a failed `origin` fetch rather than
erroring. The **autonomous** self-deploy path is the fail-**closed** one: it
builds its drift source with `origin_strict` (`head_fallback: false`), so an
unresolved `origin/main` errors instead of deploying an unverified local `HEAD` —
the daemon refuses to act rather than promote unverified code. The proposed
convergence decision reads that drift with `try_detect`, whose explicit "unknown"
blocks a convergence attempt instead of silently reading as "no drift."


## Step 2 — Classify the red canary: transient or regression

A red canary has two causes with opposite fixes. Read the last canary failure
evidence. **Today** that evidence is in the daemon's `deploy-refused`
notification and logs; the dedicated summary command below is **proposed** (part
of this spec):

```console
# Proposed — not yet implemented:
$ simard self-deploy --canary-evidence
last_canary_error: build failed to start: No such file or directory (os error 2)
classification:    transient
```

| `classification` | Meaning | What to do |
|------------------|---------|------------|
| `transient` | The canary **build/verify** failed for an environmental reason (spawn/IO error, missing artifact, stale warm target dir) — the commit itself builds green in CI. | **Converge** — go to Step 3. |
| `non-transient` | The build completed with a compiler error, or a test/health gate failed — the commit is a **genuine regression**. | **Do not deploy.** Land a fix commit on `main`; the red canary is correct. |

Cross-check against CI: if the target commit's required checks are green on
`main`, a red canary is almost certainly `transient`. If CI is red for that
commit, the canary is correctly refusing a regression.

> **Never** try to "unstick" a `non-transient` red canary by forcing a deploy.
> There is no force flag, and adding one would ship unverified code — a
> gate-weakening change is rejected in review.

## Step 3 — Drive one convergence attempt

For a `transient` red canary, trigger exactly one bounded deploy attempt:

```console
$ simard self-deploy
[self-deploy] drift: behind_commits=1 needs_deploy=YES (operator fail-safe read)
[self-deploy] last_refusal=RedCanary classification=transient → converging
[self-deploy] rebuilding canary for 9f3c1a2b8de4 (warm target dir)
[self-deploy] verify_canary: 4/4 gates passed
[self-deploy] evaluate_deploy_gate: OK (canary_passed=true)
[self-deploy] promoting 9f3c1a2b8de4 …
```

What this does, in order:

1. Reads drift. The manual `simard self-deploy` uses the fail-**safe** operator
   read (`ReconcileDetector::detect`), like `--check`; the *autonomous overseer*
   convergence path is the fail-**closed** one (`try_detect`), where an unknown
   drift state aborts the unattended deploy rather than acting on a stale local
   `HEAD`.
2. Rebuilds the canary for the target commit into the warm target dir, with
   ambient `GIT_DIR`/`GIT_WORK_TREE` neutralized so the embedded
   `SIMARD_GIT_HASH` is correct.
3. Runs `verify_canary` and feeds `all_gates_passed(...)` into
   `evaluate_deploy_gate`.
4. Promotes the binary **only if the gate passes**. The gate is unchanged: a
   still-red canary yields `RedCanary` again and no promotion.

`simard self-deploy` performs a single attempt and returns; it does not loop. The
min-interval anti-thrash guard prevents a commit that keeps failing from being
retried on a hot loop.

## Step 4 — Verify the running commit advanced

```console
$ simard self-deploy --check
simard self-deploy --check:
  running commit : 9f3c1a2b8de4
  merged head    : 9f3c1a2b8de4
  behind commits : 0
  drifted pins   : (none)
  needs deploy   : no
```

`behind commits: 0` and `needs deploy: no` confirm the deploy converged and
the running daemon is now at merged `main`.

If the canary went red again on the rebuild, `self-deploy --check` still shows
`needs deploy: YES` and the daemon re-emits a `deploy-refused` (`RedCanary`)
notification. Re-read the canary evidence (Step 2): a now-`non-transient`
classification means the "transient" diagnosis was wrong — treat it as a
regression (Step 2, second row).

## Rollback

If a converged deploy misbehaves after promotion, use the existing rollback
path — convergence changes nothing about recovery:

```console
$ simard rollback
```

See [verify-and-roll-back-a-self-deploy](./verify-and-roll-back-a-self-deploy.md).

## What this runbook will never tell you to do

- Weaken, bypass, or add a "force" flag to the deploy gate.
- Promote a binary while `canary_passed == false`.
- Treat an unknown drift state as "no drift" or "current".
- Build/promote an un-merged, detached, or untrusted ref.

## See also

- [Deploy-canary convergence reference](../reference/deploy-canary-convergence.md)
- [Self-deploy API reference](../reference/self-deploy-api.md)
- [Reconcile and self-deploy](../concepts/reconcile-and-self-deploy.md)
