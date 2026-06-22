//! Tests for the V1 high-signal *core* benchmark set vs. the opt-in *extended*
//! set (issue #2087 / `Specs/ProductArchitecture.md` line 214).
//!
//! The default gym surfaces must resolve to exactly the four spec-mandated
//! core classes; every other class must be preserved but reachable only via an
//! explicit opt-in.

use std::collections::BTreeSet;

use super::scenarios::{benchmark_scenarios, benchmark_scenarios_for, core_benchmark_scenarios};
use super::types::{BenchmarkClass, BenchmarkScenarioSet};

fn distinct_classes(scenarios: &[super::types::BenchmarkScenario]) -> BTreeSet<String> {
    scenarios.iter().map(|s| s.class.to_string()).collect()
}

#[test]
fn core_const_is_the_four_spec_classes() {
    assert_eq!(
        BenchmarkClass::CORE,
        [
            BenchmarkClass::RepoExploration,
            BenchmarkClass::Documentation,
            BenchmarkClass::SafeCodeChange,
            BenchmarkClass::SessionQuality,
        ]
    );
}

#[test]
fn is_core_agrees_with_core_const() {
    for class in BenchmarkClass::CORE {
        assert!(class.is_core(), "{class} must be core");
    }
    // A representative extended class must NOT be core.
    assert!(!BenchmarkClass::ChaosEngineering.is_core());
    assert!(!BenchmarkClass::EventSourcing.is_core());
    assert!(!BenchmarkClass::RateLimiting.is_core());
}

#[test]
fn default_core_set_classes_are_exactly_the_four_spec_classes() {
    let classes = distinct_classes(core_benchmark_scenarios());
    let expected: BTreeSet<String> = [
        "repo-exploration",
        "documentation",
        "safe-code-change",
        "session-quality",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        classes, expected,
        "the default (core) gym set must contain exactly the four spec-mandated classes"
    );
}

#[test]
fn every_core_scenario_has_a_core_class() {
    for scenario in core_benchmark_scenarios() {
        assert!(
            scenario.class.is_core(),
            "core set leaked a non-core class via scenario '{}'",
            scenario.id
        );
    }
}

#[test]
fn core_set_is_non_empty_and_strict_subset_of_full_registry() {
    let core = core_benchmark_scenarios().len();
    let all = benchmark_scenarios().len();
    assert!(core > 0, "core set must be non-empty");
    assert!(
        core < all,
        "core ({core}) must be a strict subset of the full registry ({all})"
    );
}

#[test]
fn extended_classes_are_reachable_only_via_opt_in() {
    let core_classes = distinct_classes(core_benchmark_scenarios());
    let all = benchmark_scenarios_for(BenchmarkScenarioSet::Extended);
    let all_classes = distinct_classes(all);

    // There must be extra classes beyond the core four (work is preserved).
    let extra_classes: Vec<&String> = all_classes.difference(&core_classes).collect();
    assert!(
        !extra_classes.is_empty(),
        "extended set must expose classes beyond the core four"
    );

    // Concrete, non-tautological proof: specific known extended classes are
    // absent from the default/core set but present in the opt-in extended set.
    for extended_class in ["chaos-engineering", "event-sourcing", "rate-limiting"] {
        assert!(
            !core_classes.contains(extended_class),
            "extended class '{extended_class}' must be excluded from the default core set"
        );
        assert!(
            all_classes.contains(extended_class),
            "extended class '{extended_class}' must be reachable via the extended opt-in"
        );
    }

    // Every extra-class scenario in the full registry is absent from the core
    // set by id (proves the gating, not just class-set membership).
    let core_ids: BTreeSet<&str> = core_benchmark_scenarios().iter().map(|s| s.id).collect();
    let mut saw_extra = false;
    for scenario in all.iter().filter(|s| !s.class.is_core()) {
        saw_extra = true;
        assert!(
            !core_ids.contains(scenario.id),
            "extended scenario '{}' (class {}) leaked into the core set",
            scenario.id,
            scenario.class
        );
    }
    assert!(saw_extra, "expected at least one extended-class scenario");
}

#[test]
fn core_ids_equal_full_registry_filtered_by_is_core() {
    let expected: Vec<&str> = benchmark_scenarios()
        .iter()
        .filter(|s| s.class.is_core())
        .map(|s| s.id)
        .collect();
    let actual: Vec<&str> = core_benchmark_scenarios().iter().map(|s| s.id).collect();
    assert_eq!(
        actual, expected,
        "core set must be exactly the full registry filtered to core classes, in registry order"
    );
}

#[test]
fn known_extended_scenario_is_excluded_from_core_but_in_extended() {
    // `test-writing-unit-case` is class `test-writing`, an extended-only class.
    let in_core = core_benchmark_scenarios()
        .iter()
        .any(|s| s.id == "test-writing-unit-case");
    let in_extended = benchmark_scenarios_for(BenchmarkScenarioSet::Extended)
        .iter()
        .any(|s| s.id == "test-writing-unit-case");
    assert!(
        !in_core,
        "extended scenario 'test-writing-unit-case' must not be in the default core set"
    );
    assert!(
        in_extended,
        "extended scenario 'test-writing-unit-case' must be reachable via the extended opt-in"
    );
}

#[test]
fn selector_core_matches_core_accessor() {
    let via_selector: Vec<&str> = benchmark_scenarios_for(BenchmarkScenarioSet::Core)
        .iter()
        .map(|s| s.id)
        .collect();
    let via_accessor: Vec<&str> = core_benchmark_scenarios().iter().map(|s| s.id).collect();
    assert_eq!(via_selector, via_accessor);
}

#[test]
fn selector_extended_matches_full_registry() {
    let via_selector = benchmark_scenarios_for(BenchmarkScenarioSet::Extended).len();
    let full = benchmark_scenarios().len();
    assert_eq!(
        via_selector, full,
        "extended selector must expose the full registry"
    );
}
