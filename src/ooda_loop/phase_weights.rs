//! Per-OODA-phase ranked-recall weights (issue #2329).
//!
//! Simard owns the *policy* — which scoring signals matter in each OODA phase —
//! while the `amplihack-memory-lib` ranked recall owns the *mechanism* (the
//! scoring math). This module maps each [`OodaPhase`] to a
//! [`RecallWeightSet`], applied automatically during preparation recall.
//!
//! `cognitive_memory` must stay a leaf module (it must not import `ooda_loop`),
//! so the `OodaPhase -> RecallWeightSet` mapping lives here — the only layer
//! that knows about [`OodaPhase`]. [`crate::ooda_loop::cycle`] computes the
//! weights for the live phase and threads them down into preparation; the
//! `RecallWeightSet -> amplihack_memory::RecallWeights` conversion is then
//! performed adapter-local.
//!
//! ## Defaults (fields: text_relevance, confidence, importance, recency, usage, graph)
//!
//! | Phase | text_rel | confidence | importance | recency | usage | graph | Bias |
//! |---|---|---|---|---|---|---|---|
//! | Observe | 0.8 | 0.5 | 0.5 | **1.0** | 0.4 | 0.5 | Favor recency. |
//! | Orient | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced (library default). |
//! | Decide | 1.0 | **1.0** | 0.6 | 0.3 | 0.3 | 0.5 | Favor confidence/relevance. |
//! | Act | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced. |
//! | Sleep | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced (no prep recall). |
//!
//! Observe is recency-heavy so the brain sees the freshest declarative state
//! first; Decide is confidence-heavy so commitments lean on trusted facts. The
//! divergence means the same fact set can be ordered differently per phase —
//! see [`docs/reference/cognitive-memory-ranked-recall.md`].

use crate::cognitive_memory::RecallWeightSet;

use super::OodaPhase;

/// Ranked-recall weights for a given OODA phase.
///
/// `Observe` favors recency (surface what changed lately), `Decide` favors
/// confidence/relevance (commit on trusted facts), and every other phase uses
/// the balanced library default ([`RecallWeightSet::default`]). The numeric
/// presets are pinned by unit tests so the documented table cannot silently
/// drift from the code.
pub fn weights_for_phase(phase: OodaPhase) -> RecallWeightSet {
    match phase {
        OodaPhase::Observe => RecallWeightSet {
            text_relevance: 0.8,
            confidence: 0.5,
            importance: 0.5,
            recency: 1.0,
            usage: 0.4,
            graph: 0.5,
        },
        OodaPhase::Decide => RecallWeightSet {
            text_relevance: 1.0,
            confidence: 1.0,
            importance: 0.6,
            recency: 0.3,
            usage: 0.3,
            graph: 0.5,
        },
        OodaPhase::Orient | OodaPhase::Act | OodaPhase::Sleep => RecallWeightSet::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_favors_recency() {
        let w = weights_for_phase(OodaPhase::Observe);
        assert_eq!(w.text_relevance, 0.8);
        assert_eq!(w.confidence, 0.5);
        assert_eq!(w.importance, 0.5);
        assert_eq!(w.recency, 1.0);
        assert_eq!(w.usage, 0.4);
        assert_eq!(w.graph, 0.5);
        // Recency is the dominant Observe signal.
        assert!(w.recency > w.confidence, "Observe must favor recency");
    }

    #[test]
    fn decide_favors_confidence() {
        let w = weights_for_phase(OodaPhase::Decide);
        assert_eq!(w.text_relevance, 1.0);
        assert_eq!(w.confidence, 1.0);
        assert_eq!(w.importance, 0.6);
        assert_eq!(w.recency, 0.3);
        assert_eq!(w.usage, 0.3);
        assert_eq!(w.graph, 0.5);
        // Confidence beats recency for commitments.
        assert!(w.confidence > w.recency, "Decide must favor confidence");
    }

    #[test]
    fn orient_act_sleep_use_balanced_default() {
        let default = RecallWeightSet::default();
        assert_eq!(weights_for_phase(OodaPhase::Orient), default);
        assert_eq!(weights_for_phase(OodaPhase::Act), default);
        assert_eq!(weights_for_phase(OodaPhase::Sleep), default);
    }

    #[test]
    fn observe_and_decide_diverge() {
        // The two deliberate divergences (Observe vs Decide) must differ so the
        // same fact set can be ordered differently per phase.
        assert_ne!(
            weights_for_phase(OodaPhase::Observe),
            weights_for_phase(OodaPhase::Decide),
        );
        // Specifically: Observe weights recency above Decide; Decide weights
        // confidence above Observe.
        let o = weights_for_phase(OodaPhase::Observe);
        let d = weights_for_phase(OodaPhase::Decide);
        assert!(o.recency > d.recency, "Observe more recency-biased");
        assert!(d.confidence > o.confidence, "Decide more confidence-biased");
    }
}
