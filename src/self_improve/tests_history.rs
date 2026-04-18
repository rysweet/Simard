use super::history::ImprovementHistory;
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

fn make_cycle(
    overall: f64,
    decision: ImprovementDecision,
    changes: Vec<ProposedChange>,
) -> ImprovementCycle {
    ImprovementCycle {
        baseline: make_score(overall),
        proposed_changes: changes,
        post_score: None,
        regressions: Vec::new(),
        decision: Some(decision),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    }
}

fn temp_history() -> (tempfile::TempDir, ImprovementHistory) {
    let dir = tempfile::TempDir::new().unwrap();
    let hist = ImprovementHistory::open(dir.path()).unwrap();
    (dir, hist)
}

#[test]
fn empty_history_loads_empty() {
    let (_dir, hist) = temp_history();
    let cycles = hist.load().unwrap();
    assert!(cycles.is_empty());
    assert_eq!(hist.cycle_count().unwrap(), 0);
}

#[test]
fn append_and_load_single_cycle() {
    let (_dir, hist) = temp_history();
    let cycle = make_cycle(
        0.7,
        ImprovementDecision::Commit {
            net_improvement: 0.05,
        },
        vec![],
    );
    hist.append(&cycle).unwrap();
    let loaded = hist.load().unwrap();
    assert_eq!(loaded.len(), 1);
    assert!((loaded[0].baseline.overall - 0.7).abs() < 1e-9);
}

#[test]
fn append_multiple_cycles() {
    let (_dir, hist) = temp_history();
    for i in 0..5 {
        let cycle = make_cycle(
            0.5 + (i as f64) * 0.05,
            ImprovementDecision::Commit {
                net_improvement: 0.05,
            },
            vec![],
        );
        hist.append(&cycle).unwrap();
    }
    assert_eq!(hist.cycle_count().unwrap(), 5);
}

#[test]
fn corrupt_lines_skipped() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("improvement_history.jsonl");
    // Write a valid line, a corrupt line, then another valid line
    let cycle = make_cycle(
        0.7,
        ImprovementDecision::Commit {
            net_improvement: 0.05,
        },
        vec![],
    );
    let json = serde_json::to_string(&cycle).unwrap();
    std::fs::write(&path, format!("{json}\nNOT_VALID_JSON\n{json}\n")).unwrap();

    let hist = ImprovementHistory::open_file(path);
    let loaded = hist.load().unwrap();
    assert_eq!(loaded.len(), 2);
}

#[test]
fn blank_lines_skipped() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("improvement_history.jsonl");
    let cycle = make_cycle(
        0.7,
        ImprovementDecision::Commit {
            net_improvement: 0.05,
        },
        vec![],
    );
    let json = serde_json::to_string(&cycle).unwrap();
    std::fs::write(&path, format!("{json}\n\n\n{json}\n")).unwrap();

    let hist = ImprovementHistory::open_file(path);
    let loaded = hist.load().unwrap();
    assert_eq!(loaded.len(), 2);
}

#[test]
fn was_previously_reverted_true() {
    let (_dir, hist) = temp_history();
    let change = ProposedChange {
        file_path: "src/lib.rs".into(),
        description: "refactor error handling".into(),
        expected_impact: "fewer panics".into(),
    };
    let cycle = make_cycle(
        0.5,
        ImprovementDecision::Revert {
            reason: "too risky".into(),
        },
        vec![change.clone()],
    );
    hist.append(&cycle).unwrap();
    assert!(hist.was_previously_reverted(&change).unwrap());
}

#[test]
fn was_previously_reverted_false_when_committed() {
    let (_dir, hist) = temp_history();
    let change = ProposedChange {
        file_path: "src/lib.rs".into(),
        description: "refactor error handling".into(),
        expected_impact: "fewer panics".into(),
    };
    let cycle = make_cycle(
        0.5,
        ImprovementDecision::Commit {
            net_improvement: 0.1,
        },
        vec![change.clone()],
    );
    hist.append(&cycle).unwrap();
    assert!(!hist.was_previously_reverted(&change).unwrap());
}

#[test]
fn was_previously_reverted_false_when_empty() {
    let (_dir, hist) = temp_history();
    let change = ProposedChange {
        file_path: "src/lib.rs".into(),
        description: "anything".into(),
        expected_impact: "anything".into(),
    };
    assert!(!hist.was_previously_reverted(&change).unwrap());
}

#[test]
fn dedup_proposals_filters_reverted() {
    let (_dir, hist) = temp_history();
    let reverted_change = ProposedChange {
        file_path: "a.rs".into(),
        description: "bad change".into(),
        expected_impact: "none".into(),
    };
    let good_change = ProposedChange {
        file_path: "b.rs".into(),
        description: "good change".into(),
        expected_impact: "improvement".into(),
    };
    let cycle = make_cycle(
        0.5,
        ImprovementDecision::Revert {
            reason: "regression".into(),
        },
        vec![reverted_change.clone()],
    );
    hist.append(&cycle).unwrap();

    let filtered = hist
        .dedup_proposals(&[reverted_change, good_change.clone()])
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].file_path, "b.rs");
}

#[test]
fn dedup_proposals_keeps_all_when_no_reverts() {
    let (_dir, hist) = temp_history();
    let proposals = vec![
        ProposedChange {
            file_path: "a.rs".into(),
            description: "x".into(),
            expected_impact: "y".into(),
        },
        ProposedChange {
            file_path: "b.rs".into(),
            description: "x".into(),
            expected_impact: "y".into(),
        },
    ];
    let filtered = hist.dedup_proposals(&proposals).unwrap();
    assert_eq!(filtered.len(), 2);
}

#[test]
fn history_path_reflects_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let hist = ImprovementHistory::open(dir.path()).unwrap();
    assert!(hist.path().ends_with("improvement_history.jsonl"));
    assert!(hist.path().starts_with(dir.path()));
}

#[test]
fn open_file_direct() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("custom.jsonl");
    let hist = ImprovementHistory::open_file(path.clone());
    assert_eq!(hist.path(), path);
}

#[test]
fn prune_keeps_most_recent() {
    let (_dir, hist) = temp_history();
    for i in 0..5 {
        let cycle = make_cycle(
            0.5 + (i as f64) * 0.05,
            ImprovementDecision::Commit {
                net_improvement: 0.05,
            },
            vec![],
        );
        hist.append(&cycle).unwrap();
    }
    assert_eq!(hist.cycle_count().unwrap(), 5);
    hist.prune(3).unwrap();
    let cycles = hist.load().unwrap();
    assert_eq!(cycles.len(), 3);
    assert!((cycles[0].baseline.overall - 0.60).abs() < 1e-9);
    assert!((cycles[2].baseline.overall - 0.70).abs() < 1e-9);
}

#[test]
fn prune_noop_when_under_limit() {
    let (_dir, hist) = temp_history();
    let cycle = make_cycle(
        0.7,
        ImprovementDecision::Commit {
            net_improvement: 0.05,
        },
        vec![],
    );
    hist.append(&cycle).unwrap();
    hist.prune(10).unwrap();
    assert_eq!(hist.cycle_count().unwrap(), 1);
}

#[test]
fn prune_empty_history_noop() {
    let (_dir, hist) = temp_history();
    hist.prune(5).unwrap();
    assert_eq!(hist.cycle_count().unwrap(), 0);
}

#[test]
fn reverted_count_zero_when_empty() {
    let (_dir, hist) = temp_history();
    let change = ProposedChange {
        file_path: "src/lib.rs".into(),
        description: "anything".into(),
        expected_impact: "anything".into(),
    };
    assert_eq!(hist.reverted_count(&change).unwrap(), 0);
}

#[test]
fn reverted_count_tracks_multiple_reverts() {
    let (_dir, hist) = temp_history();
    let change = ProposedChange {
        file_path: "src/lib.rs".into(),
        description: "risky change".into(),
        expected_impact: "might break".into(),
    };
    for _ in 0..3 {
        let cycle = make_cycle(
            0.5,
            ImprovementDecision::Revert {
                reason: "regression".into(),
            },
            vec![change.clone()],
        );
        hist.append(&cycle).unwrap();
    }
    let committed = make_cycle(
        0.6,
        ImprovementDecision::Commit {
            net_improvement: 0.1,
        },
        vec![change.clone()],
    );
    hist.append(&committed).unwrap();
    assert_eq!(hist.reverted_count(&change).unwrap(), 3);
}

#[test]
fn reverted_count_ignores_different_proposals() {
    let (_dir, hist) = temp_history();
    let change_a = ProposedChange {
        file_path: "a.rs".into(),
        description: "change a".into(),
        expected_impact: "a".into(),
    };
    let change_b = ProposedChange {
        file_path: "b.rs".into(),
        description: "change b".into(),
        expected_impact: "b".into(),
    };
    let cycle = make_cycle(
        0.5,
        ImprovementDecision::Revert {
            reason: "regression".into(),
        },
        vec![change_a],
    );
    hist.append(&cycle).unwrap();
    assert_eq!(hist.reverted_count(&change_b).unwrap(), 0);
}

#[test]
fn last_n_cycles_returns_recent() {
    let (_dir, hist) = temp_history();
    for i in 0..5 {
        let cycle = make_cycle(
            0.5 + (i as f64) * 0.05,
            ImprovementDecision::Commit {
                net_improvement: 0.05,
            },
            vec![],
        );
        hist.append(&cycle).unwrap();
    }
    let last2 = hist.last_n_cycles(2).unwrap();
    assert_eq!(last2.len(), 2);
    // Should be the last two: overall 0.65 and 0.70
    assert!((last2[0].baseline.overall - 0.65).abs() < 1e-9);
    assert!((last2[1].baseline.overall - 0.70).abs() < 1e-9);
}

#[test]
fn last_n_cycles_returns_all_when_fewer() {
    let (_dir, hist) = temp_history();
    let cycle = make_cycle(
        0.7,
        ImprovementDecision::Commit {
            net_improvement: 0.05,
        },
        vec![],
    );
    hist.append(&cycle).unwrap();
    let result = hist.last_n_cycles(10).unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn last_n_cycles_empty_history() {
    let (_dir, hist) = temp_history();
    let result = hist.last_n_cycles(5).unwrap();
    assert!(result.is_empty());
}
