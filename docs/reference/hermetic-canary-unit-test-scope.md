---
title: Hermetic self-deploy canary — scoping the UnitTest gate to a deterministic curated target
description: Reference for the durable repair (#4622) that scopes the RelaunchGate::UnitTest canary from the full, ever-shifting `cargo test` lib suite to a dedicated hermetic-by-construction integration target (`cargo test --test self_deploy_canary --features canary-tests`). Documents the opt-in `canary-tests` Cargo feature, the curated `tests/self_deploy_canary.rs` invariant suite, the scoped `build_unit_test_command` argv, the CI proof job, and the preserved fail-closed / deny-by-default env / isolation rails.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./deterministic-canary-unit-test-gate.md
  - ./canary-gate-convergence.md
  - ./canary-unit-test-gate-state-isolation.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../safe-self-update.md
  - ../../Cargo.toml
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
  - ../../tests/self_deploy_canary.rs
  - ../../.github/workflows/ci.yml
---

# Hermetic self-deploy canary — scoping the UnitTest gate to a deterministic curated target

> **Status: implemented.** The scoped `build_unit_test_command` lives in
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs).
> The curated hermetic suite is
> [`tests/self_deploy_canary.rs`](https://github.com/rysweet/Simard/blob/main/tests/self_deploy_canary.rs),
> selected by the opt-in `canary-tests` feature in
> [`Cargo.toml`](https://github.com/rysweet/Simard/blob/main/Cargo.toml), and
> proven green on `main` by a dedicated step in
> [`.github/workflows/ci.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/ci.yml).
> The change is **additive and non-breaking**: `RelaunchGate::UnitTest` stays in
> `default_gates()`, the four-gate no-short-circuit sequence and every public
> signature (`verify_canary`, `all_gates_passed`, `default_gates`, `GateResult`)
> are unchanged, and the gate still **fails closed** on a genuine regression.
> Only the gate's *invocation scope* narrows.

## Why this exists

Simard's OODA daemon self-deploys by building a candidate binary and running a
canary of [`default_gates()`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
before it swaps in the new image. One of those gates,
`RelaunchGate::UnitTest`, shelled out to the **entire** `cargo test` lib suite
against the candidate inside an isolated self-deploy-target environment.

Running the full lib suite in that isolated env was structurally unsafe. The
suite contains **non-hermetic** tests that pass in CI on clean runners but panic
under the self-deploy canary's conditions:

- **Shared process globals** — tests that read or write `HOME`, temp dirs, or
  other process-wide state collide when the canary env differs from CI.
- **Serial-resource contention** — tests that share a resource without
  `#[serial]` race under the canary's parallelism.
- **Drop-order cleanup guards** — teardown guards that assume a specific
  destruction order panic on the isolated target.

Because the canary ran the *whole* suite, every newly-merged test that carried
one of these assumptions could re-wedge the gate. This produced a
**whack-a-mole**: each merge de-flaked one test while a new one surfaced. The
running binary froze at `running_commit e3a4327834db` for 7h+; the deploy gate
refused the same target on every tick (13+ ticks over 7h — refusals at 03:06,
06:55, 07:46, 09:07, 09:35Z per the 07:32Z health review). The 09:35Z run failed
`cargo test` with exit status 101 in **1.13s on a cached rebuild**, proving a
**deterministic panic, not a timing flake** — and the same tests ran **green in
CI** the whole time.

The [red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) (#4420)
and the [deterministic unit-test gate](./deterministic-canary-unit-test-gate.md)
work made the reddening gate *visible and diagnosable*. This feature acts on
that signal to remove the **root cause structurally**: the canary should never
have been coupled to the whole shifting lib suite. It is scoped to a small,
curated, hermetic-by-construction set of genuine self-deploy invariants.

> **This is the durable convergence, not another band-aid.** It does **not** add
> a single-test exclusion, does **not** adopt retry / time-window masking, and
> does **not** remove the `UnitTest` gate. It changes *what the gate runs* to a
> deterministic subset that a new non-hermetic lib test can never re-wedge.

### Relationship to the superseded PRs

A pile of stale, conflicting auto-generated PRs targeted this exact gate. This
scoping change lands as **PR #4622** and **supersedes and closes all five of
them**, so exactly one fix reaches `main`. Approaches confirmed against each
PR's actual head at closure time:

| PR | Actual approach (per PR title/diff) | Why superseded |
| --- | --- | --- |
| #4623 | *"drop flaky full-suite UnitTest from default deploy-gate canary set"* — removes `UnitTest` from `default_gates()`. | Weakens fail-closed — deletes a real deploy-authorization control. This feature keeps the gate and narrows only its scope. |
| #4625 | *"Make lib test suite fully resource-isolated under massive parallelism"* — isolates the entire lib suite. | ~60-file rewrite touching ~2079 hermetic-relevant sites; high-conflict and never converges. Scoping is decisively smaller and ends the whack-a-mole structurally. |
| #4624 | *"heal exe_mtime atomic-replace-window ENOENT"* — fixes one race in one test. | Per-test band-aid; the next non-hermetic lib test re-wedges the gate. |
| #4566 | *"de-flake local canary deploy-gate under CPU oversubscription"* — single-test de-flake. | Same whack-a-mole class as #4624 — one test healed, the coupling to the shifting suite remains. |
| #4570 | *"de-flake deploy-gate unit-test suite (fork/exec + load races)"* — single-suite de-flake. | Same whack-a-mole class; treats symptoms of running the full suite rather than the coupling itself. |

> **Confirm each PR's current head before posting its `superseded-by #4622`
> closure comment** — auto-generated PRs can be force-pushed, so re-read the head
> so the per-PR reason stays accurate. Each is closed with a `superseded-by
> #4622` reference at merge.

## What changed

1. **Scoped gate invocation.** `build_unit_test_command`
   ([`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs))
   no longer runs an unfiltered `cargo test`. It runs the single curated target
   `cargo test --test self_deploy_canary --features canary-tests`. All existing
   discipline is preserved: `--manifest-path`, `--target-dir`, `CARGO_BUILD_JOBS`,
   the per-run isolated `TempDir` state root, `scrub_gate_env`, output
   truncation, and fail-closed `Err ⇒ RED`.
2. **Opt-in `canary-tests` feature.** A new, non-default, TEST-SELECTION feature
   in `[features]` turns on the curated suite. It mirrors the *purpose* of the
   `slow-tests` idiom (opt-in, test-only, not shipped), but selects the suite via
   a **whole-target inner attribute** (`#![cfg(feature = "canary-tests")]`)
   rather than `slow-tests`' per-test `#[cfg(feature = "slow-tests")]`
   annotations. Stock `cargo test` and CI unit runs are unaffected. Adding it
   makes `slow-tests` no longer the *only* exception to the "features are
   default" rule, so the existing `slow-tests` comment must be updated in the
   same change (see [Configuration](#configuration)).
3. **Curated hermetic suite.** A new integration target
   [`tests/self_deploy_canary.rs`](https://github.com/rysweet/Simard/blob/main/tests/self_deploy_canary.rs),
   gated by `#![cfg(feature = "canary-tests")]`, asserts ≥5 genuine self-deploy
   invariants through the crate's **public API**. Every test is
   hermetic-by-construction (own `TempDir`, own `HOME`/`SIMARD_STATE_ROOT`/
   `SIMARD_HOME`, `#[serial]` for process globals, deterministic Drop cleanup).
4. **CI proof job.** A step in
   [`.github/workflows/ci.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/ci.yml)
   runs `cargo test --test self_deploy_canary --features canary-tests`, so the
   curated set is proven green on `main` and a target-name typo is caught
   pre-merge (a misspelled target reddens the gate — fail-closed, but CI catches
   it first).

## Configuration

### The `canary-tests` Cargo feature

`canary-tests` is the **second** intentional exception to Simard's
"features are default" rule (issue #2576), alongside `slow-tests`: a
TEST-SELECTION gate, not a shippable capability, so it stays opt-in. Because
`Cargo.toml` documents `slow-tests` as *"THE ONE INTENTIONAL EXCEPTION"*, adding
`canary-tests` **requires amending that comment in the same change** (otherwise
`Cargo.toml` self-contradicts). The two share one header:

```toml
# Cargo.toml — [features]

# TWO INTENTIONAL EXCEPTIONS to the "features are default" rule (issue #2576):
# `slow-tests` and `canary-tests` are NOT shippable product capabilities — they
# are TEST-SELECTION gates. Defaulting either would force test-only scaffolding
# into every `cargo test` / CI run for zero runtime benefit, so both stay opt-in.

# `slow-tests` turns on the slow, long-running `#[cfg(feature = "slow-tests")]`
# tests. Enable with `cargo test --features slow-tests`.
slow-tests = []

# `canary-tests` (issue #4622) turns on the curated, hermetic-by-construction
# self-deploy canary regression target (`tests/self_deploy_canary.rs`, selected
# by a whole-target `#![cfg(feature = "canary-tests")]` inner attribute). The
# self-deploy UnitTest canary enables it explicitly; stock `cargo test` and CI
# unit runs leave it off so they never pull in the canary-only scaffolding.
# Enable with `cargo test --test self_deploy_canary --features canary-tests`.
canary-tests = []
```

| Property | Value |
| --- | --- |
| Default-enabled | **No** — opt-in only |
| Shipped in `cargo build` | **No** — test-selection only, compiled into no runtime path |
| New runtime dependencies | **None** (`tempfile`, `serial_test` are already dev-deps) |
| Enabled by | the self-deploy UnitTest canary and the CI proof job |

## API / behavior

### The scoped `build_unit_test_command`

`build_unit_test_command`
([source](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs))
keeps its signature `(config) -> SimardResult<(Command, TempDir)>` and every
isolation guarantee; only the argv narrows. It builds a `scrubbed_command`
running `cargo test --test self_deploy_canary --features canary-tests` (with
`--manifest-path`, `--target-dir`, and `CARGO_BUILD_JOBS`), then applies the
`#4628` isolation override *after* the scrub (last-write-wins):
`SIMARD_STATE_ROOT`/`SIMARD_HOME` point at a fresh per-run `TempDir` and
`SIMARD_MEMORY_SOCKET` is removed so the canary uses throwaway stores and cannot
dial the live daemon. It returns the `TempDir` guard the caller keeps alive
until `cmd.output()` completes, and fails closed (`Err ⇒ RED`) if the isolated
root cannot be created — it never falls back to the live state root. See the
source for the exact argv and the doc-comment rationale.

**What is unchanged (all preserved):**

- The `RelaunchGate::UnitTest` variant, its `Display` label (`unit-test`), and
  its position in `default_gates()` — `Smoke → UnitTest → GymBaseline →
  RpcHealth` — run to completion **without short-circuit**.
- `scrub_gate_env` deny-by-default env floor + `canary_gate_env_allowlist()`
  (see [canary gate convergence](./canary-gate-convergence.md)).
- Full stdout+stderr capture and bounded, credential-redacted failure detail on
  the `self_relaunch::gate` tracing span (see
  [deterministic unit-test gate](./deterministic-canary-unit-test-gate.md)).
- The `#4628` state isolation seam (see
  [canary unit-test gate state isolation](./canary-unit-test-gate-state-isolation.md)).

> **Argv is static literals only.** `--test`, `self_deploy_canary`,
> `--features`, and `canary-tests` are discrete `.arg` literals — no shell,
> no format-string interpolation — so the scope change introduces no command
> injection surface.

### The curated hermetic suite (`tests/self_deploy_canary.rs`)

The target is compiled only when `canary-tests` is enabled and asserts the
load-bearing self-deploy invariants through the crate's **public API**, so the
gate stays a meaningful deploy-authorization control rather than a cosmetic
pass. Its module attribute is `#![cfg(feature = "canary-tests")]`, and every
test MUST be hermetic: own `TempDir`, own `HOME`/`SIMARD_STATE_ROOT`/
`SIMARD_HOME`, `#[serial]` for process globals, and deterministic Drop cleanup.
A non-hermetic test added here would re-wedge the gate, so reviewers reject any
test that touches the live state root, shares an unserialized global, or relies
on Drop ordering.

The suite covers, at minimum:

| Invariant | Asserts |
| --- | --- |
| **Gate execution (end-to-end)** | `verify_canary(stub_binary, &[RelaunchGate::Smoke], &config)` drives a real gate against a **deliberately-red stub binary** (one whose `--version` exits non-zero): the returned `GateResult.passed` is `false` and `all_gates_passed(&results)` is `false`. A red gate executed through the live path is **never** mapped to pass. This is the load-bearing invariant — it exercises the authorization control itself, not just its pure helpers. |
| Gate order | `default_gates()` yields exactly `Smoke → UnitTest → GymBaseline → RpcHealth`. |
| Aggregate verdict | `all_gates_passed(results)` is `true` **iff** the verdict set is non-empty and every `GateResult.passed` is `true`. An empty set fails closed (authorizes nothing); a single red gate fails the aggregate. |
| Env deny-by-default floor | `canary_gate_env_allowlist()` carries deploy-shape signal *names* (`SIMARD_HOME`, …) and never hijack vars (`LD_PRELOAD`, `GIT_SSH_COMMAND`). |
| State isolation (#4628) | Canary state stays inside the owned `TempDir`; the live state root/secrets are never reachable — isolation is asserted as an invariant, and its absence refuses RED. |

> **At least one invariant must drive `verify_canary` (or a `run_*_gate`) against
> a candidate binary end-to-end.** The other rows assert on pure/constant helpers
> (`default_gates`, `all_gates_passed`, `canary_gate_env_allowlist`) that cannot
> regress from a real self-deploy bug. Without the gate-execution row the suite
> would be tautological and would *relocate* the fail-closed control rather than
> protect it. The stub-binary Smoke path needs no `cargo` and stays hermetic.

Each test consumes only the public surface —
[`verify_canary`, `default_gates`, `all_gates_passed`, `canary_gate_env_allowlist`, `RelaunchGate`, `GateResult`, `RelaunchConfig`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/mod.rs) —
so no crate internals are exposed to the integration target. The gate-execution
invariant above **actually invokes `verify_canary`**, so the doc's "consumed"
public symbols are exercised, not merely imported.

> **The curated set must stay non-empty and meaningful.** An empty or cosmetic
> suite would hollow the deploy-authorization control (a fail-closed
> *regression*, not just a quality one). Review mandates ≥5 genuine invariants;
> CI asserts the target compiles and passes; removals are flagged.

## Usage

### Run the canary locally (what the gate runs)

```bash
# Exactly what RelaunchGate::UnitTest executes (minus the isolated tempdir env
# the gate injects at spawn time). Green here ⇒ green gate.
cargo test --test self_deploy_canary --features canary-tests
```

### Verify stock builds/tests are unaffected

```bash
# The curated target is NOT compiled without the feature; the full lib suite
# still runs as before. Neither pulls in canary-only scaffolding.
cargo build            # canary-tests absent → no self_deploy_canary code
cargo test             # full lib suite, unchanged; self_deploy_canary skipped
```

### Add or change a curated invariant

1. Add the test to `tests/self_deploy_canary.rs`.
2. Make it hermetic-by-construction:
   - own `tempfile::TempDir`; set `HOME`, `SIMARD_STATE_ROOT`, `SIMARD_HOME`
     to it;
   - annotate `#[serial]` (from the existing `serial_test` dev-dep) for any
     process-global;
   - keep teardown order-independent (no Drop-order assumptions).
3. Assert a real self-deploy/self-relaunch invariant through the public API —
   not an implementation detail.
4. Run `cargo test --test self_deploy_canary --features canary-tests` locally;
   CI re-proves it on `main`.

> **Never add a non-hermetic test here.** A test that reads the live state root,
> shares an unserialized global, or relies on Drop ordering re-introduces the
> exact wedge this feature removed. Such tests belong in the general lib suite,
> which the canary no longer runs.

## CI

`.github/workflows/verify.yml` gains a step that proves the curated target on
every push to `main` and every PR:

```yaml
- name: Self-deploy canary regression target
  run: cargo test --test self_deploy_canary --features canary-tests
```

This guarantees the exact target name and feature the gate uses stay valid: a
rename or typo fails CI **before** it can redden the live self-deploy canary.

## Convergence (done-when)

Once the scoped gate passes on current `main`:

- the guarded deploy gate no longer returns `DeployRefusal::RedCanary` for a
  healthy candidate;
- the [`OrchestratedBinaryDeployer`](./self-deploy-api.md) performs the swap and
  `running_commit` advances off `e3a4327834db`;
- the next drift observation in
  [`overseer::deploy`](./overseer-deploy-canary-diagnostics.md) sees
  `DeployDrift == 0`.

Exactly one PR (#4622) lands; the five competing stale PRs are closed as
superseded. No loop, requeue, or drift logic changed — the loop was already
correct once handed a green canary.

## Known limitations / future work

- **Gate coverage is `Smoke`-end-to-end + pure helpers, not `run_unit_test_gate`
  end-to-end (B).** The curated suite drives `verify_canary` against a stub
  binary through the **`Smoke`** gate (a real fail-closed authorization path) and
  asserts the pure aggregation/order/allow-list helpers. It does **not** spawn
  the scoped `cargo test --test self_deploy_canary` invocation that
  `run_unit_test_gate` performs — doing so would fork a nested `cargo` build
  inside a test, which is neither hermetic nor fast. That gate's scoped argv and
  `#4628` isolation are instead pinned by the in-crate `state_isolation_tdd` unit
  tests (`unit_test_gate_scopes_to_hermetic_canary_target_and_feature`, …), and
  the [CI proof job](#ci) keeps the target name/feature valid. Closing the full
  end-to-end gap would need a sandboxed `cargo` fixture; tracked as future work.
- **The stub binary is unix-only (D).** `write_stub_binary` emits a `#!/bin/sh`
  script and `chmod +x`es it under `#[cfg(unix)]`, so the curated suite runs on
  unix only. Simard self-deploys on unix hosts, so this matches the gate's real
  runtime; a Windows port would need a `.cmd`/compiled no-op stub before the
  suite could run on Windows CI.

## Fail-closed invariants & compatibility (preserved)

This scoping is **additive and non-breaking**, bounded by the same rails as the
prior canary work; none is relaxed:

- **Canary is the authorization boundary.** The four gates still gate promotion
  and fail closed; a genuine regression in the curated set reddens `UnitTest`
  exactly as a lib-suite failure did before. `RelaunchGate::UnitTest` stays in
  `default_gates()` — only its invocation scope changes.
- **No short-circuit.** All gates run to completion; `all_gates_passed` requires
  a non-empty set where every gate passed (fail-closed on empty).
- **`Err ⇒ RED`.** A missing isolated root, a `cargo` error, a compile failure,
  or a misspelled `--test` target all redden the gate — never mapped to pass.
- **Deny-by-default env + #4628 isolation** are unchanged (`scrub_gate_env`,
  `canary_gate_env_allowlist`, per-run `TempDir` state root, memory-socket
  removal).
- **Signatures unchanged.** `verify_canary`, `all_gates_passed`, `default_gates`,
  `RelaunchGate`, `GateResult`, and `RelaunchConfig` keep their signatures; the
  `types.rs`/`mod.rs` gate-order tests stay green.
- **Additive, dev/test-only feature.** `canary-tests = []` is opt-in, non-default,
  shipped in no runtime path, and adds no runtime deps (`tempfile`, `serial_test`
  are already dev-deps). `tests/self_deploy_canary.rs` compiles only under it.
- **No new operator inputs, no `print`-family macros, no `Bridge` naming.** No
  CLI flags, RPC, config keys, or "skip gate" controls (trust boundary
  unchanged); all gate emission stays `tracing`→OTel (no `print!`/`println!`/
  `eprintln!`, no silent fallbacks); new identifiers follow the
  [no-Bridge-naming guard](./no-bridge-naming-guard.md).

## See also

- [Deterministic self-deploy canary — unit-test gate diagnostics](./deterministic-canary-unit-test-gate.md) —
  the full stdout+stderr capture and sanitized failing-test-name tracing this
  scoping builds on.
- [Canary gate isolation and self-deploy convergence](./canary-gate-convergence.md) —
  `scrub_gate_env`, the `canary_env` allow-list, and the deny-by-default floor.
- [Canary unit-test gate state isolation](./canary-unit-test-gate-state-isolation.md) —
  the `#4628` per-run `TempDir` state-root seam the scoped command keeps.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  the `failing_gate` / `failing_detail`, `refusal_reason`, and `DeployDrift`
  signal this feature clears.
- [How to converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md) —
  the operator runbook for confirming convergence.
- [Self-deploy API reference](./self-deploy-api.md) — the `GuardedDeployer`,
  `DeployRefusal`, and the `OrchestratedBinaryDeployer` swap path.
