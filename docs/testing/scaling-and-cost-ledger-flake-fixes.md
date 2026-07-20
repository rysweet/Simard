---
title: Scaling and cost-ledger test flake fixes (red-main after #4361)
description: >
  Why `scaler_current_max_can_override_config` and
  `meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective` are
  hermetic, and how the cognitive_memory serial-guard was extended to cover the
  SIMARD_SCALING / SIMARD_OODA_MAX_CONCURRENT / OodaConfig::default() env-read
  surface. These fixes restore the `verify` workflow to green without touching
  any production or overseer code from PR #4361.
last_updated: 2026-07-20
review_schedule: when a new process-global env var is read from a test constructor, or when OodaConfig::default() gains a new env read
owner: ooda-core
doc_type: reference
related:
  - ./cognitive-memory-serial-isolation.md
  - ./hermetic-tests.md
  - ./deflaking-known-flaky-tests.md
  - ../reference/run-command-pipe-drain.md
---

# Scaling and cost-ledger test flake fixes

The `verify` workflow went RED on `main` at `fd6bf8fc` (PR #4361, the overseer
agentic health-review rail); the prior run at `650f3795` was green. The
regression was **not** in the overseer feature code — that production code is
correct and is not modified. It was two pre-existing tests whose reliance on the
process-global environment turned into an intermittent failure under the test
scheduling that #4361 happened to perturb. Both are classic glibc
`setenv`/`getenv` races of the family documented in
[serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md):
the OS environment is process-global and `setenv`/`getenv` are not thread-safe,
so one test mutating an env var can tear a *concurrent* env read in an unrelated
test, even a read of a different variable.

These fixes make the two tests hermetic and extend the regression guard so the
class cannot silently return.

## TL;DR

- **Flake A — `scaler_current_max_can_override_config`** (`tests/adaptive_scaling.rs`).
  The test built its `OodaConfig` with `..OodaConfig::default()`, and
  `OodaConfig::default()` reads `SIMARD_OODA_MAX_CONCURRENT`,
  `SIMARD_MAX_CONCURRENT_ACTIONS`, and `SIMARD_SCALING` from the process
  environment (`src/ooda_loop/types.rs`). Concurrently with a test that mutates
  `SIMARD_SCALING` (e.g. the `SIMARD_SCALING=auto` cases in `types.rs`), that
  read could tear. **Fix:** construct the config hermetically — set **every**
  field explicitly (including `scaler: None`) so `..OodaConfig::default()` is not
  called at all and no env read happens. Note that `OodaConfig { scaler: None,
  ..OodaConfig::default() }` does *not* help, because struct-update still runs
  `default()` and its env reads first (see below). If dropping `default()` is
  undesirable, keep it but add `#[serial(cognitive_memory)]`.
- **Flake B — `meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective`**
  (`src/base_type_copilot/tests.rs`). The test redirects `HOME` to a temp dir so
  the cost ledger (`$HOME/.simard/costs/ledger.jsonl`) is isolated, then asserts
  the recorded prompt tokens exceed the bare objective. **Fix:** assert against
  the ledger path the meeting adapter *actually used* (the resolved temp-`HOME`
  path), not ambient `HOME`, closing the concurrent `HOME`-writer race. The test
  keeps `#[serial(cognitive_memory)]` because mutating `HOME` requires the key.
- **Regression guard.** The `cognitive_memory` serial-guard meta-test /
  convention is extended to flag any hand-written test that derives config from
  `OodaConfig::default()` (or otherwise reads `SIMARD_SCALING` /
  `SIMARD_OODA_MAX_CONCURRENT`) without the serial key. The `SIMARD_SCALING`
  case is documented in
  [cognitive-memory-serial-isolation.md](./cognitive-memory-serial-isolation.md).
- **No production changes.** `OodaConfig::default()`'s env precedence
  (issue #2935 semantics) is unchanged; no overseer/`src/ooda_loop/*` production
  code is touched. The fixes live entirely in tests plus this doc and the guard.

## Flake A: `scaler_current_max_can_override_config`

### The read that could tear

`OodaConfig::default()` resolves the concurrency ceiling and scaler from the
environment at construction time:

```rust
// src/ooda_loop/types.rs (Default for OodaConfig)
let max_concurrent_actions = if std::env::var("SIMARD_OODA_MAX_CONCURRENT").is_ok() {
    env_u32_bounded("SIMARD_OODA_MAX_CONCURRENT", ...)
} else {
    env_u32_bounded("SIMARD_MAX_CONCURRENT_ACTIONS", ...)
};
let scaler = match std::env::var("SIMARD_SCALING").as_deref() {
    Ok("auto") => Some(Arc::new(AdaptiveScaler::new(max_concurrent_actions, 1, ceiling))),
    _ => None,
};
```

The test only cares that `decide` respects `scaler.current_max() == 2`, but by
spreading `..OodaConfig::default()` it performs three env reads that can race a
concurrent `SIMARD_SCALING` writer.

### Critical subtlety: struct-update still calls `default()`

Rust's struct-update syntax evaluates the base expression **in full** before
applying the named overrides. So this does **not** deflake:

```rust
// STILL RACY: OodaConfig::default() runs first and performs every env read
// (SIMARD_SCALING, SIMARD_OODA_MAX_CONCURRENT, SIMARD_MAX_CONCURRENT_ACTIONS,
// SIMARD_DAILY_BUDGET_USD, …) before `scaler: None` overrides one field.
let config = OodaConfig { scaler: None, ..OodaConfig::default() };
```

Setting `scaler: None` only replaces the *result* of the env read; the read
itself already happened inside `default()` and can still tear a concurrent
`SIMARD_SCALING` writer. There are therefore exactly two correct fixes.

### Fix option 1 — fully explicit (truly hermetic, no serial key)

Construct **every** field so `OodaConfig::default()` is never called. As of this
change `OodaConfig` (`src/ooda_loop/types.rs`) has these fields:
`max_concurrent_actions`, `improvement_threshold`, `gym_suite_id`,
`daily_budget_usd`, `weekly_budget_usd`, `distill_min_episodes`,
`distill_interval_cycles`, `lesson_recurrence_threshold`, `scaler`.

```rust
#[test]
fn scaler_current_max_can_override_config() {
    use simard::ooda_loop::{OodaConfig, Priority, decide};

    let scaler = AdaptiveScaler::new(2, 1, 8);
    let priorities: Vec<Priority> = (1..=5)
        .map(|i| Priority { goal_id: format!("g{i}"), urgency: 1.0 - (i as f64 * 0.1), reason: format!("priority {i}") })
        .collect();

    // Hermetic: no ..OodaConfig::default(), so no process-env read happens.
    let config = OodaConfig {
        max_concurrent_actions: scaler.current_max(),
        improvement_threshold: 0.02,
        gym_suite_id: "progressive".to_string(),
        daily_budget_usd: 500.0,
        weekly_budget_usd: 2500.0,
        distill_min_episodes: 25,
        distill_interval_cycles: 50,
        lesson_recurrence_threshold: 2,
        scaler: None,
    };

    let actions = decide(&priorities, &config).unwrap();
    assert!(actions.len() <= 2, "decide should respect scaler's current_max of 2; got {}", actions.len());
}
```

No assertion depends on a value read from the process environment, so no serial
key is required.

### Fix option 2 — keep `default()`, add the serial key

If enumerating every field is undesirable (e.g. to track future field
additions), keep `..OodaConfig::default()` but serialize the env read against
all other env writers:

```rust
#[test]
#[serial_test::serial(cognitive_memory)]
fn scaler_current_max_can_override_config() {
    // ..OodaConfig::default() still reads env, but the serial key guarantees no
    // concurrent SIMARD_SCALING writer, so the getenv cannot tear.
    let config = OodaConfig { max_concurrent_actions: scaler.current_max(), ..OodaConfig::default() };
    // …
}
```

Option 1 is preferred (no serialization cost, fastest suite); option 2 is the
fallback per the Annotation Decision Rule. There is **no** new
`OodaConfig::hermetic_for_test()` constructor — adding one would be a production
API change, out of scope for a test-only flake fix.

## Flake B: meeting cost-ledger assertion

The test writes a unique `session_id`, redirects `HOME` to a `TempDir`, runs a
fake meeting turn, and reads back the ledger. The fix ensures the assertion reads
the **resolved** ledger path under the temp `HOME` the adapter used:

```rust
let ledger = home.path().join(".simard").join("costs").join("ledger.jsonl");
```

matched by the unique session id so a concurrent meeting test sharing the
process-global temp `HOME` cannot substitute its own entry. `HOME` is restored
before any panic is propagated, and the test keeps
`#[serial_test::serial(cognitive_memory)]` because it mutates `HOME`.

### Path-safety note

The resolved ledger path must stay inside the `TempDir` root — assert against the
canonicalized path, never a `..`-relative or symlinked location. Only benign
explicit values (a temp `HOME`, `scaler: None`) are injected; no real
credentials, tokens, or absolute user paths appear in fixtures.

## Regression guard extension

The serial-guard meta-test (`src/test_support/serial_guard.rs`) parses the
source tree with `syn` and fails the build when a test touches the watched env
surface without the `cognitive_memory` key. Two facts about the existing guard
shape this extension:

- **Writes are already covered.** Since issue #2375 the *write* watch is
  `EnvWatch::AnyVar`, so a test that calls `set_var`/`remove_var` on
  `SIMARD_SCALING` (or any var) without the key is *already* flagged. No change
  is needed on the write side.
- **The gap is the indirect *read*.** The *read* watch (`READ_WATCHED_VARS`)
  is a fixed list — `SIMARD_STATE_ROOT`, `SIMARD_MEMORY_SOCKET`,
  `SIMARD_LLM_PROVIDER`, `SIMARD_MEETINGS_DIR`, `SIMARD_MEETINGS_ROOT`,
  `SIMARD_HANDOFF_DIR` — and the parser only recognises a *direct*
  `std::env::var("…")` call. It cannot see that `OodaConfig::default()` reads
  `SIMARD_SCALING` transitively.

The extension therefore has two parts:

1. Add `SIMARD_SCALING` and `SIMARD_OODA_MAX_CONCURRENT` to `READ_WATCHED_VARS`
   so a *direct* read of either in a `#[test]` requires the key.
2. Add a **call-expression rule** that flags an `OodaConfig::default()` call
   inside a `#[test]` (the transitive read the var-name matcher cannot detect).
   Such a test must either be hermetic (fix option 1 — no `default()`) or carry
   `#[serial(cognitive_memory)]` (fix option 2).

This prevents an ordinary PR from silently reintroducing the race. The
`SIMARD_SCALING` case is also documented in
[cognitive-memory-serial-isolation.md](./cognitive-memory-serial-isolation.md).

## Verification gate

Reproduce and prove the fix with the exact CI command under high thread
concurrency:

```bash
# Loop the whole verify command; the race only appears when the SIMARD_SCALING /
# HOME writers run concurrently with these readers, so never name-filter.
for i in $(seq 1 100); do
  cargo test --all-features --locked --no-fail-fast \
    -- --skip install_packages_runs_and_self_installs || { echo "FAIL on run $i"; break; }
done
```

Finished state: 100% green across looped runs at high `--test-threads`, the
`verify` workflow and all required checks green on `main`, and **no** changes to
`src/overseer/*` or production `src/ooda_loop/*`.
