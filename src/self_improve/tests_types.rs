use super::types::*;
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::GymSuiteScore;

fn make_score(v: f64) -> GymSuiteScore {
    GymSuiteScore {
        suite_id: "test".into(),
        overall: v,
        dimensions: ScoreDimensions {
            factual_accuracy: v,
            specificity: v * 0.9,
            temporal_awareness: v * 0.8,
            source_attribution: v * 0.7,
            confidence_calibration: v * 0.85,
        },
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    }
}

#[test]
fn improvement_phase_display_all_variants() {
    assert_eq!(ImprovementPhase::Eval.to_string(), "eval");
    assert_eq!(ImprovementPhase::Analyze.to_string(), "analyze");
    assert_eq!(ImprovementPhase::Research.to_string(), "research");
    assert_eq!(ImprovementPhase::Improve.to_string(), "improve");
    assert_eq!(ImprovementPhase::ReEval.to_string(), "re-eval");
    assert_eq!(ImprovementPhase::Decide.to_string(), "decide");
}

#[test]
fn improvement_phase_clone_and_eq() {
    let phase = ImprovementPhase::Research;
    let cloned = phase;
    assert_eq!(phase, cloned);
    assert_ne!(ImprovementPhase::Eval, ImprovementPhase::Decide);
}

#[test]
fn proposed_change_construction() {
    let change = ProposedChange {
        file_path: "src/lib.rs".into(),
        description: "refactor error handling".into(),
        expected_impact: "reduce .expect() calls".into(),
    };
    assert_eq!(change.file_path, "src/lib.rs");
    assert!(!change.description.is_empty());
    assert!(!change.expected_impact.is_empty());
}

#[test]
fn proposed_change_clone_and_eq() {
    let change = ProposedChange {
        file_path: "a.rs".into(),
        description: "d".into(),
        expected_impact: "e".into(),
    };
    let cloned = change.clone();
    assert_eq!(change, cloned);
}

#[test]
fn improvement_decision_commit() {
    let d = ImprovementDecision::Commit {
        net_improvement: 0.05,
    };
    match &d {
        ImprovementDecision::Commit { net_improvement } => {
            assert!((net_improvement - 0.05).abs() < 1e-9);
        }
        _ => panic!("expected Commit"),
    }
}

#[test]
fn improvement_decision_revert() {
    let d = ImprovementDecision::Revert {
        reason: "regression too large".into(),
    };
    match &d {
        ImprovementDecision::Revert { reason } => {
            assert!(reason.contains("regression"));
        }
        _ => panic!("expected Revert"),
    }
}

#[test]
fn improvement_config_default_all_fields() {
    let cfg = ImprovementConfig::default();
    assert_eq!(cfg.suite_id, "progressive");
    assert!((cfg.min_net_improvement - 0.02).abs() < 1e-9);
    assert!((cfg.max_single_regression - 0.05).abs() < 1e-9);
    assert!(cfg.proposed_changes.is_empty());
    assert!(!cfg.auto_apply);
    assert!((cfg.weak_threshold - 0.6).abs() < 1e-9);
    assert!(cfg.target_dimension.is_none());
}

#[test]
fn improvement_config_custom_target_dimension() {
    let cfg = ImprovementConfig {
        target_dimension: Some("specificity".into()),
        ..Default::default()
    };
    assert_eq!(cfg.target_dimension.as_deref(), Some("specificity"));
}

#[test]
fn improvement_cycle_minimal() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.5),
        proposed_changes: Vec::new(),
        post_score: None,
        regressions: Vec::new(),
        decision: None,
        final_phase: ImprovementPhase::Eval,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    };
    assert!(cycle.proposed_changes.is_empty());
    assert!(cycle.post_score.is_none());
    assert!(cycle.decision.is_none());
    assert_eq!(cycle.final_phase, ImprovementPhase::Eval);
}

#[test]
fn improvement_cycle_with_target_dimension() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.5),
        proposed_changes: vec![ProposedChange {
            file_path: "src/a.rs".into(),
            description: "improve specificity".into(),
            expected_impact: "better scores".into(),
        }],
        post_score: Some(make_score(0.7)),
        regressions: Vec::new(),
        decision: Some(ImprovementDecision::Commit {
            net_improvement: 0.2,
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: vec!["specificity".into()],
        weak_dimension_details: Vec::new(),
        target_dimension: Some("specificity".into()),
    };
    assert_eq!(cycle.target_dimension.as_deref(), Some("specificity"));
    assert_eq!(cycle.proposed_changes.len(), 1);
    assert_eq!(cycle.weak_dimensions.len(), 1);
}

#[test]
fn improvement_cycle_display_contains_baseline() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.7),
        proposed_changes: Vec::new(),
        post_score: None,
        regressions: Vec::new(),
        decision: None,
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    };
    let display = cycle.to_string();
    assert!(display.contains("Baseline"));
    assert!(display.contains("70.0%"));
}

#[test]
fn is_committed_true_for_commit_decision() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.7),
        proposed_changes: Vec::new(),
        post_score: Some(make_score(0.8)),
        regressions: Vec::new(),
        decision: Some(ImprovementDecision::Commit {
            net_improvement: 0.1,
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    };
    assert!(cycle.is_committed());
    assert!(!cycle.is_reverted());
}

#[test]
fn is_reverted_true_for_revert_decision() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.7),
        proposed_changes: Vec::new(),
        post_score: None,
        regressions: Vec::new(),
        decision: Some(ImprovementDecision::Revert {
            reason: "test".into(),
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    };
    assert!(cycle.is_reverted());
    assert!(!cycle.is_committed());
}

#[test]
fn is_committed_and_reverted_false_when_no_decision() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.7),
        proposed_changes: Vec::new(),
        post_score: None,
        regressions: Vec::new(),
        decision: None,
        final_phase: ImprovementPhase::Eval,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    };
    assert!(!cycle.is_committed());
    assert!(!cycle.is_reverted());
}

#[test]
fn weak_dimension_serde_round_trip() {
    let wd = WeakDimension {
        name: "specificity".into(),
        deficit: 0.15,
    };
    let json = serde_json::to_string(&wd).unwrap();
    let parsed: WeakDimension = serde_json::from_str(&json).unwrap();
    assert_eq!(wd, parsed);
}

#[test]
fn improvement_phase_serde_round_trip() {
    for phase in [
        ImprovementPhase::Eval,
        ImprovementPhase::Analyze,
        ImprovementPhase::Research,
        ImprovementPhase::Improve,
        ImprovementPhase::ReEval,
        ImprovementPhase::Decide,
    ] {
        let json = serde_json::to_string(&phase).unwrap();
        let parsed: ImprovementPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(phase, parsed);
    }
}

#[test]
fn improvement_decision_serde_round_trip() {
    let commit = ImprovementDecision::Commit {
        net_improvement: 0.05,
    };
    let json = serde_json::to_string(&commit).unwrap();
    let parsed: ImprovementDecision = serde_json::from_str(&json).unwrap();
    assert_eq!(commit, parsed);

    let revert = ImprovementDecision::Revert {
        reason: "regression too large".into(),
    };
    let json = serde_json::to_string(&revert).unwrap();
    let parsed: ImprovementDecision = serde_json::from_str(&json).unwrap();
    assert_eq!(revert, parsed);
}

#[test]
fn improvement_cycle_serde_round_trip() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.7),
        proposed_changes: vec![ProposedChange {
            file_path: "src/a.rs".into(),
            description: "fix".into(),
            expected_impact: "better".into(),
        }],
        post_score: Some(make_score(0.8)),
        regressions: Vec::new(),
        decision: Some(ImprovementDecision::Commit {
            net_improvement: 0.1,
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: vec!["specificity".into()],
        weak_dimension_details: vec![WeakDimension {
            name: "specificity".into(),
            deficit: 0.1,
        }],
        target_dimension: Some("specificity".into()),
    };
    let json = serde_json::to_string(&cycle).unwrap();
    let parsed: ImprovementCycle = serde_json::from_str(&json).unwrap();
    assert_eq!(cycle.baseline.overall, parsed.baseline.overall);
    assert_eq!(cycle.decision, parsed.decision);
    assert_eq!(
        cycle.weak_dimension_details.len(),
        parsed.weak_dimension_details.len()
    );
    assert_eq!(cycle.target_dimension, parsed.target_dimension);
}

#[test]
fn improvement_cycle_serde_defaults_for_missing_details() {
    // Verify that weak_dimension_details defaults to empty when missing from JSON
    // (for backward compatibility with cycles serialized before this field was added)
    let json = r#"{
        "baseline": {"suite_id":"t","overall":0.5,"dimensions":{"factual_accuracy":0.5,"specificity":0.45,"temporal_awareness":0.4,"source_attribution":0.35,"confidence_calibration":0.425},"scenario_count":4,"scenarios_passed":4,"pass_rate":1.0,"recorded_at_unix_ms":null},
        "proposed_changes": [],
        "post_score": null,
        "regressions": [],
        "decision": null,
        "final_phase": "Eval",
        "weak_dimensions": [],
        "target_dimension": null
    }"#;
    let parsed: ImprovementCycle = serde_json::from_str(json).unwrap();
    assert!(parsed.weak_dimension_details.is_empty());
}
