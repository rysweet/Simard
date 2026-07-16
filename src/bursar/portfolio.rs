//! A small but genuinely runnable portfolio backtest / risk / rebalancing
//! engine.
//!
//! The engine models the analytical core the Bursar uses to turn a target
//! allocation into evidence:
//! - a deterministic monthly backtest of each asset class and the blended
//!   portfolio,
//! - a risk report (annualised return, volatility, max drawdown, Sharpe), and
//! - a drift-based rebalancing plan that emits **order proposals only**.
//!
//! Everything is in-memory and deterministic, so it can be scaffolded from a
//! [`PortfolioPlan`](super::mandate::PortfolioPlan) and exercised end-to-end in
//! a test or example without any market-data feed.
//!
//! **Advisory only.** The engine never executes, routes, or simulates the
//! settlement of an order. A [`RebalancePlan`] is a research recommendation; the
//! [`RebalancePlan::advisory_only`] flag is always `true`.

use serde::{Deserialize, Serialize};

use super::BursarError;
use super::mandate::{AllocationSlice, AssetClass, PortfolioPlan, TOTAL_BPS};

/// Annual risk-free rate used for the Sharpe ratio, in basis points.
pub const RISK_FREE_BPS: i32 = 200;

/// Number of trading months modelled per year.
const MONTHS_PER_YEAR: u32 = 12;

/// A single asset position seeded from the target allocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Position {
    class: AssetClass,
    target_weight_bps: u32,
    /// Current market value of the position, in cents.
    value_cents: f64,
}

/// The runnable portfolio engine.
#[derive(Clone, Debug)]
pub struct PortfolioEngine {
    positions: Vec<Position>,
}

/// The month-by-month result of a backtest run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Backtest {
    /// Number of monthly periods simulated.
    pub months: u32,
    /// Portfolio value (cents) at the end of each month, in order.
    pub value_path_cents: Vec<u64>,
    /// Portfolio total return for each month (fractional, e.g. 0.01 = +1%).
    pub monthly_returns: Vec<f64>,
    /// Realised weights (bps) at the end of the backtest, after drift.
    pub end_weights: Vec<AllocationSlice>,
    pub start_value_cents: u64,
    pub end_value_cents: u64,
}

/// Portfolio-level risk metrics measured from a backtest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskMetrics {
    /// Annualised (geometric) return, in basis points.
    pub annualized_return_bps: i32,
    /// Annualised volatility of monthly returns, in basis points.
    pub annualized_volatility_bps: u32,
    /// Maximum peak-to-trough drawdown over the path, in basis points of the
    /// peak (0..=10000).
    pub max_drawdown_bps: u32,
    /// Sharpe ratio against [`RISK_FREE_BPS`], rounded to 1/100ths.
    pub sharpe_ratio: f64,
}

/// The action side of a rebalance proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RebalanceAction {
    Buy,
    Sell,
}

impl RebalanceAction {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

/// A single proposed rebalancing trade. **A recommendation, never executed.**
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebalanceOrder {
    pub class: AssetClass,
    pub action: RebalanceAction,
    /// Size of the adjustment, in basis points of the total portfolio.
    pub delta_bps: u32,
}

/// A drift-based rebalancing recommendation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebalancePlan {
    /// Drift tolerance band that triggered the proposals, in basis points.
    pub tolerance_bps: u32,
    /// Whether any position drifted beyond the tolerance band.
    pub drifted: bool,
    pub orders: Vec<RebalanceOrder>,
    /// Always `true`: the Bursar recommends, it never executes.
    pub advisory_only: bool,
}

impl PortfolioEngine {
    /// Seed an engine from a target allocation, splitting the initial capital by
    /// the plan's target weights.
    #[must_use]
    pub fn from_plan(plan: &PortfolioPlan) -> Self {
        let capital = plan.brief.initial_capital_cents;
        let positions = plan
            .allocation
            .slices
            .iter()
            .map(|slice| Position {
                class: slice.class,
                target_weight_bps: slice.weight_bps,
                value_cents: value_for_weight(capital, slice.weight_bps),
            })
            .collect();
        Self { positions }
    }

    /// Number of asset positions in the portfolio.
    #[must_use]
    pub fn position_count(&self) -> usize {
        self.positions.len()
    }

    /// Run a deterministic monthly backtest over `months` periods.
    ///
    /// Each asset class compounds a deterministic monthly return derived from
    /// its capital-market assumptions plus a bounded, seed-stable shock, so the
    /// same plan always produces the same path.
    #[must_use]
    pub fn backtest(&self, months: u32) -> Backtest {
        let months = months.max(1);
        let mut values: Vec<f64> = self.positions.iter().map(|p| p.value_cents).collect();
        let start_value_cents = round_cents(values.iter().sum());

        let mut value_path_cents = Vec::with_capacity(months as usize);
        let mut monthly_returns = Vec::with_capacity(months as usize);

        for month in 0..months {
            let prev_total: f64 = values.iter().sum();
            for (pos, value) in self.positions.iter().zip(values.iter_mut()) {
                let r = monthly_return(pos.class, month);
                *value *= 1.0 + r;
            }
            let new_total: f64 = values.iter().sum();
            let period_return = if prev_total > 0.0 {
                new_total / prev_total - 1.0
            } else {
                0.0
            };
            monthly_returns.push(period_return);
            value_path_cents.push(round_cents(new_total));
        }

        let final_total: f64 = values.iter().sum();
        let end_weights = realized_weights(&self.positions, &values, final_total);

        Backtest {
            months,
            value_path_cents,
            monthly_returns,
            end_weights,
            start_value_cents,
            end_value_cents: round_cents(final_total),
        }
    }

    /// Compute portfolio-level risk metrics from a backtest.
    #[must_use]
    pub fn risk_metrics(&self, backtest: &Backtest) -> RiskMetrics {
        let start = backtest.start_value_cents.max(1) as f64;
        let end = backtest.end_value_cents as f64;
        let years = f64::from(backtest.months) / f64::from(MONTHS_PER_YEAR);

        let annualized_return = if years > 0.0 && start > 0.0 {
            (end / start).powf(1.0 / years) - 1.0
        } else {
            0.0
        };

        let monthly_vol = std_dev(&backtest.monthly_returns);
        let annualized_vol = monthly_vol * f64::from(MONTHS_PER_YEAR).sqrt();

        let max_drawdown = max_drawdown_fraction(&backtest.value_path_cents);

        let risk_free = f64::from(RISK_FREE_BPS) / f64::from(TOTAL_BPS);
        let sharpe = if annualized_vol > 0.0 {
            (annualized_return - risk_free) / annualized_vol
        } else {
            0.0
        };

        RiskMetrics {
            annualized_return_bps: to_bps_i32(annualized_return),
            annualized_volatility_bps: to_bps_u32(annualized_vol),
            max_drawdown_bps: to_bps_u32(max_drawdown),
            sharpe_ratio: round_2dp(sharpe),
        }
    }

    /// Build a drift-based rebalancing recommendation from a backtest's realised
    /// weights. Positions that drifted beyond `tolerance_bps` from target get a
    /// proposed buy/sell that would restore the target. **Advisory only.**
    #[must_use]
    pub fn rebalance_plan(&self, backtest: &Backtest, tolerance_bps: u32) -> RebalancePlan {
        let mut orders = Vec::new();
        let mut drifted = false;

        for pos in &self.positions {
            let realized = backtest
                .end_weights
                .iter()
                .find(|s| s.class == pos.class)
                .map_or(0, |s| s.weight_bps);
            let target = pos.target_weight_bps;
            let (action, delta) = if realized > target {
                (RebalanceAction::Sell, realized - target)
            } else {
                (RebalanceAction::Buy, target - realized)
            };
            if delta > tolerance_bps {
                drifted = true;
                orders.push(RebalanceOrder {
                    class: pos.class,
                    action,
                    delta_bps: delta,
                });
            }
        }

        RebalancePlan {
            tolerance_bps,
            drifted,
            orders,
            advisory_only: true,
        }
    }

    /// Apply a rebalance plan's proposals to the realised weights and return the
    /// resulting weights (bps). Used only to **verify** that the proposals would
    /// restore the target; it moves no money and executes nothing.
    #[must_use]
    pub fn post_rebalance_weights(
        &self,
        backtest: &Backtest,
        plan: &RebalancePlan,
    ) -> Vec<AllocationSlice> {
        backtest
            .end_weights
            .iter()
            .map(|slice| {
                let mut weight = i64::from(slice.weight_bps);
                for order in &plan.orders {
                    if order.class == slice.class {
                        match order.action {
                            RebalanceAction::Buy => weight += i64::from(order.delta_bps),
                            RebalanceAction::Sell => weight -= i64::from(order.delta_bps),
                        }
                    }
                }
                AllocationSlice {
                    class: slice.class,
                    weight_bps: u32::try_from(weight.max(0)).unwrap_or(0),
                }
            })
            .collect()
    }
}

/// Validate that a backtest is internally consistent.
///
/// # Errors
/// Returns [`BursarError::BacktestFailed`] if the value path length does not
/// match the month count or the portfolio was wiped out (non-positive value).
pub fn validate_backtest(backtest: &Backtest) -> Result<(), BursarError> {
    if backtest.value_path_cents.len() != backtest.months as usize {
        return Err(BursarError::BacktestFailed {
            reason: format!(
                "value path has {} points but {} months were requested",
                backtest.value_path_cents.len(),
                backtest.months
            ),
        });
    }
    if backtest.end_value_cents == 0 {
        return Err(BursarError::BacktestFailed {
            reason: "portfolio ended with zero value".to_string(),
        });
    }
    Ok(())
}

fn value_for_weight(capital_cents: u64, weight_bps: u32) -> f64 {
    capital_cents as f64 * f64::from(weight_bps) / f64::from(TOTAL_BPS)
}

fn realized_weights(positions: &[Position], values: &[f64], total: f64) -> Vec<AllocationSlice> {
    positions
        .iter()
        .zip(values.iter())
        .map(|(pos, value)| {
            let weight = if total > 0.0 {
                (value / total * f64::from(TOTAL_BPS)).round()
            } else {
                0.0
            };
            AllocationSlice {
                class: pos.class,
                weight_bps: to_bps_u32_raw(weight),
            }
        })
        .collect()
}

/// Deterministic monthly total return for an asset class in a given month.
///
/// Combines the class's expected monthly return with a bounded pseudo-random
/// shock scaled by its monthly volatility. The shock is seeded from the class
/// and month index, so runs are reproducible.
fn monthly_return(class: AssetClass, month: u32) -> f64 {
    let drift =
        f64::from(class.expected_return_bps()) / f64::from(TOTAL_BPS) / f64::from(MONTHS_PER_YEAR);
    let monthly_vol = f64::from(class.volatility_bps())
        / f64::from(TOTAL_BPS)
        / f64::from(MONTHS_PER_YEAR).sqrt();
    let shock = deterministic_shock(class, month);
    drift + monthly_vol * shock
}

/// A deterministic shock in roughly [-1.7, 1.7] derived from an integer seed.
fn deterministic_shock(class: AssetClass, month: u32) -> f64 {
    // A tiny splitmix64-style hash → uniform in [0,1), then mapped to a
    // symmetric, mean-zero shape via two draws (a bounded Bates-like average).
    let base = seed_for(class);
    let u1 = unit_from_seed(base.wrapping_add(u64::from(month).wrapping_mul(0x9E37_79B9)));
    let u2 = unit_from_seed(
        base.wrapping_add(u64::from(month).wrapping_mul(0x85EB_CA6B))
            .wrapping_add(1),
    );
    // Average of two uniforms is mean 0.5; center and scale to widen the range.
    ((u1 + u2) - 1.0) * 1.7
}

fn seed_for(class: AssetClass) -> u64 {
    class
        .code()
        .bytes()
        .fold(0xCBF2_9CE4_8422_2325_u64, |acc, b| {
            (acc ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3)
        })
}

fn unit_from_seed(seed: u64) -> f64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Top 53 bits → [0, 1).
    (z >> 11) as f64 / ((1_u64 << 53) as f64)
}

fn std_dev(samples: &[f64]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    let variance = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    variance.max(0.0).sqrt()
}

fn max_drawdown_fraction(path: &[u64]) -> f64 {
    let mut peak = 0_f64;
    let mut max_dd = 0_f64;
    for &v in path {
        let v = v as f64;
        if v > peak {
            peak = v;
        }
        if peak > 0.0 {
            let dd = (peak - v) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }
    max_dd
}

fn round_cents(value: f64) -> u64 {
    if value <= 0.0 {
        0
    } else {
        value.round() as u64
    }
}

fn round_2dp(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Convert a fractional rate to signed basis points.
fn to_bps_i32(fraction: f64) -> i32 {
    (fraction * f64::from(TOTAL_BPS)).round() as i32
}

/// Convert a non-negative fractional rate to basis points (floored at 0).
fn to_bps_u32(fraction: f64) -> u32 {
    let bps = (fraction.max(0.0) * f64::from(TOTAL_BPS)).round();
    to_bps_u32_raw(bps)
}

fn to_bps_u32_raw(bps: f64) -> u32 {
    if bps <= 0.0 {
        0
    } else if bps >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        bps as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bursar::mandate::{InvestmentBrief, RiskTolerance, design_allocation};

    fn sample_plan(risk: RiskTolerance) -> PortfolioPlan {
        let brief = InvestmentBrief::new("Test", "obj", risk, 10, 10_000_000, vec![]);
        design_allocation(&brief).unwrap()
    }

    #[test]
    fn engine_splits_capital_by_target_weight() {
        let plan = sample_plan(RiskTolerance::Balanced);
        let engine = PortfolioEngine::from_plan(&plan);
        assert_eq!(engine.position_count(), plan.allocation.slices.len());
    }

    #[test]
    fn backtest_is_deterministic() {
        let plan = sample_plan(RiskTolerance::Growth);
        let engine = PortfolioEngine::from_plan(&plan);
        let a = engine.backtest(120);
        let b = engine.backtest(120);
        assert_eq!(a, b);
    }

    #[test]
    fn backtest_path_length_matches_months() {
        let plan = sample_plan(RiskTolerance::Aggressive);
        let engine = PortfolioEngine::from_plan(&plan);
        let bt = engine.backtest(60);
        assert_eq!(bt.months, 60);
        assert_eq!(bt.value_path_cents.len(), 60);
        assert_eq!(bt.monthly_returns.len(), 60);
        assert!(validate_backtest(&bt).is_ok());
    }

    #[test]
    fn end_weights_sum_close_to_total() {
        let plan = sample_plan(RiskTolerance::Balanced);
        let engine = PortfolioEngine::from_plan(&plan);
        let bt = engine.backtest(240);
        let sum: u32 = bt.end_weights.iter().map(|s| s.weight_bps).sum();
        // Rounding to whole bps can leave a tiny residual; must be within a few.
        assert!(
            (i64::from(sum) - i64::from(TOTAL_BPS)).abs() <= 5,
            "sum was {sum}"
        );
    }

    #[test]
    fn risk_metrics_are_sane() {
        let plan = sample_plan(RiskTolerance::Growth);
        let engine = PortfolioEngine::from_plan(&plan);
        let bt = engine.backtest(180);
        let risk = engine.risk_metrics(&bt);
        assert!(risk.annualized_volatility_bps > 0);
        assert!(risk.max_drawdown_bps <= TOTAL_BPS);
        assert!(risk.sharpe_ratio.is_finite());
    }

    #[test]
    fn rebalance_orders_net_to_zero() {
        let plan = sample_plan(RiskTolerance::Aggressive);
        let engine = PortfolioEngine::from_plan(&plan);
        let bt = engine.backtest(360);
        let rebal = engine.rebalance_plan(&bt, 100);
        assert!(rebal.advisory_only);
        let buys: i64 = rebal
            .orders
            .iter()
            .filter(|o| o.action == RebalanceAction::Buy)
            .map(|o| i64::from(o.delta_bps))
            .sum();
        let sells: i64 = rebal
            .orders
            .iter()
            .filter(|o| o.action == RebalanceAction::Sell)
            .map(|o| i64::from(o.delta_bps))
            .sum();
        // Drift is zero-sum across the portfolio, so buy pressure ≈ sell
        // pressure within whole-bps rounding.
        assert!((buys - sells).abs() <= 5, "buys={buys} sells={sells}");
    }

    #[test]
    fn post_rebalance_restores_target_within_band() {
        let plan = sample_plan(RiskTolerance::Growth);
        let engine = PortfolioEngine::from_plan(&plan);
        let bt = engine.backtest(360);
        let rebal = engine.rebalance_plan(&bt, 50);
        let restored = engine.post_rebalance_weights(&bt, &rebal);
        for slice in &plan.allocation.slices {
            let after = restored
                .iter()
                .find(|s| s.class == slice.class)
                .map_or(0, |s| s.weight_bps);
            assert!(
                (i64::from(after) - i64::from(slice.weight_bps)).abs()
                    <= i64::from(rebal.tolerance_bps) + 5,
                "class {:?} after={after} target={}",
                slice.class,
                slice.weight_bps
            );
        }
    }

    #[test]
    fn no_drift_yields_no_orders() {
        let plan = sample_plan(RiskTolerance::Conservative);
        let engine = PortfolioEngine::from_plan(&plan);
        let bt = engine.backtest(1);
        // Over a single month drift is tiny; a very wide band means no orders.
        let rebal = engine.rebalance_plan(&bt, TOTAL_BPS);
        assert!(!rebal.drifted);
        assert!(rebal.orders.is_empty());
    }
}
