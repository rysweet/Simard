---
title: Hermetic dashboard activity tests
description: >
  How the dashboard activity handlers (traces, activity) are tested
  without touching the operator's live state — tempdir isolation,
  RAII env-var guards, XDG_DATA_HOME redirection, and the
  dashboard_state serial group.
last_updated: 2026-06-10
review_schedule: when dashboard activity handlers or daemon_health paths change
owner: simard
doc_type: reference
related:
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
  - ../reference/state-root-resolution.md
  - ../reference/dashboard-e2e-tests.md
  - ../dashboard.md
---

# Hermetic dashboard activity tests

The dashboard activity module (`src/operator_commands_dashboard/activity.rs`)
exposes two Axum handlers — `traces()` and `activity()` — that read
operator state from disk. Before hermetisation, tests for these handlers
wrote to the operator's live paths (`~/.simard/costs/ledger.jsonl` and
`<XDG_DATA_HOME>/simard/daemon_health.json`), which meant:

- A `cargo test` run could overwrite real dashboard data.
- CI runs contaminated host state.
- A panicking assertion left the live file modified (no cleanup on
  unwind).
- Concurrent `cargo test` invocations raced on the same global paths.

The test module `tests_activity.rs` fixes all four problems using the
patterns documented on this page.

---

## Architecture

The activity module reads two distinct file paths at runtime:

| Handler | File | Resolution mechanism |
|---------|------|---------------------|
| `traces()` | Cost ledger | `resolve_state_root().join("costs/ledger.jsonl")` |
| `activity()` | Daemon health | `dirs::data_local_dir().join("simard/daemon_health.json")` |

The two paths use different resolution mechanisms, so the test
isolation strategy differs for each:

- **Cost ledger** routes through `resolve_state_root()`, which reads
  `$SIMARD_STATE_ROOT`. Setting that env var to a tempdir is sufficient.
  `HermeticState::new()` handles this automatically.
- **Daemon health** routes through `dirs::data_local_dir()`, which reads
  `$XDG_DATA_HOME` on Linux. Tests redirect this to a tempdir via an
  `EnvGuard` on `XDG_DATA_HOME`.

---

## `HermeticState` for the cost ledger path

The `traces()` handler resolves the ledger path via
`resolve_state_root().join("costs/ledger.jsonl")`. This is the same
state-root helper documented in
[State-root resolution](../reference/state-root-resolution.md), so
`HermeticState::new()` pins `SIMARD_STATE_ROOT` to a tempdir and the
ledger path follows automatically:

```rust
use simard::test_support::HermeticState;

#[test]
#[serial_test::serial(dashboard_state)]
fn traces_reads_cost_ledger_when_present() {
    let state = HermeticState::new();

    // Write test fixture into the hermetic state root
    let costs_dir = state.state_root().join("costs");
    std::fs::create_dir_all(&costs_dir).unwrap();
    std::fs::write(
        costs_dir.join("ledger.jsonl"),
        r#"{"model":"gpt-4","tokens":100}"#,
    ).unwrap();

    // traces() now reads from the tempdir, not ~/.simard
    let result = tokio::runtime::Runtime::new().unwrap()
        .block_on(traces());
    assert!(result.0["span_count"].as_u64().unwrap() > 0);
}
```

When `state` drops, the `SIMARD_STATE_ROOT` env var is restored and the
tempdir is reaped — regardless of whether the test panicked.

---

## `EnvGuard` for the daemon health path

The `activity()` handler reads daemon health from
`dirs::data_local_dir()/simard/daemon_health.json`. On Linux,
`dirs::data_local_dir()` reads `$XDG_DATA_HOME` (defaulting to
`~/.local/share`). Tests redirect this to a tempdir with a local
`EnvGuard`:

```rust
#[test]
#[serial_test::serial(dashboard_state)]
fn activity_reads_daemon_health_when_present() {
    let state = HermeticState::new();

    // Redirect XDG_DATA_HOME so dirs::data_local_dir() resolves
    // inside the tempdir
    let xdg_dir = state.state_root().join("xdg_data");
    std::fs::create_dir_all(&xdg_dir).unwrap();
    let _xdg_guard = EnvGuard::set("XDG_DATA_HOME", &xdg_dir);

    // Write daemon_health.json inside the redirected path
    let health_dir = xdg_dir.join("simard");
    std::fs::create_dir_all(&health_dir).unwrap();
    std::fs::write(
        health_dir.join("daemon_health.json"),
        r#"{"status":"running","cycle_number":42}"#,
    ).unwrap();

    let result = tokio::runtime::Runtime::new().unwrap()
        .block_on(activity());
    assert_eq!(result.0["daemon"]["status"], "running");
    assert_eq!(result.0["daemon"]["current_cycle"], 42);
}
```

The `EnvGuard` is an RAII struct whose `Drop` restores the previous
`XDG_DATA_HOME` value (or removes it if it was previously unset). This
makes cleanup panic-safe — if an assertion fails, the guard still fires.

---

## The `dashboard_state` serial group

Both mutating tests are annotated with
`#[serial_test::serial(dashboard_state)]`. This serialises them so they
never run concurrently within the same `cargo test` process. The group
name is distinct from `cognitive_memory` (used by `HermeticState`-only
tests) because the dashboard tests also mutate `XDG_DATA_HOME`, which
is orthogonal to the cognitive-memory env vars.

```rust
#[test]
#[serial_test::serial(dashboard_state)]
fn traces_reads_cost_ledger_when_present() { … }

#[test]
#[serial_test::serial(dashboard_state)]
fn activity_reads_daemon_health_when_present() { … }
```

Read-only tests (those that do not write fixture files or mutate env
vars) do not need `#[serial]` and run in parallel.

### Why both tempdir AND serial?

Tempdir isolation is the primary defence — it prevents tests from
touching the operator's live state. The `#[serial]` annotation is a
belt-and-suspenders guard against env-var races: `std::env::set_var` is
process-global, so two tests setting `XDG_DATA_HOME` concurrently would
produce undefined behaviour even if each wrote to a different tempdir.

With both in place, individual tests are hermetic (tempdir), serialised
(no env races), and panic-safe (RAII cleanup).

---

## RAII cleanup guarantees

The cleanup model is layered:

1. **`EnvGuard::drop()`** — restores or removes the env var.
2. **`HermeticState::drop()`** — restores `SIMARD_STATE_ROOT`,
   restores `SIMARD_MEMORY_SOCKET`, then drops the inner `TempDir`.
3. **`TempDir::drop()`** — recursively removes the temp directory tree.

Rust's Drop order is reverse declaration order, so:

```rust
let state = HermeticState::new();      // dropped last  → TempDir reaped
let _xdg_guard = EnvGuard::set(…);     // dropped first → env restored
```

If an assertion panics, Rust unwinds and runs Drop impls for all
live locals. The only scenario where cleanup is skipped is
`std::process::abort()` or a double panic, neither of which occurs in
normal test execution.

---

## Production change: `traces()` uses `resolve_state_root()`

Before hermetisation, `traces()` hardcoded the ledger path:

```rust
// BEFORE (non-hermetic)
let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
let path = PathBuf::from(&home).join(".simard/costs/ledger.jsonl");
```

This was changed to use the shared state-root helper:

```rust
// AFTER (hermetic-friendly)
let path = resolve_state_root().join("costs/ledger.jsonl");
```

This is a production behaviour change — the ledger path now honours
`$SIMARD_STATE_ROOT` — but it is consistent with every other
state-root-aware path in the dashboard (see
[State-root resolution](../reference/state-root-resolution.md)) and
was the minimal change needed to make the path testable.

The `activity()` handler's daemon health path was **not** changed in
production. It continues to read from `dirs::data_local_dir()`, which
is the correct XDG-compliant location for runtime health data. Tests
redirect it via `XDG_DATA_HOME` only.

---

## Configuration

### Environment variables used by tests

| Variable | Set by | Purpose |
|----------|--------|---------|
| `SIMARD_STATE_ROOT` | `HermeticState` | Redirects `resolve_state_root()` to tempdir |
| `SIMARD_MEMORY_SOCKET` | `HermeticState` (unset) | Prevents socket-path leakage |
| `XDG_DATA_HOME` | `EnvGuard` | Redirects `dirs::data_local_dir()` to tempdir |

### Environment variables used at runtime

| Variable | Default | Effect on dashboard |
|----------|---------|-------------------|
| `SIMARD_STATE_ROOT` | `~/.simard` | Relocates cost ledger path |
| `XDG_DATA_HOME` | `~/.local/share` | Relocates daemon health path |

---

## Running the tests

```bash
# Run only the activity tests
cargo test -- tests_activity

# Run with output to verify isolation
cargo test -- tests_activity --nocapture

# Stress-test for flakiness (50 iterations)
for i in $(seq 1 50); do
    cargo test -- tests_activity --test-threads=4 2>&1 | tail -1
done
```

Every iteration should report `test result: ok`.

---

## Adding a new mutating activity test

1. Add `let state = HermeticState::new();` as the first line.
2. If the handler reads from `dirs::data_local_dir()`, add an
   `EnvGuard` for `XDG_DATA_HOME` pointing into the tempdir.
3. Write fixture files inside `state.state_root()`, not under `$HOME`.
4. Annotate with `#[serial_test::serial(dashboard_state)]`.
5. Do not add explicit cleanup — RAII handles it.

Read-only tests that do not write fixtures or mutate env vars can skip
steps 1–4 and run in parallel.

---

## What NOT to do

- **Do not write to `~/.simard/costs/`** in a test. The cost ledger is
  operator data. Use `state.state_root().join("costs/…")` instead.
- **Do not write to `~/.local/share/simard/`** in a test. Use the
  `XDG_DATA_HOME` redirect pattern above.
- **Do not use `finally`-style cleanup after assertions.** Rust
  assertions panic, which skips subsequent code. RAII (Drop impls) is
  the only reliable cleanup mechanism in Rust tests.
- **Do not skip `#[serial]` because "my test uses a unique tempdir".**
  The env vars are process-global. Two tests setting `XDG_DATA_HOME`
  concurrently race even if their tempdirs differ.
- **Do not add `scopeguard` as a dependency.** The `EnvGuard` struct
  and `HermeticState` already provide Drop-based cleanup. A third-party
  scope guard adds no value and a new dependency.

---

## Related

- [Writing hermetic tests against cognitive memory](./hermetic-tests.md)
  — the broader hermeticity contract for `SIMARD_STATE_ROOT`.
- [CI-resilient test patterns](./ci-resilient-test-patterns.md) —
  complementary patterns for constant-relative assertions, lazy config,
  and serial env-var tests.
- [State-root resolution](../reference/state-root-resolution.md) —
  the helper that `traces()` now uses.
- [Dashboard E2E tests](../reference/dashboard-e2e-tests.md) —
  Playwright tests that exercise the dashboard end-to-end.
- [Dashboard overview](../dashboard.md) — top-level dashboard docs.
