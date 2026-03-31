//! Integration tests for self-improvement loop and self-relaunch canary.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::gym_bridge::GymBridge;
use simard::self_improve::{
    ImprovementConfig, ImprovementDecision, ImprovementPhase, ProposedChange,
    run_improvement_cycle, summarize_cycle,
};
use simard::self_relaunch::{GateResult, RelaunchGate, all_gates_passed, default_gates, handover};

fn suite_json(suite_id: &str, overall: f64) -> serde_json::Value {
    let d = |v: f64| {
        serde_json::json!({
            "factual_accuracy": v, "specificity": v * 0.9,
            "temporal_awareness": v * 0.8, "source_attribution": v * 0.7,
            "confidence_calibration": v * 0.85
        })
    };
    serde_json::json!({
        "suite_id": suite_id, "success": true, "overall_score": overall,
        "dimensions": d(overall),
        "scenario_results": [{
            "scenario_id": "L1", "success": true, "score": overall,
            "dimensions": d(overall), "question_count": 5,
            "questions_answered": 5, "degraded_sources": []
        }],
        "scenarios_passed": 1, "scenarios_total": 1, "degraded_sources": []
    })
}

fn fixed_score_bridge(score: f64) -> GymBridge {
    let t = InMemoryBridgeTransport::new("gym", move |method, _| match method {
        "gym.run_suite" => Ok(suite_json("progressive", score)),
        _ => Ok(serde_json::json!([])),
    });
    GymBridge::new(Box::new(t))
}

fn improving_bridge(base: f64, post: f64) -> GymBridge {
    let n = AtomicUsize::new(0);
    let t = InMemoryBridgeTransport::new("gym", move |method, _| match method {
        "gym.run_suite" => {
            let s = if n.fetch_add(1, Ordering::SeqCst) == 0 {
                base
            } else {
                post
            };
            Ok(suite_json("progressive", s))
        }
        _ => Ok(serde_json::json!([])),
    });
    GymBridge::new(Box::new(t))
}

fn regressing_bridge(base: f64, post_overall: f64, post_spec: f64) -> GymBridge {
    let n = AtomicUsize::new(0);
    let t = InMemoryBridgeTransport::new("gym", move |method, _| match method {
        "gym.run_suite" => {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(suite_json("progressive", base))
            } else {
                Ok(serde_json::json!({
                    "suite_id": "progressive", "success": true,
                    "overall_score": post_overall,
                    "dimensions": {
                        "factual_accuracy": post_overall, "specificity": post_spec,
                        "temporal_awareness": post_overall * 0.8,
                        "source_attribution": post_overall * 0.7,
                        "confidence_calibration": post_overall * 0.85
                    },
                    "scenario_results": [], "scenarios_passed": 1,
                    "scenarios_total": 1, "degraded_sources": []
                }))
            }
        }
        _ => Ok(serde_json::json!([])),
    });
    GymBridge::new(Box::new(t))
}

fn sample_changes() -> Vec<ProposedChange> {
    vec![ProposedChange {
        file_path: "prompt_assets/engineer_system.md".to_string(),
        description: "Add evidence-citing instruction".to_string(),
        expected_impact: "Improve source_attribution".to_string(),
    }]
}

fn cfg(changes: Vec<ProposedChange>) -> ImprovementConfig {
    ImprovementConfig {
        suite_id: "progressive".to_string(),
        min_net_improvement: 0.02,
        max_single_regression: 0.05,
        proposed_changes: changes,
    }
}

#[test]
fn cycle_with_no_changes_stops_at_analyze() {
    let gym = fixed_score_bridge(0.70);
    let config = cfg(vec![]);

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    assert_eq!(cycle.final_phase, ImprovementPhase::Analyze);
    assert!(cycle.post_score.is_none(), "no re-eval without changes");
    assert!(
        matches!(&cycle.decision, Some(ImprovementDecision::Revert { reason }) if reason.contains("no changes proposed")),
        "should revert when no changes: {:?}",
        cycle.decision,
    );
}

#[test]
fn cycle_commits_on_sufficient_improvement() {
    let gym = improving_bridge(0.70, 0.75);
    let config = cfg(sample_changes());

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    assert_eq!(cycle.final_phase, ImprovementPhase::Decide);
    assert!(cycle.post_score.is_some());

    match &cycle.decision {
        Some(ImprovementDecision::Commit { net_improvement }) => {
            assert!(
                (*net_improvement - 0.05).abs() < 1e-9,
                "expected ~5% improvement, got {net_improvement}"
            );
        }
        other => panic!("expected commit, got {other:?}"),
    }
}

#[test]
fn cycle_reverts_when_improvement_too_small() {
    let gym = improving_bridge(0.70, 0.71);
    let config = cfg(sample_changes());

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    assert_eq!(cycle.final_phase, ImprovementPhase::Decide);
    assert!(
        matches!(&cycle.decision, Some(ImprovementDecision::Revert { reason }) if reason.contains("below minimum")),
        "should revert: {:?}",
        cycle.decision,
    );
}

#[test]
fn cycle_reverts_on_dimension_regression() {
    // Overall improves from 0.70 to 0.80, but specificity drops from 0.63 to 0.40
    let gym = regressing_bridge(0.70, 0.80, 0.40);
    let config = cfg(sample_changes());

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    assert_eq!(cycle.final_phase, ImprovementPhase::Decide);
    assert!(!cycle.regressions.is_empty(), "should detect regressions");
    assert!(
        matches!(&cycle.decision, Some(ImprovementDecision::Revert { reason }) if reason.contains("regression")),
        "should revert on regression: {:?}",
        cycle.decision,
    );
}

#[test]
fn cycle_records_baseline_accurately() {
    let gym = fixed_score_bridge(0.82);
    let config = ImprovementConfig {
        suite_id: "progressive".to_string(),
        min_net_improvement: 0.02,
        max_single_regression: 0.05,
        proposed_changes: vec![],
    };

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    assert!(
        (cycle.baseline.overall - 0.82).abs() < 1e-9,
        "baseline should be 0.82"
    );
    assert_eq!(cycle.baseline.suite_id, "progressive");
}

#[test]
fn summarize_cycle_includes_key_info() {
    let gym = improving_bridge(0.70, 0.75);
    let config = cfg(sample_changes());

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    let summary = summarize_cycle(&cycle);
    assert!(
        summary.contains("Baseline"),
        "summary should contain baseline"
    );
    assert!(
        summary.contains("Post-change"),
        "summary should contain post-change"
    );
    assert!(
        summary.contains("Decision"),
        "summary should contain decision"
    );
    assert!(
        summary.contains("COMMIT"),
        "summary should show commit decision"
    );
}

#[test]
fn bridge_error_propagates_from_cycle() {
    let transport = InMemoryBridgeTransport::new("gym-fail", |_method, _params| {
        Err(simard::bridge::BridgeErrorPayload {
            code: -32603,
            message: "gym server crashed".to_string(),
        })
    });
    let gym = GymBridge::new(Box::new(transport));
    let config = cfg(sample_changes());

    let err = run_improvement_cycle(&gym, &config).expect_err("should propagate bridge error");
    let msg = err.to_string();
    assert!(
        msg.contains("gym server crashed"),
        "error should contain root cause: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Self-relaunch tests
// ---------------------------------------------------------------------------

#[test]
fn default_gates_is_ordered() {
    let gates = default_gates();
    assert_eq!(gates[0], RelaunchGate::Smoke);
    assert_eq!(gates[1], RelaunchGate::UnitTest);
    assert_eq!(gates[2], RelaunchGate::GymBaseline);
    assert_eq!(gates[3], RelaunchGate::BridgeHealth);
}

#[test]
fn all_gates_passed_with_mixed_results() {
    let results = vec![
        GateResult {
            gate: RelaunchGate::Smoke,
            passed: true,
            detail: "ok".to_string(),
        },
        GateResult {
            gate: RelaunchGate::UnitTest,
            passed: true,
            detail: "ok".to_string(),
        },
        GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: false,
            detail: "gym failed".to_string(),
        },
    ];
    assert!(!all_gates_passed(&results));
}

#[test]
fn handover_rejects_pid_zero() {
    let binary = PathBuf::from("/usr/bin/true");
    let err = handover(0, &binary).unwrap_err();
    assert!(err.to_string().contains("current_pid"));
}

#[test]
fn handover_rejects_nonexistent_binary() {
    let missing = PathBuf::from("/tmp/simard-nonexistent-canary-test-82719");
    let err = handover(9999, &missing).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn handover_accepts_valid_file() {
    let binary = PathBuf::from("/usr/bin/true");
    if binary.exists() {
        handover(9999, &binary).expect("handover with valid binary should succeed");
    }
}

#[test]
fn cycle_with_exact_threshold_improvement_commits() {
    // Net improvement of exactly 0.02 (the default threshold) should commit
    let gym = improving_bridge(0.70, 0.72);
    let config = cfg(sample_changes());

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    match &cycle.decision {
        Some(ImprovementDecision::Commit { net_improvement }) => {
            assert!(
                (*net_improvement - 0.02).abs() < 1e-9,
                "expected exactly 2% improvement"
            );
        }
        other => panic!("expected commit at exact threshold, got {other:?}"),
    }
}

#[test]
fn cycle_with_zero_net_change_reverts() {
    let gym = improving_bridge(0.70, 0.70);
    let config = cfg(sample_changes());

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    assert!(matches!(
        &cycle.decision,
        Some(ImprovementDecision::Revert { .. })
    ));
}

#[test]
fn cycle_with_negative_improvement_reverts() {
    let gym = improving_bridge(0.70, 0.65);
    let config = cfg(sample_changes());

    let cycle = run_improvement_cycle(&gym, &config).expect("cycle should succeed");
    assert!(matches!(
        &cycle.decision,
        Some(ImprovementDecision::Revert { .. })
    ));
    assert!(
        !cycle.regressions.is_empty(),
        "negative improvement should show regressions"
    );
}
