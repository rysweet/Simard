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

## Group: `trace_collector` — `src/trace_collector.rs`

Tracking issue: [#1751](https://github.com/rysweet/Simard/issues/1751)
(parent: [#1735](https://github.com/rysweet/Simard/issues/1735))

| Metric             | Baseline (2026-05-14) | After #1751 |
| ------------------ | --------------------: | ----------: |
| Aggregate line cov |                42.68% |      95.51% |
| Files in group     |                     1 |           1 |
| Lines covered      |               35 / 82 |   149 / 156 |

Per-file post-#1751 line coverage:

| File                     | Line cov | Func cov | Region cov |
| ------------------------ | -------: | -------: | ---------: |
| `src/trace_collector.rs` |   95.51% |   90.91% |     95.49% |

> The new unit tests drive a full span lifecycle through `SpanCollectorLayer`
> (`on_new_span` → `on_close`), drain the ring buffer with real records, and
> exercise the `SpanRecord` `Clone` / `Debug` / `Serialize` derives — all
> deterministically (no network, no external services). The line total rises
> from 82 to 156 because the in-file `#[cfg(test)]` module is counted.

### How to reproduce locally

```bash
cargo llvm-cov --lib --summary-only
# then read the `src/trace_collector.rs` row
```

## Group: `operator_commands_gym` — `src/operator_commands_gym/commands.rs`

Tracking issue: [#1752](https://github.com/rysweet/Simard/issues/1752)
(parent: [#1735](https://github.com/rysweet/Simard/issues/1735))

| Metric             | Baseline (current main) | After #1752 |
| ------------------ | ----------------------: | ----------: |
| Aggregate line cov |                  37.44% |      88.63% |
| Files in group     |                       1 |           1 |
| Lines covered      |                79 / 211 |   187 / 211 |

Per-file post-#1752 line coverage:

| File                                      | Line cov | Func cov | Region cov |
| ----------------------------------------- | -------: | -------: | ---------: |
| `src/operator_commands_gym/commands.rs`   |   88.63% |  100.00% |     85.21% |

> Issue #1752 recorded the epic baseline as 42.97% (55 / 128 lines) at commit
> `aa701b18`; the file has since grown, so the directly-comparable pre-PR
> baseline on current `main` is 37.44% (79 / 211 lines, counting the in-file
> `#[cfg(test)]` module).
>
> The new tests drive the two largest uncovered presentation paths
> deterministically: `run_gym_compare` is exercised by seeding two stored
> run-report JSON fixtures on disk and asserting the comparison renders, and
> `run_gym_scenario` is exercised end-to-end through the single-process
> `local-harness` scenario `repo-exploration-local` (the same scenario
> `tests/review.rs` already drives). Both run entirely in-process with
> `InMemory*` stores — no network, no sleeps, no external services, and no
> cognitive-memory writes.
>
> The 24 lines that remain uncovered are `run_gym_suite`'s success path
> (lines 118–147): printing a suite report requires executing the entire
> `starter` suite (every registered scenario) end-to-end, which is too slow
> and live-runtime-heavy to drive from a unit test. Its error path is covered,
> and the group aggregate of 88.63% clears the ≥ 70% target by a wide margin.

### How to reproduce locally

```bash
# File-level number (counts the in-file #[cfg(test)] module):
cargo llvm-cov --lib --summary-only -- operator_commands_gym
# then read the `src/operator_commands_gym/commands.rs` row

# The CLI-surface integration test exercises the same success paths through
# the public entry points (run_gym_compare / dispatch_legacy_gym_cli):
cargo llvm-cov --test operator_commands_gym_cli --summary-only
```

The deterministic CLI-surface integration test lives at
`tests/operator_commands_gym_cli.rs` and mirrors the pattern used for the
`bin` group: it seeds on-disk fixtures and drives the gym commands through
`simard::run_gym_compare` and `simard::dispatch_legacy_gym_cli` (no network,
no external services).

## Other groups

Tracked, but not yet attacked by a landed PR:

| Group        | Tracking issue                                                  |
| ------------ | --------------------------------------------------------------- |
| `engineer`   | [#1750](https://github.com/rysweet/Simard/issues/1750)          |
| `meeting`    | [#1753](https://github.com/rysweet/Simard/issues/1753)          |

Update this table as those PRs land.
