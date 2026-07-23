---
title: Diagnosable canary unit-test gate
description: Reference for the root-cause repair (#4522) that ends the self-deploy red-canary crash-loop on the relaunch unit-test gate — the `cargo test --lib` scope alignment that matches the proven-green baseline, the stdout+stderr capture with `parse_unit_test_failure` that surfaces the failing test name and `test result: FAILED` line, the `truncate_output_tail` bounded tail helper, and the structured `tracing` emission that keeps a genuine red loud while making it diagnosable. All additive and non-breaking.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./canary-gate-convergence.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-source-prep.md
  - ./self-deploy-api.md
  - ./overseer-tick-self-healing.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../safe-self-update.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
---

# Diagnosable canary unit-test gate

> **Status: implemented.** The `run_unit_test_gate` scope alignment
> (`cargo test --lib`), the `parse_unit_test_failure` and `truncate_output_tail`
> helpers, and the structured `tracing` emission on the gate failure path live in
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs).
> The change is **additive and non-breaking**: `verify_canary`,
> `all_gates_passed`, `default_gates`, `RelaunchGate`, `GateResult`, and
> `RelaunchConfig` keep their signatures; the new `parse_unit_test_failure` and
> `truncate_output_tail` helpers are private; the existing head-truncating
> `truncate_output` is retained unchanged for inline cases.

## Why this exists

This repair sits on top of the [canary gate isolation and convergence](./canary-gate-convergence.md)
work (#4440): that change scrubbed the gate environment (`scrub_gate_env` +
`canary_gate_env_allowlist`) and added per-gate `tracing` spans. This feature
(#4522) fixes the **`unit-test` gate specifically**, which was still reddening
the canary on **every** Overseer tick and driving a monotonic `DeployDrift`
crash-loop — the signature `red canary (one or more gates failed) (isolated)`
recurred 31× over ~6h (ticks 11:13 → 17:52), so no merged improvement could
deploy while the running binary fell 1 → 2 → 3 commits behind merged `main`.

The reddening was **not a genuine regression**. `cargo test --lib` was fully
green in a normal environment (9262 passed, 0 failed, ~191s). Two coupled
defects made the gate redden and, worse, made the loop **undiagnosable**:

1. **Scope mismatch.** The gate ran `cargo test` over **all targets**, not just
   the library. That pulls the integration binaries under `tests/` (20+ of them
   require `SIMARD_*` env and other fixtures) into the scrubbed canary
   subprocess, where they abort with exit status `101` **before** any
   `test result:` summary line is ever printed. The proven-green baseline that
   the self-deploy candidate must match is the **library** scope (`--lib`).
2. **Discarded stdout.** On failure the gate truncated only `stderr` to 200
   bytes (`truncate_output(&stderr, 200)`) and **discarded stdout entirely**.
   `cargo test` prints the failing test identity — the `test <name> ... FAILED`
   lines and the `test result: FAILED` summary — on **stdout**. Discarding it
   left the `GateResult.detail` as a generic "tests failed (exit 101)" with a
   truncated stderr tail that named no test, making the recurring red canary
   impossible to diagnose from telemetry alone.

This feature aligns the gate scope to the proven-green baseline **and** makes a
genuine failure name itself. It does **not** weaken, skip, or disable the gate,
and it does **not** relax the [scrubbed-env deny-by-default defense](./canary-gate-convergence.md#scrub_gate_env-gate-subprocess-env-discipline):
`LD_PRELOAD`/hijack-class variables remain non-allow-listable. An unhealthy
candidate still reddens — now loudly and diagnosably.

## What changed

1. **Scope aligned to the proven-green baseline.** `run_unit_test_gate` runs
   `cargo test --lib`, matching the exact scope that is green in a normal
   environment (9262/0). This is the dominant defect fix: the all-targets scope
   dragged `SIMARD_*`-dependent integration binaries into the scrubbed
   subprocess, aborting with exit `101` before any summary line.
2. **Both streams captured, failing test surfaced.** On failure the gate now
   captures **stdout and stderr**, and `parse_unit_test_failure` extracts the
   high-signal markers — the failing test name(s), the `test result: FAILED`
   line, and abort/compile signals for the no-summary exit-`101` case — then
   appends a bounded tail so `cargo`'s last words always survive.
3. **Bounded, redacted, structured emission.** The enriched detail is emitted
   through the existing per-gate `tracing` span via `bound_gate_detail`
   (credential-redacted, length-bounded); a spawn error emits `tracing::error!`.
   There are **no** `print!` / `println!` / `eprintln!` sinks on the gate path.

## Behavior

### Gate scope: `cargo test --lib`

`run_unit_test_gate` builds a scrubbed `cargo` command (via
[`scrubbed_command`](./canary-gate-convergence.md#scrub_gate_env-gate-subprocess-env-discipline),
so `env_clear()` + base floor + `canary_env` allow-list still applies) and
invokes the **library** test scope only:

```rust
fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    let mut cmd = scrubbed_command("cargo", config);
    cmd.arg("test")
        .arg("--lib")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs());
    match cmd.output() {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: true,
            detail: "all tests passed".to_string(),
        },
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = parse_unit_test_failure(&stdout, &stderr);
            tracing::warn!(
                target: "self_relaunch::gate",
                gate = %RelaunchGate::UnitTest,
                exit = %output.status,
                detail = %bound_gate_detail(&detail),
                "unit-test gate reddened"
            );
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("tests failed (exit {}): {}", output.status, detail),
            }
        }
        Err(e) => {
            tracing::error!(
                target: "self_relaunch::gate",
                gate = %RelaunchGate::UnitTest,
                error = %e,
                "unit-test gate failed to spawn cargo"
            );
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("cargo test failed to run: {e}"),
            }
        }
    }
}
```

Why `--lib` (and not all targets):

- **Matches the proven-green baseline 1:1.** The self-deploy candidate must be
  verified against the same scope that is green in a normal environment. The
  library scope is that baseline; the integration scope is not, because those
  binaries need fixtures the scrubbed canary deliberately withholds.
- **The gate-set invariant only mandates the *presence* of a blocking
  unit-test gate.** The `default_gates_returns_four_in_order` and
  `default_gates_has_all_four` invariants require the `UnitTest` gate to be
  present at index 1 of the fixed four-gate `default_gates()` sequence; they do
  **not** constrain its *scope*, and `--lib` **is** the unit scope. Integration
  coverage belongs to CI, which runs the full-env suite.
- **Additive and non-breaking.** A single `--arg("--lib")` on the existing
  command; no gate order, signature, or default changed.

### `parse_unit_test_failure` — surface the failing test

`parse_unit_test_failure(stdout, stderr) -> String` is a pure helper that scans
the combined captured output for high-signal markers, deduplicates them, caps
the marker set, and then appends a bounded tail of the combined output so the
`cargo` summary (which is printed **last**) always survives truncation.

```rust
/// Extract the diagnosable identity of a `cargo test --lib` failure from the
/// captured stdout+stderr. `cargo` prints the failing test name(s) and the
/// `test result: FAILED` summary on STDOUT; the exit-101 "aborted before any
/// summary" case (a compile error, a panic-abort, or an integration binary
/// dragged in by a scope mismatch) leaves signal on STDERR. This surfaces both,
/// then a bounded tail so `cargo`'s last words are never lost to truncation.
fn parse_unit_test_failure(stdout: &str, stderr: &str) -> String { /* ... */ }
```

Markers surfaced, in priority order:

| Marker | Example line | Why it matters |
| --- | --- | --- |
| Failing test name | `test my_module::my_failing_case ... FAILED` | Names the exact test — the identity that was previously invisible. |
| `failures:` block names | `    my_module::my_failing_case` | The consolidated failing-test list `cargo` prints before the summary. |
| Test summary | `test result: FAILED. 9261 passed; 1 failed; ...` | Confirms a genuine test failure vs. an abort. |
| Abort / compile signal | `error[E0433]: ...`, `error: could not compile ...`, `thread '...' panicked at ...` | Covers the exit-`101` **no-summary** case, so a scope/env abort is still explained. |

The extracted markers are deduplicated and capped (≈20 lines) so a large
`failures:` block cannot dominate the detail, then a bounded tail of the
combined output is appended via `truncate_output_tail`. If **no** marker is
found (an unusual abort shape), the bounded tail alone is returned — the gate
still fails loudly with `cargo`'s own final output rather than an empty detail.

**Markers are emitted first, by design — that is the end-to-end guarantee.**
The high-signal markers (failing test name, `test result: FAILED`, abort/compile
signal) are placed at the **head** of the returned detail, ahead of the appended
raw tail. This matters because every downstream consumer re-bounds the detail
with a **head** truncation to 512 bytes: `bound_gate_detail` at `tracing`
emission, and [`CanaryResult`'s `DETAIL_CAP`](./overseer-deploy-canary-diagnostics.md)
(512 bytes, redact-then-bound at population) for the persisted `failing_detail`.
Head-positioned markers therefore survive that 512-byte bound, so the failing
test identity reaches the operator notification, the OTel span, and persisted
deploy state — not just the in-process `GateResult`. The appended raw tail is
**best-effort** context that survives end-to-end only when the marker set leaves
room under 512 bytes; for the rare no-marker abort it is the head of that tail
that is retained downstream. The extracted markers, not the tail helper, are
what guarantee the verdict survives.

### `truncate_output_tail` — keep the summary

```rust
/// Char-boundary-safe TAIL truncation: keeps the LAST `max_len` bytes of `s`
/// and prefixes `...`. Companion to `truncate_output` (which keeps the HEAD).
/// Used for `cargo test` output because the `test result:` summary lives at the
/// tail — head truncation would discard exactly the line that names the verdict.
fn truncate_output_tail(s: &str, max_len: usize) -> String { /* ... */ }
```

`truncate_output_tail` bounds the appended tail to **8 KiB**. This is distinct
from the existing `truncate_output` (retained, unchanged), which keeps the
**head** and is used for the short inline gate details. The tail variant exists
solely because `cargo`'s summary is emitted last; head truncation on a long
run would keep the build noise and drop the verdict — so the tail is captured
into the returned `GateResult.detail` **before** `parse_unit_test_failure`'s
head-positioned markers are prepended (see above). The returned
`GateResult.detail` is thus transiently up to ~8 KiB, but it is never persisted
or emitted at that size: `bound_gate_detail` (512-byte head bound) gates the
`tracing` path and `CanaryResult::DETAIL_CAP` (512-byte head bound) gates the
persisted `failing_detail`, so there is **no per-tick state or telemetry bloat**
from the richer detail.

> **Bounded, never the environment.** Only `cargo`'s own captured stdout/stderr
> is surfaced — never the process environment, never env **values**. The 8 KiB
> tail cap is a DoS guard against a pathological multi-megabyte test log. The
> detail is additionally routed through `bound_gate_detail`
> ([`redact_credentials`](./self-deploy-source-prep.md) + a 512-byte bound)
> before it reaches a `tracing` / OTel span attribute, so a token-bearing URL in
> test output is redacted and the span attribute stays small.

### Emission: structured `tracing` only

The failure path emits a `tracing::warn!` (target `self_relaunch::gate`) with
structured `gate` / `exit` / `detail` fields; a `cargo` spawn error emits
`tracing::error!` with the `error` field. Fields are structured key=value, not
a format-string interpolation of raw output, for log-injection resistance. This
`warn!` is a deliberate **severity elevation** on top of the `tracing::info!`
"canary gate evaluated" event `verify_canary` already emits for every gate
(with the same `bound_gate_detail`): a reddening `unit-test` gate therefore
surfaces at `warn`, carries the explicit `exit` status, and is still observable
on the aggregate per-gate span `verify_canary` opens. The two events carry the
same bounded detail by design — the `info` event records the verdict uniformly,
the `warn` event raises the reddening gate to alertable severity. No
`print`-family macro is used anywhere on the gate path, consistent with the
[no-silent-fallback](./overseer-tick-self-healing.md) posture.

## Security invariants (preserved)

This repair is orthogonal to the scrubbed-env defense and does not touch it:

- **Deny-by-default env unchanged.** `scrubbed_command` / `scrub_gate_env` /
  `canary_gate_env_allowlist` are untouched. `env_clear()` + base floor +
  `SIMARD_HOME` / `SIMARD_STATE_ROOT` / `SIMARD_PROMPT_ASSETS_DIR` allow-list
  still applies to the `cargo` subprocess.
- **Hijack-class never re-admitted.** `LD_PRELOAD`, `LD_LIBRARY_PATH`,
  `LD_AUDIT`, `DYLD_*`, `GIT_SSH_COMMAND`, and the rest of `is_hijack_class_env`
  remain non-allow-listable. The scope fix does not widen the allow-list.
- **No shell interpolation.** `--lib`, `--manifest-path`, `--target-dir`, and
  their values are passed as discrete `Command::arg()` values — never through
  `sh -c`.
- **No silent fallback.** A genuine red canary still fails loudly; the richer
  detail explains **why** without masking a real failure as green.

## Regression tests

The change ships tests proving both a healthy pass and a diagnosable red, plus
the retained security assertions:

| Test | Asserts |
| --- | --- |
| `unit_test_gate_passes_for_healthy_candidate` | A hermetic temp fixture crate (minimal `Cargo.toml` + one passing `#[test]`) run through the gate under `--lib` returns `passed == true`. Fails loudly if the toolchain is missing — **no silent skip**. |
| `unit_test_failure_surfaces_failing_test_name` | Canned `cargo` stdout/stderr containing `test my_failing_case ... FAILED` + `test result: FAILED` fed to `parse_unit_test_failure` yields a detail containing **both** `my_failing_case` and `test result: FAILED`. No `cargo` invocation. |
| `unit_test_abort_without_summary_surfaces_tail` | The exit-`101` **no-summary** shape (a compile `error[...]` / panic with no `test result:` line) still yields a non-empty detail carrying `cargo`'s tail — the previously-undiagnosable case. |
| `truncate_output_tail_keeps_summary` | `truncate_output_tail` keeps the **last** `max_len` bytes and prefixes `...`, so a trailing `test result:` line survives while a long head is dropped. Char-boundary-safe on multi-byte input. |
| `canary_gate_env_allowlist_carries_deploy_shape_names_not_hijack_vars` (retained) | The allow-list includes the `SIMARD_*` deploy-shape names and excludes `LD_PRELOAD`/hijack-class — the scrub defense is not weakened by the scope fix. |

The failing-name and no-summary tests are pure (canned input, no `cargo`), so
they are fast and hermetic; the healthy-pass test spawns a **tiny** `--lib`-only
fixture crate and fails loudly if the toolchain is unavailable.

## Compatibility

- **Additive only.** `parse_unit_test_failure` and `truncate_output_tail` are
  new private helpers; `truncate_output` is retained unchanged. No public
  signature changed: `verify_canary`, `all_gates_passed`, `default_gates`,
  `RelaunchGate`, `GateResult`, `RelaunchConfig` are all as before.
- **Gate order and gate-set invariant preserved.** `Smoke → UnitTest →
  GymBaseline → RpcHealth`, no short-circuit; the blocking unit-test gate stays
  in `default_gates()`, so `default_gates_returns_four_in_order` and
  `default_gates_has_all_four` still hold.
- **Scope narrowed intentionally.** `--lib` narrows coverage vs. all-targets;
  this is the fix, not a regression — the integration suite remains CI's job and
  the gate-set invariant only requires the unit-test gate's *presence* in the
  four-gate sequence, not its scope.
- **No new operator inputs.** No CLI flags, RPC, config keys, or "skip gate"
  controls; the trust boundary is unchanged.
- **CI-green, merge-ready.** All new tests pass under `cargo test --lib`.

## See also

- [Canary gate isolation and self-deploy convergence](./canary-gate-convergence.md) —
  the #4440 scrubbed-env + per-gate-span work this repair builds on
  (`scrub_gate_env`, `canary_gate_env_allowlist`, `bound_gate_detail`).
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  the #4420 `failing_gate` / `failing_detail` / `refusal_reason` observability
  that names *which* gate reddens; this feature makes the `unit-test` gate's
  *detail* name the failing test.
- [How to converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md) —
  the operator runbook; the `unit-test` gate detail now names the failing test.
- [Self-deploy source preparation](./self-deploy-source-prep.md) — the
  `redact_credentials` scrubber `bound_gate_detail` reuses, and `scrub_git_env`,
  the model the gate env-scrub mirrors.
- [Overseer tick self-healing](./overseer-tick-self-healing.md) — the
  `is_transient` fail-closed classifier: a `target_canary` failure is never a
  transient blip, so a genuine red is not retried away.
