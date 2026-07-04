//! M4 — bounded self-tuning of the `SIMARD_OVERSEER_*` thresholds within
//! **clamped floors/ceilings**. The Overseer may adapt its own sensitivity, but
//! never off a cliff: every knob is clamped, so self-tuning can never drive a
//! threshold to zero (a hot loop) or to infinity (blindness).
//!
//! This is deliberately pure — the tuning logic has no I/O — so "stays within
//! clamps" and "no unbounded growth" are exhaustively unit-tested. Applying the
//! tuned values (as env overrides) is the caller's job.

use crate::overseer::config::{DEFAULT_OVERSEER_INTERVAL_SECS, MIN_OVERSEER_INTERVAL_SECS};

/// A single tunable knob clamped to `[floor, ceil]`. `raise`/`lower` step by
/// `step` and saturate at the bounds — never beyond.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClampedKnob {
    value: f64,
    floor: f64,
    ceil: f64,
    step: f64,
}

impl ClampedKnob {
    /// Construct, clamping `value` into `[floor, ceil]`. `floor <= ceil` and
    /// `step > 0` are assumed; a degenerate range collapses to `floor`.
    pub fn new(value: f64, floor: f64, ceil: f64, step: f64) -> Self {
        let (floor, ceil) = if floor <= ceil {
            (floor, ceil)
        } else {
            (ceil, floor)
        };
        Self {
            value: value.clamp(floor, ceil),
            floor,
            ceil,
            step: step.abs(),
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn floor(&self) -> f64 {
        self.floor
    }

    pub fn ceil(&self) -> f64 {
        self.ceil
    }

    /// Raise by one step, saturating at the ceiling.
    pub fn raise(&mut self) -> f64 {
        self.value = (self.value + self.step).min(self.ceil);
        self.value
    }

    /// Lower by one step, saturating at the floor.
    pub fn lower(&mut self) -> f64 {
        self.value = (self.value - self.step).max(self.floor);
        self.value
    }

    /// Tune in response to feedback. For a *threshold* knob, raising it reduces
    /// noise (fires less), lowering it increases sensitivity.
    pub fn tune(&mut self, feedback: Feedback) {
        match feedback {
            Feedback::TooNoisy => {
                self.raise();
            }
            Feedback::TooQuiet => {
                self.lower();
            }
            Feedback::Stable => {}
        }
    }

    /// Invariant: the value is always within `[floor, ceil]`.
    pub fn within_clamps(&self) -> bool {
        self.value >= self.floor && self.value <= self.ceil
    }
}

/// Signal-quality feedback that drives a tuning step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    /// Too many low-value signals (false positives) — reduce sensitivity.
    TooNoisy,
    /// Real problems slipped through — increase sensitivity.
    TooQuiet,
    /// Signal quality is good — hold.
    Stable,
}

/// The Overseer's tunable thresholds, each clamped. Floors/ceilings are derived
/// from the config knobs so tuning can never diverge from the shipped bounds
/// (e.g. the observer cadence can never dip below `MIN_OVERSEER_INTERVAL_SECS`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverseerTuning {
    /// Distillation parse-failure percentage that raises a signal.
    pub distill_pct: ClampedKnob,
    /// Observer cadence (seconds), clamped to the config floor.
    pub interval_secs: ClampedKnob,
    /// Fraction of the daily budget at which budget pressure fires.
    pub budget_fraction: ClampedKnob,
}

impl Default for OverseerTuning {
    fn default() -> Self {
        Self {
            distill_pct: ClampedKnob::new(20.0, 10.0, 60.0, 5.0),
            interval_secs: ClampedKnob::new(
                DEFAULT_OVERSEER_INTERVAL_SECS as f64,
                MIN_OVERSEER_INTERVAL_SECS as f64,
                3600.0,
                300.0,
            ),
            budget_fraction: ClampedKnob::new(0.8, 0.5, 0.95, 0.05),
        }
    }
}

impl OverseerTuning {
    /// Apply one feedback step to the sensitivity knobs. A noisy observer both
    /// raises its firing threshold AND lengthens its cadence; a too-quiet one
    /// does the reverse — always within clamps.
    pub fn apply(&mut self, feedback: Feedback) {
        self.distill_pct.tune(feedback);
        self.interval_secs.tune(feedback);
        // Budget fraction is deliberately conservative: only widen (raise) it
        // under noise, never lower it below its floor.
        if feedback == Feedback::TooNoisy {
            self.budget_fraction.raise();
        }
    }

    /// True iff every knob is within its clamps (a total invariant).
    pub fn within_clamps(&self) -> bool {
        self.distill_pct.within_clamps()
            && self.interval_secs.within_clamps()
            && self.budget_fraction.within_clamps()
    }

    /// The tuned observer cadence in whole seconds (clamped ≥ floor).
    pub fn interval_secs_u64(&self) -> u64 {
        self.interval_secs.value().round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn new_clamps_out_of_range_input() {
        assert!(approx(ClampedKnob::new(-5.0, 0.0, 10.0, 1.0).value(), 0.0));
        assert!(approx(ClampedKnob::new(99.0, 0.0, 10.0, 1.0).value(), 10.0));
        // Reversed bounds are normalised.
        let k = ClampedKnob::new(5.0, 10.0, 0.0, 1.0);
        assert!(approx(k.floor(), 0.0) && approx(k.ceil(), 10.0));
    }

    #[test]
    fn raise_never_exceeds_ceiling_no_unbounded_growth() {
        let mut k = ClampedKnob::new(20.0, 10.0, 60.0, 5.0);
        for _ in 0..1000 {
            k.raise();
        }
        assert!(approx(k.value(), 60.0), "saturates at the ceiling");
        assert!(k.within_clamps());
    }

    #[test]
    fn lower_never_drops_below_floor() {
        let mut k = ClampedKnob::new(20.0, 10.0, 60.0, 5.0);
        for _ in 0..1000 {
            k.lower();
        }
        assert!(approx(k.value(), 10.0), "saturates at the floor");
        assert!(k.within_clamps());
    }

    #[test]
    fn tune_direction_matches_feedback() {
        let mut k = ClampedKnob::new(20.0, 10.0, 60.0, 5.0);
        k.tune(Feedback::TooNoisy);
        assert!(approx(k.value(), 25.0), "noisy raises the threshold");
        k.tune(Feedback::TooQuiet);
        assert!(approx(k.value(), 20.0), "quiet lowers it back");
        k.tune(Feedback::Stable);
        assert!(approx(k.value(), 20.0), "stable holds");
    }

    #[test]
    fn overseer_tuning_stays_within_clamps_under_any_sequence() {
        let mut t = OverseerTuning::default();
        assert!(t.within_clamps());
        // Hammer it with a long, adversarial feedback sequence.
        let seq = [
            Feedback::TooNoisy,
            Feedback::TooNoisy,
            Feedback::TooQuiet,
            Feedback::Stable,
        ];
        for i in 0..10_000 {
            t.apply(seq[i % seq.len()]);
            assert!(t.within_clamps(), "every knob stays clamped at step {i}");
        }
    }

    #[test]
    fn interval_never_tunes_below_the_config_floor() {
        let mut t = OverseerTuning::default();
        for _ in 0..1000 {
            t.apply(Feedback::TooQuiet); // pushes the cadence down
        }
        assert!(
            t.interval_secs_u64() >= MIN_OVERSEER_INTERVAL_SECS,
            "self-tuning can never breach the observer-cadence floor (no hot loop)"
        );
    }

    #[test]
    fn budget_fraction_only_widens_and_stays_bounded() {
        let mut t = OverseerTuning::default();
        let start = t.budget_fraction.value();
        for _ in 0..1000 {
            t.apply(Feedback::TooQuiet);
        }
        assert!(
            approx(t.budget_fraction.value(), start),
            "quiet feedback never lowers the budget fraction"
        );
        for _ in 0..1000 {
            t.apply(Feedback::TooNoisy);
        }
        assert!(
            t.budget_fraction.value() <= 0.95 + 1e-9,
            "bounded by the ceiling"
        );
    }
}
