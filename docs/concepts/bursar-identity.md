# Bursar Identity — Investment Portfolio Research & Management

The **Bursar** is a pluggable Simard identity (`simard-bursar`) for the
investment-research domain. It does two jobs, in order:

1. **Construct the portfolio** — from an objective + constraints brief, produce a
   concrete mandate and a target asset allocation whose weights sum to 100%.
2. **Prove it with evidence** — backtest the allocation, produce a risk report
   (annualized return, volatility, max drawdown, Sharpe), and generate a
   drift-based rebalancing recommendation.

It is *done* when it can take an **objective + constraints brief to a
backtested, risk-reported allocation end-to-end** — a fully-invested target
allocation, a backtest over the horizon that retains value, a risk report, and a
rebalancing plan, with invariants verified.

## Research/advisory only — never order execution

The Bursar is **research/advisory only**. It never places, routes, simulates the
settlement of, or executes an order. The runnable core enforces this:
`BursarOutcome::order_execution_performed` is always `false`, and every run
verifies the advisory-only invariant. The operator probe prints
`Order execution: none (advisory only)`.

## Where it lives

| Surface | Location |
|---|---|
| Identity | `simard-bursar` in `src/identity/loader.rs` (mode: `orchestrator`) |
| Runnable domain module | `src/bursar/` (`mandate`, `portfolio`, orchestrator) |
| System prompt | `prompt_assets/simard/bursar_system.md` |
| Allocation / risk prompts | `prompt_assets/simard/bursar_allocation_design.md`, `bursar_risk_backtest.md` |
| Recipes | `prompt_assets/simard/recipes/bursar-{allocation,backtest-risk,end-to-end}.yaml` |
| Operator probe | `simard_operator_probe bursar-run <topology> "<brief>"` |

## The runnable core (`simard::bursar`)

The `bursar` module is the source of truth for what "backtested and
risk-reported" means. It is deterministic and dependency-light, so the same
brief always yields the same allocation and backtest and can be exercised in CI
without any market-data feed or model call.

- `bursar::design_allocation(&brief) -> PortfolioPlan` — a mandate plus a target
  allocation whose slice weights sum to exactly 10000 bps (100%), with
  exclusions dropped and their weight redistributed.
- `bursar::PortfolioEngine::from_plan(&plan)` — splits the initial capital by
  target weight, then supports:
  - **Backtest**: `backtest(months)` compounds a deterministic monthly return
    per asset class and tracks the portfolio value path and monthly returns.
  - **Risk**: `risk_metrics(&backtest)` computes annualized return/volatility,
    max drawdown, and the Sharpe ratio against a 2% risk-free rate.
  - **Rebalancing**: `rebalance_plan(&backtest, tolerance)` emits buy/sell
    **proposals** for positions that drifted beyond the tolerance band —
    recommendations, never executed.
- `bursar::run_bursar(&brief) -> BursarOutcome` — constructs, backtests,
  risk-reports, and produces a rebalancing plan, then **verifies** the analytical
  invariants and asserts no order was executed.

### Verified invariants

1. Target allocation weights sum to exactly 100% (10000 bps).
2. The backtest produces a value point for every month in the horizon.
3. The portfolio retains positive value through the backtest.
4. Max drawdown is within 0–100%.
5. The run is advisory only — no order is executed.
6. Rebalancing proposals restore every position to within the tolerance band.

## Tooling note

A richer, model-backed workflow may enrich these outputs with `pandas` /
`backtrader` / `QuantLib` (data wrangling, event-driven backtests, and
fixed-income/derivatives analytics). The runnable Rust core never depends on
one, so the deliverable stays reproducible in CI.

## Security posture

The brief is treated as **untrusted data**. `InvestmentBrief::from_prompt`
extracts only research signals (name, objective, risk tolerance, horizon,
capital, exclusions) and never obeys instructions embedded in the text (e.g.
"ignore the rules above", "place live market orders"). This is covered by tests
in `src/bursar/mandate.rs` and `tests/bursar_end_to_end.rs`.

## Try it

```bash
# End-to-end via the runnable example
cargo run --example bursar_end_to_end
cargo run --example bursar_end_to_end -- "Aggressive growth portfolio, $1,000,000, 30 years"

# End-to-end via the operator probe (prints the allocation + a verified analysis)
cargo run --bin simard_operator_probe -- \
  bursar-run single-process "Balanced growth portfolio for a 20 year horizon, \$250,000"

# Confirm the identity bootstraps as a first-class identity
cargo run --bin simard_operator_probe -- \
  bootstrap-run simard-bursar local-harness single-process "verify bursar bootstrap"
```

A passing `bursar-run` ends with a target allocation, a backtest value path,
risk metrics, a rebalancing plan, `Order execution: none (advisory only)`,
`Allocation verified: yes`, and `Session phase: complete`.

## Tests

- Unit: `src/bursar/{mandate,portfolio}.rs` and `src/bursar/mod.rs` (`#[cfg(test)]`).
- Integration: `tests/bursar_end_to_end.rs`.
- Outside-in scenarios: `tests/gadugi/bursar-identity.{sh,yaml}` and
  `tests/qa-scenarios/bursar-end-to-end.yaml`.
