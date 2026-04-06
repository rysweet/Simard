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
