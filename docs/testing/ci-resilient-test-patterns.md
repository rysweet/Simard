---
title: CI-resilient test patterns
description: >
  Four patterns that prevent common CI-only test failures: constant-relative
  assertions, lazy config resolution, serial env-var tests, and shape-only
  assertions for compile-time version constants.
last_updated: 2026-06-28
review_schedule: when MAX_ACTIVE_GOALS, agent proxy construction, or the CLI
  version-string test change
owner: simard
doc_type: reference
related:
  - ./hermetic-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ./COVERAGE_BASELINE.md
  - ../reference/meeting-backend-api.md
  - ../operations/meeting-handoffs.md
  - ../reference/goal-board-api.md
  - ../architecture/gym-eval-library-adapter.md
---

# CI-resilient test patterns

This page documents four patterns that keep the test suite green in CI
where the environment differs from a developer workstation (no config
files, no env vars, parallel test execution). Patterns 1–3 were
introduced to fix CI-blocking failures on `main` (issue
[#2197](https://github.com/rysweet/Simard/issues/2197)); Pattern 4 was
added later for a recurring version-literal merge blocker (issue
[#2483](https://github.com/rysweet/Simard/issues/2483)).

---

## Pattern 1: Constant-relative assertions in goal curation tests

### Problem

Tests that hardcode numeric limits (e.g. "create 7 goals, expect 5
active and 2 in backlog") break silently whenever the underlying
constant changes. When `MAX_ACTIVE_GOALS` was bumped from 5 → 7, the
overflow test created exactly 7 decisions — all of which now fit in the
active set, leaving the backlog empty and failing the assertion.

### Rule

**Never hardcode `MAX_ACTIVE_GOALS` (or any capacity constant) in a
test.** Always express quantities relative to the constant:

```rust
use crate::goal_curation::MAX_ACTIVE_GOALS;

// Create enough decisions to overflow: MAX fills active, 2 overflow to backlog
let decisions: Vec<MeetingDecision> = (1..=(MAX_ACTIVE_GOALS + 2))
    .map(|i| sample_decision(&format!("Goal {i}")))
    .collect();
```

Then assert against the constant:

```rust
assert_eq!(active.len(), MAX_ACTIVE_GOALS);
assert_eq!(backlog.len(), 2);
```

### Where this applies

Any test in `src/ooda_loop/curate.rs` or `src/goal_curation/` that
exercises capacity boundaries. The canonical example is
`check_meeting_handoffs_overflow_goes_to_backlog`.

### How to verify

```bash
cargo test -p simard --lib ooda_loop::curate::tests
```

The test self-adjusts to any future value of `MAX_ACTIVE_GOALS` without
code changes.

### Current value

`MAX_ACTIVE_GOALS` is defined in `src/goal_curation/types.rs`:

```rust
pub const MAX_ACTIVE_GOALS: usize = 7;
```

A test-time assertion in the same module (`max_active_goals_constant`
asserts `MAX_ACTIVE_GOALS == 7`) guards against accidental changes.
If you intentionally change the constant, update that test — the
overflow test will adapt automatically.

---

## Pattern 2: Lazy config resolution in `PersistentAgentProxy`

### Problem

`PersistentAgentProxy::new()` called `resolve_agent_command()`, which
reads `RuntimeConfig` (env vars + `config.toml`). In CI, neither
`SIMARD_LLM_PROVIDER` nor `config.toml` exist, so `new()` returned
`MissingRequiredConfig` and the construction-only test
`new_creates_proxy` failed.

### Rule

**Constructor (`new()`) must not require runtime configuration.**
Config resolution is deferred to `open()`, where it runs before the
agent is first validated.

The lifecycle is:

```
new()        → allocates struct, reads optional SIMARD_MEETING_TURN_TIMEOUT_SECS
               (with fallback default), does NOT call RuntimeConfig::load()
open()       → resolves agent command from RuntimeConfig, validates agent
run_turn()   → sends a prompt and collects the response
Drop         → tears down the agent process
```

### API

```rust
impl PersistentAgentProxy {
    /// Creates a new proxy.
    ///
    /// Does NOT read config or validate the agent — call `open()` first.
    /// This is intentional: tests and builders can construct proxies
    /// without a runtime environment.
    pub fn new() -> SimardResult<Self> { … }
}

impl BaseTypeSession for PersistentAgentProxy {
    /// Resolves the agent command from RuntimeConfig, then validates
    /// that the agent binary exists and is executable.
    ///
    /// Must be called before `run_turn()`. Calling `run_turn()` on an
    /// unopened proxy returns an error.
    fn open(&mut self) -> SimardResult<()> { … }

    /// Sends a prompt to the agent and returns the response.
    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> { … }
}
```

### Internal fields

`agent_cmd` and `agent_base_args` are initialized to empty defaults in
`new()` and populated by `resolve_agent_command()` inside `open()`. The
call chain guarantees fields are populated before use:

```
open()
  └─ resolve_agent_command()   ← sets agent_cmd, agent_base_args
  └─ validate_agent()          ← uses agent_cmd
run_turn()
  └─ invoke_agent()            ← uses agent_cmd + agent_base_args
```

### Testing

```rust
#[test]
fn new_creates_proxy() {
    // No config.toml, no env vars — new() must succeed
    let proxy = PersistentAgentProxy::new();
    assert!(proxy.is_ok());
    let proxy = proxy.unwrap();
    assert!(!proxy.is_open);
    assert!(!proxy.is_closed);
    assert_eq!(proxy.turn_count, 0);
}
```

### Configuration

The agent command is resolved from `RuntimeConfig` at `open()` time.
`resolve_agent_command()` loads `RuntimeConfig` and matches on the
configured `llm_provider`:

- `LlmProvider::Copilot` → `copilot --allow-all-tools --allow-all-paths`
- `LlmProvider::RustyClawd` → `claude --allowedTools all`

`RuntimeConfig::load()` reads `SIMARD_LLM_PROVIDER` env var and/or
`config.toml`. If neither is available (as in CI), `open()` returns
`MissingRequiredConfig` — but `new()` succeeds regardless.

---

## Pattern 3: `#[serial]` for env-var-mutating tests

### Problem

Tests in `src/gym_runner_bridge.rs` set and unset the `SIMARD_SKIP_GYM`
environment variable to exercise conditional code paths (skip-gym
synthetic mode vs real execution). When `cargo test` runs these in
parallel, two tests race on the same process-wide env var: one sets
`SIMARD_SKIP_GYM=1`, another removes it, and the assertion in the
first test observes the wrong value — producing intermittent failures.

### Rule

**Any test that mutates a process-wide environment variable must be
annotated with `#[serial]`.** The `serial_test` crate (already in
`dev-dependencies`) serializes annotated tests so they never run
concurrently.

```rust
use serial_test::serial;

#[test]
#[serial]
fn run_scenario_skip_gym_returns_synthetic_success() {
    // SAFETY: test-only
    unsafe {
        std::env::set_var("SIMARD_SKIP_GYM", "1");
    }
    // … test body …
    unsafe {
        std::env::remove_var("SIMARD_SKIP_GYM");
    }
}
```

### Where this applies

The five env-var-mutating tests in `src/gym_runner_bridge.rs` (migrated from
the deleted `src/native_gym.rs` when the gym engine moved to the
[`amplihack-agent-eval` library](../architecture/gym-eval-library-adapter.md)):

| Test function                                    | Env var mutated       |
| ------------------------------------------------ | --------------------- |
| `run_scenario_skip_gym_dimensions_present_and_zero` | `SIMARD_SKIP_GYM`  |
| `run_scenario_skip_gym_bypasses_engine_for_any_valid_id` | `SIMARD_SKIP_GYM` |
| `run_scenario_skip_gym_returns_synthetic_success` | `SIMARD_SKIP_GYM`    |
| `run_suite_skip_gym_reports_zero_scenarios`       | `SIMARD_SKIP_GYM`    |
| `run_suite_skip_gym_returns_synthetic_success`    | `SIMARD_SKIP_GYM`    |

### Relationship to `HermeticState`

This pattern complements the `HermeticState` guard documented in
[hermetic-tests.md](./hermetic-tests.md). `HermeticState` isolates
cognitive-memory state roots; `#[serial]` isolates process-wide env
vars. Both address the same root cause: `cargo test` runs tests in
parallel within a single process, and process-wide mutations are not
thread-safe.

Use `HermeticState` for cognitive-memory tests. Use `#[serial]` for
any other test that calls `std::env::set_var` / `std::env::remove_var`.

### How to verify

```bash
# Run the gym_runner_bridge tests — they should pass deterministically now
cargo test -p simard --lib gym_runner_bridge::tests

# Stress-test for flakiness (run 50 times)
for i in $(seq 1 50); do
    cargo test -p simard --lib gym_runner_bridge::tests -- --test-threads=4 2>&1 \
        | tail -1
done
```

Every iteration should report `test result: ok`.

---

## Pattern 4: Shape-only assertions for compile-time version constants

### Problem

A test that pins the crate version to an exact literal is a recurring,
systemic merge blocker. The CLI golden test
`version_string_is_semver` in `tests/cli_golden.rs` previously asserted
both the semver *shape* of the version **and** an exact string literal:

```rust
// ❌ brittle: must be hand-edited on every version bump
assert_eq!(VERSION, "0.23.0", "bump this assertion when version changes");
```

Because `VERSION` is defined as the compile-time crate version:

```rust
const VERSION: &str = env!("CARGO_PKG_VERSION");
```

…the literal `"0.23.0"` is merely a hand-maintained mirror of the value
already owned by `Cargo.toml`. It adds no durable invariant — only the
upkeep cost of keeping the two copies identical — yet any
`Cargo.toml` version bump that does not also hand-edit the assertion
fails the `pre-commit` CI job and blocks the PR. Default-workflow
recipes and engineer goals routinely produce such bumps as a side
effect, so the literal broke unrelated PRs repeatedly (observed on
[#2474](https://github.com/rysweet/Simard/issues/2474) and
[#2482](https://github.com/rysweet/Simard/issues/2482); tracked and
fixed in [#2483](https://github.com/rysweet/Simard/issues/2483)).

### Rule

**Never pin a compile-time version constant to an exact string
literal in a test.** Validate the *shape* of the version instead — that
it is well-formed semver (three dot-separated, numeric components). The
shape is the durable invariant; the exact value is owned by
`Cargo.toml` and must never require a parallel manual edit.

`version_string_is_semver` now asserts shape only:

```rust
#[test]
fn version_string_is_semver() {
    // The binary has no --version flag; verify the Cargo-embedded version
    // matches the expected semver pattern so the constant stays in sync.
    let parts: Vec<&str> = VERSION.split('.').collect();
    assert_eq!(parts.len(), 3, "version should be semver: {VERSION}");
    for part in &parts {
        part.parse::<u32>()
            .unwrap_or_else(|_| panic!("non-numeric version component '{part}' in {VERSION}"));
    }
}
```

This catches the failures that actually matter — a malformed,
non-numeric, or wrong-segment-count version — while letting the version
float freely. A `Cargo.toml` bump (`0.23.0 → 0.23.1`, or any future
value) requires **zero** test edits.

### Where this applies

Any test that reads `env!("CARGO_PKG_VERSION")` (directly or via a
`VERSION` constant) and asserts on it. The canonical example is
`version_string_is_semver` in `tests/cli_golden.rs`. The other golden
tests in that file (help-text, subcommand, and error-message
invariants) are unaffected.

> **Release control.** If an exact version *must* be gated for a
> release, do it where the version is defined — gate the `Cargo.toml`
> bump itself (review, tag, or a dedicated release check) rather than
> mirroring the literal into a unit test. Shape-only validation and
> release gating are separate concerns.

### How to verify

```bash
cargo test --test cli_golden
```

The test self-adjusts to any future crate version without code changes.
To confirm independence from the version value, bump `Cargo.toml`'s
`version` field locally and re-run — `version_string_is_semver` stays
green.

### Related but not an instance of this pattern

`tests/install_real.rs` defines
`const EXPECTED_VERSION: &str = env!("CARGO_PKG_VERSION");` and asserts
that the *installed* binary's `--version` output equals
`simard {EXPECTED_VERSION}`. This is **not** the brittle anti-pattern:
`EXPECTED_VERSION` is derived from the same compile-time constant rather
than a hand-edited literal, so it tracks the crate version automatically
and never needs a manual edit on a version bump. It is a legitimate
end-to-end check (the installed binary reports the version it was built
with) and should be left as-is.

---

## Summary

| Pattern               | Problem                     | Fix                              | Scope                       |
| --------------------- | --------------------------- | -------------------------------- | --------------------------- |
| Constant-relative     | Hardcoded capacity limits   | Use `MAX_ACTIVE_GOALS + N`       | `src/ooda_loop/curate.rs`   |
| Lazy config resolution| Constructor reads config    | Defer to `open()`                | `src/meeting_backend/agent_proxy.rs` |
| Serial env-var tests  | Parallel env-var races      | `#[serial]` annotation           | `src/gym_runner_bridge.rs`  |
| Shape-only version    | Exact version literal pin   | Assert semver shape, not literal | `tests/cli_golden.rs`       |

All four patterns are enforced by CI: the affected tests run on every
PR and will fail if the pattern is violated.
