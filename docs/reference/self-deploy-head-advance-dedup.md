---
title: Self-deploy head-advance & per-SHA dedupe API reference
description: >
  The additive self-deploy reconcile surface that advances the running head to
  the merged base-allowlist head, dedupes redeploys per target SHA to stop
  self-deploy thrash, validates the target SHA as lowercase hex before any argv,
  and reconciles the not-loaded overseer systemd unit. Documents the extended
  file-backed SelfDeployState (last_deploy_target_sha / last_deploy_result), the
  head-advance reconcile, the SHA validation helper, the systemd-unit reconcile,
  and the SIMARD_OVERSEER_AUTONOMOUS_DEPLOY / SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS
  guards. Addresses #4390, #4387, #4305.
last_updated: 2026-07-21
owner: simard
doc_type: reference
status: partially implemented
related:
  - ../concepts/reconcile-and-self-deploy.md
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ./overseer-operator-notifications.md
  - ./overseer-tick-details.md
  - ../safe-self-update.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../howto/run-self-deploy-from-any-directory.md
  - ../../src/self_deploy/head_advance.rs
  - ../../src/self_deploy/orchestrator.rs
  - ../../src/self_deploy/restart.rs
  - ../../src/overseer/deploy.rs
  - ../../src/overseer/deploy_trigger.rs
---

# Self-deploy head-advance & per-SHA dedupe API reference

> **Status: partially implemented.** The pure decision layer — the extended
> `DeployHeadState` (`last_deploy_target_sha` / `last_deploy_result`), the
> head-advance reconcile decision, the SHA-validation helper, the per-SHA dedupe
> decision, and the systemd-unit-load classification — is implemented and
> unit-tested in
> [`src/self_deploy/head_advance.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/head_advance.rs)
> (re-exported from `src/self_deploy/mod.rs`). The **effectful wiring** that
> consumes these decisions in
> [`src/self_deploy/orchestrator.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/orchestrator.rs),
> [`src/self_deploy/restart.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/restart.rs),
> [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs)
> and
> [`src/overseer/deploy_trigger.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy_trigger.rs)
> is a tracked follow-up and is **not yet integrated** into the running deploy
> loop. The dedupe, head-advance, SHA-rejection, and opt-out decision paths are
> covered by unit tests. This page **extends** the
> [self-deploy API reference](./self-deploy-api.md) and specifies the surface the
> wiring will consume.

This reference specifies the additive reconcile surface that closes the
merged-but-undeployed head gap (issues
[#4390](https://github.com/rysweet/Simard/issues/4390),
[#4387](https://github.com/rysweet/Simard/issues/4387),
[#4305](https://github.com/rysweet/Simard/issues/4305)). For the rationale and
the end-to-end flow, see
[reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md).

**One-line summary (specified target behavior):** once the wiring lands, the
running head advances to the merged
base-allowlist head; each target SHA is deployed **at most once** (per-SHA
dedupe on top of the existing min-interval anti-thrash); the target SHA is
validated as lowercase hex before any `gh`/`systemctl` argv; and a **not-loaded**
overseer systemd unit is reconciled so deploys become service-managed.

## The gap this closes

`simard status` reported `DAEMON/UPTIME unavailable (systemctl: unit not
loaded)` — no service-managed deploy — while the running binary (`0.31.0`)
lagged the merged head (`0.33.1`). Self-deploy could also redeploy the **same**
head repeatedly (thrash). This surface adds three additive guards:
per-SHA dedupe, head-advance reconcile, and systemd-unit reconcile.

## Contents

- [`SelfDeployState` (extended)](#selfdeploystate-extended)
- [SHA validation](#sha-validation)
- [Head-advance reconcile](#head-advance-reconcile)
- [Per-SHA dedupe](#per-sha-dedupe)
- [systemd unit-not-loaded reconcile](#systemd-unit-not-loaded-reconcile)
- [Environment configuration](#environment-configuration)
- [Edge-case matrix](#edge-case-matrix)

## `SelfDeployState` (extended)

A small file-backed JSON state (mirroring the existing `SelfRelaunchState` in
[`restart.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/restart.rs))
records the last **target** SHA and its result, enabling per-SHA dedupe and
head-advance reconciliation across restarts.

```rust
#[derive(serde::Deserialize, serde::Serialize)]
struct SelfDeployState {
    /// Last SHA the daemon attempted to deploy TO (40- or 64-char lowercase hex).
    last_deploy_target_sha: Option<String>,
    /// Outcome of that attempt, for dedupe + operator reporting.
    last_deploy_result: DeployResult,
    /// Unix seconds of the last attempt (feeds the min-interval anti-thrash).
    last_deploy_unix_secs: u64,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, PartialEq)]
enum DeployResult {
    Succeeded,
    Failed,
    RolledBack,
}
```

**Durability contract.** The state file is written `0600`. An **unparseable**
state file is treated as *no known prior deploy* and, combined with the guards
below, results in **no deploy** rather than an unguarded one (fail-closed).

## SHA validation

Every target SHA is validated **before** it is placed on any argv (`gh`,
`git`, `systemctl`) or branch/path.

```rust
/// True iff `s` is a 40- or 64-char all-lowercase hex string (git SHA-1/SHA-256).
/// Rejects uppercase, whitespace, refs, and anything that could inject an argv flag.
pub fn is_valid_deploy_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
```

A SHA that fails validation aborts the deploy with a structured `tracing::warn!`
and **no** subprocess is spawned.

## Head-advance reconcile

The orchestrator reconciles the **running** head to the **merged
base-allowlist** head from the authenticated remote (`origin/main` root of
trust; see the
[security prerequisites](./self-deploy-api.md#security-prerequisites)). It only
advances to a verified head on the base allow-list — never an arbitrary or fork
ref — and only when that head differs from the running binary's head.

```text
if merged_head != running_head
   && is_valid_deploy_sha(merged_head)
   && on_base_allowlist(merged_head)
   && not deduped(merged_head)          # see below
   && min_interval_elapsed()            # existing anti-thrash
   && autonomous_deploy_enabled()
then deploy_to(merged_head)
```

## Per-SHA dedupe

On top of the existing **min-interval** anti-thrash
(`SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS`), the daemon **skips a target SHA it
has already successfully deployed**. This prevents redeploying the same head on
every tick (the #4387 self-deploy dedupe requirement).

```text
deduped(sha) :=
    state.last_deploy_target_sha == Some(sha)
    && state.last_deploy_result == Succeeded
```

A `Failed`/`RolledBack` result for the same SHA is **not** deduped — a genuine
retry is still allowed (subject to the min-interval guard), so a transient
failure does not permanently wedge the head.

## systemd unit-not-loaded reconcile

When the overseer systemd unit is **not loaded** (the `simard status`
`unit not loaded` condition), the reconcile step detects it and re-establishes
service management using **fixed** `systemctl` subcommands and a **constant**
unit name — no interpolated/user-derived unit strings.

```rust
const OVERSEER_UNIT: &str = "simard-overseer.service";

// Detect: `systemctl --user is-active <constant-unit>` / is-enabled.
// Reconcile: load/enable the unit so the next deploy is service-managed.
```

The unit name is a compile-time constant; only fixed subcommands
(`is-active`, `is-enabled`, `restart`) are invoked. No external data is ever
interpolated into a `systemctl` argument.

## Environment configuration

These guards reuse the existing autonomous-deploy env surface; **no new opt-in
is required** for head-advance/dedupe (they are additive safety guards on the
already-existing autonomous path).

| Variable | Type | Default | Meaning |
|---|---|---|---|
| `SIMARD_OVERSEER_AUTONOMOUS_DEPLOY` | bool (opt-out) | on | Set to `0` to disable autonomous drift-triggered deploy entirely. Honored by the head-advance reconcile — with `0`, no head-advance deploy is attempted. |
| `SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS` | u64 seconds | (existing default) | Minimum wall-clock interval between deploy attempts (existing anti-thrash). Per-SHA dedupe stacks on top of this. |
| `SIMARD_SELF_DEPLOY_REPO` | string | auto-detected | Source repo override (existing; see [source prep](./self-deploy-source-prep.md)). |

## Edge-case matrix

| Situation | Result |
|---|---|
| `merged_head == running_head` | No deploy (already at head) |
| Same SHA already `Succeeded` | Deduped — skipped (no thrash) |
| Same SHA previously `Failed` | Retry allowed once min-interval elapses |
| Target SHA not lowercase hex / is a ref | Rejected before argv — no subprocess |
| Head not on base allow-list / from a fork | Rejected — not deployed |
| `SIMARD_OVERSEER_AUTONOMOUS_DEPLOY=0` | No head-advance deploy attempted |
| Min-interval not elapsed | Deferred until the window passes |
| systemd unit not loaded | Reconciled via fixed subcommands + constant unit name |
| `SelfDeployState` file unparseable | Treated as no prior deploy ⇒ fail-closed (no deploy) |

## Telemetry

Each reconcile decision (advance, dedupe-skip, SHA-reject, unit-reconcile,
opt-out) emits a structured `tracing` event (OTel) and — per the existing
"notify on every attempt" invariant — every actual deploy attempt notifies the
operator (see
[overseer operator notifications](./overseer-operator-notifications.md)). No
`println!`, no secrets, and SHAs are logged only after validation.
