//! Parity test: the recipe-driven self-improvement path produces the same
//! `ImprovementCycle` as the legacy Rust path (`run_improvement_cycle`-style
//! synthesis using the same primitives).
//!
//! This locks the architectural pivot into CI: when we delete the in-Rust
//! orchestration, the recipe-driven path must remain equivalent for the
//! deterministic phases (eval, analyze, decide). Agentic phases
//! (generate-patch, review) are exercised separately by an end-to-end smoke
//! test that uses the recipe runner's --dry-run mode.

use std::process::{Command, Stdio};
use std::io::Write;

use serde_json::json;

use simard::gym_bridge::ScoreDimensions;
use simard::gym_scoring::{GymSuiteScore, detect_regression};
use simard::self_improve::{
    ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase, ProposedChange,
    decide, find_weak_dimensions,
};

fn ss(suite: &str, overall: f64, dims: ScoreDimensions) -> GymSuiteScore {
    GymSuiteScore {
        suite_id: suite.to_string(),
        overall,
        dimensions: dims,
        scenario_count: 5,
        scenarios_passed: 5,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    }
}

fn baseline_fixture() -> GymSuiteScore {
    ss(
        "parity-suite",
        0.65,
        ScoreDimensions {
            factual_accuracy: 0.80,
            specificity: 0.55, // weak
            temporal_awareness: 0.60, // weak
            source_attribution: 0.85,
            confidence_calibration: 0.45, // weak
        },
    )
}

fn post_fixture() -> GymSuiteScore {
    ss(
        "parity-suite",
        0.78,
        ScoreDimensions {
            factual_accuracy: 0.82,
            specificity: 0.75,
            temporal_awareness: 0.74,
            source_attribution: 0.85,
            confidence_calibration: 0.74,
        },
    )
}

/// Synthesize an ImprovementCycle the way `run_improvement_cycle` would.
/// This is the "Rust path" against which the recipe path is compared.
fn rust_path_cycle(proposal: &str, target_dim: Option<&str>) -> ImprovementCycle {
    let baseline = baseline_fixture();
    let post = post_fixture();
    let weak = find_weak_dimensions(&baseline, 0.7, target_dim);
    let weak_names: Vec<String> = weak.iter().map(|w| w.name.clone()).collect();

    if proposal.is_empty() {
        return ImprovementCycle {
            baseline,
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Revert {
                reason: format!(
                    "no changes proposed; weak dimensions: {}",
                    if weak_names.is_empty() { "none".to_string() } else { weak_names.join(", ") }
                ),
            }),
            final_phase: ImprovementPhase::Analyze,
            weak_dimensions: weak_names,
            weak_dimension_details: weak,
            target_dimension: target_dim.map(String::from),
        };
    }

    let config = ImprovementConfig {
        suite_id: "recipe".to_string(),
        weak_threshold: 0.7,
        min_net_improvement: 0.01,
        max_single_regression: 0.05,
        target_dimension: target_dim.map(String::from),
        auto_apply: false,
        proposed_changes: vec![ProposedChange {
            file_path: ".".to_string(),
            description: proposal.to_string(),
            expected_impact: format!("recipe-driven; research_decision=CONTINUE"),
        }],
    };
    let regressions = detect_regression(&post, &baseline);
    let decision = decide(&config, &baseline, &post, &regressions);

    ImprovementCycle {
        baseline,
        proposed_changes: config.proposed_changes,
        post_score: Some(post),
        regressions,
        decision: Some(decision),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: weak_names,
        weak_dimension_details: weak,
        target_dimension: target_dim.map(String::from),
    }
}

fn helper_bin() -> String {
    // Resolve the helper bin path from CARGO_MANIFEST_DIR + target/debug.
    // CARGO_TARGET_DIR may override; use cargo's standard env.
    let target = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| format!("{}/target", env!("CARGO_MANIFEST_DIR")));
    format!("{target}/debug/simard-improve-step")
}

fn run_helper(args: &[&str], stdin_data: Option<&str>) -> String {
    let mut cmd = Command::new(helper_bin());
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn helper");
    if let Some(data) = stdin_data {
        child.stdin.as_mut().unwrap().write_all(data.as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait helper");
    if !out.status.success() {
        panic!(
            "helper {:?} failed: status={} stderr={}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Drive the recipe phases by invoking the helper bin in sequence — same as
/// the recipe's bash steps would do at runtime. This avoids needing the live
/// recipe runner in unit tests.
fn recipe_path_cycle(proposal: &str, target_dim: Option<&str>) -> ImprovementCycle {
    let baseline = baseline_fixture();
    let post = post_fixture();
    let baseline_json = serde_json::to_string(&baseline).unwrap();
    let post_json = serde_json::to_string(&post).unwrap();

    // Phase 1: eval (fixture mode)
    let baseline_out = run_helper(
        &["eval", "--workspace", ".", "--suite-id", "parity-suite",
          "--baseline-fixture-json", &baseline_json],
        None,
    );
    assert_eq!(serde_json::from_str::<GymSuiteScore>(&baseline_out).unwrap(), baseline);

    // Phase 2: analyze
    let target = target_dim.unwrap_or("");
    let weak_out = run_helper(
        &["analyze", "--baseline-json", &baseline_json,
          "--weak-threshold", "0.7", "--target-dimension", target],
        None,
    );

    // Phase 3 + 6: research-decision + decide
    let research_decision = if proposal.is_empty() { "REVERT_NO_PROPOSAL" } else { "CONTINUE" };
    let apply_result_json = if proposal.is_empty() {
        "null".to_string()
    } else {
        // Phase 4 simulated: review says should_commit=true, no findings
        let fake_apply = json!({"kind": "Applied", "findings": []});
        fake_apply.to_string()
    };

    let cycle_out = run_helper(
        &[
            "decide",
            "--baseline-json", &baseline_json,
            "--post-json", if proposal.is_empty() { "null" } else { &post_json },
            "--weak-dimensions-json", &weak_out,
            "--proposal", proposal,
            "--research-decision", research_decision,
            "--apply-result-json", &apply_result_json,
            "--target-dimension", target,
        ],
        None,
    );

    serde_json::from_str(&cycle_out).expect("parse cycle JSON")
}

fn assert_cycles_equivalent(rust: &ImprovementCycle, recipe: &ImprovementCycle) {
    // Strict equality on everything except the proposal expected_impact suffix
    // (the rust path doesn't emit research_decision in the impact string today).
    assert_eq!(rust.baseline, recipe.baseline, "baseline mismatch");
    assert_eq!(rust.post_score, recipe.post_score, "post_score mismatch");
    assert_eq!(rust.regressions, recipe.regressions, "regressions mismatch");
    assert_eq!(rust.weak_dimensions, recipe.weak_dimensions, "weak_dimensions list mismatch");
    assert_eq!(
        rust.weak_dimension_details.len(),
        recipe.weak_dimension_details.len(),
        "weak detail count mismatch"
    );
    for (a, b) in rust.weak_dimension_details.iter().zip(&recipe.weak_dimension_details) {
        assert_eq!(a.name, b.name, "weak dim name");
        assert!((a.deficit - b.deficit).abs() < 1e-9, "deficit drift: {} vs {}", a.deficit, b.deficit);
    }
    assert_eq!(rust.target_dimension, recipe.target_dimension, "target dim mismatch");
    assert_eq!(rust.final_phase, recipe.final_phase, "final phase mismatch");
    assert_eq!(rust.decision, recipe.decision, "decision mismatch");
    assert_eq!(
        rust.proposed_changes.len(),
        recipe.proposed_changes.len(),
        "proposed_changes length mismatch"
    );
}

#[test]
fn parity_no_proposal_reverts_with_weak_dimensions() {
    let rust = rust_path_cycle("", None);
    let recipe = recipe_path_cycle("", None);
    assert_cycles_equivalent(&rust, &recipe);
    // Specifically: must mention the weak dims in the revert reason
    if let Some(ImprovementDecision::Revert { reason }) = &recipe.decision {
        assert!(reason.contains("specificity") || reason.contains("weak"));
    } else {
        panic!("expected Revert decision");
    }
}

#[test]
fn parity_with_proposal_commits_when_net_positive() {
    let rust = rust_path_cycle("Strengthen specificity prompts", None);
    let recipe = recipe_path_cycle("Strengthen specificity prompts", None);
    assert_cycles_equivalent(&rust, &recipe);
    assert!(matches!(recipe.decision, Some(ImprovementDecision::Commit { .. })));
}

#[test]
fn parity_with_target_dimension_filters_weak_list() {
    let rust = rust_path_cycle("Boost calibration", Some("confidence_calibration"));
    let recipe = recipe_path_cycle("Boost calibration", Some("confidence_calibration"));
    assert_cycles_equivalent(&rust, &recipe);
    // Only confidence_calibration should be in weak list
    assert_eq!(recipe.weak_dimensions, vec!["confidence_calibration"]);
}
