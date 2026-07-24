---
title: Hermetic unit-test canary gate
description: Reference for the #4558 root-cause repair that stops the live daemon from red-canarying a green tree. The unit-test gate runs `cargo test` in an isolated per-run temp state root (SIMARD_STATE_ROOT/SIMARD_HOME/HOME/TMPDIR + manifest current_dir) with CARGO_HOME/RUSTUP_HOME pinned from the real HOME so an in-process lib-test cannot bind the daemon's socket or lock its WAL/cognitive-store and the toolchain still resolves, and captures BOTH stdout and stderr, extracting the failing test name into failing_detail via extract_failure_detail with a 4096-byte structured-marker clamp.
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
  - ./state-root-resolution.md
  - ./string-truncation-helpers.md
  - ./overseer-tick-self-healing.md
  - ../howto/diagnose-a-red-canary-unit-test-gate.md
  - ../safe-self-update.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
---

# Hermetic unit-test canary gate

> **Status: implemented.** The `unit-test` canary gate
> ([`run_unit_test_gate`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs))
> now runs `cargo test` in a **hermetic, per-run isolation directory** and
> extracts the failing test name into `failing_detail`. Both changes live at the
> smallest responsible site
> ([`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs));
> `canary.rs`, `types.rs`, and `overseer/deploy.rs` are unchanged (deploy still
> wires results/refusal only). The change is **additive and non-breaking**:
> `verify_canary`, `all_gates_passed`, `default_gates`, `RelaunchConfig`, and the
> four-gate no-short-circuit sequence keep their signatures and semantics.

## Why this exists

The [canary gate convergence repair](./canary-gate-convergence.md) (#4440)
scrubbed the gate environment and supplied deploy-shape signals so a healthy
candidate could render a true verdict. It did **not** isolate the one gate that
runs a full in-process test binary. That left a systemic crash-loop:

- **Symptom (#4558).** Every self-deploy for 6+ hours was refused on the
  `unit-test` gate — 20–21 consecutive red-canary refusals — pinning
  `running_commit` while `DeployDrift` grew from 7 to 8 commits behind and
  stranding all merged work. The journal signature was:

  ```
  WARN overseer::deploy: self-deploy refused by deploy gate
      failing_gate="unit-test"
      failing_detail="tests failed (exit exit status: 101)"
  ```

- **Root cause.** `run_unit_test_gate` invoked
  `cargo test --manifest-path <dir>/Cargo.toml --target-dir <canary_target_dir>`
  as a **child of the live daemon**. The lib-test binary aborted with exit
  status `101` in under 1.3s (`Running unittests src/lib.rs …` then exit) — even
  though `cargo test --lib` on the *identical* source passed clean standalone
  (9279 passed / 0 failed / 133s). The tree was green; the **deploy-gate
  environment** reddened it: statics in the in-process suite bound the daemon's
  fixed socket / port or took a lock on the shared cognitive-store / WAL under
  the daemon's state root, and aborted immediately.

- **Second failure — undiagnosable.** The gate captured only `stderr` and
  truncated it to 200 bytes. On a red tree the 200-byte head landed on a
  progress-spinner fragment (`Drop t…`), hiding *which* test failed. The
  operator saw `tests failed (exit …)` with no test name.

This feature fixes both: the gate is made **hermetic** (a running daemon can no
longer red-canary a passing suite) and **diagnosable** (a real failure names the
failing test).

This does **not** weaken, skip, or disable the gate. A genuinely failing test
still reddens; only the environment-induced false red is removed.

## What changed

1. **Hermetic execution.** After `scrub_gate_env`, `run_unit_test_gate`
   overrides four isolation keys — `SIMARD_STATE_ROOT`, `SIMARD_HOME`, `HOME`,
   `TMPDIR` — to a fresh per-run temp directory, and sets `current_dir` to the
   manifest dir. The in-process test suite therefore resolves an **empty,
   private** state root ([`default_state_root`](./state-root-resolution.md):
   `SIMARD_STATE_ROOT` else `$HOME/.simard`) and cannot open the live daemon's
   WAL / cognitive-store or bind its socket.
2. **Toolchain pin.** Because the `HOME` override would otherwise strand
   `cargo`/`rustup` (they fall back to `$HOME/.cargo` / `$HOME/.rustup` when
   `CARGO_HOME` / `RUSTUP_HOME` are unset), a new private helper
   `resolve_toolchain_home()` computes absolute `CARGO_HOME` / `RUSTUP_HOME`
   from the **real, pre-override** `HOME` (preferring ambient values when set)
   and pins them on the child **before** `HOME` is redirected. See the
   load-bearing invariant under [Isolation keys](#isolation-keys).
3. **Fail-closed isolation.** The temp dir is created via a new private helper
   `unit_test_isolation_dir() -> Result<TempDir, _>`. If temp-dir setup fails the
   gate returns a **failing** `GateResult` — never a silent non-hermetic
   fallback to the daemon's live state root.
4. **Diagnosable failure.** On non-zero exit the gate now captures **both**
   stdout and stderr and feeds them to a new pure helper
   `extract_failure_detail(stdout, stderr)`, which pulls the first structured
   marker block (`failures:` / `panicked at …` / `test … FAILED`, test-name
   first) and UTF-8-safely clamps it to **4096 bytes** — raised from the old
   200-byte stderr-only head — so the failing test **name** survives into
   `failing_detail`.

## Data model

No new or changed public types. `RelaunchConfig`
([`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs))
is byte-for-byte unchanged, so the `Default`/byte-layout compatibility guard test
stays green. The isolation directory is an ephemeral RAII
[`tempfile::TempDir`](https://docs.rs/tempfile) (already a full `[dependencies]`
entry, `=3.27.0`) that is cleaned up when the gate returns.

### Isolation keys

The four environment names overridden **per child** (never in the parent
process — no `set_var`, so no `ENV_LOCK`/`unsafe` hazard):

| Key | Overridden to | Why |
| --- | --- | --- |
| `SIMARD_STATE_ROOT` | fresh temp dir | governs WAL / cognitive-store / socket paths ([state-root resolution](./state-root-resolution.md)) |
| `SIMARD_HOME` | fresh temp dir | deploy-shape home; kept off the live tree |
| `HOME` | fresh temp dir | fallback state root (`$HOME/.simard`) when `SIMARD_STATE_ROOT` unset in a sub-suite |
| `TMPDIR` | fresh temp dir | scratch / socket-dir isolation |

Plus two **pinned** (not temp-redirected) toolchain keys, set on the child
**before** the `HOME` override so `cargo`/`rustup` can still find the toolchain:

| Key | Pinned to | Why |
| --- | --- | --- |
| `CARGO_HOME` | absolute path from the real pre-override `HOME` (or ambient value) | prevents cargo hunting the toolchain under the empty temp `$HOME/.cargo` |
| `RUSTUP_HOME` | absolute path from the real pre-override `HOME` (or ambient value) | same, for the rustup toolchain root |

These override the same-named values that `scrub_gate_env` re-injects from the
[`canary_gate_env_allowlist`](./canary-gate-convergence.md) (`SIMARD_HOME`,
`SIMARD_STATE_ROOT`). Ordering is load-bearing: the override is applied **after**
`scrub_gate_env`, via per-child `Command::env(...)`, so the isolated temp path
wins.

> **Load-bearing invariant — toolchain resolution must survive the `HOME`
> override.** `cargo`/`rustup` resolve their toolchain from `CARGO_HOME` /
> `RUSTUP_HOME` and, *only when those are unset*, fall back to `$HOME/.cargo` /
> `$HOME/.rustup`. `scrub_gate_env`'s base floor re-injects
> `CARGO_HOME` / `RUSTUP_HOME` / `RUSTUP_TOOLCHAIN` **only if they are present in
> the daemon's ambient env** (`if let Ok(val) = env::var(..)`). Under a clean
> systemd unit they are frequently *absent* — the daemon relies on the
> `$HOME/.cargo` default. If `HOME` is then overridden to an **empty** temp dir
> while `CARGO_HOME` / `RUSTUP_HOME` are unset, `cargo test` looks for the
> toolchain under the empty temp `$HOME/.cargo` and aborts — a **new
> self-inflicted false red of the exact #4558 class**. Therefore the isolation
> step MUST pin `CARGO_HOME` and `RUSTUP_HOME` to absolute paths **resolved from
> the real pre-override `HOME`** (or the ambient values) and set them explicitly
> on the child *before* overriding `HOME` — never leave them to the
> pass-through-if-present floor. `RUSTUP_TOOLCHAIN` is passed through unchanged.

## Behavior

### `run_unit_test_gate` (hermetic)

```rust
fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    // Fail closed: no isolation dir -> failing GateResult, never a live-root fallback.
    let isolation = match unit_test_isolation_dir() {
        Ok(dir) => dir,
        Err(e) => {
            return GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("unit-test gate could not create an isolated state root: {e}"),
            };
        }
    };
    let iso = isolation.path();

    // Pin the toolchain to absolute paths resolved from the REAL (pre-override)
    // HOME *before* HOME is redirected to the temp dir. Prefer the ambient
    // CARGO_HOME / RUSTUP_HOME when present; otherwise derive $HOME/.cargo and
    // $HOME/.rustup from the real HOME. Without this, overriding HOME to an
    // empty temp dir makes cargo/rustup hunt the toolchain under the empty
    // temp $HOME/.cargo and abort — a fresh #4558-class self-inflicted red.
    let (cargo_home, rustup_home) = resolve_toolchain_home();

    let mut cmd = scrubbed_command("cargo", config);
    cmd.arg("test")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs())
        // Toolchain pin — absolute, resolved from the real HOME, set explicitly
        // so the HOME override below cannot strand cargo/rustup.
        .env("CARGO_HOME", &cargo_home)
        .env("RUSTUP_HOME", &rustup_home)
        // Hermetic override — applied AFTER scrub_gate_env so it wins.
        .env("SIMARD_STATE_ROOT", iso)
        .env("SIMARD_HOME", iso)
        .env("HOME", iso)
        .env("TMPDIR", iso)
        .current_dir(&config.manifest_dir);

    match cmd.output() {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: true,
            detail: "all tests passed".to_string(),
        },
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = extract_failure_detail(&stdout, &stderr);
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("tests failed (exit {}): {}", output.status, detail),
            }
        }
        Err(e) => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: format!("cargo test failed to run: {e}"),
        },
    }
}
```

- **Explicit argv only.** `cargo test` is spawned via `Command` with an explicit
  argument vector — never `sh -c` — and the manifest dir comes from
  `config.manifest_dir` only, never a daemon- or network-controlled path.
- **Toolchain pin before `HOME` override.** `resolve_toolchain_home()` returns
  absolute `CARGO_HOME` / `RUSTUP_HOME` derived from the real pre-override
  `HOME` (preferring ambient values), set on the child so redirecting `HOME` to
  the temp dir cannot strand cargo/rustup. `RUSTUP_TOOLCHAIN` is passed through
  unchanged.
- **Canonicalized, in-root temp path.** The isolation path is canonicalized and
  rejected if it resolves outside the temp root, so a test cannot rediscover the
  production state root through a symlink.
- **Residual risk — env-derived paths only.** The four overrides redirect every
  path the runtime derives from `SIMARD_STATE_ROOT` / `$HOME` / `TMPDIR`
  (WAL, cognitive-store, default socket dir). They do **not** neutralize a test
  that hardcodes an absolute socket path or a fixed TCP port. The implementation
  step therefore grep-audits the in-process suite for fixed-port `bind` / hardcoded
  absolute socket paths and, if any is found, adds an additive per-child override
  at this same site rather than relying on the state-root redirect alone.

### `extract_failure_detail` (diagnosable)

A pure function over `(stdout, stderr)` — no subprocess, unit-testable in
isolation — that selects the first structured marker block, test-name first, and
clamps it UTF-8-safely to 4096 bytes:

```rust
/// Extract the operator-actionable failure block from a `cargo test` run.
/// Scans the COMBINED stdout+stderr stream (stdout carries `test … FAILED` /
/// `failures:`; stderr carries `panicked at …`) and returns the first matching
/// marker block with the failing test NAME preserved. Returns a bounded,
/// UTF-8-safe string (<= 4096 bytes). Marker/name selection only — never a raw
/// dump — so enlarged detail cannot leak unrelated output into logs.
fn extract_failure_detail(stdout: &str, stderr: &str) -> String { /* … */ }
```

Marker precedence (first match wins):

1. `failures:` — the cargo/libtest failure block. Note libtest emits
   `failures:` twice: the **first** heads the detailed `---- <name> stdout ----`
   panic dumps (test name **and** panic message), the **second** lists the bare
   failing test names. First-match capture therefore lands on the richer
   name+panic block — a superset of just the name list.
2. `panicked at …` — the panic site (carries the assertion / message).
3. `test <name> … FAILED` — the per-test result line.

If none of the markers is present (e.g. a linker OOM or an abort before any
test line), the extractor falls back to the tail of the combined stream so the
detail is never empty, still clamped to 4096 bytes.

**Truncation chain.** The gate clamps to **4096 bytes** (raised from 200). The
downstream deploy composer's `bound_detail` (see
[red-canary diagnostics](./overseer-deploy-canary-diagnostics.md)) still applies
its **512-byte** governing cap — but because the gate now extracts the
marker+name block *before* clamping, the failing test name survives both bounds
instead of being lost to a 200-byte spinner fragment. See
[string-truncation helpers](./string-truncation-helpers.md) for the
char-boundary-safe `truncate_output` used at each step.

### Before / after

| | Before (#4558 crash-loop) | After (hermetic gate) |
| --- | --- | --- |
| State root | live daemon's (shared WAL/socket) | fresh per-run temp dir |
| Green tree under running daemon | red-canary (exit 101 in <1.3s) | **passes** |
| Capture on failure | stderr only | **stdout + stderr** |
| Detail bound | 200-byte raw head | 4096-byte extracted marker block |
| Failing test name in `failing_detail` | lost (`Drop t…`) | **present** (`failures:` / `panicked at` block) |

## Fail-closed invariants (preserved)

- **Canary is the authorization boundary.** The four gates still gate promotion
  and still fail closed. A genuinely failing test still reddens.
- **No silent fallback.** A temp-dir/env setup failure returns a **failing**
  `GateResult`, never a non-hermetic run against the live state root
  (prevents a gate writing production runtime state).
- **No short-circuit.** Gate order `Smoke → UnitTest → GymBaseline → RpcHealth`
  runs to completion; `all_gates_passed` still requires every gate to pass.
- **Deny-by-default env unchanged.** `scrub_gate_env` still `env_clear()`s and
  re-injects only the base floor + `canary_env` allow-list; the isolation
  override adds **only** the four keys above — the allow-list is not widened and
  no daemon secret propagates into the child.
- **No privilege change.** The child inherits, never escalates, privilege (no
  `sudo`/`setuid`).
- **Detail routes through tracing/OTel only.** The enlarged detail is emitted at
  the existing gate log level as structured markers (not a raw dump) and is
  bounded by the 512-byte `bound_detail` secondary cap; no `print!`/`println!`.

## Regression tests

The change ships bidirectional tests proving the gate passes a green tree under a
simulated live daemon and names the failing test on a red tree.

| Test surface | Asserts |
| --- | --- |
| [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs) `#[cfg(test)]` (unit, no subprocess) | `extract_failure_detail` returns the failing test **name** from a `failures:` block and from a `panicked at …` block; UTF-8-boundary clamp at 4096B never splits a char; empty / no-marker input falls back to a bounded tail. |
| [`tests/fixtures/unit_test_gate_fixture/`](https://github.com/rysweet/Simard/blob/main/tests/fixtures) (integration) | A minimal fixture crate (one passing + one panicking `#[test]`) run through the gate: **(a)** the green fixture **passes** even when a simulated live daemon holds the shared `SIMARD_STATE_ROOT` (socket bound / WAL locked), proving isolation; **(b)** the red fixture's `failing_detail` **contains the failing test name** and a `FAILED`/`panicked at`/`failures:` marker — asserted **not** to be a truncated `Drop t…` fragment; **(c)** the green fixture **still passes when the daemon env has `CARGO_HOME` / `RUSTUP_HOME` unset**, proving the toolchain pin resolves them from the real `HOME` rather than the empty temp `$HOME`. |

The fixture crate is a tiny standalone crate invoked directly, so the gate test
does **not** trigger a recursive full-suite `cargo test`.

## Compatibility

- **No public API change.** `run_unit_test_gate` is private; the three new helpers
  (`unit_test_isolation_dir`, `resolve_toolchain_home`, `extract_failure_detail`)
  are private. Public types and the gate sequence are unchanged.
- **Smallest surface.** Only
  [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
  is edited. `canary.rs`, `types.rs`, and `overseer/deploy.rs` are untouched.
- **Diagnostics reused.** `CanaryResult.failing_gate` / `failing_detail`,
  `refusal_reason`, and the `overseer::deploy` WARN
  ([#4420](./overseer-deploy-canary-diagnostics.md)) are reused, not
  reimplemented — the WARN simply now carries a named test instead of a spinner
  fragment.
- **No `print`-family macros; no silent fallbacks.** All emission is `tracing`
  structured key=value.
- **No `Bridge` naming.** New identifiers follow the
  [no-Bridge-naming guard](./no-bridge-naming-guard.md).

## See also

- [How to diagnose a red-canary unit-test gate](../howto/diagnose-a-red-canary-unit-test-gate.md) —
  the operator runbook for reading the named failure and confirming isolation.
- [Canary gate isolation and self-deploy convergence](./canary-gate-convergence.md) —
  the #4440 `scrub_gate_env` / `canary_env` repair this builds on.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  the `failing_gate` / `failing_detail` / `refusal_reason` surface.
- [State-root resolution](./state-root-resolution.md) — how
  `SIMARD_STATE_ROOT` / `$HOME/.simard` selects the WAL / cognitive-store /
  socket paths the isolation override redirects.
- [String truncation helpers](./string-truncation-helpers.md) — the
  char-boundary-safe `truncate_output` used for the 4096B and 512B bounds.
- [Self-deploy API reference](./self-deploy-api.md) — the guarded deploy path
  the green canary now unblocks.
