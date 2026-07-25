---
title: Deterministic self-deploy canary — unit-test gate diagnostics and deterministic test invariants
description: Reference for the diagnosable RelaunchGate::UnitTest gate (full stdout+stderr capture on failure, sanitized bounded failing-test-name error-level tracing event on self_relaunch::gate, fail-closed preserved) and the deterministic canary test invariants that replaced load-induced flakes — the dispatch peak-concurrency gauge, the interruptible_sleep injectable-clock seam, per-test install isolation, and cost-ledger writer durability.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./overseer-deploy-canary-diagnostics.md
  - ./canary-gate-convergence.md
  - ./self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../safe-self-update.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/ooda_actions/tests_dispatch_concurrency.rs
  - ../../src/operator_commands_ooda/daemon/helpers.rs
  - ../../src/operator_commands_ooda/tests/daemon_inline.rs
  - ../../src/install/entrypoint.rs
  - ../../src/install/paths.rs
  - ../../src/cost_tracking.rs
---

# Deterministic self-deploy canary — unit-test gate diagnostics and deterministic test invariants

> **Status: implemented.** The diagnosable `run_unit_test_gate` lives in
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs).
> The deterministic test invariants live in
> [`src/ooda_actions/tests_dispatch_concurrency.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/tests_dispatch_concurrency.rs),
> [`src/operator_commands_ooda/daemon/helpers.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/helpers.rs),
> [`src/operator_commands_ooda/tests/daemon_inline.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/tests/daemon_inline.rs),
> the install path helpers, and
> [`src/cost_tracking.rs`](https://github.com/rysweet/Simard/blob/main/src/cost_tracking.rs).
> Every change is **deterministic**: no timing bound was widened, no flaky test
> was suppressed, and the gate remains **fail-closed**.

## Why this exists

Simard's OODA daemon self-deploys by building a candidate binary and running a
canary of [`default_gates()`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
before it swaps in the new image. One of those gates, `RelaunchGate::UnitTest`,
shells the **full** `cargo test` suite against the candidate with a scrubbed
environment and a separate `--target-dir`.

On the production host (64 cores, load 60–150, heavily oversubscribed by many
concurrent engineer/overseer recipes) that gate went **false-red**: a handful of
tests carried timing assumptions that only hold on an idle machine, so under
contention they failed or timed out and the canary refused a perfectly correct
deploy every cycle. Self-deploy stayed frozen at `v0.36.0` for 30+ hours while
CI (`.github/workflows/verify.yml`) ran the identical suite **green** on clean
runners.

Worse, the refusal was undiagnosable: `run_unit_test_gate` truncated **only
`stderr`** to 200 characters and discarded `stdout` entirely — but libtest
prints the failing-test name and the `has been running for over 60 seconds`
banners to **stdout**. Every refusal produced the same opaque
`tests failed (exit 101): …` tail with no failing-test name.

This feature does two things, both durable:

1. **Makes the gate diagnosable** — on failure it captures the *full* stdout and
   stderr, parses the failing test name(s), and emits a sanitized, bounded,
   error-level `tracing` event on the existing `self_relaunch::gate` target. The
   gate is unchanged on the happy path and **still fails closed** on any failure.
2. **Removes the load-induced flakes at the root** — each reddening test is
   rewritten to assert its real invariant *deterministically* (a concurrency
   counter, an interrupt-causality assertion, per-test filesystem isolation, and
   a durable ledger writer) instead of by observing wall-clock time. No bound was
   loosened; the tests now pass identically on an idle laptop and on a host at
   load 150.

> **Explicitly rejected.** This feature does **not** adopt PR #4566's approach.
> It does **not** widen any `< 1s` deadline to `< 5s`, does **not** weaken the
> `parallel * 2 <= serial` speedup relationship, and does **not** remove
> `UnitTest` from `default_gates()`. Any change that only passes by loosening a
> timing bound or hiding a gate is out of scope.

## Part 1 — the diagnosable `run_unit_test_gate`

### Behavior

`run_unit_test_gate(config)` runs `cargo test` against the candidate with the
scrubbed env and canary `--target-dir` exactly as before. The **only** behavioral
change is on the failure branch.

| Path | Old behavior | New behavior |
| --- | --- | --- |
| All tests pass | `GateResult { passed: true, detail: "all tests passed" }` | **unchanged** |
| Tests fail (`exit != 0`) | `detail` = `tests failed (exit N): <stderr truncated to 200 chars>` — stdout discarded | `detail` = failing-test summary built from **full stdout + stderr**; full capture emitted as an `error`-level `tracing` event on `self_relaunch::gate`. `passed: false` **preserved.** |
| `cargo test` fails to spawn | `detail` = `cargo test failed to run: <e>` | **unchanged** (still fail-closed) |

The gate **never** returns `passed: true` on any non-success path. Emitting the
diagnostic is best-effort: if the `tracing` emit itself fails, the `GateResult`
is returned regardless — a logging fault must never change the deploy verdict.

### The failing-test-name parser

On failure the captured stdout+stderr is scanned for libtest's failure markers to
produce a compact, human-readable summary:

- Lines of the form `test <path::to::test> ... FAILED`
- The `failures:` block listing each failed test path
- The `test <path> has been running for over 60 seconds` slow-test banner
  (surfaced separately so a *timeout* red is distinguishable from an *assertion*
  red)
- The trailing `test result: FAILED. N passed; M failed; …` summary line

The parser is **anchored and linear-time** (a single forward line-scan with a
bounded per-line match, no backtracking) and caps the number of scanned lines,
so it cannot be driven into pathological runtime (ReDoS-safe) by adversarial or
enormous test output.

Example `detail` produced for a real red:

```
tests failed (exit 101): 2 failed —
  cost_tracking::tests::concurrent_appends_never_drop_entries,
  operator_commands_ooda::daemon::helpers::tests::interruptible_sleep_exits_on_mid_sleep_shutdown
  (see the `self_relaunch::gate` log for full output)
```

### Diagnostic event (`self_relaunch::gate` target)

The full captured output is emitted as an **`error`-level `tracing` event on the
existing [`self_relaunch::gate`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
target** — the same target the per-gate span and the `"canary gate evaluated"`
event already use. It therefore lands in the Overseer's existing log stream (and,
under systemd, journald) with **no new sink** and no new plumbing: the gate
functions take only `&RelaunchConfig`, which has no `state_root`, so a `tracing`
event — not the `daemon_log`/`ooda.log` file — is the correct, already-available
channel. The entry is:

- **Credential-redacted** — it reuses the existing `bound_gate_detail`
  redaction seam (`redact_credentials`, SEC-D2) so a token-bearing remote URL in
  the captured output is scrubbed before it is emitted, exactly as the current
  gate `detail` already is.
- **Sanitized** — control characters and ANSI/terminal escape sequences are
  stripped and the bytes are decoded lossily as UTF-8, so a crafted test name or
  panic message cannot inject log lines or spoof a terminal via escape codes.
- **Parameterized** — the captured output is written as a structured field
  (`captured = %…`), never interpolated into the format string, so it cannot be
  used for log-format injection.
- **Bounded** — the body is capped generously (large enough to hold a complete
  libtest failure block, but bounded to prevent a runaway test from exhausting
  disk). The bound never truncates below a full failure block, so the failing
  name and its assertion are always retained.

Reading a refusal after this change (same invocation the convergence how-to
uses, so both the short `detail` and the full capture surface on one target):

```bash
# The canary gate's own detail (short, names the failing tests):
journalctl --user -u simard -o cat | grep 'self_relaunch::gate' | tail -n 20

# The full captured stdout+stderr block for the same refusal:
journalctl --user -u simard -o cat | grep -A200 'self_relaunch::gate'
```

### Fail-closed guarantee

`run_unit_test_gate` continues to satisfy the canary's core security property: no
non-success path can yield `passed: true`, and `UnitTest` remains in
`default_gates()`. The diagnostics are strictly **additive read-only telemetry**.
This composes with the Overseer-level
[`CanaryResult.failing_gate` / `failing_detail`](./overseer-deploy-canary-diagnostics.md)
enrichment: the gate now supplies a *meaningful* `detail` (the failing test
names) for the Overseer to thread up to the tick WARN and OTel attributes.

## Part 2 — deterministic test invariants

Each of the following tests reddened the canary under load. Each is rewritten to
assert its true invariant deterministically. None loosens a bound.

### C2 — dispatch concurrency: peak-concurrency gauge (no wall clock)

`src/ooda_actions/tests_dispatch_concurrency.rs` proves the `AdvanceGoal`
dispatch actually runs actions in parallel and never exceeds the AIMD
`max_concurrency` cap.

The parallelism is asserted by an **atomic in-flight gauge**: each fake
`run_turn` increments a `live` counter, records `peak = max(peak, live)`, then
decrements. The invariant is checked directly against the observed peak:

- With `cap = N` and ≥ N ready actions, the observed **`peak >= 2`** (real
  overlap occurred) and **`peak <= N`** (the cap was never exceeded).
- With `cap = 1`, the observed **`peak <= 1`** (dispatch serialized).

The old wall-clock ratio assertion (`parallel_elapsed * 2 <= serial_elapsed`) and
its `Instant` bindings are **removed**. The gauge is a strictly stronger, host-load-
independent statement of the same guarantee: it asserts the parallelism *directly*
rather than inferring it from elapsed time. The gauge itself is **not** weakened —
it still requires genuine overlap (`peak >= 2`) and still enforces the cap.

### C3 — `interruptible_sleep`: injectable-clock seam

`interruptible_sleep(total, shutdown)` (in
`src/operator_commands_ooda/daemon/helpers.rs`) sleeps in short ticks and wakes
early when `shutdown` is set. Its public signature is **unchanged**.

A private seam is introduced:

```rust
/// Sleep in `tick`-sized chunks until `total` elapses or `shutdown` is set,
/// driving each chunk through the injected `sleep_fn`. The public
/// `interruptible_sleep` delegates here with the real `thread::sleep`.
fn interruptible_sleep_with(
    total: Duration,
    shutdown: &AtomicBool,
    mut sleep_fn: impl FnMut(Duration),
) {
    // ... existing tick loop, calling sleep_fn(chunk) instead of thread::sleep ...
}

pub fn interruptible_sleep(total: Duration, shutdown: &AtomicBool) {
    interruptible_sleep_with(total, shutdown, |d| std::thread::sleep(d));
}
```

Tests inject a **fake sleeper** that records each requested chunk and can flip
`shutdown` after a chosen number of chunks — with **zero real sleeping** and
**zero `Instant::elapsed()`**. They assert causality and ordering deterministically:

- `Duration::ZERO` → the sleeper is never called; returns immediately.
- Already-shutdown → the sleeper is never called; the loop exits on the first
  check.
- Mid-sleep shutdown → the sleeper is invoked exactly up to the chunk on which
  the fake flips `shutdown`, then the loop exits; the recorded chunk count proves
  the wake happened *before* `total` elapsed.
- Full duration → the summed recorded chunks equal `total` and the final chunk is
  `total.min(tick)`-bounded (the tick-clamping invariant).

The old wall-clock deadline assertions (`start.elapsed() < Duration::from_secs(1)`
etc.) are **removed**, not widened.

### C4 — duplicate `interruptible_sleep` test in `daemon_inline.rs`

`src/operator_commands_ooda/tests/daemon_inline.rs` carried a parallel copy of the
wall-clock deadline test (`interruptible_sleep_very_short_duration`, which asserted
`start.elapsed() < Duration::from_secs(1)`). It is restructured onto the **same seam**
and the same deterministic causality assertions, so there is a single deterministic
source of truth for the interrupt contract and no remaining `elapsed()` bound.

### C5 — install isolation: deterministic ETXTBSY-retry (no global `serial`)

The install tests (`src/install/entrypoint.rs`, `src/install/paths.rs`) previously
serialized with `#[serial(install)]`. Ground-truthing under parallel load (all
`install::` tests at `--test-threads=14` under 64-way CPU load) proved the
serialization was **load-bearing, not a pure band-aid**:
`reconcile_replaces_ours_marker_at_entrypoint` flipped at ~1/150 runs. Classifying
an on-disk candidate means `exec`ing it — `version_banner_is_ours` runs
`<path> --version` — and a sibling test's `fork` transiently inherits a write fd to
that file across the `exec`, so the kernel returns **`ETXTBSY`** ("text file busy")
and the candidate mis-classifies as `Foreign`. Temp-rooting cannot close this
window: the shared resource is the process fork/exec fd table, not the filesystem.

The deterministic fix removes the race at its **root** rather than reserializing
around it. `version_banner_is_ours` retries the classification `exec` on the
transient `ETXTBSY` (bounded to 8 attempts, errno-only via
`raw_os_error() == Some(libc::ETXTBSY)`, mirroring the established
`retry_on_etxtbsy` pattern from Fix #4523). With the transient converted into the
correct verdict, `#[serial(install)]` and its `serial_isolation_guard` meta-test
are **removed**, and all `install::` tests run in parallel — **200/200 green under
load** (vs. the pre-fix reproduction that reddened at iteration 47). `reconcile`
still touches no process-global (`std::env`, `current_exe`, `$HOME`), so per-test
`TempDir` roots remain the filesystem-isolation contract.

### C6 — cost-ledger writer durability

`src/cost_tracking.rs` appends one JSON-lines record per cost event. The writer
already serializes in-process writers behind `LEDGER_WRITE_LOCK` and emits each
record with a single buffered `write_all` + `flush` (so `O_APPEND` keeps a lone
`write()` atomic at EOF and two writers cannot splice a torn line).

This is a **reproduction-gated** component. The ground-truth parallel-under-load
run is the source of truth:

- **If the ground-truth run shows zero dropped/torn entries**, the writer is
  already durable and is left unchanged; the green evidence is recorded in the PR
  body. The completeness assertion (`THREADS * PER_THREAD` entries all present and
  parseable) is kept as the sole invariant.
- **If the run shows real drops**, the fix targets the **writer** durability only
  (atomic append ordering / `sync_all`), never the test. The completeness
  assertion is **never** relaxed.

Under no circumstance is the flake "fixed" by loosening the completeness
assertion — a dropped cost entry is real data loss.

## Reproducing the canary red under load

The invariants are verified the same way the redders were ground-truthed:
compile the test binary into a separate target dir, then run several copies in
parallel while the host is under real load, capturing full per-run output.

```bash
# 1. Build the test binaries without running them, into an isolated target dir.
cargo test --all-features --locked --no-run --target-dir /tmp/canary-tt

# 2. Run 6–8 parallel copies of an affected test under load, full output per run.
for i in $(seq 1 8); do
  cargo test --all-features --locked --target-dir /tmp/canary-tt \
    interruptible_sleep -- --nocapture > /tmp/canary-run-$i.log 2>&1 &
done
wait
grep -l 'FAILED\|has been running for over' /tmp/canary-run-*.log || echo "all green"
```

A deterministic test passes **every** run regardless of host load. The
acceptance bar for this feature is: each affected test run 6–8× in parallel under
load yields **zero** failures, and the full suite passes:

```bash
cargo test --all-features --locked --no-fail-fast
```

which matches the CI gate in `.github/workflows/verify.yml`.

## Configuration

No new configuration is introduced. The behavior is governed by the existing
self-deploy surface:

| Field / knob | Where | Effect |
| --- | --- | --- |
| `default_gates()` | `src/self_relaunch/gates.rs` | Still includes `RelaunchGate::UnitTest`. Not configurable off by this feature. |
| `config.canary_target_dir` | `RelaunchConfig` | Isolated `--target-dir` for the canary `cargo test`. Unchanged. |
| `CARGO_BUILD_JOBS` | set by the gate | Candidate build/test parallelism. Unchanged. |
| `self_relaunch::gate` tracing target | existing Overseer log stream | Destination for the new sanitized, bounded, credential-redacted failure capture (`error` level). No new sink. |

## Source layout

```
src/self_relaunch/gates.rs
    run_unit_test_gate            # full stdout+stderr capture on failure,
                                  # failing-test-name parse, sanitized/bounded/
                                  # credential-redacted error-level tracing event
                                  # on self_relaunch::gate, fail-closed preserved
    default_gates                 # UnitTest still present (unchanged)

src/ooda_actions/tests_dispatch_concurrency.rs
                                  # peak-concurrency gauge is the sole parallelism
                                  # invariant; wall-clock ratio assertion removed

src/operator_commands_ooda/daemon/helpers.rs
    interruptible_sleep_with      # private injectable-clock seam
    interruptible_sleep           # unchanged public signature, delegates to seam
    #[cfg(test)] mod tests        # deterministic fake-sleeper causality tests

src/operator_commands_ooda/tests/daemon_inline.rs
                                  # duplicate interruptible_sleep tests moved onto
                                  # the same seam; elapsed() bounds removed

src/install/entrypoint.rs         # version_banner_is_ours: bounded ETXTBSY-retry
src/install/paths.rs              #   fixes the fork/exec classification flip;
                                  # #[serial(install)] + serial_isolation_guard removed

src/cost_tracking.rs
    append_line                   # LEDGER_WRITE_LOCK + single buffered write_all
                                  # + flush (durability fix only if repro shows drops)
```

## Security considerations

- **Fail-closed (dominant).** No non-success path in `run_unit_test_gate` yields
  `passed: true`; `UnitTest` stays in `default_gates()`. Diagnostics are
  additive and read-only.
- **Log-injection / terminal-escape spoofing.** Captured test output is treated
  as untrusted data: control and ANSI escape sequences are stripped, bytes are
  decoded lossily as UTF-8, and the output is emitted as a parameterized
  `tracing` field, never as a format string. It also passes through the existing
  `bound_gate_detail` credential redaction (SEC-D2) so token-bearing URLs are
  scrubbed.
- **ReDoS.** The failing-test-name parser is anchored, linear-time, and
  line-count-capped so hostile or huge output cannot stall the gate path.
- **Disk exhaustion.** The event body is bounded (generously, never below a
  full failure block) so a runaway test cannot fill the log through the gate.
- **Availability over logging.** A `tracing`/logging fault never changes the gate
  verdict — the `GateResult` is returned regardless.
- **Filesystem isolation.** `reconcile` touches no process-global, so per-test
  `TempDir` roots keep the filesystem hermetic. The removed `#[serial(install)]`
  guarded a genuine fork/exec `ETXTBSY` race (a sibling `fork` inheriting a write
  fd across the classification `exec`), now fixed at the root by an errno-scoped,
  bounded retry — no timing widening, no serialization.
- **Supply chain.** No new crates; the `--locked` surface is unchanged.
