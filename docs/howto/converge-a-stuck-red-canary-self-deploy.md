---
title: How to converge a stuck red-canary self-deploy
description: Operator runbook for the case where the Overseer refuses the same deploy on a red canary every tick and the running binary falls behind main — read the enriched refusal to name the reddening gate, decide regression vs missing-signal, apply the narrow canary_env allow-list fix, and confirm the self-deploy loop advances past the stuck SHA.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
status: active
related:
  - ../reference/canary-gate-convergence.md
  - ../reference/canary-unit-test-gate-hermetic-isolation.md
  - ../reference/overseer-deploy-canary-diagnostics.md
  - ../reference/self-deploy-api.md
  - ../reference/overseer-tick-self-healing.md
  - ../safe-self-update.md
---

# How to converge a stuck red-canary self-deploy

> **Status: active.** This describes shipped behaviour: the per-gate
> `self_relaunch::gate` spans, the `RelaunchConfig.canary_env` allow-list, and
> the convergence guarantee. For the full design and API, see
> [Canary gate isolation and self-deploy convergence](../reference/canary-gate-convergence.md).

Use this runbook when the Overseer keeps refusing the **same** deploy on a red
canary every tick — the running binary is several commits behind merged `main`,
`DeployDrift` climbs, and the tick log repeats an identical `deploy_gate`
refusal.

## 1. Confirm the symptom

The signature is an identical refusal on every tick with drift climbing:

```bash
# Are recent ticks refusing deploy on a red canary, unchanged?
journalctl --user -u simard -o cat | grep -E 'overseer::(tick|deploy)' | tail -n 40
```

You are looking for the enriched red-canary refusal repeating across ticks
against the **same** `target_commit`, e.g.:

```
WARN overseer::deploy: self-deploy refused by deploy gate
    target_commit=928cd7d running_commit=3b7e4d0
    failing_gate=rpc-health
    failing_detail="rpc health failed (exit 1): connection refused"
    refusal="red canary (gate rpc-health: rpc health failed (exit 1): connection refused)"
```

If instead the refusal varies per tick, or names different commits, this runbook
does not apply — treat each refusal on its own.

## 2. Read which gate is red — do not re-run blindly

The [#4420 diagnostics](../reference/overseer-deploy-canary-diagnostics.md)
already name the reddening gate and its detail in the WARN above and in the
`deploy_refused` operator notification. The [#4440 per-gate spans](../reference/canary-gate-convergence.md)
show every gate's verdict on a single run — filter for them:

```bash
journalctl --user -u simard -o cat | grep 'self_relaunch::gate' | tail -n 20
```

Note the `failing_gate` value: `smoke`, `unit-test`, `gym-baseline`, or
`rpc-health`.

## 3. Decide: genuine regression vs missing signal

| Symptom | Likely cause | Action |
| --- | --- | --- |
| `unit-test` fails reproducibly on merged `main` (assertion failures, exit `1`) | **Genuine regression** | Fix the failing source/test at its origin so the canary goes green legitimately. Do **not** disable the gate. |
| `unit-test` fails with `cargo test` exit **`101`** (test process aborts) every tick, but the same tests pass in a normal `cargo test` run | **Non-hermetic gate** — the gate's `cargo test` collides with the **live daemon** through the allow-listed `SIMARD_STATE_ROOT` | Already repaired by [unit-test gate hermetic isolation](../reference/canary-unit-test-gate-hermetic-isolation.md) (#4522): the gate runs against a private per-run state root. See step 4b. |
| `rpc-health` / `gym-baseline` fails with `connection refused`, a missing socket/endpoint, or an absent env var — but the same probe passes against the running binary | **Missing signal** in the ephemeral canary context | Supply the required signal through the `canary_env` allow-list (step 4). |
| Non-deterministic pass/fail | **Flaky gate** | Correct the gate's logic/threshold so it stops false-reddening **while still failing closed** on real regressions. |

The convergence stall this runbook targets is the **missing-signal** row: the
gate is correctly failing closed on an absent signal, and a healthy candidate
never gets a fair verdict.

## 4. Apply the narrow allow-list (missing-signal case)

The canary build supplies gates a **deny-by-default** environment via
`scrub_gate_env`; only names in `RelaunchConfig.canary_env` are inherited from
the daemon. If the reddening gate needs a variable the daemon has but the canary
context strips, add that **name** (not value) to the allow-list returned by
`canary_gate_env_allowlist()` in
[`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs).

Rules — these keep the gate a real authorization control, not a rubber stamp:

- **Names only.** The value is read from the live environment at spawn time and
  never logged or persisted.
- **Deny by default.** Never allow-list `LD_PRELOAD`-class variables or
  `GIT_SSH_COMMAND`; they are the hijack class the scrub exists to strip.
- **Never weaken the gate.** Do not delete, skip, or short-circuit a gate to
  force green. Supplying the missing signal is the only sanctioned fix here.
- **Absent name → still red.** If an allow-listed name is missing from the
  environment, the gate proceeds without it and reddens — that is intended.

## 4b. `unit-test` gate crash-looping with exit `101` (hermetic-isolation case)

If `failing_gate=unit-test` and `failing_detail` reports `cargo test` exit
**`101`** (the test binary *aborted*, not an ordinary assertion `1`) on every
tick, the cause is almost always a **non-hermetic gate**, not a source
regression. The `unit-test` gate shells out to `cargo test`, and the Simard test
suite reads `SIMARD_STATE_ROOT`. Because that name is allow-listed for the
process-probe gates (step 4), the tests would otherwise inherit the **running
daemon's live state root** and race it — reading a half-written record or
colliding on a lock — aborting with exit `101`.

This is already repaired by
[unit-test gate hermetic isolation](../reference/canary-unit-test-gate-hermetic-isolation.md)
(#4522): `run_unit_test_gate` injects a **private, per-run state root** (mode
`0700`, auto-cleaned) into the gate's scrubbed env, overriding the live value for
that one gate. Confirm the fix is in the running binary and that the gate now
goes green:

```bash
# Reproduce the gate's env locally: scrubbed env + an isolated state root.
# The load-bearing override is SIMARD_STATE_ROOT — the same single override the
# fix applies. A healthy candidate must pass; if it does, the live-daemon
# collision was the cause. (TMPDIR is only added if a run shows the suite needs
# a private scratch dir; it is not part of the base fix — see the reference doc.)
env -i PATH="$PATH" HOME="$HOME" \
    CARGO_HOME="$CARGO_HOME" RUSTUP_HOME="$RUSTUP_HOME" \
    SIMARD_STATE_ROOT="$(mktemp -d)" \
    cargo test --locked -p simard self_relaunch::gates
```

If exit `101` **persists** even against a private state root, it is a genuine
abort in the candidate's tests — treat it as the "genuine regression" row of the
step-3 table and fix the failing test at its origin. Never allow-list a way to
skip the gate.

## 5. Confirm convergence

Convergence has two halves — verify **both**:

1. **A healthy candidate now passes.** After the fix, a fresh canary run against
   the merged head reports all gates green:

   ```bash
   journalctl --user -u simard -o cat | grep 'self_relaunch::gate' | tail -n 8
   # every line: "gate passed"; no "gate reddened the canary"
   ```

2. **The loop advances past the stuck SHA.** The next drift observation shows
   `DeployDrift == 0` and the running commit now matches `main`:

   ```bash
   simard status | grep -Ei 'deploy_drift|running_commit'
   ```

   The Overseer emits a `deploy_starting` → successful swap instead of another
   `deploy_refused` on the same commit.

If gates are green but the loop still refuses, the refusal is coming from a
**different** gate or a non-canary rail (`NoOp` / `Rollback` / `CrashLoop`) — go
back to step 2 and read the new `failing_gate` / `refusal`.

## 6. Do not defeat the fail-closed rails

- A `deploy_gate` / `target_canary` capability failure is **never** transient
  ([self-healing classifier](../reference/overseer-tick-self-healing.md)) — even
  if the enriched detail contains `timeout` or `503`. Do not try to make it
  retry away; a real red must latch.
- The four gates still run to completion without short-circuit, and promotion
  still verifies the exact artifact it ships. Never add a "skip gate" control.

## Verify end-to-end

```bash
# 1. Gates render both verdicts (fail-closed preserved + healthy passes):
cargo test --locked -p simard self_relaunch::gates
cargo test --locked -p simard tests_deploy_drift

# 2. No print-family sinks and no Bridge naming in the changed surface:
! grep -RnE '\b(print|println|eprintln)!' src/self_relaunch/gates.rs src/self_relaunch/types.rs
! grep -RniE 'bridge' src/self_relaunch/gates.rs src/self_relaunch/types.rs src/self_deploy/source_prep.rs

# 3. Live loop converged:
simard status | grep -Ei 'deploy_drift|running_commit'
```

## Related reading

- [Canary unit-test gate hermetic isolation](../reference/canary-unit-test-gate-hermetic-isolation.md) —
  the #4522 fix for the `unit-test` exit-`101` crash-loop: a private per-run
  state root so the gate stops colliding with the live daemon.
- [Canary gate isolation and self-deploy convergence](../reference/canary-gate-convergence.md) —
  the full design: per-gate spans, `canary_env`, `scrub_gate_env`, and the
  preserved fail-closed invariants.
- [Overseer deploy red-canary diagnostics](../reference/overseer-deploy-canary-diagnostics.md) —
  how the reddening gate is named in the tick WARN and the operator
  notification.
- [Self-deploy API reference](../reference/self-deploy-api.md) — the
  `GuardedDeployer`, `DeployRefusal`, and the swap path.
- [Overseer tick self-healing](../reference/overseer-tick-self-healing.md) — the
  `is_transient` guard that keeps a red canary from being retried as a blip.
