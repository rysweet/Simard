use super::improve::{
    ReviewVerdict, TacticProposal, analyze_and_review, analyze_failures, review_proposal,
};
use super::target_loader::TargetSet;
use super::types::{Outcome, OutcomeCode, RunReport, Strategy, Target, TargetFamily};

fn target(id: &str, project: &str, harness: &str, file: &str) -> Target {
    Target {
        id: id.to_string(),
        project: project.to_string(),
        commit: "c".to_string(),
        harness: harness.to_string(),
        file: file.to_string(),
        line: 100,
        line_end: None,
        family: TargetFamily::Frontier,
    }
}

fn target_set() -> TargetSet {
    TargetSet {
        snapshot: "snap".to_string(),
        pinned: vec![
            target(
                "libraw-fuji-480",
                "libraw",
                "libraw_raf_fuzzer",
                "src/metadata/fuji.cpp",
            ),
            target("liboqs-kem-88", "liboqs", "kem_fuzzer", "src/kem/kem.c"),
            target("widget-logic-12", "widget", "logic_fuzzer", "src/logic.c"),
            target("reached-one", "libraw", "libraw_raf_fuzzer", "src/x.cpp"),
        ],
        held_out_fresh: Vec::new(),
    }
}

fn outcome(id: &str, code: OutcomeCode) -> Outcome {
    Outcome {
        target_id: id.to_string(),
        family: TargetFamily::Frontier,
        code,
        cost_usd: 0.0,
    }
}

fn report_with(outcomes: Vec<Outcome>) -> RunReport {
    RunReport {
        run_id: "r1".to_string(),
        model: "m".to_string(),
        strategy: Strategy::Baseline,
        snapshot: "snap".to_string(),
        started_at_unix_ms: 0,
        outcomes,
        offline_scaffold: true,
    }
}

#[test]
fn analyze_only_proposes_for_unreached_failures() {
    let report = report_with(vec![
        outcome("libraw-fuji-480", OutcomeCode::WrongInput),
        outcome("liboqs-kem-88", OutcomeCode::TimedOut),
        outcome("widget-logic-12", OutcomeCode::NoSubmission),
        outcome("reached-one", OutcomeCode::Reached), // no proposal
    ]);
    let proposals = analyze_failures(&report, &target_set());
    assert_eq!(proposals.len(), 3);
    let ids: Vec<&str> = proposals.iter().map(|p| p.target_id.as_str()).collect();
    assert!(ids.contains(&"libraw-fuji-480"));
    assert!(!ids.contains(&"reached-one"));
}

#[test]
fn abstain_is_not_treated_as_failure() {
    let report = report_with(vec![outcome("libraw-fuji-480", OutcomeCode::Abstained)]);
    assert!(analyze_failures(&report, &target_set()).is_empty());
}

#[test]
fn analyst_tactics_are_general_and_accepted_by_gate() {
    let report = report_with(vec![
        outcome("libraw-fuji-480", OutcomeCode::WrongInput),
        outcome("liboqs-kem-88", OutcomeCode::WrongInput),
        outcome("widget-logic-12", OutcomeCode::WrongInput),
    ]);
    let set = target_set();
    let proposals = analyze_failures(&report, &set);
    for p in &proposals {
        let reviewed = review_proposal(p, &set);
        assert_eq!(
            reviewed.verdict,
            ReviewVerdict::Accept,
            "analyst tactic should generalise: {}",
            p.tactic
        );
    }
    // Category-specific wording.
    let decoder = proposals
        .iter()
        .find(|p| p.target_id == "libraw-fuji-480")
        .unwrap();
    assert!(decoder.tactic.contains("format-gated decoders"));
    let crypto = proposals
        .iter()
        .find(|p| p.target_id == "liboqs-kem-88")
        .unwrap();
    assert!(crypto.tactic.contains("cryptographic state machines"));
    let generic = proposals
        .iter()
        .find(|p| p.target_id == "widget-logic-12")
        .unwrap();
    assert!(generic.tactic.contains("guarding predicate"));
}

fn proposal(tactic: &str) -> TacticProposal {
    TacticProposal {
        id: "p".to_string(),
        target_id: "libraw-fuji-480".to_string(),
        tactic: tactic.to_string(),
        evidence: "ev".to_string(),
    }
}

#[test]
fn gate_rejects_memorised_input_language() {
    let set = target_set();
    for bad in [
        "Just hardcode the exact bytes that worked before.",
        "Memorize this input and replay it.",
        "Reuse the known input for the target.",
    ] {
        let reviewed = review_proposal(&proposal(bad), &set);
        assert_eq!(
            reviewed.verdict,
            ReviewVerdict::Reject,
            "should reject: {bad}"
        );
    }
}

#[test]
fn gate_rejects_target_specific_keys() {
    let set = target_set();
    // Names a specific target id.
    let by_id = proposal("For libraw-fuji-480 specifically, flip the branch.");
    assert_eq!(review_proposal(&by_id, &set).verdict, ReviewVerdict::Reject);

    // Keys off a specific project.
    let by_project = proposal("When fuzzing libraw, prefer RAF headers.");
    assert_eq!(
        review_proposal(&by_project, &set).verdict,
        ReviewVerdict::Reject
    );

    // Names a specific locator.
    let by_locator = proposal("Drive to libraw:src/metadata/fuji.cpp:100 directly.");
    assert_eq!(
        review_proposal(&by_locator, &set).verdict,
        ReviewVerdict::Reject
    );
}

#[test]
fn analyze_and_review_summarises_counts_and_phase5_note() {
    let report = report_with(vec![
        outcome("libraw-fuji-480", OutcomeCode::WrongInput),
        outcome("liboqs-kem-88", OutcomeCode::TimedOut),
    ]);
    let out = analyze_and_review(&report, &target_set());
    assert_eq!(out.analyzed, 2);
    assert_eq!(out.accepted, 2);
    assert_eq!(out.rejected, 0);
    assert_eq!(out.proposals.len(), 2);
    assert!(out.note.contains("Phase 5"));
    assert_eq!(out.proposals[0].verdict_label(), "ACCEPT");
}
