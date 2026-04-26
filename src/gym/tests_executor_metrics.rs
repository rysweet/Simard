use super::executor::*;

// imports inherited from super::executor::*

#[test]
fn derive_metrics_all_required_actions() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    facts.record_required_action();
    facts.record_required_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, Some(0));
}

#[test]
fn derive_metrics_with_unnecessary_actions() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    facts.record_unnecessary_action();
    facts.record_unnecessary_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, Some(2));
}

#[test]
fn derive_metrics_unmeasured_action_yields_none() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_required_action();
    facts.record_unmeasured_action();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.unnecessary_action_count, None);
}

#[test]
fn derive_metrics_primary_attempts_no_retries() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(0));
}

#[test]
fn derive_metrics_retry_counted() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_primary_attempt();
    facts.record_retry_attempt();
    facts.record_retry_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, Some(2));
}

#[test]
fn derive_metrics_unmeasured_attempt_yields_none() {
    let mut facts = BenchmarkMetricFacts::default();
    facts.record_unmeasured_attempt();
    let derived = derive_benchmark_metrics(&facts);
    assert_eq!(derived.retry_count, None);
}

#[test]
fn derive_metrics_empty_facts_returns_zero() {
    let facts = BenchmarkMetricFacts::default();
    let derived = derive_benchmark_metrics(&facts);
    // Empty iterator fold starts at 0 and never encounters None
    assert_eq!(derived.unnecessary_action_count, Some(0));
    assert_eq!(derived.retry_count, Some(0));
}
