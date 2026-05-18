# Test-coverage baseline

This document records the most recent line-coverage baseline for each Cargo
target group and links each group to the issue that drives it toward the
project-wide ≥ 70% target. Update this file whenever a coverage-targeted PR
lands.

The numbers below come from:

```bash
cargo llvm-cov --no-fail-fast --summary-only
```

Per-group rows are produced by filtering the `Filename` column for the
matching path prefix (e.g. `src/bin/`, `src/operator/`).

## Group: `bin` — `src/bin/*.rs`

Tracking issue: [#1749](https://github.com/rysweet/Simard/issues/1749)
(parent: [#1735](https://github.com/rysweet/Simard/issues/1735))

| Metric             | Baseline (2026-05-14) | After #1749 |
| ------------------ | --------------------: | ----------: |
| Aggregate line cov |                 0.58% |      76.07% |
| Files in group     |                     7 |           7 |
| Lines covered      |               3 / 519 | 839 / 1 103 |

Per-file post-#1749 line coverage:

| File                                  | Line cov | Func cov | Region cov |
| ------------------------------------- | -------: | -------: | ---------: |
| `simard_engineer_loop_recipe.rs`      |   86.25% |  100.00% |     90.38% |
| `simard_engineer_step.rs`             |   74.22% |   76.00% |     68.14% |
| `simard_gym.rs`                       |  100.00% |  100.00% |    100.00% |
| `simard_improve_step.rs`              |   87.46% |   83.33% |     82.05% |
| `simard_ooda_step.rs`                 |   60.36% |   55.17% |     64.79% |
| `simard_operator_probe.rs`            |  100.00% |  100.00% |    100.00% |
| `simard_self_improve_recipe.rs`       |   87.04% |  100.00% |     82.00% |

> `simard_ooda_step.rs` falls below 70% at the file level because its
> `cmd_observe` and `cmd_act` paths are bridge-dependent and require a live
> cognitive-memory / runtime state-root. The acceptance criterion for #1749
> is the **group aggregate**, which sits at 76.07%.

### How to reproduce locally

```bash
cargo llvm-cov --no-fail-fast --summary-only \
  --bin simard-engineer-loop-recipe \
  --bin simard-engineer-step \
  --bin simard-gym \
  --bin simard-improve-step \
  --bin simard-ooda-step \
  --bin simard-self-improve-recipe \
  --test bin_simard_engineer_loop_recipe_cli \
  --test bin_simard_engineer_step_cli \
  --test bin_simard_gym_cli \
  --test bin_simard_improve_step_cli \
  --test bin_simard_ooda_step_cli \
  --test bin_simard_operator_probe_cli \
  --test bin_simard_self_improve_recipe_cli
```

The seven `bin_simard_*` integration tests live under `tests/` and exercise
each CLI's argument-parsing and error-envelope surface deterministically
(no network, no external services).

## Other groups

Tracked, but not yet attacked by a landed PR:

| Group        | Tracking issue                                                  |
| ------------ | --------------------------------------------------------------- |
| `engineer`   | [#1750](https://github.com/rysweet/Simard/issues/1750)          |
| `operator`   | [#1751](https://github.com/rysweet/Simard/issues/1751)          |
| `runtime`    | [#1752](https://github.com/rysweet/Simard/issues/1752)          |
| `meeting`    | [#1753](https://github.com/rysweet/Simard/issues/1753)          |

Update this table as those PRs land.
