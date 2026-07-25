---
title: Canary unit-test gate state-root isolation
description: Reference for the root-cause repair (#4628) that stops the self-deploy canary unit-test gate crash-looping with exit 101. The gate now runs cargo test under a fresh, ephemeral, per-run SIMARD_STATE_ROOT / SIMARD_HOME (the two contention-breaking overrides; a defensive SIMARD_MEMORY_SOCKET env_remove is also applied) so its unit tests no longer contend with the live daemon's single-writer cognitive-store and typed-OODA sqlite locks. Covers the subprocess-scoped env overrides applied after scrub_gate_env, the fail-closed TempDir handling, the scope (only run_unit_test_gate), and the security posture.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./canary-gate-convergence.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../testing/cognitive-memory-serial-isolation.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
---

# Canary unit-test gate state-root isolation

> **Status: implemented.** The `unit-test` canary gate
> ([`run_unit_test_gate`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
> in [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs))
> now runs `cargo test` under a fresh, ephemeral, per-run state root
> (`SIMARD_STATE_ROOT` / `SIMARD_HOME` pointed at a randomized `TempDir` — the two
> overrides that actually break the lock contention; a defensive
> `SIMARD_MEMORY_SOCKET` `env_remove` is also applied, see
> [below](#what-changed)) so the canary's unit tests no longer collide
> with the **running** daemon's exclusive cognitive-store and typed-OODA sqlite
> locks. The change is **additive and non-breaking**: `verify_canary`,
> `run_gate`, the four-gate no-short-circuit sequence, the gate ordering, and the
> fail-closed refusal semantics all keep their signatures and behavior; only the
> `unit-test` gate subprocess gains an isolated state directory.

## Why this exists

The [red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) (#4420)
surfaced the reddening gate, and the
[convergence repair](./canary-gate-convergence.md) (#4440) fixed a gate that
failed closed on a *missing* signal. This feature fixes a different, systemic
failure mode: the `unit-test` gate reddened on **every** Overseer tick for 8+
hours with

```
deploy_gate: red canary (gate unit-test: tests failed (exit exit status: 101))
```

failing ~1.7 s into `Running unittests src/lib.rs`, at the `Drop` teardown of a
test. `DeployDrift` climbed monotonically from 9 to 13 commits behind `main`
while Simard ran increasingly stale code and never self-deployed.

### Root cause

The `unit-test` gate shells out to `cargo test` against a target that, before
this change, **shared the live daemon's state root**. Several unit tests open
Simard's single-writer stores — the **lbug cognitive store** and the
**typed-OODA sqlite outcome store** — and those opens collided with the
already-running daemon that holds the exclusive locks. The OODA journal showed
the two colliding surfaces directly:

```
persistent store cognitive-open-lock failed during acquire_open_lock:
  cognitive store is held open by another process (PID …);
  refusing to open a second concurrent handle … use an isolated state root for this run

typed OODA outbox startup recovery incomplete:
  typed outcome persistence failed: database is locked
```

Both stores are **correct** to refuse a second concurrent writer — that
single-writer guard is a safety invariant, not a bug. The defect was that the
canary ran its tests *inside the live daemon's state root* and therefore
guaranteed the collision on every tick. The fix gives the canary its own
throwaway state root so the guard is never tripped.

> **Detection stays intact.** The gate is not weakened, skipped, or made to
> ignore failures. It still runs the full `cargo test` suite and still fails
> closed on a genuine regression. Only the *state directory* the tests write to
> is isolated.

## What changed

`run_unit_test_gate` builds its `cargo test` command through
[`scrubbed_command`](./canary-gate-convergence.md#scrub_gate_env) exactly as
before (deny-by-default `env_clear()` + the base floor + the
`canary_env` allow-list). **After** that scrub, it applies two
contention-breaking overrides plus one defensive removal, all so they win
last-write-wins over any inherited `SIMARD_*` value:

| Env var | Action | Why |
| --- | --- | --- |
| `SIMARD_STATE_ROOT` | set to a fresh absolute `TempDir` | canonical state root; both colliding stores resolve their paths from it |
| `SIMARD_HOME` | set to the same `TempDir` | keep home/state consistent for any code that derives paths from `SIMARD_HOME` |
| `SIMARD_MEMORY_SOCKET` | **removed** (`env_remove`) | defence in depth — the canary must not dial the live daemon's memory socket |

The overrides are applied **only** on the `unit-test` gate's command. No other
gate is touched.

> **`SIMARD_MEMORY_SOCKET` removal is belt-and-suspenders, not the primary
> mechanism.** [`scrub_gate_env`](#ordering-matters) already runs `env_clear()`
> and re-injects only the `BASE` floor plus the three
> [`canary_gate_env_allowlist`](./canary-gate-convergence.md#scrub_gate_env)
> names (`SIMARD_HOME`, `SIMARD_PROMPT_ASSETS_DIR`, `SIMARD_STATE_ROOT`).
> `SIMARD_MEMORY_SOCKET` is in neither set, so it is **already absent** from the
> gate subprocess after the scrub. The explicit `env_remove` therefore only
> matters if a future caller adds `SIMARD_MEMORY_SOCKET` to
> [`RelaunchConfig::canary_env`]; it is kept as a deliberate guard against that
> regression. The two contention-breaking overrides are `SIMARD_STATE_ROOT` and
> `SIMARD_HOME`. Because of this, the env-absence test (below) is a
> regression guard on the allow-list, not proof the `env_remove` line did the
> work.

### Ordering matters

The overrides are applied **after** `scrubbed_command` returns, mirroring the
existing `CARGO_BUILD_JOBS` override on the same command. `scrub_gate_env` calls
`env_clear()` at construction time, so applying the overrides afterward
guarantees they survive and take precedence:

```rust
fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    // Deny-by-default scrub happens here (env_clear + base floor + allow-list).
    let mut cmd = scrubbed_command("cargo", config);

    // Fresh, per-run isolated state root. Fail closed on error (never fall
    // back to the live daemon's root — see "Fail-closed TempDir handling").
    let state_root = match tempfile::TempDir::new() {
        Ok(dir) => dir,
        Err(e) => return unit_test_gate_failed_closed(e),
    };
    // TempDir::path() is already absolute (mkdtemp); assert it explicitly so
    // REQ-SEC-2 holds even if the temp implementation ever changes.
    let abs_path = state_root.path();
    debug_assert!(abs_path.is_absolute());

    // SEC-D3: confine the post-scrub overrides to exactly these SIMARD_* vars.
    // Do NOT copy this pattern into other gates — rpc-health intentionally dials
    // the live daemon and must keep the shared state root. Overrides win
    // last-write-wins over the scrub's re-injected values.
    cmd.env("SIMARD_STATE_ROOT", abs_path)
        .env("SIMARD_HOME", abs_path)
        .env_remove("SIMARD_MEMORY_SOCKET"); // defensive; see "What changed"

    cmd.arg("test")
        .arg("--manifest-path").arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir").arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs());

    // `state_root` stays bound here so its TempDir outlives cmd.output() and is
    // cleaned up on drop only AFTER the subprocess exits.
    let output = cmd.output();
    // … existing pass/fail mapping unchanged …
}
```

### Fail-closed TempDir handling

If `TempDir` creation fails, the gate **fails closed**: it emits a structured
`tracing::error!` and returns a reddened `GateResult` (the canary does not
deploy). It **never** falls back to the live daemon's state root — doing so
would reintroduce #4628 and leak canary test writes into production state.

```rust
// Helper referenced by run_unit_test_gate's TempDir match arm above.
fn unit_test_gate_failed_closed(e: std::io::Error) -> GateResult {
    tracing::error!(
        target: "self_relaunch::gate",
        error = %e,
        "unit-test gate: failed to create isolated state root; failing closed"
    );
    GateResult {
        gate: RelaunchGate::UnitTest,
        passed: false,
        detail: "unit-test gate: could not create isolated state root".to_string(),
    }
}
```

### TempDir lifetime

The `TempDir` is bound to a local variable in `run_unit_test_gate` that
outlives `cmd.output()`. It is created **before** the run and dropped (deleted)
**after** the subprocess exits. Consequences:

- The state root cannot be deleted mid-run.
- No residue is left under the live `SIMARD_HOME` — the directory lives under
  the OS temp root and is removed on drop.

## Scope — only the unit-test gate

Isolation is confined to `run_unit_test_gate`. The other gates are unchanged:

| Gate | Isolated? | Reason |
| --- | --- | --- |
| `smoke` | no | runs `<binary> --version`; opens no store |
| `unit-test` | **yes** | shells out to `cargo test`, which opens the cognitive + typed-OODA stores |
| `gym-baseline` | no | runs `<binary> gym list`; does not contend on the writer lock |
| `rpc-health` | **no — deliberately** | must dial the **live** daemon via the shared `SIMARD_STATE_ROOT`; isolating it would break the probe |

> **Do not isolate `rpc-health`.** It is designed to reach the currently running
> daemon. Pointing it at an ephemeral state root would make it probe an empty,
> daemon-less directory and false-redden a healthy candidate.

## Behavior: reproduce-then-confirm

The repair ships with two tests in
[`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs):

1. **Env-target assertion.** Builds the `unit-test` gate command and asserts
   that `SIMARD_STATE_ROOT` and `SIMARD_HOME` point at a **unique, absolute
   temp directory** (not the live state root) and that `SIMARD_MEMORY_SOCKET` is
   **absent** from the command environment.

2. **Lock-contention reproduce-then-confirm.** In a serial `cognitive_memory`
   test group (see
   [cognitive-memory serial isolation](../testing/cognitive-memory-serial-isolation.md)),
   the test acquires the cognitive-store and typed-OODA locks
   (`acquire_open_lock`) and then invokes the gate:
   - **Pre-fix** path exhibits the exit-101 / `database is locked` /
     `cognitive store is held open by another process` contention.
   - **Post-fix** path passes, because the gate writes to an isolated state
     root and never contends with the held locks.

## Observability

- All emission is structured `tracing` at the appropriate level on
  `target: "self_relaunch::gate"` (the fail-closed path logs
  `tracing::error!`). Simard's tracing layer is the OTel bridge, so these are
  the OTel attributes — no separate SDK call site.
- There are **no** `print!` / `println!` / `eprintln!` sinks.
- There are **no** silent fallbacks: the TempDir failure path reddens the gate
  loudly rather than degrading to the live root.
- Temp paths are non-secret; existing gate-detail redaction
  (`bound_gate_detail`, 512-byte cap) is unchanged.

## Security posture

| ID | Guarantee |
| --- | --- |
| REQ-SEC-2 | The temp state root is an **absolute** path (asserted `is_absolute`), so it cannot resolve against the subprocess CWD and escape/collide. |
| REQ-SEC-3 | Paths are passed via `Command::env` only — no shell, no string interpolation. |
| REQ-SEC-4 | `tempfile::TempDir` uses a randomized name created via `mkdtemp` (mode `0700`) — no predictable `/tmp` path, preventing TOCTOU / symlink attacks on a shared host. |
| REQ-SEC-5 | Cleanup is guaranteed on drop after `cmd.output()`; creation error **fails closed** with `tracing::error!` — never a live-root fallback. |
| REQ-SEC-6 | Overrides are applied after the scrub, last-write-wins, touching **only** the three named `SIMARD_*` vars; the allow-list is not widened and `env_clear` is not re-run. `SIMARD_MEMORY_SOCKET` is removed, not re-bound. |
| SEC-D2 | Gate detail continues through `bound_gate_detail` (512-byte cap, credential redaction); no secrets logged. |
| SEC-D3 | The post-scrub `.env()` pattern is confined to `run_unit_test_gate` with an in-code comment; it is **not** copied into other gates. |

## Compatibility

- **No struct changes.** `RelaunchConfig` already carries `manifest_dir`,
  `canary_target_dir`, and `canary_env`; the isolation is internal to the gate
  function. Existing allow-list and gate tests remain valid.
- **Production deploy path unchanged** except that the canary's `unit-test`
  gate now writes to an isolated state root. Gate ordering, the gate set, and
  the refusal logic are identical.
- **No new inputs.** No new endpoints, RPC, CLI flags, config keys, or operator
  "skip gate" controls. The trust boundary is unchanged.
- **No `Bridge` naming** is introduced.

## Acceptance

- `cargo build` and `cargo test` are green locally and in CI.
- The `unit-test` gate no longer emits exit 101 while the daemon holds its
  store locks; the canary goes green and `DeployDrift` stops increasing.

## See also

- [Canary gate isolation and self-deploy convergence](./canary-gate-convergence.md) —
  the #4440 environment allow-list repair this builds on.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  the `failing_gate` / `failing_detail` telemetry that surfaced this crash-loop.
- [How to converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md) —
  the operator runbook.
- [Cognitive-memory serial test isolation](../testing/cognitive-memory-serial-isolation.md) —
  why the lock-contention test runs in a serial group.
- [Self-deploy API reference](./self-deploy-api.md) — the `GuardedDeployer` and
  `evaluate_deploy_gate` surface.
