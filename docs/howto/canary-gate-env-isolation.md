---
title: How to diagnose and confirm the canary gate env-isolation fix
description: >
  Operator runbook for the persistently-RED self-deploy canary caused by
  gate environment leakage — recognise the "green in CI, exit-101 on the host"
  signature, confirm the hermetic gate environment (env_clear() + base floor +
  closed allow-list), reproduce a healthy true-GREEN candidate, and verify
  DeployDrift clears. Covers what NOT to do (never #[ignore] or delete the test).
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../reference/canary-gate-convergence.md
  - ../reference/self-deploy-api.md
  - ../reference/state-root-resolution.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../concepts/deploy-aware-done-gate.md
---

# How to diagnose and confirm the canary gate env-isolation fix

> **Status: implemented.** The self-deploy canary gates in
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
> scrub their subprocess environment with `scrub_gate_env` before every spawn.
> For the full contract (allow-list, base floor, redaction, security posture)
> see [the canary gate env-isolation reference](../reference/canary-gate-convergence.md).

This runbook is for the failure where the self-deploy **canary is stuck RED for
hours**, the running binary falls a commit behind merged `main` (DeployDrift),
and yet the code is provably healthy: `cargo test` is green in CI and locally.
That contradiction is the tell of **gate environment leakage**, and this page
shows how to confirm the fix cleared it.

## When you need this

Use this runbook when you see **all** of:

- `deploy_gate` reports a **red canary** every overseer cycle.
- The failing gate is `unit-test`, with detail like
  `tests failed (exit 101): Running unittests src/lib.rs …`.
- `cargo test --lib --all-features` **passes** locally and in CI.
- GitHub CI `verify.yml` on `main` is **green**.
- The daemon stays **one commit behind** merged `main` (DeployDrift) and cannot
  self-update.

If CI is *also* red, this is **not** your problem — fix the real test failure
first.

## Why it happens (30-second version)

The canary `unit-test` gate spawns `cargo test` **in-process, under the running
daemon**. Before the fix it inherited the daemon's live environment —
`SIMARD_HOME`, `SIMARD_STATE_ROOT`, `SIMARD_PROMPT_ASSETS_DIR`, and a live
`HOME`. Env-sensitive library tests then ran against **live** state instead of
their hermetic fixtures and panicked (exit `101`) — but only on the deploy host,
never under a clean CI shell. The red gate pinned `deploy_gate` red, so the
daemon could never hand over to the healthy candidate.

The fix makes each gate subprocess **hermetic**: `env_clear()` first, then a
minimal base floor, then a **profile layer**. The candidate-binary gates
(`smoke`, `gym-baseline`, `rpc-health`) get a **closed three-name deploy-signal
allow-list** and the live `HOME` so the candidate resolves like the daemon; the
**`unit-test` gate gets no `SIMARD_*` and a neutral scratch `HOME`**, so
env-sensitive tests use their own fixtures. (Injecting the deploy signals — or
even a live `HOME` — into `cargo test` would re-trigger the leak, because
`SIMARD_HOME` falls back to `$HOME/.simard`.) See
[the reference](../reference/canary-gate-convergence.md#two-gate-env-profiles)
for the contract.

## Step 1 — Confirm the "green in CI, red on host" signature

```bash
# Healthy in a clean shell:
cargo test --lib --all-features
# → passes, 0 failed

# CI on main is green:
gh run list --repo rysweet/Simard --workflow verify.yml --branch main --limit 8
```

If the tests pass here but the canary is red, you have leaked-env RED, not a
real regression.

## Step 2 — Reproduce the leak (proves the root cause)

Run the env-sensitive tests **with the daemon's live variables leaked in**, the
way the pre-fix gate did:

```bash
# Simulate the leaked daemon environment (pre-fix behaviour).
SIMARD_HOME="$HOME/.simard" \
SIMARD_STATE_ROOT="$HOME/.simard/state" \
SIMARD_PROMPT_ASSETS_DIR="$HOME/.simard/prompt-assets" \
  cargo test --lib --all-features
# → reproduces the exit-101 panic that the host-side canary saw
```

Reproducing exit 101 here — while a clean shell is green — **confirms** the
defect is environment leakage, not a deterministic test bug.

## Step 3 — Verify the gate is now hermetic

Confirm `scrub_gate_env` is applied to **all four** gate spawns, `env_clear()`
first:

```bash
# scrub_gate_env is called by every gate spawn:
grep -n "scrub_gate_env" src/self_relaunch/gates.rs
# expect: smoke, unit-test, gym-baseline, rpc-health spawns

# env_clear() precedes any re-injection:
grep -n "env_clear" src/self_relaunch/gates.rs

# The unit-test gate uses the UnitTest profile (neutral HOME, NO SIMARD_*);
# the candidate-binary gates use CandidateBinary (allow-list + live HOME):
grep -n "GateEnvProfile::UnitTest\|GateEnvProfile::CandidateBinary" src/self_relaunch/gates.rs
# expect: unit-test -> UnitTest; smoke/gym/rpc -> CandidateBinary

# The candidate-binary allow-list is the closed 3-name set:
grep -n "canary_gate_env_allowlist" -A6 src/self_relaunch/types.rs
# expect exactly: SIMARD_HOME, SIMARD_PROMPT_ASSETS_DIR, SIMARD_STATE_ROOT
```

Then confirm a **hermetic** run — the leaked variables from Step 2 no longer
reach the test — stays GREEN:

```bash
# The gate scrubs env, so even a "dirty" ambient shell yields a clean run.
cargo test --all-features --locked --no-fail-fast
# → passes; no exit 101
```

## Step 4 — Confirm the canary renders true-GREEN and DeployDrift clears

On the deploy host, watch the overseer take the healthy candidate green:

```bash
# Self-deploy / canary health from the operator surface:
simard self-health

# Overseer deploy state — canary should be GREEN, deploy_gate green:
simard overseer status
```

Expected after the fix:

- The `unit-test` gate reports **PASS** (`all tests passed`).
- `deploy_gate` is **green**.
- The daemon self-deploys to merged `main`; **DeployDrift clears** (no longer a
  commit behind).

## Step 5 — Confirm failure detail is safe

Gate failure detail (and `refusal_reason`) is **redacted then bounded**
(≤512 B, UTF-8-safe). A red gate whose stderr contains a credential-bearing URL
must show the userinfo stripped:

```bash
# Look at a captured red-gate detail — credentials must be redacted,
# output bounded (no multi-KB stderr dumps, no user:pass@host).
simard overseer status --json | jq '.deploy.canary.gates[] | select(.passed==false) | .detail'
```

## What NOT to do

- ❌ **Never** `#[ignore]`, delete, or trivially rewrite the env-sensitive /
  `Drop` test to force the gate green. The test is healthy; the *environment*
  was wrong. The fix isolates the env — it does not weaken any test.
- ❌ **Don't** give the `unit-test` (`cargo test`) gate the deploy-signal
  allow-list or a live `HOME`. Because `SIMARD_HOME` falls back to
  `$HOME/.simard`, either one re-points env-sensitive tests at live state and
  reproduces the exit-101 leak. Only the **candidate-binary** gates
  (`smoke`, `gym-baseline`, `rpc-health`) get the allow-list + live `HOME`.
- ❌ **Don't** open another PR against `deploy.rs` chasing the "failing Drop
  test" symptom. Five prior PRs thrashed on that framing; the fix lives in
  `src/self_relaunch/gates.rs` + `types.rs`.
- ❌ **Don't** widen the allow-list or make it config/CLI/env-extensible. It is
  a closed, code-defined set by design (deny-by-default). Adding a wildcard
  `SIMARD_*` pass-through re-introduces the leak.
- ❌ **Don't** relax fail-closed behaviour. A genuinely unhealthy candidate must
  still go RED; a red `deploy_gate` stays fatal.

## Troubleshooting

| Symptom | Likely cause | Action |
|---|---|---|
| `unit-test` gate still exit-101 after fix | It was given live `SIMARD_*` or a live `HOME` (allow-list applied to the wrong profile) | Ensure `unit-test` uses `GateEnvProfile::UnitTest`: neutral scratch `HOME`, **no** `SIMARD_*` |
| Gate still exit-101 after fix | A leaked var not covered by `env_clear()`; or base floor too wide | Confirm `env_clear()` runs before any `.env(...)`; check no ambient var re-enters via the floor |
| Toolchain can't run under the gate (`cargo: not found`) | Base floor too narrow | Ensure `PATH`, `CARGO_HOME`, `RUSTUP_HOME` are in the base floor (a neutral `HOME` is fine once these are pinned) |
| Candidate that should be healthy still RED | Candidate-binary allow-list missing a needed deploy signal | The three signals are `SIMARD_HOME`, `SIMARD_PROMPT_ASSETS_DIR`, `SIMARD_STATE_ROOT`; verify the daemon actually exports them |
| Unhealthy candidate went GREEN | Fail-open regression | This is a bug — the allow-list/floor must never mask a real failure; treat as fail-closed violation |
| Overseer tick panicked on gate detail | Non-UTF-8/oversized stderr | Confirm `bound_gate_detail` redact-then-bound (char-boundary-safe) is applied |

## Related

- [Canary gate env-isolation contract (reference)](../reference/canary-gate-convergence.md)
- [Self-Deploy API](../reference/self-deploy-api.md)
- [State-Root Resolution](../reference/state-root-resolution.md)
- [How to verify and roll back a self-deploy](./verify-and-roll-back-a-self-deploy.md)
- [Reconcile-and-Self-Deploy](../concepts/reconcile-and-self-deploy.md)
- [Deploy-Aware Done-Gate](../concepts/deploy-aware-done-gate.md)
