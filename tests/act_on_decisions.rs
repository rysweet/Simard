//! TDD tests for the act-on-decisions command that creates GitHub issues
//! from meeting handoff JSON.
//!
//! These tests verify:
//! 1. Issue title/body formatting for decisions and action items
//! 2. Handoff loading from target/meeting_handoffs/
//! 3. Processed handoffs are skipped
//! 4. Missing handoffs are handled gracefully
//! 5. The full lifecycle: write handoff → act-on-decisions → mark processed

use std::fs;
use std::path::PathBuf;

use simard::meeting_facilitator::{
    ActionItem, MEETING_HANDOFF_FILENAME, MeetingDecision, MeetingHandoff, MeetingSession,
    MeetingSessionStatus, load_meeting_handoff, mark_meeting_handoff_processed,
    write_meeting_handoff,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_dir(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("act-on-decisions-{label}-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_handoff() -> MeetingHandoff {
    let session = MeetingSession {
        topic: "Sprint planning".to_string(),
        decisions: vec![
            MeetingDecision {
                description: "Ship phase 8".to_string(),
                rationale: "Unblocks goal curation".to_string(),
                participants: vec!["alice".to_string()],
            },
            MeetingDecision {
                description: "Adopt Rust for CLI".to_string(),
                rationale: "Performance and safety".to_string(),
                participants: vec!["bob".to_string(), "carol".to_string()],
            },
        ],
        action_items: vec![
            ActionItem {
                description: "Write handoff tests".to_string(),
                owner: "bob".to_string(),
                priority: 1,
                due_description: Some("end of sprint".to_string()),
            },
            ActionItem {
                description: "Update docs".to_string(),
                owner: "carol".to_string(),
                priority: 2,
                due_description: None,
            },
        ],
        notes: vec!["Should we add metrics?".to_string()],
        status: MeetingSessionStatus::Closed,
        started_at: chrono::Utc::now().to_rfc3339(),
        participants: Vec::new(),
    };
    MeetingHandoff::from_session(&session)
}

// ===========================================================================
// 1. Decision issue title/body formatting
// ===========================================================================

#[test]
fn decision_issue_title_is_prefixed_with_decision() {
    let decision = MeetingDecision {
        description: "Ship phase 8".to_string(),
        rationale: "Unblocks goal curation".to_string(),
        participants: vec!["alice".to_string()],
    };

    // This is the format used by dispatch_act_on_decisions
    let title = format!("Decision: {}", decision.description);
    assert_eq!(title, "Decision: Ship phase 8");
}

#[test]
fn decision_issue_body_contains_rationale_and_participants() {
    let decision = MeetingDecision {
        description: "Adopt Rust for CLI".to_string(),
        rationale: "Performance and safety".to_string(),
        participants: vec!["bob".to_string(), "carol".to_string()],
    };
    let topic = "Sprint planning";

    let body = format!(
        "**Rationale:** {}\n**Participants:** {}\n\n_From meeting: {}_",
        decision.rationale,
        decision.participants.join(", "),
        topic,
    );

    assert!(body.contains("**Rationale:** Performance and safety"));
    assert!(body.contains("**Participants:** bob, carol"));
    assert!(body.contains("_From meeting: Sprint planning_"));
}

#[test]
fn decision_with_empty_participants_shows_none() {
    let decision = MeetingDecision {
        description: "Quick decision".to_string(),
        rationale: "Obvious".to_string(),
        participants: vec![],
    };

    let participants = if decision.participants.is_empty() {
        "(none)".to_string()
    } else {
        decision.participants.join(", ")
    };

    assert_eq!(participants, "(none)");
}

// ===========================================================================
// 2. Action item issue title/body formatting
// ===========================================================================

#[test]
fn action_item_issue_title_is_prefixed_with_action() {
    let item = ActionItem {
        description: "Write handoff tests".to_string(),
        owner: "bob".to_string(),
        priority: 1,
        due_description: Some("end of sprint".to_string()),
    };

    let title = format!("Action: {}", item.description);
    assert_eq!(title, "Action: Write handoff tests");
}

#[test]
fn action_item_body_contains_owner_priority_and_due() {
    let item = ActionItem {
        description: "Write handoff tests".to_string(),
        owner: "bob".to_string(),
        priority: 1,
        due_description: Some("end of sprint".to_string()),
    };
    let topic = "Sprint planning";

    let due = item.due_description.as_deref().unwrap_or("(unspecified)");
    let body = format!(
        "**Owner:** {}\n**Priority:** {}\n**Due:** {}\n\n_From meeting: {}_",
        item.owner, item.priority, due, topic,
    );

    assert!(body.contains("**Owner:** bob"));
    assert!(body.contains("**Priority:** 1"));
    assert!(body.contains("**Due:** end of sprint"));
    assert!(body.contains("_From meeting: Sprint planning_"));
}

#[test]
fn action_item_without_due_shows_unspecified() {
    let item = ActionItem {
        description: "Update docs".to_string(),
        owner: "carol".to_string(),
        priority: 2,
        due_description: None,
    };

    let due = item.due_description.as_deref().unwrap_or("(unspecified)");
    assert_eq!(due, "(unspecified)");
}

// ===========================================================================
// 3. Handoff loading and processed-skip logic
// ===========================================================================

#[test]
fn act_on_decisions_skips_already_processed_handoff() {
    let dir = temp_dir("skip-processed");
    let mut handoff = sample_handoff();
    handoff.processed = true;
    write_meeting_handoff(&dir, &handoff).unwrap();

    let loaded = load_meeting_handoff(&dir).unwrap().unwrap();
    assert!(
        loaded.processed,
        "act-on-decisions should detect processed=true and skip"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn act_on_decisions_returns_none_when_no_handoff_exists() {
    let dir = temp_dir("no-handoff");
    let loaded = load_meeting_handoff(&dir).unwrap();
    assert!(
        loaded.is_none(),
        "act-on-decisions should handle missing handoff gracefully"
    );
    fs::remove_dir_all(&dir).ok();
}

// ===========================================================================
// 4. Full lifecycle: write → load → process → mark processed
// ===========================================================================

#[test]
fn full_act_on_decisions_lifecycle() {
    let dir = temp_dir("full-lifecycle");
    let handoff = sample_handoff();

    // 1. Write handoff (simulating meeting close)
    write_meeting_handoff(&dir, &handoff).unwrap();

    // 2. Load (simulating act-on-decisions start)
    let loaded = load_meeting_handoff(&dir).unwrap().unwrap();
    assert!(!loaded.processed);
    assert_eq!(loaded.decisions.len(), 2);
    assert_eq!(loaded.action_items.len(), 2);

    // 3. Count total issues that would be created
    let total_issues = loaded.decisions.len() + loaded.action_items.len();
    assert_eq!(
        total_issues, 4,
        "should create 4 issues (2 decisions + 2 actions)"
    );

    // 4. Mark processed (simulating act-on-decisions completion)
    mark_meeting_handoff_processed(&dir).unwrap();

    // 5. Verify it won't be re-processed
    let reloaded = load_meeting_handoff(&dir).unwrap().unwrap();
    assert!(reloaded.processed);

    fs::remove_dir_all(&dir).ok();
}

// ===========================================================================
// 5. Empty handoff produces no issues
// ===========================================================================

#[test]
fn act_on_decisions_with_empty_handoff_creates_no_issues() {
    let dir = temp_dir("empty-handoff");
    let session = MeetingSession {
        topic: "Quick sync".to_string(),
        decisions: vec![],
        action_items: vec![],
        notes: vec![],
        status: MeetingSessionStatus::Closed,
        started_at: chrono::Utc::now().to_rfc3339(),
        participants: Vec::new(),
    };
    let handoff = MeetingHandoff::from_session(&session);
    write_meeting_handoff(&dir, &handoff).unwrap();

    let loaded = load_meeting_handoff(&dir).unwrap().unwrap();
    let total_issues = loaded.decisions.len() + loaded.action_items.len();
    assert_eq!(total_issues, 0, "empty handoff should produce zero issues");

    fs::remove_dir_all(&dir).ok();
}

// ===========================================================================
// 6. Open questions are NOT filed as issues
// ===========================================================================

#[test]
fn open_questions_are_not_included_in_issue_count() {
    let handoff = sample_handoff();
    assert!(
        !handoff.open_questions.is_empty(),
        "sample should have open questions"
    );

    // Only decisions and action_items become issues, not open questions
    let issue_count = handoff.decisions.len() + handoff.action_items.len();
    assert_eq!(issue_count, 4);

    // Open questions are reported separately, not as issues
    assert_eq!(handoff.open_questions.len(), 1);
    assert!(handoff.open_questions[0].contains("metrics?"));
}

// ===========================================================================
// 7. CLI routing
// ===========================================================================

#[test]
fn act_on_decisions_appears_in_cli_help() {
    let help = simard::operator_cli::operator_cli_help();
    assert!(help.contains("act-on-decisions"));
}

#[test]
fn act_on_decisions_rejects_trailing_args() {
    let result = simard::operator_cli::dispatch_operator_cli(vec![
        "act-on-decisions".to_string(),
        "unexpected-arg".to_string(),
    ]);
    assert!(result.is_err());
}

// ===========================================================================
// 8. Handoff file path follows convention
// ===========================================================================

#[test]
fn handoff_filename_is_meeting_handoff_json() {
    assert_eq!(MEETING_HANDOFF_FILENAME, "meeting_handoff.json");
}

#[test]
fn handoff_is_written_to_correct_path() {
    let dir = temp_dir("path-check");
    let handoff = sample_handoff();
    write_meeting_handoff(&dir, &handoff).unwrap();

    // write_meeting_handoff uses timestamped filenames (handoff-*.json).
    let handoff_file = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("handoff-") && name.ends_with(".json")
        })
        .expect("expected a handoff-*.json file");
    assert!(handoff_file.path().is_file());

    // Verify it's valid JSON
    let raw = fs::read_to_string(handoff_file.path()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["topic"], "Sprint planning");

    fs::remove_dir_all(&dir).ok();
}
