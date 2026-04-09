#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BenchmarkAttemptClassification {
    Primary,
    Retry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BenchmarkAttemptFact {
    pub(super) classification: Option<BenchmarkAttemptClassification>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BenchmarkActionClassification {
    Required,
    Unnecessary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BenchmarkActionFact {
    pub(super) classification: Option<BenchmarkActionClassification>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct BenchmarkMetricFacts {
    pub(super) attempts: Vec<BenchmarkAttemptFact>,
    pub(super) actions: Vec<BenchmarkActionFact>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DerivedBenchmarkMetrics {
    pub(super) unnecessary_action_count: Option<u32>,
    pub(super) retry_count: Option<u32>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl BenchmarkMetricFacts {
    pub(super) fn record_primary_attempt(&mut self) {
        self.attempts.push(BenchmarkAttemptFact {
            classification: Some(BenchmarkAttemptClassification::Primary),
        });
    }

    pub(super) fn record_retry_attempt(&mut self) {
        self.attempts.push(BenchmarkAttemptFact {
            classification: Some(BenchmarkAttemptClassification::Retry),
        });
    }

    pub(super) fn record_unmeasured_attempt(&mut self) {
        self.attempts.push(BenchmarkAttemptFact {
            classification: None,
        });
    }

    pub(super) fn record_required_action(&mut self) {
        self.actions.push(BenchmarkActionFact {
            classification: Some(BenchmarkActionClassification::Required),
        });
    }

    pub(super) fn record_unnecessary_action(&mut self) {
        self.actions.push(BenchmarkActionFact {
            classification: Some(BenchmarkActionClassification::Unnecessary),
        });
    }

    pub(super) fn record_unmeasured_action(&mut self) {
        self.actions.push(BenchmarkActionFact {
            classification: None,
        });
    }
}

pub(super) fn derive_benchmark_metrics(facts: &BenchmarkMetricFacts) -> DerivedBenchmarkMetrics {
    DerivedBenchmarkMetrics {
        unnecessary_action_count: facts.actions.iter().try_fold(0_u32, |count, fact| {
            match fact.classification {
                Some(BenchmarkActionClassification::Required) => Some(count),
                Some(BenchmarkActionClassification::Unnecessary) => count.checked_add(1),
                None => None,
            }
        }),
        retry_count: facts.attempts.iter().try_fold(0_u32, |count, fact| {
            match fact.classification {
                Some(BenchmarkAttemptClassification::Primary) => Some(count),
                Some(BenchmarkAttemptClassification::Retry) => count.checked_add(1),
                None => None,
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_metric_facts_default_is_empty() {
        let facts = BenchmarkMetricFacts::default();
        assert!(facts.attempts.is_empty());
        assert!(facts.actions.is_empty());
    }

    #[test]
    fn record_primary_attempt() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_primary_attempt();
        assert_eq!(facts.attempts.len(), 1);
        assert_eq!(
            facts.attempts[0].classification,
            Some(BenchmarkAttemptClassification::Primary)
        );
    }

    #[test]
    fn record_retry_attempt() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_retry_attempt();
        assert_eq!(
            facts.attempts[0].classification,
            Some(BenchmarkAttemptClassification::Retry)
        );
    }

    #[test]
    fn record_unmeasured_attempt() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_unmeasured_attempt();
        assert_eq!(facts.attempts[0].classification, None);
    }

    #[test]
    fn record_required_action() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_required_action();
        assert_eq!(
            facts.actions[0].classification,
            Some(BenchmarkActionClassification::Required)
        );
    }

    #[test]
    fn record_unnecessary_action() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_unnecessary_action();
        assert_eq!(
            facts.actions[0].classification,
            Some(BenchmarkActionClassification::Unnecessary)
        );
    }

    #[test]
    fn record_unmeasured_action() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_unmeasured_action();
        assert_eq!(facts.actions[0].classification, None);
    }

    #[test]
    fn derive_metrics_empty_facts() {
        let facts = BenchmarkMetricFacts::default();
        let metrics = derive_benchmark_metrics(&facts);
        assert_eq!(metrics.unnecessary_action_count, Some(0));
        assert_eq!(metrics.retry_count, Some(0));
    }

    #[test]
    fn derive_metrics_counts_correctly() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_primary_attempt();
        facts.record_retry_attempt();
        facts.record_retry_attempt();
        facts.record_required_action();
        facts.record_unnecessary_action();
        let metrics = derive_benchmark_metrics(&facts);
        assert_eq!(metrics.retry_count, Some(2));
        assert_eq!(metrics.unnecessary_action_count, Some(1));
    }

    #[test]
    fn derive_metrics_unmeasured_yields_none() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_primary_attempt();
        facts.record_unmeasured_attempt();
        facts.record_required_action();
        facts.record_unmeasured_action();
        let metrics = derive_benchmark_metrics(&facts);
        assert_eq!(metrics.retry_count, None);
        assert_eq!(metrics.unnecessary_action_count, None);
    }
}
