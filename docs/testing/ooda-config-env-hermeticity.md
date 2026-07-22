---
title: Hermetic OodaConfig::default() tests — the cognitive_memory serial group
description: >
  How ooda_config_default_values was made deterministic under a self-hosted
  runner whose live ecosystem exports SIMARD_OODA_MAX_CONCURRENT: the test
  neutralizes the two concurrency env vars that OodaConfig::default() reads
  (SIMARD_OODA_MAX_CONCURRENT and legacy SIMARD_MAX_CONCURRENT_ACTIONS) through
  an RAII EnvGuard under the repo-mandated cognitive_memory serial key, and
  asserts against the DEFAULT_MAX_CONCURRENT_ACTIONS constant rather than a magic 24.
last_updated: 2026-07-22
review_schedule: when OodaConfig::default() reads a new process-global env var, when serial_guard's REQUIRED_KEY changes, or when serial_test is upgraded
owner: simard
doc_type: reference
related:
  - ./hermetic-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ./deflaking-known-flaky-tests.md
  - ./ci-resilient-test-patterns.md
---

# Hermetic `OodaConfig::default()` tests — the `cognitive_memory` serial group

This page is the test-author and reviewer contract for
`src/ooda_loop/tests_types.rs::ooda_config_default_values` and any future
sibling test that constructs `OodaConfig::default()` and asserts on a field
that `Default` derives from the process environment.

It documents a **test-hermeticity** fix only. No production behaviour changed:
`OodaConfig::default()` in `src/ooda_loop/types.rs` and the Issue
[#2935](https://github.com/rysweet/Simard/issues/2935) default-24 / env-override
precedence are frozen and byte-for-byte unchanged.

## Why this exists — the non-hermetic failure

Before this fix, `ooda_config_default_values` asserted a magic literal against
a value that `Default` reads from ambient process env:

```rust
// old — non-hermetic
let config = OodaConfig::default();
assert_eq!(config.max_concurrent_actions, 24);
```

`OodaConfig::default()` resolves `max_concurrent_actions` from the environment
(see [Configuration](#configuration-what-oodaconfigdefault-reads) below), only
falling back to `24` when **both** variables are unset. On the **self-hosted
CI runner** these PRs execute on, the live Simard ecosystem exports
`SIMARD_OODA_MAX_CONCURRENT` (tuned to a non-24 value) into the runner's
process environment. So `config.max_concurrent_actions` picked up the ambient
tuned value and the `== 24` assertion panicked at
`src/ooda_loop/tests_types.rs:253`.

Because the leak came from the *runner environment* and not the PR under test,
the symptom was a single, identical `pre-commit` failure —
`9070 passed; 1 failed` — appearing across several otherwise-unrelated open
PRs, while `main` stayed green on clean (env-free) runners. This is the classic
"green on my machine, red on the shared runner" shape covered generally in
[CI-resilient test patterns](./ci-resilient-test-patterns.md); this page is the
specific, closed instance for the OODA config surface.

## The contract — what every `OodaConfig::default()` test must guarantee

A test that constructs `OodaConfig::default()` and asserts on an
**env-derived** field MUST guarantee, at the moment of construction:

- **(O1)** Every environment variable feeding the asserted field is
  **neutralized** to its unset state for the duration of the test, so the
  assertion observes the compiled-in default and never an ambient value.
- **(O2)** Neutralization is **restored** to the process's prior state when the
  test returns — including on panic — so no test can leak a mutation into a
  later test.
- **(O3)** The test carries the `#[serial(cognitive_memory)]` key, because env
  mutation is process-global and would otherwise race any parallel test that
  reads or writes the same variables. This is also the key **mandated** by the
  `serial_guard` meta-test (see below) for *every* env-mutating test in the
  crate — a dedicated per-feature key is not an accepted alternative.
- **(O4)** The assertion compares against the **public default constant**
  (`DEFAULT_MAX_CONCURRENT_ACTIONS`), not a hand-copied literal, so the test
  tracks the PRD source of truth instead of drifting from it.

Neutralization (O1/O2) is the non-negotiable fix — it is what makes the test
hermetic. The constant assertion (O4) is a robustness/readability bonus: on its
own it does **not** fix hermeticity, because `Default` still reads ambient env
and a non-24 ambient value would still make the field differ from the constant.

## Configuration — what `OodaConfig::default()` reads

`max_concurrent_actions` is resolved with **independent fail-closed
precedence** (unchanged production logic, quoted here so test authors know
exactly which variables to neutralize):

| Precedence | Env variable                     | Behaviour                                                                                                 |
| ---------- | -------------------------------- | --------------------------------------------------------------------------------------------------------- |
| 1          | `SIMARD_OODA_MAX_CONCURRENT`     | Preferred. If **present**, its bounds-validated value is used; present-but-invalid fails closed to `24`.  |
| 2          | `SIMARD_MAX_CONCURRENT_ACTIONS`  | Legacy. Consulted **only** when the preferred var is entirely unset.                                      |
| 3          | *(neither set)*                  | Compiled-in `DEFAULT_MAX_CONCURRENT_ACTIONS = 24`.                                                         |

Both variables are bounds-validated to
`[MAX_CONCURRENT_MIN, MAX_CONCURRENT_MAX]` = `[1, 64]`; a present-but-invalid
value is treated as an operator misconfiguration and fails closed to the
default rather than falling through to a lower-precedence source.

Relevant constants (`src/ooda_loop/types.rs`):

| Constant                        | Value | Meaning                                              |
| ------------------------------- | ----- | ---------------------------------------------------- |
| `DEFAULT_MAX_CONCURRENT_ACTIONS`| `24`  | Issue #2935 per-cycle parallelism ceiling default.   |
| `MAX_CONCURRENT_MIN`            | `1`   | Lower bound for a validated override.                |
| `MAX_CONCURRENT_MAX`           | `64`  | Upper bound; larger values are rejected (fail closed).|

Because precedence reads **both** variables, a hermetic default test must
neutralize **both** `SIMARD_OODA_MAX_CONCURRENT` and
`SIMARD_MAX_CONCURRENT_ACTIONS`.

## The `cognitive_memory` serial key

Environment mutation is process-global, so the neutralizing test is annotated
with the crate-wide env-mutation serial key:

```rust
#[serial(cognitive_memory)]
```

This is **not** a free choice — it is an enforced repo contract. The meta-test
`every_env_mutating_test_is_serialized` in
`src/test_support/serial_guard.rs` audits every test under `src/` and, because
its watch mode is `EnvWatch::AnyVar`, requires that *any* test mutating process
env carry `REQUIRED_KEY = "cognitive_memory"`. Its failure message is explicit:
*"add `#[serial_test::serial(cognitive_memory)]`."* A dedicated per-feature key
(e.g. a hypothetical `ooda_config_env`) is **rejected** — there is no allowlist
entry for one, and serial groups are mutually independent, so a separate key
would run **concurrently** with the real writers below and reintroduce the very
race O3 exists to close.

Sharing `cognitive_memory` is also functionally required, not just contractual:
the ~15 sibling tests in `src/ooda_loop/types.rs` that write
`SIMARD_OODA_MAX_CONCURRENT` / `SIMARD_MAX_CONCURRENT_ACTIONS` directly (e.g.
`simard_ooda_max_concurrent_overrides_default`,
`new_var_takes_precedence_over_legacy_var`) all carry
`#[serial(cognitive_memory)]`. Only by joining the **same** group does this
test serialize against those writers so that no parallel writer can mutate the
variables between `EnvGuard::unset` and `OodaConfig::default()`.

> **Caveat on the `budget_env` precedent.** `monitoring.rs`'s `budget_env`
> guard uses a distinct key and passes the audit **only** through a known
> false-negative: the meta-test detects direct in-body `set_var`/`remove_var`,
> not mutation performed inside an `EnvGuard`/`Drop` associated method
> (`serial_guard.rs` documents this blind spot). Do **not** treat that as a
> licence to invent your own key — a non-`cognitive_memory` key would compile
> green while silently *not* serializing against the real writers. Use
> `cognitive_memory`.

See [serial(cognitive_memory) isolation](./cognitive-memory-serial-isolation.md)
for the canonical description of the group and its guard.

## The `EnvGuard` RAII helper

Neutralization uses a module-local `EnvGuard`. It records the prior value on
construction and restores it exactly (set-if-present, remove-if-absent) on
`Drop`, so restoration runs during normal return **and** during panic
unwinding:

```rust
use serial_test::serial;

struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    /// Remove `key` from the process env, remembering its prior value.
    fn unset(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: serialized via `#[serial(cognitive_memory)]`.
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: serialized via `#[serial(cognitive_memory)]`.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
```

Rules for the helper:

- Each `unsafe` env mutation cites `#[serial(cognitive_memory)]` in its
  `// SAFETY:` comment — the serial key is what makes the process-global
  `set_var`/`remove_var` sound under glibc.
- **Never** restore the environment by hand in the test body. Restoration lives
  only in `Drop` so it is panic-safe; a manual restore would double-restore or
  be skipped on early panic.
- Bind each guard to a **named** local (`let _g1 = …`), not `let _ = …`. A
  bare `_` binding drops immediately and would restore the variable *before*
  `OodaConfig::default()` runs, defeating the guard.

## The hermetic test

```rust
#[test]
#[serial(cognitive_memory)]
fn ooda_config_default_values() {
    // Neutralize BOTH concurrency env vars so Default observes the
    // compiled-in ceiling regardless of the runner's ambient environment.
    // Guards restore the prior values on drop (including on panic).
    let _g1 = EnvGuard::unset("SIMARD_OODA_MAX_CONCURRENT");
    let _g2 = EnvGuard::unset("SIMARD_MAX_CONCURRENT_ACTIONS");

    let config = OodaConfig::default();

    // Issue #2935: the per-OODA-cycle goal-coverage parallelism ceiling.
    // Assert against the constant (PRD source of truth), not a magic `24`.
    assert_eq!(config.max_concurrent_actions, DEFAULT_MAX_CONCURRENT_ACTIONS);
    assert!((config.improvement_threshold - 0.02).abs() < f64::EPSILON);
    assert_eq!(config.gym_suite_id, "progressive");
}
```

`DEFAULT_MAX_CONCURRENT_ACTIONS` is already in scope via the module's
`use super::types::*;` — no new import beyond `serial_test::serial` is needed.
The `serial_test` crate is already a dev-dependency
(`serial_test = "=3.4.0"`); this change adds **no** manifest entry.

> **Why `EnvGuard` here and not the `types.rs` `ENV_LOCK`?** The env tests
> inside `types.rs` share a module-local `static ENV_LOCK: Mutex<()>` plus a
> `clear_concurrency_env()` helper. That `Mutex` is private to the `types.rs`
> test module and cannot be referenced from the separate `tests_types.rs`
> module, so this test uses the RAII `EnvGuard` for restore-on-drop instead.
> The shared `#[serial(cognitive_memory)]` key — not the `Mutex` — is what
> serializes the two modules' env tests against each other.

## Verifying the fix

The test must pass deterministically **regardless of ambient env**. Verify both
directions explicitly:

```bash
# 1. With the runner-style pollution present — this is what used to fail.
SIMARD_OODA_MAX_CONCURRENT=48 \
  cargo test -p simard ooda_config_default_values -- --exact

# 2. With the legacy variable present.
SIMARD_MAX_CONCURRENT_ACTIONS=12 \
  cargo test -p simard ooda_config_default_values -- --exact

# 3. With a clean environment (the historically-green path).
cargo test -p simard ooda_config_default_values -- --exact

# 4. Whole suite — no regressions (9070+ pass, 0 fail).
cargo test -p simard

# 5. The gate that was red across the PR cluster.
pre-commit run --all-files

# 6. Restoration check — a pre-set var survives the test unchanged.
export SIMARD_OODA_MAX_CONCURRENT=48
cargo test -p simard ooda_config_default_values -- --exact
test "$SIMARD_OODA_MAX_CONCURRENT" = 48 && echo "restored OK"
unset SIMARD_OODA_MAX_CONCURRENT
```

All must be green; cases 1, 2 and 6 must leave
`SIMARD_OODA_MAX_CONCURRENT` / `SIMARD_MAX_CONCURRENT_ACTIONS` set to their
original values afterward (the guard restores them).

## Extending the pattern

When you add a new `#[test]` that constructs `OodaConfig::default()` and asserts
on a field that `Default` reads from the environment (for example
`daily_budget_usd` from `SIMARD_DAILY_BUDGET_USD`, or
`distill_min_episodes` from `SIMARD_DISTILL_MIN_EPISODES`):

1. Add `#[serial(cognitive_memory)]` to the test — the key mandated by
   `serial_guard` and shared by the existing env writers.
2. `EnvGuard::unset(...)` **every** variable the asserted field reads, binding
   each guard to a named local.
3. Assert against the field's **default constant**, never a copied literal.

This keeps the test in the crate's single `cognitive_memory` env group —
mutually serialized with every other env writer — and keeps the assertions
anchored to the PRD constants rather than drifting from them.

## Scope note

This test-hermeticity fix covers the two crate tests that construct
`OodaConfig::default()` and assert on the env-derived `max_concurrent_actions`
field: `src/ooda_loop/tests_types.rs::ooda_config_default_values` and its
sibling `src/operator_commands_ooda/tests/report_tests.rs::ooda_config_default_values`.
Both now neutralize the concurrency env vars under `#[serial(cognitive_memory)]`
and assert against `DEFAULT_MAX_CONCURRENT_ACTIONS`. The operator merge of
PRs #4344 / #4145 and the runner load-saturation concern are **operations**
items, not part of this test-hermeticity change.
