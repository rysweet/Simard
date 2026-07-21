---
title: "Concept: canary-gate diagnostics and the candidate RPC-health probe"
description: How Simard's self-deploy names the exact gate that reddened the canary, surfaces it on the deploy-refusal and the operator notification, and probes candidate RPC health in-process against the candidate's own transport so a healthy build is no longer falsely refused.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - reconcile-and-self-deploy.md
  - ../reference/canary-gate-diagnostics-api.md
  - ../reference/self-deploy-api.md
  - ../reference/overseer-operator-notifications.md
  - ../howto/diagnose-a-red-canary-deploy-refusal.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
  - ../../src/overseer/deploy.rs
---

# Concept: canary-gate diagnostics and the candidate RPC-health probe

> **Status: implemented.** The `RedCanaryDetail` payload, the `first_failure`
> accessor, the structured `tracing` fields on every gate emitter, and the
> corrected `run_rpc_health_gate` probe live in
> [`src/self_relaunch/`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
> and [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs).

## The problem this closes

Simard's self-deploy runs a **canary** — it builds the target commit and runs it
through an ordered list of [`RelaunchGate`s](../reference/canary-gate-diagnostics-api.md#gate-order)
(`smoke → unit-test → gym-baseline → rpc-health`). If any gate fails, the
[deploy gate](../reference/self-deploy-api.md) refuses the deploy as a **red canary** and the
running daemon stays one or more commits behind merged `main`.

Two defects made that refusal both *opaque* and *permanent* for healthy builds:

1. **Opaque refusal.** `DeployRefusal::RedCanary` rendered only the generic
   string *"red canary (one or more gates failed)"*. The operator log and the
   deploy notification never named *which* gate failed or *why*, so every
   overseer tick logged the same line with no path to root-cause. The observed
   symptom was five-plus consecutive ticks refusing deploy `56b10bef5057` with
   that identical message while `DeployDrift` reported the binary *1 commit
   behind merged main*.

2. **False-negative RPC-health gate.** `run_rpc_health_gate` invokes the freshly
   built candidate as `Command::new(binary).args(["probe", "rpc", …])`, but the
   operator CLI had **no `probe` command**: `dispatch_operator_cli` fell through
   to its `unsupported command` arm, so the candidate exited **non-zero on every
   invocation**. The `rpc-health` gate therefore scored **red on every tick**
   regardless of how healthy the candidate actually was — a perfectly good build
   could never clear the gate, and self-deploy was wedged. (The gate was never
   validating candidate health; it was validating a command that did not exist.)

Together these produced the stuck loop: an every-tick red canary with no
diagnostic to prove which gate was at fault.

## The fix, in two additive bricks

### Brick 1 — diagnostics first (evidence before repair)

Rather than guess which gate reddened, Brick 1 makes the failing gate **visible**
without changing any verdict:

- A new **`RedCanaryDetail`** payload is attached to `DeployRefusal::RedCanary`.
  It carries the **first failing gate** (in the deterministic gate order) and
  that gate's sanitized `detail` string.
- A **`first_failure`** accessor over the ordered `GateResult`s returns that
  first failing gate deterministically, so two runs with the same failures
  always name the same culprit.
- Every gate emitter in `gates.rs` emits **structured `tracing` fields**
  (`canary.gate`, `canary.passed`, `canary.detail`) — a side-channel only; the
  returned `GateResult` is byte-for-byte unchanged.
- The mandatory operator notification (invariant #2590 — **both** channels fire
  on every deploy attempt, including a refusal) now includes the sanitized
  failing-gate detail, at parity across the Signal and email channels.

The verdict logic (`evaluate_deploy_gate`, `all_gates_passed`) is **untouched**:
a run refuses or proceeds exactly as before. Only the *explanation* is richer.
This let the fix confirm — from real telemetry — that the `rpc-health` gate was
the every-tick culprit before any gate logic was altered.

### Brick 2 — probe the candidate, not the live daemon

With the evidence in hand, Brick 2 makes `run_rpc_health_gate` validate the
**candidate's own** RPC health instead of shelling a command that did not exist:

- The candidate is asked to answer an RPC health check **in-process** — it drives
  the pre-existing `bridge.health` method against a fresh in-process native
  transport (the same dispatch the daemon uses). This exercises *this* binary's
  RPC health path and never dials the shared daemon socket, so a drifted live
  daemon can no longer redden the gate.
- The check is **fail-closed**: a spawn error, a non-zero exit, or a timeout
  scores the gate **red**. There is no default-pass and no force-pass.
- The check is **least-privilege and self-cleaning by construction**: it is
  in-process, so **no loopback port is bound and no child process or socket is
  spawned** — there is nothing to leak on the error path. It is bounded by
  `RelaunchConfig::health_timeout`, and an in-process dispatch cannot hang.

**Precondition.** The operator CLI had no `probe` command at all. Brick 2 adds an
**additive** `probe rpc --self-check [--timeout=SECS]` command that runs the
in-process self-check described above and exits `0` only when the candidate
reports healthy. `run_rpc_health_gate` invokes *that* command. No RPC method is
renamed — the pre-existing `bridge.health` handler is reused, not replaced — and
no other command's behavior changes.

A healthy candidate built from the target commit now passes `rpc-health`, the
deploy gate proceeds, the binary swaps, and `DeployDrift` clears.

## What is deliberately *not* changed

The canary's safety intent is preserved. The fix does **not**:

- disable, remove, or reorder any gate in
  [`default_gates()`](../reference/canary-gate-diagnostics-api.md#gate-order);
- force `canary_passed = true` or otherwise weaken `evaluate_deploy_gate`;
- alter the notify invariant #2590, the `with_self_deploy_build_lock`
  serialization, or the `SIMARD_GIT_HASH` / `version_advanced` integrity gate;
- rename or touch the pre-existing `bridge.health` RPC (no "Bridge" naming is
  introduced).

An unhealthy candidate is still refused; the only behavior change is that a
*healthy* candidate is no longer falsely refused, and every refusal now names
its cause.

## Telemetry hygiene

The diagnostic surface is treated as untrusted candidate output:

- Gate `detail` strings are truncated (`≤ 512` chars) and trimmed via the
  existing char-boundary-safe `truncate_output` helper before they reach any
  `tracing` field or notification.
- Values are emitted as **structured field values**, never interpolated into a
  format string, so candidate stderr cannot inject log lines.
- No secrets, environment values, or home paths are surfaced; the sanitized
  detail is identical across both operator channels.

## See also

- [Canary-gate diagnostics API reference](../reference/canary-gate-diagnostics-api.md)
- [Self-deploy API reference](../reference/self-deploy-api.md)
- [Overseer operator notifications](../reference/overseer-operator-notifications.md)
- [How to diagnose a red-canary deploy refusal](../howto/diagnose-a-red-canary-deploy-refusal.md)
- [Concept: reconcile-and-self-deploy](reconcile-and-self-deploy.md)
