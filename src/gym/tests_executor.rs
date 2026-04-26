use super::executor::{BenchmarkMetricFacts, derive_benchmark_metrics};

#[test]
fn metric_derivation_counts_retries_and_unnecessary_actions_from_recorded_facts() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_retry_attempt();
    facts.record_required_action();
    facts.record_unnecessary_action();
    facts.record_unnecessary_action();

    let derived = derive_benchmark_metrics(&facts);

    assert_eq!(derived.retry_count, Some(1));
    assert_eq!(derived.unnecessary_action_count, Some(2));
}

#[test]
fn metric_derivation_returns_unmeasured_when_facts_are_incomplete() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_unmeasured_attempt();
    facts.record_required_action();
    facts.record_unmeasured_action();

    let derived = derive_benchmark_metrics(&facts);

    assert_eq!(derived.retry_count, None);
    assert_eq!(derived.unnecessary_action_count, None);
}

#[test]
fn metric_derivation_empty_facts_returns_zero_counts() {
    let facts = BenchmarkMetricFacts::default();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(0));
    assert_eq!(derived.unnecessary_action_count, Some(0));
}

#[test]
fn metric_derivation_only_primary_attempts_yields_zero_retries() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_primary_attempt();
    facts.record_primary_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(0));
}

#[test]
fn metric_derivation_only_required_actions_yields_zero_unnecessary() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    facts.record_required_action();
    facts.record_required_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, Some(0));
}

#[test]
fn metric_derivation_multiple_retries() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_retry_attempt();
    facts.record_retry_attempt();
    facts.record_retry_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(3));
}

#[test]
fn metric_derivation_unmeasured_attempt_at_start_returns_none() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unmeasured_attempt();
    facts.record_primary_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, None);
}

#[test]
fn metric_derivation_unmeasured_action_at_end_returns_none() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    facts.record_unnecessary_action();
    facts.record_unmeasured_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, None);
}

#[test]
fn metric_derivation_actions_independent_of_attempts() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_unmeasured_attempt();
    facts.record_required_action();
    facts.record_unnecessary_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, None);
    assert_eq!(derived.unnecessary_action_count, Some(1));
}

#[test]
fn metric_facts_default_has_empty_collections() {
    let facts = BenchmarkMetricFacts::default();
    assert!(facts.attempts.is_empty());
    assert!(facts.actions.is_empty());
}

#[test]
fn metric_facts_record_methods_grow_collections() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    assert_eq!(facts.attempts.len(), 1);
    facts.record_retry_attempt();
    assert_eq!(facts.attempts.len(), 2);
    facts.record_unmeasured_attempt();
    assert_eq!(facts.attempts.len(), 3);
    facts.record_required_action();
    assert_eq!(facts.actions.len(), 1);
    facts.record_unnecessary_action();
    assert_eq!(facts.actions.len(), 2);
    facts.record_unmeasured_action();
    assert_eq!(facts.actions.len(), 3);
}

#[test]
fn benchmark_attempt_fact_classifications() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_retry_attempt();
    facts.record_unmeasured_attempt();
    assert_eq!(
        facts.attempts[0].classification,
        Some(super::executor::BenchmarkAttemptClassification::Primary)
    );
    assert_eq!(
        facts.attempts[1].classification,
        Some(super::executor::BenchmarkAttemptClassification::Retry)
    );
    assert_eq!(facts.attempts[2].classification, None);
}

#[test]
fn benchmark_action_fact_classifications() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    facts.record_unnecessary_action();
    facts.record_unmeasured_action();
    assert_eq!(
        facts.actions[0].classification,
        Some(super::executor::BenchmarkActionClassification::Required)
    );
    assert_eq!(
        facts.actions[1].classification,
        Some(super::executor::BenchmarkActionClassification::Unnecessary)
    );
    assert_eq!(facts.actions[2].classification, None);
}

// ---- derive_benchmark_metrics additional coverage ----

#[test]
fn metric_derivation_all_retries_no_primary() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_retry_attempt();
    facts.record_retry_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(2));
}

#[test]
fn metric_derivation_all_unnecessary_actions() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unnecessary_action();
    facts.record_unnecessary_action();
    facts.record_unnecessary_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, Some(3));
}

#[test]
fn metric_derivation_single_unmeasured_attempt_returns_none() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unmeasured_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, None);
}

#[test]
fn metric_derivation_single_unmeasured_action_returns_none() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unmeasured_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, None);
}

#[test]
fn metric_derivation_mixed_required_and_unnecessary() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    facts.record_unnecessary_action();
    facts.record_required_action();
    facts.record_unnecessary_action();
    facts.record_required_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, Some(2));
}

#[test]
fn metric_derivation_alternating_primary_and_retry() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_retry_attempt();
    facts.record_primary_attempt();
    facts.record_retry_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(2));
}

#[test]
fn metric_derivation_unmeasured_in_middle_of_attempts() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_unmeasured_attempt();
    facts.record_retry_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, None);
}

#[test]
fn metric_derivation_unmeasured_in_middle_of_actions() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    facts.record_unmeasured_action();
    facts.record_unnecessary_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, None);
}

// ---- struct construction and equality tests ----

#[test]
fn benchmark_metric_facts_clone_preserves_data() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_required_action();
    let cloned = facts.clone();
    assert_eq!(facts, cloned);
}

#[test]
fn benchmark_attempt_fact_debug_format() {
    let fact = super::executor::BenchmarkAttemptFact {
        classification: Some(super::executor::BenchmarkAttemptClassification::Primary),
    };
    let debug = format!("{:?}", fact);
    assert!(debug.contains("Primary"));
}

#[test]
fn benchmark_action_fact_debug_format() {
    let fact = super::executor::BenchmarkActionFact {
        classification: Some(super::executor::BenchmarkActionClassification::Required),
    };
    let debug = format!("{:?}", fact);
    assert!(debug.contains("Required"));
}

#[test]
fn derived_metrics_equality() {
    let a = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: Some(1),
        retry_count: Some(2),
    };
    let b = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: Some(1),
        retry_count: Some(2),
    };
    assert_eq!(a, b);
}

#[test]
fn derived_metrics_inequality() {
    let a = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: Some(1),
        retry_count: Some(2),
    };
    let b = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: Some(0),
        retry_count: Some(2),
    };
    assert_ne!(a, b);
}

#[test]
fn derived_metrics_none_vs_some() {
    let a = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: None,
        retry_count: None,
    };
    let b = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: Some(0),
        retry_count: Some(0),
    };
    assert_ne!(a, b);
}

#[test]
fn benchmark_attempt_classification_eq() {
    assert_eq!(
        super::executor::BenchmarkAttemptClassification::Primary,
        super::executor::BenchmarkAttemptClassification::Primary
    );
    assert_ne!(
        super::executor::BenchmarkAttemptClassification::Primary,
        super::executor::BenchmarkAttemptClassification::Retry
    );
}
