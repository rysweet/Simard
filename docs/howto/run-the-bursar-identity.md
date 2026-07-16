---
title: How to run the Bursar identity
description: Bootstrap the simard-bursar identity and drive an objective-and-constraints brief to a backtested, risk-reported portfolio allocation using the Bursar recipes — research and advisory only, never order execution.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/bursar-identity.md
  - ../concepts/pluggable-identity.md
  - ../reference/runtime-contracts.md
  - ../reference/simard-cli.md
---

# How to run the Bursar identity

The `simard-bursar` identity researches and manages investment portfolios. This
guide bootstraps it and runs its recipes to take an **objective + constraints**
brief to a **backtested, risk-reported allocation**.

!!! warning "Research & advisory only"
    Bursar **never** places, submits, routes, or executes any order or trade. It
    produces target allocations, backtests, risk reports, and trade **proposals**
    for a human or downstream system to review. It refuses any instruction — even
    one embedded in the brief or market data — that asks it to cross the
    execution boundary. See [The Bursar identity](../concepts/bursar-identity.md).

## Prerequisites

- The Simard binary built: `cargo build --quiet`.
- Analytics libraries available to the session's tool environment for real runs:
  `pandas`, `backtrader`, `QuantLib`, `numpy`/`scipy`.

## 1. Verify the identity bootstraps

`simard-bursar` is a built-in identity, so no `identity.toml` is required. Use
the operator probe to bootstrap it and confirm the runtime resolves it:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  bootstrap-run simard-bursar local-harness single-process \
  "verify bursar identity bootstrap"
```

Expected output includes:

```text
Probe mode: bootstrap-run
Identity: simard-bursar
Selected base type: local-harness
Topology: single-process
Session phase: complete
Shutdown: stopped
```

For real analytics work, select the `terminal-shell` base type so the session
can run `pandas`/`backtrader`/`QuantLib`:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  bootstrap-run simard-bursar terminal-shell single-process \
  "construct a backtested, risk-reported allocation"
```

## 2. Write the objective + constraints brief

Bursar starts from a brief with two parts:

- **Objective** — what the portfolio is for (e.g. "maximise risk-adjusted return
  vs. a 60/40 benchmark over 10 years").
- **Constraints** — investable universe, currency, min/max weight per asset or
  sector, ESG exclusions, turnover cadence, risk budget (max drawdown /
  volatility target), leverage (none unless explicitly permitted), and horizon.

If a required constraint is missing, Bursar asks for it or states a clearly
labelled default rather than silently inventing one.

## 3. Run the end-to-end recipe

The `bursar-portfolio-construction` recipe chains all five capabilities. It runs
through the recipe runner, passing the brief and output file paths as context:

```bash
amplihack recipe run bursar-portfolio-construction \
  -c objective="Maximise risk-adjusted return vs a 60/40 benchmark over 10 years" \
  -c constraints="US-listed ETFs; USD; max 30% per asset; no leverage; max drawdown 20%" \
  -c universe="VTI, VXUS, BND, VNQ, GLD" \
  -c data_window="2014-01-01..2024-12-31" \
  -c risk_budget="max volatility 12%, max drawdown 20%" \
  -c allocation_output=/tmp/bursar/allocation.json \
  -c backtest_output=/tmp/bursar/backtest.json \
  -c risk_output=/tmp/bursar/risk.json \
  -c rebalance_output=/tmp/bursar/rebalance.json \
  -c report_output=/tmp/bursar/report.md
```

Each step writes its structured result to the given `*_output` file and the next
step reads it back by path. When the run completes, `/tmp/bursar/report.md`
contains the consolidated, decision-ready report with the advisory disclaimer.

## 4. Run individual capabilities

You can also run each capability on its own:

| Capability | Recipe |
| --- | --- |
| Asset allocation | `bursar-asset-allocation` |
| Backtesting | `bursar-backtesting` |
| Risk analysis | `bursar-risk-analysis` |
| Rebalancing (proposal only) | `bursar-rebalancing` |
| Reporting | `bursar-reporting` |

For example, to compute just an allocation:

```bash
amplihack recipe run bursar-asset-allocation \
  -c objective="Preserve capital with modest income" \
  -c constraints="max 40% equities; investment-grade bonds only; USD" \
  -c universe="BND, VTI, VTIP, VMBS" \
  -c method="min-variance" \
  -c allocation_output=/tmp/bursar/allocation.json
```

## Definition of done

An engagement is done when the allocation, backtest, risk report, optional
rebalancing proposal, and consolidated report all exist and verify — **not** by
executing a trade.

## See also

- [The Bursar identity](../concepts/bursar-identity.md)
- [Pluggable identity](../concepts/pluggable-identity.md)
- [Simard CLI reference](../reference/simard-cli.md)
