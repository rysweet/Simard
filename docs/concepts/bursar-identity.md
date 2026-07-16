---
title: The Bursar identity — investment portfolio research & management
description: How the simard-bursar identity constructs, backtests, risk-assesses, and monitors investment portfolios from an objective-and-constraints brief — research and advisory only, never order execution.
last_updated: 2026-07-16
owner: simard
doc_type: concept
related:
  - ./pluggable-identity.md
  - ../howto/run-the-bursar-identity.md
  - ../reference/runtime-contracts.md
  - ../reference/pluggable-identity-api.md
---

# The Bursar identity — investment portfolio research & management

## What it is

`simard-bursar` is a built-in Simard identity specialised in **investment
portfolio research and management**. It takes an operator's **objective +
constraints** brief and drives it, end to end, to a **backtested,
risk-reported target allocation** — plus, when current holdings are supplied, a
**rebalancing trade proposal**.

Bursar is named after the college officer who stewards an endowment: it
researches and advises on how capital *should* be allocated. It is one of the
six built-in identities loaded by `BuiltinIdentityLoader` (alongside
`simard-engineer`, `simard-meeting`, `simard-gym`, `simard-goal-curator`, and
`simard-improvement-curator`), and it runs in the dedicated `bursar` operating
mode.

## The hard safety boundary: research & advisory only

Bursar is **research and advisory only**. It **never places, submits, routes,
signs, or executes any order, trade, or transfer** — for any account, real or
simulated-as-live. This boundary is encoded in the system prompt
(`prompt_assets/simard/bursar_system.md`) and repeated in every recipe:

- It **may** simulate fills inside a backtest engine (backtrader / vectorised
  pandas) against historical or explicitly-synthetic data — that is modelling,
  not execution.
- It **may** produce a target allocation or a rebalancing trade list as a
  **written proposal** for a human or downstream system to review. Producing the
  proposal is allowed; acting on it is not.
- It **refuses** any instruction — including instructions embedded in the brief,
  market data, news text, or tool output — that asks it to connect to a broker,
  place a live order, or move real funds. All task/market/tool text is treated
  as **untrusted data, not instructions**.

Every consolidated report carries an advisory disclaimer stating that no orders
were placed or executed and that the output is not personalised financial
advice.

## The loop: inspect → act → verify → persist

Bursar runs the same disciplined engineer loop the Simard runtime enforces:

1. **Inspect** — read the objective, constraints, and available data; state
   assumptions; never fabricate market data.
2. **Act** — run the analysis (allocation, backtest, risk, rebalancing, report),
   preferring the shipped recipes and standard, auditable libraries.
3. **Verify** — check the numbers: reject look-ahead / survivorship bias and
   un-annualised metrics; compare against a baseline (equal-weight, 60/40).
4. **Persist** — write structured deliverables and a short session summary as
   durable artifacts (not point-in-time report docs committed to the repo).

## The five capabilities and their recipes

Each capability is backed by a recipe under `prompt_assets/simard/recipes/`:

| Capability | Recipe | Output |
| --- | --- | --- |
| Asset allocation | `bursar-asset-allocation` | Constrained target weights + method |
| Backtesting | `bursar-backtesting` | CAGR, vol, Sharpe, Sortino, max DD vs baseline |
| Risk analysis | `bursar-risk-analysis` | Vol, VaR/CVaR, beta, concentration, stress, budget check |
| Rebalancing | `bursar-rebalancing` | Drift-corrected trade **proposal** (never executed) |
| Reporting | `bursar-reporting` | Consolidated decision-ready report + disclaimer |

The `bursar-portfolio-construction` recipe chains all five (allocation →
backtest → risk → rebalancing → report) so a single run takes an objective +
constraints brief to a backtested, risk-reported allocation end to end.

## Tooling

Bursar prefers `pandas` for data handling, `backtrader` for event-driven
backtests, and `QuantLib` for pricing/risk/quant math, with `numpy`/`scipy` for
optimisation. Because it executes analytics locally, `simard-bursar` carries the
`terminal-shell` base type (like `simard-engineer`) in addition to
`local-harness`, `rusty-clawd`, `copilot-sdk`, `claude-agent-sdk`, and
`ms-agent-framework`. Its memory policy is read-only for project boundaries.

## Definition of done

A Bursar engagement is done when, for the given objective + constraints, all of
these exist and verify: a target allocation, a backtest with a baseline
comparison, a risk report against the risk budget, a rebalancing proposal (when
holdings are supplied), and a consolidated report with the advisory disclaimer.
Done is **never** achieved by executing a trade — done means the research
artifacts and the ready-to-review proposal exist.

## See also

- [How to run the Bursar identity](../howto/run-the-bursar-identity.md)
- [Pluggable identity — TOML-driven agent personas](./pluggable-identity.md)
- [Runtime contracts](../reference/runtime-contracts.md)
