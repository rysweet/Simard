---
title: Canary unit-test gate hermetic isolation
description: Reference for the root-cause repair (#4522) that stops the self-deploy unit-test canary gate crash-looping with `cargo test` exit status 101 — the per-gate isolated state root injected into the scrubbed gate environment by run_unit_test_gate, why the allow-listed SIMARD_STATE_ROOT made the gate non-hermetic against the live daemon, the preserved deny-by-default / deny-over-allow env discipline, and the self-deploy / DeployDrift loop converging once the gate renders a true verdict.
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./canary-gate-convergence.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ./overseer-tick-self-healing.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
---

# Canary unit-test gate hermetic isolation

> **Status: implemented.** The `unit-test` canary gate
> ([`run_unit_test_gate`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs))
> now runs `cargo test` under an **isolated, per-run state root** injected into
> the already-scrubbed gate environment, so the gate no longer collides with the
> currently running daemon's live state and no longer crash-loops with
> `cargo test` exit status `101`. The change is **additive and non-breaking**:
> `run_unit_test_gate`, `scrub_gate_env`, `canary_gate_env_allowlist`, and
> `is_hijack_class_env` keep their signatures; the deny-by-default env floor and
> the deny-over-allow hijack guard are untouched. This repair builds directly on
> the [#4440 canary gate isolation and convergence](./canary-gate-convergence.md)
> work — it closes a hermeticity gap that surfaced *after* the `canary_env`
> allow-list began forwarding `SIMARD_STATE_ROOT` into gate subprocesses.
>
> Tracking issue: [#4522](https://github.com/rysweet/Simard/issues/4522).

## Why this exists

The [#4440 convergence repair](./canary-gate-convergence.md) added
`canary_gate_env_allowlist()` so a healthy candidate's gates inherit the Simard
deploy-shape signals (`SIMARD_HOME`, `SIMARD_PROMPT_ASSETS_DIR`,
`SIMARD_STATE_ROOT`) the deployed systemd unit provides. That is correct for the
process-probe gates (`smoke`, `gym-baseline`, `rpc-health`), which must dial the
**same** state root and socket the running daemon uses.

But the `unit-test` gate is different: it shells out to `cargo test`, and the
Simard test suite reads `SIMARD_STATE_ROOT` (via `resolve_state_root()`) to
locate cognitive-memory state, the meetings directory, and related on-disk
records. With `SIMARD_STATE_ROOT` allow-listed, every canary `cargo test` run
inherited the value pointing at the **live daemon's own state root**. Tests that
open, migrate, or write that state then raced the running daemon — reading a
half-written record, colliding on a lock, or tripping a migration guard — and
aborted the test process with exit status `101` (the code `cargo test` returns
when the test binary itself aborts, as opposed to `1` for an ordinary assertion
failure).

Because the reddening was driven by the ambient state root — present on every
Overseer tick — the `unit-test` gate failed **deterministically** every cycle:

```
WARN overseer::deploy: self-deploy refused by deploy gate
    target_commit=<merged-main> running_commit=<stale>
    failing_gate=unit-test
    failing_detail="tests failed (exit status: 101): ..."
    refusal="red canary (gate unit-test: tests failed (exit status: 101))"
```

The running binary fell several commits behind merged `main` (`DeployDrift`
climbed), and the OODA daemon eventually went `stale (sleep)` with no main PID.
The gate was **correct to fail closed** — it observed a genuine crash — but the
crash was an artifact of the *gate's own non-hermetic environment*, not a source
regression in the candidate. The fix makes the gate hermetic so a healthy
candidate's tests render a true verdict.

> **This is not a "skip the gate" shortcut.** The `unit-test` gate still runs the
> full `cargo test` suite and still fails closed on a real test failure. The
> repair removes an *environmental collision*; it does not weaken, mask, or
> `|| true` the check. A genuinely broken candidate still reddens with exit `101`
> or `1`.

## What changed

1. **Isolated state root for the unit-test gate.** Before spawning `cargo test`,
   `run_unit_test_gate` creates a fresh, private temporary directory (mode
   `0700`) and injects it as `SIMARD_STATE_ROOT` on the gate command — *after*
   `scrub_gate_env` has run. This **overrides** the allow-listed live value for
   this one gate only, so the test suite reads and writes an empty, throwaway
   state root that no other process touches. The directory is removed when the
   gate returns. `TMPDIR` is only additionally pointed at a **nested subdir** of
   that root if step-1 reproduction shows the suite needs a private scratch dir
   (see [Scope note on `TMPDIR`](#scope-note-on-tmpdir)); the core repair for
   hypothesis #1 is the `SIMARD_STATE_ROOT` override alone.
2. **Nothing else in the env discipline moves.** The deny-by-default base floor,
   the `canary_env` allow-list, and the `is_hijack_class_env` deny-over-allow
   guard are unchanged. The other three gates still inherit the live
   `SIMARD_STATE_ROOT` (they *must* dial the running daemon). Only the
   `unit-test` gate substitutes an isolated root, because only it forks a test
   process that mutates that root.
3. **Convergence.** With the collision removed, a healthy candidate's
   `unit-test` gate goes green, the guarded deploy gate stops returning
   `DeployRefusal::RedCanary`, and the self-deploy / `DeployDrift` loop advances
   past the stuck target SHA. No loop logic changed.

## Behavior

### The isolated state root (unit-test gate only)

`run_unit_test_gate`
([`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs))
builds its `cargo test` command through a small, rebuild-free seam —
`build_unit_test_command(config, state_root_path) -> Command` — so the override
is unit-testable via `Command::get_envs()` **without** spawning a real
`cargo test` (see [Regression tests](#regression-tests)). It still goes through
`scrubbed_command` (so the deny-by-default scrub applies), then layers the
isolation on top:

```rust
fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    // Hermetic state root (#4522): `cargo test` reads `SIMARD_STATE_ROOT` via
    // `resolve_state_root()`. The `canary_env` allow-list (#4440) forwards the
    // *live daemon's* root, which is right for the process-probe gates but makes
    // the test suite collide with the running daemon (exit 101). Give this gate —
    // and only this gate — a private, empty root so its tests are hermetic.
    //
    // `IsolatedStateRoot` is a thin RAII wrapper over `tempfile::TempDir` (already
    // a dependency): unique-named, mode 0700, removed on drop. No bespoke temp
    // logic is reinvented.
    let state_root = match IsolatedStateRoot::create() {
        Ok(root) => root,
        Err(e) => {
            return GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("could not create isolated gate state root: {e}"),
            };
        }
    };

    // Rebuild-free seam: builds the fully-scrubbed command with the isolated
    // root already overriding the allow-listed live value. Testable via get_envs().
    let mut cmd = build_unit_test_command(config, state_root.path());

    // ... run, map exit status to GateResult exactly as before ...
    // `state_root` (an RAII guard) removes the temp dir on drop.
}

fn build_unit_test_command(config: &RelaunchConfig, state_root: &Path) -> Command {
    let mut cmd = scrubbed_command("cargo", config);
    cmd.arg("test")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs())
        // Override the allow-listed live root with the private one. Applied
        // AFTER `scrubbed_command` so it wins over the re-injected value.
        .env("SIMARD_STATE_ROOT", state_root);
    cmd
}
```

**Ordering matters.** The isolated `SIMARD_STATE_ROOT` is set on the command
*after* `scrub_gate_env` has re-injected the allow-listed live value, so the
private root wins. This keeps the change to a single, local override — the
allow-list itself is not edited, so the other gates are unaffected.

| Aspect | Before (#4522) | After (#4522) |
| --- | --- | --- |
| `unit-test` `SIMARD_STATE_ROOT` | live daemon root (allow-listed) | private per-run temp root (mode `0700`) |
| `unit-test` `TMPDIR` | cleared by scrub (unset) | unchanged unless reproduction requires it, then a nested subdir of the private root |
| `smoke` / `gym-baseline` / `rpc-health` state root | live daemon root | live daemon root (unchanged) |
| Healthy candidate `unit-test` verdict | RED (exit `101`, collision) | GREEN |
| Broken candidate `unit-test` verdict | RED | RED (unchanged, fail-closed) |

### Scope note on `TMPDIR`

The ranked root cause (hypothesis #1) is the `SIMARD_STATE_ROOT` collision, so the
minimal repair overrides **only** that name. `TMPDIR` is deliberately *not* part
of the base fix: under `scrub_gate_env` it is already cleared, so `cargo`/`rustc`
fall back to the system temp dir, which does not collide with the live daemon.
Inject `TMPDIR` only if step-1 reproduction shows the suite writes temp state that
must be private — and then point it at a **nested subdir** of the isolated root
(e.g. `<root>/tmp`), never the root itself, so cargo scratch files do not pollute
the empty state root the tests read. Per ruthless simplicity, ship the smallest
override that turns the gate hermetic.

### Isolated-root lifecycle

- **Creation.** The directory is created via `tempfile::TempDir` (already a
  dependency): a process-and-thread-unique path under the system temp dir,
  created with mode `0700` (owner-only) so no other user can read the candidate's
  transient test state or plant a symlink (TOCTOU defense). `IsolatedStateRoot`
  is only a thin RAII/newtype wrapper over it — no bespoke unique-naming or
  cleanup logic is reimplemented.
- **Scope.** It is passed only to the `unit-test` gate subprocess. It is never
  written into `RelaunchConfig`, never logged, and never shared across gates.
- **Cleanup.** An RAII guard removes the directory when `run_unit_test_gate`
  returns, whether the gate passed, failed, or errored. A cleanup failure is
  logged at `WARN` via `tracing` and never changes the gate verdict.
- **Failure to create.** If the isolated root cannot be created, the gate fails
  closed (`passed: false`) with a descriptive `detail` — it never silently falls
  back to the live root, because that would reintroduce the collision.

### Observability

The `unit-test` gate keeps emitting through the existing
[per-gate `self_relaunch::gate` span](./canary-gate-convergence.md#per-gate-tracing-spans);
its `detail` still flows through `bound_gate_detail` (credential-redacted, then
bounded to 512 bytes). No new sink is added, and the isolated root's **path** is
emitted only at `tracing` `DEBUG` for troubleshooting — never a value from inside
it. Emission is structured `tracing`/OTel key=value only; there are no
`print!` / `println!` / `eprintln!` sinks.

## Convergence

Once the `unit-test` gate renders a true green verdict for a healthy candidate,
the guarded deploy gate no longer returns `DeployRefusal::RedCanary`; the
[`OrchestratedBinaryDeployer`](./self-deploy-api.md) performs the swap, the next
drift observation sees `DeployDrift == 0`, and the OODA daemon resumes with a
live main PID. The self-deploy loop advances past the previously stuck target SHA
instead of re-queuing the identical exit-`101` refusal. No loop, requeue, or
drift logic changed — the loop was already correct; it was simply never handed a
green `unit-test` gate.

## Fail-closed invariants (preserved)

This repair is bounded by the same rails as the
[#4440 convergence work](./canary-gate-convergence.md#fail-closed-invariants-preserved);
none is relaxed:

- **Gate still runs the full suite.** `cargo test` runs the whole test binary;
  the gate is not narrowed, filtered, or `|| true`-ed. A real failure still
  reddens.
- **Deny by default, deny over allow.** `scrub_gate_env` still `env_clear()`s and
  re-injects only the base floor plus the `canary_env` allow-list, and
  `is_hijack_class_env` still refuses `LD_PRELOAD`-class / `GIT_SSH*` / `BASH_ENV`
  names even if they appear in the allow-list. The isolated root is an
  **override of one allow-listed name's value**, not a new inheritance path — no
  additional ambient variable reaches the gate.
- **No fallback to the live root.** If isolation cannot be established the gate
  fails closed; it never runs `cargo test` against the live daemon's state.
- **Other gates unchanged.** `smoke`, `gym-baseline`, and `rpc-health` still dial
  the live daemon (they must), so `rpc-health` convergence from #4440 is
  unaffected.
- **No new operator inputs.** No CLI flags, RPC, config keys, or "skip gate"
  controls. The trust boundary is unchanged.

## Security considerations

- **Least-privilege additivity.** The smallest change that makes the gate
  hermetic: a single value override on one gate, no new inherited names, no
  widening of the base floor.
- **No secret leakage.** The isolated root's contents are transient test state;
  its path is logged only at `DEBUG`, never its contents, and gate `detail`
  remains credential-redacted.
- **Race-safe temp state.** The root is created with mode `0700` via
  `tempfile`-style unique naming and cleaned up on drop, preventing symlink /
  TOCTOU attacks and cross-run collisions on a predictable path.
- **No production-state corruption.** Because `cargo test` writes only into the
  private root, a canary test run can never mutate or corrupt the **running
  daemon's** live cognitive-memory state.

## Regression tests

The change ships tests in `mod convergence_tests`
([`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs),
`#[cfg(all(test, unix))]`) proving the gate is hermetic and still fails closed.
The load-bearing assertions inspect the **command built by
`build_unit_test_command`** via `Command::get_envs()` — they never spawn a real
`cargo test`, so reproduction cannot trigger a full workspace rebuild or the
associated OOM. Any test that mutates the ambient `SIMARD_STATE_ROOT` must be
serialized under the existing whole-binary key
(`#[serial_test::serial(cognitive_memory)]`), the same key the cognitive-memory
suite uses, so it cannot race a concurrent state-root read.

| Test surface | Asserts |
| --- | --- |
| Isolated root overrides the allow-listed live root (rebuild-free) | With an ambient/allow-listed `SIMARD_STATE_ROOT` pointing at a "busy" root, `build_unit_test_command`'s `get_envs()` maps `SIMARD_STATE_ROOT` to the **isolated** path, not the ambient one. Fails before the fix (no override present), passes after. No `cargo test` spawned. |
| Override wins because of ordering | The isolated value is the one present after `scrub_gate_env` re-injects the allow-listed value — asserted by constructing under a set ambient value and checking `get_envs()` resolves to the isolated path. |
| Isolation does not weaken the hijack guard | With a known hijack var (e.g. `LD_PRELOAD`) present in `config.canary_env` and the ambient env, `get_envs()` shows it is **absent** from the built command even though the gate now sets an extra state-root override — deny-over-allow precedence holds. |
| Isolated root is cleaned up | The temporary state root created by `IsolatedStateRoot`/`tempfile::TempDir` no longer exists after the guard drops. |
| Fail-closed on real failure (unchanged) | A candidate whose test process aborts still yields `passed == false` with the exit status in `detail`. Kept as the existing synthetic fake-candidate test (Smoke-style), not a real `cargo` run, to stay rebuild-free. |

Keeping the assertions at the command-construction layer (`get_envs()`) — rather
than end-to-end green gate runs — is what lets the regression suite prove the fix
without rebuilding the workspace.

## Compatibility

- **Additive, local override.** Only `run_unit_test_gate` changed (plus its
  private `build_unit_test_command` seam); it overrides one env var on its own
  command and manages a temp dir. No public signature, type, or config field
  changed.
- **`canary_gate_env_allowlist` unchanged.** `SIMARD_STATE_ROOT` stays
  allow-listed for the process-probe gates; the unit-test gate simply overrides
  its value locally.
- **No `print`-family macros.** All new emission is `tracing` structured
  key=value; no silent fallbacks.
- **No `Bridge` naming.** New identifiers follow the no-Bridge-naming guard.

## See also

- [Canary gate isolation and self-deploy convergence](./canary-gate-convergence.md) —
  the #4440 root-cause repair this builds on: per-gate spans,
  `RelaunchConfig.canary_env`, `scrub_gate_env`, and the fail-closed invariants.
- [How to converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md) —
  the operator runbook, including the `unit-test` exit-`101` crash-loop case.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  the #4420 observability that names `failing_gate=unit-test` / `failing_detail`.
- [Self-deploy API reference](./self-deploy-api.md) — the `GuardedDeployer`,
  `DeployRefusal`, and the `OrchestratedBinaryDeployer` swap path.
- [Self-deploy source preparation](./self-deploy-source-prep.md) — `scrub_git_env`,
  the model `scrub_gate_env` mirrors.
