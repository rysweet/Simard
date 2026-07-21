---
title: How to diagnose a red-canary deploy refusal
description: Operator runbook for a wedged self-deploy — read the failing-gate name now surfaced on the deploy refusal and the operator notification, confirm the RPC-health gate is probing the candidate, and verify a healthy build clears the gate and DeployDrift resolves.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/canary-gate-diagnostics.md
  - ../reference/canary-gate-diagnostics-api.md
  - ../reference/self-deploy-api.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../reference/overseer-operator-notifications.md
---

# How to diagnose a red-canary deploy refusal

Use this runbook when self-deploy is **wedged on a red canary**: every overseer
tick logs `deploy_gate: red canary` while `DeployDrift` reports the running
binary is one or more commits behind merged `main`, so merged improvements never
ship.

Before this feature the refusal named no gate, so root-causing meant guessing.
Now the **first failing gate** is surfaced on both the log/refusal and the
operator notification. Follow the steps below.

## 1. Read the failing gate from the refusal

The deploy refusal now names the gate. In the overseer logs, look for the
enriched `RedCanary` line:

```text
red canary (gate `rpc-health` failed: rpc health failed (exit 1): connection refused)
```

The gate slug is one of `smoke`, `unit-test`, `gym-baseline`, `rpc-health`
(evaluated in that order — the **first** failure is the one reported). See the
[gate order](../reference/canary-gate-diagnostics-api.md#gate-order).

If you see the legacy wording *"red canary (one or more gates failed)"* with no
gate name, the `RedCanaryDetail` payload was `Default` (no specific gate known) —
fall through to the structured logs in step 2.

## 2. Read the structured tracing fields

Every gate emits `canary.gate`, `canary.passed`, and `canary.detail` as
structured `tracing` fields. Filter the daemon's OTel/`tracing` output for the
red one:

```bash
# journalctl example — adjust to your log sink
journalctl --user -u simard-ooda | grep 'canary gate evaluated'
```

Look for the event with `canary.passed=false`; its `canary.gate` is the culprit
and `canary.detail` is the sanitized reason.

## 3. Read the operator notification

The mandatory deploy notification (invariant #2590 — both Signal and email fire
on every attempt, including a refusal) now embeds the sanitized failing-gate
detail, identical on both channels. The Signal message / email body names the
gate and reason directly — no need to reach the host to triage.

## 4. Confirm the RPC-health gate probes the candidate

The cause of the *permanent* red canary was `rpc-health` shelling the candidate
as `simard probe rpc …` when the operator CLI had **no `probe` command** — the
candidate exited non-zero on every tick, so the gate was red regardless of
candidate health. With the fix, `run_rpc_health_gate` invokes the candidate's
`probe rpc --self-check` mode, which validates the candidate's own RPC health
**in-process** (driving `bridge.health` on an in-process transport — no shared
socket, no bound port) and is **fail-closed** (probe error / non-zero exit /
timeout ⇒ red).

You can run the self-check directly against any candidate/installed binary:

```bash
# Exits 0 when this binary's own RPC health path is sound; non-zero otherwise.
simard probe rpc --self-check
```

Confirm the healthy path by inspecting the candidate's own health surface. The
canary gates run automatically inside the overseer's self-deploy tick; use the
health report and the structured `canary.*` fields from step 2 to confirm the
candidate is sound:

```bash
# Candidate/self health report as JSON (probes, version_advanced, etc.).
simard self-health --json
```

A healthy candidate reports every probe healthy and, once deployed,
`version_advanced: true`. If `rpc-health` still reddens for a candidate you
believe is healthy (per its `canary.detail`):

- check `RelaunchConfig::health_timeout` (default 30s) is not too tight for a
  cold start;
- run `simard probe rpc --self-check` against the candidate binary directly to
  reproduce the exit status the gate observed;
- read `canary.detail` — a genuine candidate defect (panic on boot, missing RPC
  route) is a **correct** red and must be fixed in the candidate, not the gate.

## 5. Verify the deploy proceeds and drift clears

Once the failing gate passes, the deploy gate proceeds, the binary swaps, and
`DeployDrift` resolves. Verify:

```bash
simard self-health --json        # every probe healthy; version_advanced true
simard status                    # DeployDrift.needs_deploy == false
```

`version_advanced` confirms the running commit now matches the merged target;
`needs_deploy: false` confirms drift has cleared. For the full post-deploy
verification and rollback path, see
[How to verify and roll back a self-deploy](verify-and-roll-back-a-self-deploy.md).

## What this runbook does not do

- It does **not** ask you to disable, force-pass, or reorder any gate — a red
  canary for a genuinely broken candidate is the gate working as designed.
- It does **not** touch the notify invariant, the build lock, or the
  `SIMARD_GIT_HASH` integrity gate.

If a gate reddens for a candidate that is genuinely healthy and the detail points
at gate mechanics (not a candidate defect), that is a bug in the gate — capture
the `canary.gate` / `canary.detail` fields and
[file an issue](report-a-bug-or-request-a-feature.md).

## See also

- [Canary-gate diagnostics concept](../concepts/canary-gate-diagnostics.md)
- [Canary-gate diagnostics API reference](../reference/canary-gate-diagnostics-api.md)
- [How to verify and roll back a self-deploy](verify-and-roll-back-a-self-deploy.md)
- [Self-deploy API reference](../reference/self-deploy-api.md)
