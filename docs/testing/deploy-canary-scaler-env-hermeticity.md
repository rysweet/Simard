---
title: Deploy-canary scaler env hermeticity (SIMARD_SCALING)
description: Test-author contract that keeps the deploy-gate canary unit-test path deterministic under an inherited SIMARD_SCALING=auto — why OodaConfig::default() injects an AdaptiveScaler from the ambient env, why the ..OodaConfig::default() spread made scaler_current_max_can_override_config panic (exit 101) and stall self-deploy, the pin-the-scaler fix pattern, and the local reproduction command.
last_updated: 2026-07-24
review_schedule: at every SIMARD_SCALING / OodaConfig::default / scrub_gate_env change
owner: simard
doc_type: reference
status: implemented
related:
  - ./hermetic-tests.md
  - ./deflaking-known-flaky-tests.md
  - ./ci-resilient-test-patterns.md
  - ../reference/adaptive-scaling-api.md
  - ../concepts/adaptive-scaling.md
  - ../howto/configure-adaptive-scaling.md
  - ../reference/canary-gate-convergence.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../../src/ooda_loop/adaptive_scaling.rs
  - ../../src/ooda_loop/types.rs
  - ../../src/ooda_loop/decide.rs
  - ../../src/self_relaunch/gates.rs
  - ../../tests/adaptive_scaling.rs
---

# Deploy-canary scaler env hermeticity (`SIMARD_SCALING`)

> **Status: implemented.** The `scaler_current_max_can_override_config`
> integration test in
> [`tests/adaptive_scaling.rs`](https://github.com/rysweet/Simard/blob/main/tests/adaptive_scaling.rs)
> is env-hermetic: it pins an explicit `AdaptiveScaler` and does **not** spread
> `..OodaConfig::default()`, so a deploy-gate subprocess that inherits
> `SIMARD_SCALING=auto` can no longer flip its assertion into a panic. This
> closes the recurring **exit status `101`** on the `unit-test` deploy-gate
> canary path that left the running daemon stuck behind merged `main`. The
> change is **test-only, additive, and non-breaking**: no production behavior,
> signatures, or the AIMD algorithm change.

## Why this exists

The deploy gate runs the candidate binary's own unit tests before the Overseer
promotes it — [`run_unit_test_gate`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
shells out to `cargo test`. When that test process **panics**, `cargo test`
exits `101`, the gate returns `passed: false`, and self-deploy refuses to
advance. Observed symptom: the `unit-test` canary reddened on **every** Overseer
tick for 6+ hours and the `release` workflow could not converge, so the deployed
daemon fell several commits behind `main`.

The panic was **not** flaky and **not** a genuine product regression. It was an
**env-fragility bug in one test**, triggered only when the ambient environment
carried `SIMARD_SCALING=auto`:

1. [`OodaConfig::default()`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/types.rs)
   reads `SIMARD_SCALING`. When it is `auto`, `default()` injects
   `scaler: Some(AdaptiveScaler::new(max, 1, ceiling))` (ceiling `24`). When it
   is unset or `fixed`, `scaler` is `None`.
2. [`decide()`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/decide.rs)
   **honors the scaler over the config field**: if `config.scaler` is `Some`,
   the per-cycle limit is `scaler.adjust()`, not `max_concurrent_actions`.
3. The old test body built its config with a spread:

   ```rust
   // OLD — env-fragile. Do not reintroduce.
   let scaler = AdaptiveScaler::new(2, 1, 8);
   let config = OodaConfig {
       max_concurrent_actions: scaler.current_max(), // 2
       ..OodaConfig::default()                       // ← may inject scaler=Some(ceiling 24)
   };
   let actions = decide(&priorities, &config).unwrap();
   assert!(actions.len() <= 2); // panics when the injected scaler yields ~5
   ```

   Under `SIMARD_SCALING=auto`, the `..OodaConfig::default()` spread overwrote
   the intended (absent) scaler with an ambient one whose ceiling is `24`. With
   5 over-quota priorities, `decide()` returned ~5 actions, the `<= 2` assertion
   failed, the test panicked, `cargo test` exited `101`, and the gate reddened.

The sibling unit test `decide_respects_max_concurrent_actions`
([`src/ooda_loop/decide.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/decide.rs))
was already hardened for this exact class (issue #2732) by pinning
`scaler: None`. `scaler_current_max_can_override_config` was the one
scaler-sensitive test the earlier sweep missed.

### Relationship to `scrub_gate_env`

The gate spawner
[`scrub_gate_env`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
already `env_clear()`s and re-injects only a deny-by-default allow-list;
`SIMARD_SCALING` is **not** allow-listed, so a *scrub-bearing* candidate binary
strips it before `cargo test` runs. This test fix and the env scrub are
**belt-and-suspenders**:

- **Env scrub** stops the leak at the gate boundary — but only for candidates
  new enough to carry the scrub.
- **Test hermeticity** makes the test correct **regardless of ambient env**, so
  it stays green even when run by hand, in CI, or through an older daemon whose
  gate predates the scrub.

Keeping both is deliberate: neither alone covers the historical daemon that
leaked `SIMARD_SCALING` into a gate whose binary lacked the scrub.

## The contract — scaler-sensitive tests

A test is **scaler-sensitive** if it calls `decide()` (or otherwise depends on
the per-cycle action limit) and asserts on the number of actions or on
`max_concurrent_actions`. Every scaler-sensitive test MUST guarantee, at the
moment it constructs its `OodaConfig`:

- **(S1) The `scaler` field is pinned explicitly** — either `scaler: None` to
  test the raw `max_concurrent_actions` field, or `scaler: Some(<a scaler you
  constructed>)` to test scaler behavior. It must never be left to
  `OodaConfig::default()`.
- **(S2) A `..OodaConfig::default()` spread must not be relied on for any
  env-derived field.** The spread reads `SIMARD_SCALING` (→ `scaler`) and
  `SIMARD_OODA_MAX_CONCURRENT` / `SIMARD_MAX_CONCURRENT_ACTIONS`
  (→ `max_concurrent_actions`) from the ambient env, so it can inject values you
  did not intend. Two conforming forms exist: (a) **full enumeration** with no
  spread — the form this fix uses, preferred for a fix whose whole point is
  hermeticity; or (b) a spread that **explicitly overrides every env-derived
  field** before `..OodaConfig::default()` — as the sibling
  `decide_uses_scaler_adjusted_limit_when_scaler_is_present` and
  `decide_ignores_scaler_when_none` tests do by pinning `scaler:` (and, where the
  count is asserted, `max_concurrent_actions:`). What is never acceptable is
  letting the spread supply a scaler-sensitive field, as the old
  `scaler_current_max_can_override_config` did.
- **(S3) The assertion holds under both `SIMARD_SCALING=auto` and unset.** If
  changing that one env var changes the verdict, the config is not pinned.

These three rules make the outcome a function of the test's own inputs, not of
the machine, CI runner, or daemon that happens to launch it.

## The fix — pin the scaler, drop the spread

`scaler_current_max_can_override_config` now constructs a fully-specified
`OodaConfig` with an explicit pinned scaler and **no** default spread:

```rust
#[test]
fn scaler_current_max_can_override_config() {
    use simard::ooda_loop::{OodaConfig, Priority, decide};

    // Pin the scaler explicitly (ceiling == floor == current == 2) so its
    // adjust() is deterministically 2, independent of SIMARD_SCALING.
    let scaler = std::sync::Arc::new(AdaptiveScaler::new(2, 2, 2));

    // 5 over-quota priorities: would produce 5 actions without a cap.
    let priorities: Vec<Priority> = (1..=5)
        .map(|i| Priority {
            goal_id: format!("g{i}"),
            urgency: 1.0 - (i as f64 * 0.1),
            reason: format!("priority {i}"),
        })
        .collect();

    // Fully specified — NO `..OodaConfig::default()`. All nine OodaConfig
    // fields are enumerated so no env-derived value (scaler, budgets, distill
    // schedule, lesson threshold) can leak in. The config's
    // max_concurrent_actions is 8; the pinned scaler ceiling of 2 must win,
    // proving the scaler overrides max_concurrent_actions regardless of env.
    let config = OodaConfig {
        max_concurrent_actions: 8,
        improvement_threshold: 0.02,
        gym_suite_id: "progressive".to_string(),
        daily_budget_usd: 500.0,
        weekly_budget_usd: 2500.0,
        distill_min_episodes: 25,
        distill_interval_cycles: 50,
        lesson_recurrence_threshold: 2,
        scaler: Some(scaler),
    };

    let actions = decide(&priorities, &config).unwrap();
    assert!(
        actions.len() <= 2,
        "pinned scaler ceiling of 2 must override config max_concurrent_actions=8; got {} actions",
        actions.len()
    );
}
```

The test still proves its original intent — *the scaler's `current_max`
overrides the config field* — but now does so with a ceiling (`2`) that is
pinned in the test rather than inherited (`24`) from the environment.

> Removing the `..OodaConfig::default()` spread means all nine `OodaConfig`
> fields must be listed. A missing or misnamed field is a **compile error**
> caught by `cargo test`, never a silent behavior change — this is the intended
> trade-off: explicit and loud over implicit and env-coupled.

## Reproduce the deploy-gate canary locally

Reproduce the exact failure/fix the deploy gate observes by exporting the
leaking env var and running the same test path the gate shells out to.

**Prove the assertion is env-stable (both must PASS):**

```bash
# Ambient leak present — the historical failure condition.
SIMARD_SCALING=auto cargo test --test adaptive_scaling \
  scaler_current_max_can_override_config

# Ambient leak absent — the clean condition.
cargo test --test adaptive_scaling \
  scaler_current_max_can_override_config
```

Both runs pass with the fix. Run against the old (spread) body, the first
command panics and `cargo test` exits `101` — the canary-reddening signature.

**Prove the full gate command is deterministic (exit 0 every run):**

```bash
# Mirrors run_unit_test_gate's invocation with the worst-case leak set.
for i in 1 2 3; do
  SIMARD_SCALING=auto cargo test --no-fail-fast \
    --manifest-path ./Cargo.toml \
    --target-dir "$(mktemp -d)"
  echo "run $i exit: $?"
done
```

Expect `exit: 0` on all three runs and zero panics.

## Bootstrap / adoption note

The env-scrub in `scrub_gate_env` only protects candidates whose binary
**contains** the scrub. A daemon deployed **before** the scrub landed still
leaks `SIMARD_SCALING` into a pre-scrub gate. Because the test fix cannot
deploy through its own still-leaking gate, the running daemon must be
**relaunched once, manually,** onto a scrub-bearing binary. After that
one-time relaunch, future self-updates flow through a clean gate and the canary
stays green on its own. See the
[stuck-red-canary convergence runbook](../howto/converge-a-stuck-red-canary-self-deploy.md).

## Scope

- **In scope:** hermeticity of the `unit-test` deploy-gate canary path with
  respect to `SIMARD_SCALING`; the `scaler_current_max_can_override_config`
  test.
- **Out of scope:** the `release` workflow's separate duplicate-tag publish
  failure (`release.yml` runs no `cargo test`; a green `unit-test` gate does
  **not** turn `release` green — track that independently); the AIMD algorithm
  itself (unchanged — see the [adaptive-scaling API](../reference/adaptive-scaling-api.md));
  general `verify` changes; broad pipeline refactors.

## Related

- [Writing hermetic tests against cognitive memory](./hermetic-tests.md) — the
  broader hermeticity contract this follows.
- [Deflaking known-flaky tests](./deflaking-known-flaky-tests.md) — how a
  genuinely env-fragile test is classified and fixed rather than retried.
- [Adaptive scaling API reference](../reference/adaptive-scaling-api.md) —
  `AdaptiveScaler`, `SIMARD_SCALING`, and how `decide()` consumes the scaler.
- [Canary gate isolation and self-deploy convergence](../reference/canary-gate-convergence.md)
  — `scrub_gate_env`, the gate allow-list, and fail-closed gate ordering.
