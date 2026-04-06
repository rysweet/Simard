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

#[test]
fn benchmark_action_classification_eq() {
    assert_eq!(
        super::executor::BenchmarkActionClassification::Required,
        super::executor::BenchmarkActionClassification::Required
    );
    assert_ne!(
        super::executor::BenchmarkActionClassification::Required,
        super::executor::BenchmarkActionClassification::Unnecessary
    );
}

#[test]
fn metric_facts_large_sequence() {
    let mut facts = BenchmarkMetricFacts::default();
    for _ in 0..100 {
        facts.record_primary_attempt();
        facts.record_required_action();
    }
    for _ in 0..50 {
        facts.record_retry_attempt();
        facts.record_unnecessary_action();
    }
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(50));
    assert_eq!(derived.unnecessary_action_count, Some(50));
    assert_eq!(facts.attempts.len(), 150);
    assert_eq!(facts.actions.len(), 150);
}

#[test]
fn metric_derivation_only_retries_counts_correctly() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_retry_attempt();
    facts.record_retry_attempt();
    facts.record_retry_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(3));
}

#[test]
fn metric_derivation_only_unnecessary_counts_correctly() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unnecessary_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, Some(1));
}

#[test]
fn benchmark_metric_facts_default_eq() {
    let a = BenchmarkMetricFacts::default();
    let b = BenchmarkMetricFacts::default();
    assert_eq!(a, b);
}

#[test]
fn metric_derivation_independent_dimensions() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unmeasured_attempt();
    facts.record_required_action();
    facts.record_required_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, None);
    assert_eq!(derived.unnecessary_action_count, Some(0));
}

// ---- BenchmarkMetricFacts: comprehensive recording tests ----

#[test]
fn metric_facts_interleaved_attempts_and_actions() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_required_action();
    facts.record_retry_attempt();
    facts.record_unnecessary_action();
    facts.record_primary_attempt();
    facts.record_required_action();
    assert_eq!(facts.attempts.len(), 3);
    assert_eq!(facts.actions.len(), 3);
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(1));
    assert_eq!(derived.unnecessary_action_count, Some(1));
}

#[test]
fn metric_facts_all_unmeasured() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unmeasured_attempt();
    facts.record_unmeasured_attempt();
    facts.record_unmeasured_action();
    facts.record_unmeasured_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, None);
    assert_eq!(derived.unnecessary_action_count, None);
}

#[test]
fn metric_derivation_single_primary_zero_retries() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(0));
}

#[test]
fn metric_derivation_single_required_zero_unnecessary() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, Some(0));
}

// ---- BenchmarkAttemptFact / BenchmarkActionFact: clone/copy ----

#[test]
fn benchmark_attempt_fact_clone() {
    let fact = super::executor::BenchmarkAttemptFact {
        classification: Some(super::executor::BenchmarkAttemptClassification::Retry),
    };
    let cloned = fact;
    assert_eq!(fact, cloned);
}

#[test]
fn benchmark_action_fact_clone() {
    let fact = super::executor::BenchmarkActionFact {
        classification: Some(super::executor::BenchmarkActionClassification::Unnecessary),
    };
    let cloned = fact;
    assert_eq!(fact, cloned);
}

#[test]
fn benchmark_attempt_fact_none_classification() {
    let fact = super::executor::BenchmarkAttemptFact {
        classification: None,
    };
    assert_eq!(fact.classification, None);
}

#[test]
fn benchmark_action_fact_none_classification() {
    let fact = super::executor::BenchmarkActionFact {
        classification: None,
    };
    assert_eq!(fact.classification, None);
}

// ---- DerivedBenchmarkMetrics: structural tests ----

#[test]
fn derived_metrics_debug_format() {
    let m = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: Some(5),
        retry_count: None,
    };
    let debug = format!("{m:?}");
    assert!(debug.contains("5"));
    assert!(debug.contains("None"));
}

#[test]
fn derived_metrics_clone() {
    let m = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: Some(3),
        retry_count: Some(1),
    };
    let cloned = m;
    assert_eq!(m, cloned);
}

#[test]
fn derived_metrics_both_none() {
    let m = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: None,
        retry_count: None,
    };
    assert_eq!(m.unnecessary_action_count, None);
    assert_eq!(m.retry_count, None);
}

#[test]
fn derived_metrics_both_zero() {
    let m = super::executor::DerivedBenchmarkMetrics {
        unnecessary_action_count: Some(0),
        retry_count: Some(0),
    };
    assert_eq!(m.unnecessary_action_count, Some(0));
    assert_eq!(m.retry_count, Some(0));
}

// ---- BenchmarkMetricFacts: ordering sensitivity ----

#[test]
fn metric_derivation_unmeasured_after_measured_returns_none() {
    // retry: primary, retry, unmeasured → None (short-circuits)
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_retry_attempt();
    facts.record_unmeasured_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, None);
}

#[test]
fn metric_derivation_unmeasured_then_measured_returns_none() {
    // unmeasured first → None regardless of what follows
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unmeasured_action();
    facts.record_required_action();
    facts.record_unnecessary_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, None);
}

// ---- BenchmarkAttemptClassification / BenchmarkActionClassification ----

#[test]
fn attempt_classification_debug_primary() {
    let c = super::executor::BenchmarkAttemptClassification::Primary;
    assert_eq!(format!("{c:?}"), "Primary");
}

#[test]
fn attempt_classification_debug_retry() {
    let c = super::executor::BenchmarkAttemptClassification::Retry;
    assert_eq!(format!("{c:?}"), "Retry");
}

#[test]
fn action_classification_debug_required() {
    let c = super::executor::BenchmarkActionClassification::Required;
    assert_eq!(format!("{c:?}"), "Required");
}

#[test]
fn action_classification_debug_unnecessary() {
    let c = super::executor::BenchmarkActionClassification::Unnecessary;
    assert_eq!(format!("{c:?}"), "Unnecessary");
}

#[test]
fn attempt_classification_clone_eq() {
    let a = super::executor::BenchmarkAttemptClassification::Primary;
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn action_classification_clone_eq() {
    let a = super::executor::BenchmarkActionClassification::Required;
    let b = a;
    assert_eq!(a, b);
}

// ---- BenchmarkMetricFacts: equality after different recording orders ----

#[test]
fn metric_facts_different_order_same_result() {
    let mut facts_a = BenchmarkMetricFacts::default();
    facts_a.record_primary_attempt();
    facts_a.record_retry_attempt();
    facts_a.record_required_action();
    facts_a.record_unnecessary_action();

    let mut facts_b = BenchmarkMetricFacts::default();
    facts_b.record_primary_attempt();
    facts_b.record_retry_attempt();
    facts_b.record_required_action();
    facts_b.record_unnecessary_action();

    assert_eq!(facts_a, facts_b);
    assert_eq!(
        derive_benchmark_metrics(&facts_a),
        derive_benchmark_metrics(&facts_b)
    );
}

#[test]
fn metric_facts_different_content_not_equal() {
    let mut facts_a = BenchmarkMetricFacts::default();
    facts_a.record_primary_attempt();

    let mut facts_b = BenchmarkMetricFacts::default();
    facts_b.record_retry_attempt();

    assert_ne!(facts_a, facts_b);
}

// ---- Stress test: many records ----

#[test]
fn metric_derivation_1000_records() {
    let mut facts = BenchmarkMetricFacts::default();
    for _ in 0..500 {
        facts.record_primary_attempt();
    }
    for _ in 0..500 {
        facts.record_retry_attempt();
    }
    for _ in 0..700 {
        facts.record_required_action();
    }
    for _ in 0..300 {
        facts.record_unnecessary_action();
    }
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(500));
    assert_eq!(derived.unnecessary_action_count, Some(300));
}
