---
title: Canary-gate Drop-test determinism (self-deploy exit-101 flake)
description: >
  How the environment-sensitive `Drop` unit test that intermittently exited
  101 under the self-deploy canary `unit-test` gate was made deterministic.
  The fix is isolation-only: the flaky test is keyed into the shared
  `cognitive_memory` serial group (or its env/state-root guard lifetime is
  extended to cover every dependent operation), and the static
  `serial_guard` audit is widened to statically flag the previously-missed
  indirect-reader / Drop-teardown pattern so the class of flake cannot
  silently return. The canary gate's env scrub and allowlist are NOT
  weakened.
last_updated: 2026-07-24
review_schedule: when a new env-mutating or env-reading test is added to the lib binary, or when the canary unit-test gate command changes
owner: simard
doc_type: reference
related:
  - ./cognitive-memory-serial-isolation.md
  - ./deflaking-known-flaky-tests.md
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
  - ../reference/canary-gate-convergence.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
---

# Canary-gate Drop-test determinism

This page documents the finished state of the work that stopped the
self-deploy canary `unit-test` gate from intermittently exiting **101** on an
environment-sensitive `Drop` unit test. It is the test-author and reviewer
contract for the isolation fix and for the strengthened static audit that
keeps the flake closed.

The symptom was a **canary-only** failure: GitHub default-branch CI
(`verify` / `release`) on the same commit SHA was `SUCCESS`, while the
self-deploy canary's `unit-test` gate — which runs `cargo test` under a
deliberately **scrubbed** environment (`env_clear` + a small base floor +
[`canary_gate_env_allowlist()`]) — exited 101 on a `Drop`-bearing test. Because
the canary gate fails closed, self-deploy stalled and the running binary fell
several commits behind merged `main`.

This was **not** a product regression. It was the same process-global
environment race documented in
[cognitive-memory-serial-isolation.md](./cognitive-memory-serial-isolation.md)
and [deflaking-known-flaky-tests.md](./deflaking-known-flaky-tests.md),
re-surfaced by the scrubbed canary environment, which changes test timing and
removes ambient env vars and so makes a latent isolation gap observable that
full-env CI happened not to hit.

> **TL;DR**
>
> - **Root cause class:** an isolation/ordering race, not a panic-in-`Drop`
>   product bug. A test whose `Drop` (typically a
>   [`HermeticState`](./hermetic-tests.md) / env guard) restores or removes a
>   process-global env var runs concurrently with a sibling test that reads
>   that surface but is missing the shared `cognitive_memory` serial key. Under
>   the scrubbed canary env the torn read surfaces as a panic → the `cargo test`
>   process exits 101 → the gate goes RED.
> - **Fix:** isolation-only. Add `#[serial_test::serial(cognitive_memory)]` to
>   the offending test (or extend the env/state-root guard lifetime so it spans
>   every operation that depends on the bound var). No assertion change, no
>   production-behaviour change.
> - **Regression lock:** the static `serial_guard` meta-test is widened so the
>   previously-missed **indirect-reader / Drop-teardown** pattern is flagged at
>   build time — the guard now fails the build for this pattern instead of
>   letting it reach the canary.
> - **Not done:** the canary gate is never weakened to force green — no edits
>   to [`scrub_gate_env`], [`canary_gate_env_allowlist()`], or the gate's
>   parallelism; no `--test-threads=1`. Determinism comes from test hermeticity
>   only.

---

## The failure, precisely

The self-deploy canary runs the candidate binary's own test suite as a gate.
The gate command (`run_unit_test_gate`) is
`cargo test --manifest-path <manifest_dir>/Cargo.toml --target-dir <canary_target_dir>`
(plus `CARGO_BUILD_JOBS`), spawned through `scrubbed_command`. Note it does
**not** pass `--lib`: the gate runs the crate's full `cargo test` — the lib
unit-test binary plus any integration/doc-test binaries — each as a child of
the scrubbed gate process. The child inherits **only**:

- the universal base floor in [`scrub_gate_env`] (`PATH`, `HOME`, the
  Cargo/rustup toolchain vars `CARGO_HOME` / `RUSTUP_HOME` /
  `RUSTUP_TOOLCHAIN`, `SSH_AUTH_SOCK`, and locale/user basics), plus
- the explicit deploy-shape names returned by
  [`canary_gate_env_allowlist()`] (`SIMARD_HOME`, `SIMARD_PROMPT_ASSETS_DIR`,
  `SIMARD_STATE_ROOT`).

Everything else is dropped (deny-by-default); hijack-class variables
(`LD_PRELOAD` and friends) are never allow-listable. See
[canary-gate-convergence.md](../reference/canary-gate-convergence.md) for the
full gate model.

Under that scrubbed env, a test intermittently panicked during or after a
`Drop` that mutates process-global environment state. The gate's `cargo test`
compiles and runs **every** test binary in the crate (the lib unit-test binary
plus any integration binaries), each as a separate child process, so the
exit-101 can in principle originate from any of them. In this case the
offending test — and every module the fix touches (`test_support`,
`self_deploy`, `meeting_backend::persist`) — lives in the lib crate's
`#[cfg(test)]` modules, which is why `--lib` is a valid *local narrowing* once
the failure is located (it is **not** the gate command). Because the panic
happened on a worker thread inside that test-binary process, the binary exited
with status **101**, the gate recorded a failure, and self-deploy stayed RED.

The race mechanism is exactly the one enforced by the
`serial(cognitive_memory)` contract: `environ` is process-global and glibc
`setenv`/`getenv`/`remove_var` are not thread-safe, so a `Drop` that runs
`set_var`/`remove_var` (restoring the pre-test value) can `realloc(environ)`
and free the array a concurrent `getenv` in an unrelated test is mid-read —
even when the two tests never touch the *same* variable.

---

## The fix (finished state)

### 1. The offending test is serialized (isolation-only)

The test surfaced by reproducing the gate is keyed into the shared
`cognitive_memory` serial group:

```rust
#[test]
#[serial_test::serial(cognitive_memory)]
fn the_previously_flaky_test() {
    let state = HermeticState::new();
    // ... exercises a cognitive-memory / env-derived path ...
    // On return, HermeticState's Drop restores SIMARD_STATE_ROOT /
    // SIMARD_MEMORY_SOCKET. Because this test now shares the
    // cognitive_memory key, that Drop can never run concurrently with a
    // sibling test's env read, so it cannot tear one.
}
```

This is the **Annotation Decision Rule** from
[cognitive-memory-serial-isolation.md](./cognitive-memory-serial-isolation.md):
every lib-binary test that mutates *any* process-global env var — including
implicitly, via a guard's `Drop` — or that reads the cognitive-memory
state-root surface, must carry the `cognitive_memory` key. The change is
annotation-only; the test body and its assertions are unchanged.

### 2. Guard lifetime (only if a genuine Drop-lifetime race is proven)

If reproduction shows the panic is a **guard-lifetime** bug rather than a
missing serial key — i.e. a [`HermeticState`](./hermetic-tests.md) / env guard
is dropped *before* an operation that still depends on the bound
`SIMARD_STATE_ROOT` — the guard is bound to a longer-lived local so it outlives
every dependent operation:

```rust
// WRONG — guard drops at the end of the statement; the writer below then
// reads a restored/absent SIMARD_STATE_ROOT.
let root = HermeticState::new().state_root().to_path_buf();
let writer = launch_writer_client(&root)?;   // env already restored → race

// RIGHT — guard lives until the writer has fully drained.
let state = HermeticState::new();
let writer = launch_writer_client(state.state_root())?;
save_goal_board(&board, writer.ops())?;
// `state` drops here, AFTER the writer is done.
```

`HermeticState` already restores env bindings **before** reaping its `TempDir`
(field order in the struct guarantees this), and restores on every exit path
including panic. No change is made to that restore-on-`Drop` guarantee.

> **Diagnostic note — ambient `SIMARD_STATE_ROOT` must not mask the guard.**
> `SIMARD_STATE_ROOT` is one of the three deploy-shape names forwarded into the
> scrubbed gate by [`canary_gate_env_allowlist()`], so the gate's `cargo test`
> child inherits whatever value the daemon was running under. `HermeticState`
> nonetheless **binds `SIMARD_STATE_ROOT` per-test** (`EnvBinding::set(STATE_ROOT_ENV, …)`
> to its own `TempDir`) and restores the *prior* value — here the inherited
> ambient one — on `Drop`. That per-test binding is authoritative for the
> duration of the guard: an ambient `SIMARD_STATE_ROOT` from the gate env must
> never be read by the code under test in place of the guard's binding. If
> reproduction shows a test resolving the *ambient* state root instead of the
> guarded temp path (e.g. it read `SIMARD_STATE_ROOT` before the guard was
> constructed, or after it dropped), that is the guard-lifetime bug above — fix
> the lifetime, do **not** remove `SIMARD_STATE_ROOT` from the allowlist. The
> allowlist forwarding is deliberate deploy-shape parity and stays intact.

### 3. The static audit is widened (regression-as-code)

The `serial_guard` meta-test (`src/test_support/serial_guard.rs`) already fails
the build when a hand-written `#[test]` mutates a watched env var without the
`cognitive_memory` key, watching **`EnvWatch::AnyVar`** so any writer is in
scope. The finished state extends the audit so the **indirect-reader /
Drop-teardown** pattern that slipped past it — a test that touches the env
surface only through a helper's `Drop`, or through an indirect env-reading
handler — is also flagged statically:

- The env-reading handler set and the read-watched variable list are extended
  so the indirect reader that raced is recognized as a cognitive-memory read.
- The audit stays a **pure, AST-based (`syn`) static scan**: repo-tree-bounded
  reads only, no shell-out, no network, no writes. It continues to emit **zero
  false positives** — an [`Offender`] is reported only when a concrete trigger
  is observed without the key — and produces **zero new offenders** on the
  current tree.
- Any deliberate exception must go through the audit **allowlist** with a
  written justification; an allowlist entry lacking a justification is itself a
  build failure.

The net effect: the specific test is fixed *and* the whole class is caught at
build time, so an equivalent gap can no longer reach the canary.

---

## What is explicitly NOT changed

The canary gate stays exactly as strict as before. Forbidden "fixes":

- **No** edits to [`scrub_gate_env`], its base floor, or
  [`canary_gate_env_allowlist()`] to re-admit an env var and paper over the
  race.
- **No** reduction of gate parallelism and **no** `--test-threads=1`. The suite
  keeps full parallelism; determinism comes from hermeticity.
- **No** widening of the allowlist to silence the panic.
- **No** assertion or production-behaviour change unless reproduction proves a
  genuine `Drop`-teardown product defect (a codebase scan found only one benign
  panicking `Drop`, at `operator_commands_dashboard/journal.rs`, which is out of
  scope).
- **No** `print!`/`println!`/`eprintln!` in gate or diagnostic paths — structured
  `tracing` + OpenTelemetry only. The `self_relaunch::gate` span emits variable
  **names** and pass/fail, never env **values**.

---

## Reproduce and verify

### Reproduce the canary gate locally

Reproduce the gate's command **shape** under the scrubbed environment to
surface the exit-101 panic and name the failing test before changing anything.
This is a faithful mirror, not the literal call: the real gate builds its
child via `scrubbed_command("cargo", …)` with `--manifest-path` /
`--target-dir` / `CARGO_BUILD_JOBS`, whereas the loop below uses `env -i` plus
the same base floor and allowlist to get the same scrubbed shape from a normal
checkout.

```bash
# Mirror the canary gate: env_clear + base floor + canary_gate_env_allowlist(),
# then loop under thread pressure to make the rare race observable. The gate
# runs the FULL `cargo test` (no --lib); append `--lib` for a faster local
# narrowing to the in-process lib race once you know it lives there.
for i in $(seq 1 50); do
  env -i \
    PATH="$PATH" HOME="$HOME" \
    CARGO_HOME="$CARGO_HOME" RUSTUP_HOME="$RUSTUP_HOME" \
    RUSTUP_TOOLCHAIN="$RUSTUP_TOOLCHAIN" SSH_AUTH_SOCK="$SSH_AUTH_SOCK" \
    SIMARD_HOME="$SIMARD_HOME" \
    SIMARD_PROMPT_ASSETS_DIR="$SIMARD_PROMPT_ASSETS_DIR" \
    cargo test -- --nocapture 2>&1 | tee "/tmp/repro_canary.$i.log"
  echo "run $i exit=$?"
done
```

The exit-101 panic output names the test and the panic site. Keep the repro
script and its logs **out of the repository** (`/tmp/…`) and scrub any
accidental value capture.

> If the race is too rare to reproduce, pivot to **static serial-key gap
> analysis**: run the widened `serial_guard` audit (below) across the suspect
> tests to identify the test missing the `cognitive_memory` grouping, and fix
> that. The static path does not depend on hitting the probabilistic race.

### Verify the fix

```bash
# 1. The static isolation audit — zero offenders across the whole tree.
cargo test --lib every_env_mutating_test_is_serialized

# 2. The touched modules pass.
cargo test --lib test_support::
cargo test --lib self_deploy::tests_health
cargo test --lib meeting_backend::persist::

# 3. Determinism under the scrubbed gate — loop N× with zero failures
#    (see the reproduce snippet above; expect 50/50 green).

# 4. Formatting / lints.
pre-commit run --all-files
```

Acceptance is met when: all required CI checks are green; the widened
`serial_guard` audit reports zero offenders; the previously-flaky test passes
deterministically under both full-env CI and the scrubbed canary gate; a
regression is locked in (the serialized/keyed test plus the strengthened static
audit); and the self-deploy canary `unit-test` gate goes green, unblocking
self-deploy.

---

## Why this matches the existing isolation model

This work does not introduce a new mechanism — it applies and hardens the two
that already close the sibling flakes:

| Prior flake | Mechanism | This fix |
| ----------- | --------- | -------- |
| `tests_goals_crud::full_goal_lifecycle_crud` ([#2408](https://github.com/rysweet/Simard/issues/2408) / [#2384](https://github.com/rysweet/Simard/issues/2384)) | explicit state-root threading + `cognitive_memory` key | same key/hermeticity contract, applied to the canary-surfaced test |
| prompt-delivery env race ([#2412](https://github.com/rysweet/Simard/issues/2412)) | shared `prompt_delivery_env` serial key | same "share the key so a writer never races a reader" rule |
| `EnvWatch` widened to every variable ([#2375](https://github.com/rysweet/Simard/issues/2375)) | `AnyVar` static audit | audit extended to catch the indirect-reader / Drop-teardown blind spot |

See [cognitive-memory-serial-isolation.md](./cognitive-memory-serial-isolation.md)
for the full contract and the audit's known blind spots, and
[hermetic-tests.md](./hermetic-tests.md) for the `HermeticState` guard
contract.
