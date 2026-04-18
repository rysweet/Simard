//! Additional unit tests for gym module — covering scenario resolution
//! edge cases, metric derivation boundaries, and reporting helpers.

use super::scenarios::{benchmark_scenarios, resolve_benchmark_scenario};
use super::types::*;
use super::{STARTER_SUITE_ID, default_output_root};

// ── default_output_root ─────────────────────────────────────────

#[test]
fn default_output_root_contains_simard_gym() {
    let root = default_output_root();
    let as_str = root.to_string_lossy();
    assert!(as_str.contains("simard-gym"), "got: {as_str}");
}

#[test]
fn default_output_root_is_not_absolute() {
    assert!(!default_output_root().is_absolute());
}

// ── benchmark_scenarios invariants ──────────────────────────────

#[test]
fn all_scenario_titles_are_nonempty() {
    for s in benchmark_scenarios() {
        assert!(
            !s.title.trim().is_empty(),
            "scenario {} has empty title",
            s.id
        );
    }
}

#[test]
fn all_scenario_objectives_are_nonempty() {
    for s in benchmark_scenarios() {
        assert!(
            !s.objective.trim().is_empty(),
            "scenario {} has empty objective",
            s.id
        );
    }
}

#[test]
fn all_scenario_ids_are_ascii() {
    for s in benchmark_scenarios() {
        assert!(
            s.id.is_ascii(),
            "scenario id '{}' contains non-ASCII chars",
            s.id
        );
    }
}

#[test]
fn no_duplicate_scenario_titles() {
    let titles: Vec<&str> = benchmark_scenarios().iter().map(|s| s.title).collect();
    let mut seen = std::collections::HashSet::new();
    for title in &titles {
        assert!(seen.insert(title), "duplicate scenario title: {title}");
    }
}

#[test]
fn resolve_benchmark_scenario_returns_matching_id() {
    let scenarios = benchmark_scenarios();
    let first = scenarios[0];
    let resolved = resolve_benchmark_scenario(first.id).unwrap();
    assert_eq!(resolved.id, first.id);
    assert_eq!(resolved.title, first.title);
}

#[test]
fn resolve_benchmark_scenario_not_found_error_contains_id() {
    let err = resolve_benchmark_scenario("nonexistent-scenario-xyz").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("nonexistent-scenario-xyz"),
        "error should contain the scenario id: {msg}"
    );
}

// ── starter suite constant ──────────────────────────────────────

#[test]
fn starter_suite_id_is_lowercase() {
    assert_eq!(STARTER_SUITE_ID, STARTER_SUITE_ID.to_lowercase());
}

// ── BenchmarkClass display coverage ─────────────────────────────

#[test]
fn benchmark_class_display_covers_all_variants() {
    let classes = [
        BenchmarkClass::RepoExploration,
        BenchmarkClass::Documentation,
        BenchmarkClass::SafeCodeChange,
        BenchmarkClass::TestWriting,
        BenchmarkClass::SessionQuality,
        BenchmarkClass::BugFix,
        BenchmarkClass::Refactoring,
        BenchmarkClass::DependencyAnalysis,
        BenchmarkClass::ErrorHandling,
    ];
    for class in &classes {
        let display = format!("{class}");
        assert!(!display.is_empty(), "{class:?} has empty display");
    }
}

// ── BenchmarkComparisonStatus ───────────────────────────────────

#[test]
fn comparison_status_debug_and_clone() {
    let status = BenchmarkComparisonStatus::Improved;
    let cloned = status;
    assert_eq!(format!("{status:?}"), format!("{cloned:?}"));
}

#[test]
fn comparison_status_all_variants_display() {
    let variants = [
        BenchmarkComparisonStatus::Improved,
        BenchmarkComparisonStatus::Regressed,
        BenchmarkComparisonStatus::Unchanged,
    ];
    for v in &variants {
        let display = format!("{v}");
        assert!(!display.is_empty());
    }
}

// ── BenchmarkCheckResult construction ───────────────────────────

#[test]
fn check_result_passed_has_no_detail_requirement() {
    let check = BenchmarkCheckResult {
        id: "syntax-valid".to_string(),
        passed: true,
        detail: String::new(),
    };
    assert!(check.passed);
    assert!(check.detail.is_empty());
}

#[test]
fn check_result_failed_preserves_detail() {
    let check = BenchmarkCheckResult {
        id: "output-check".to_string(),
        passed: false,
        detail: "expected foo, got bar".to_string(),
    };
    assert!(!check.passed);
    assert!(check.detail.contains("foo"));
}
