use super::handoff::*;
use super::*;
use crate::meeting_facilitator::types::{ActionItem, MeetingDecision, MeetingSessionStatus};

/// Build a minimal session for testing.
fn make_session(
    topic: &str,
    notes: Vec<&str>,
    decisions: Vec<MeetingDecision>,
    action_items: Vec<ActionItem>,
    participants: Vec<&str>,
) -> MeetingSession {
    MeetingSession {
        topic: topic.to_string(),
        decisions,
        action_items,
        notes: notes.into_iter().map(String::from).collect(),
        status: MeetingSessionStatus::Closed,
        started_at: chrono::Utc::now().to_rfc3339(),
        participants: participants.into_iter().map(String::from).collect(),
        explicit_questions: Vec::new(),
        themes: Vec::new(),
    }
}

fn sample_decision() -> MeetingDecision {
    MeetingDecision {
        description: "Ship phase 8".to_string(),
        rationale: "Unblocks goal curation".to_string(),
        participants: vec!["alice".to_string()],
    }
}

fn sample_action() -> ActionItem {
    ActionItem {
        description: "Write tests".to_string(),
        owner: "bob".to_string(),
        priority: 1,
        due_description: Some("end of sprint".to_string()),
    }
}

// -----------------------------------------------------------------------
// is_open_question / is_rhetorical unit tests
// -----------------------------------------------------------------------

#[test]
fn genuine_question_is_extracted() {
    assert!(is_open_question("What is the timeline for phase 9?"));
    assert!(is_open_question(
        "How should we handle backward compatibility?"
    ));
}

#[test]
fn rhetorical_short_question_is_filtered() {
    assert!(!is_open_question("Why not?"));
    assert!(!is_open_question("Right?"));
    assert!(!is_open_question("Isn't it?"));
    assert!(!is_open_question("So what?"));
}

#[test]
fn rhetorical_trailing_pattern_is_filtered() {
    assert!(!is_open_question(
        "We should deploy on Monday, don't you think?"
    ));
    assert!(!is_open_question("The fix looks good, right?"));
}

#[test]
fn explicit_markers_without_question_mark() {
    assert!(is_open_question("OPEN: decide on migration strategy"));
    assert!(is_open_question("todo: finalize API contract"));
    assert!(is_open_question("Question: ownership of the rollback plan"));
    assert!(is_open_question("TBD: release date"));
    assert!(is_open_question(
        "UNRESOLVED: cross-team dependency on auth service"
    ));
}

#[test]
fn plain_note_without_marker_or_question_mark_is_ignored() {
    assert!(!is_open_question("We agreed to use Postgres"));
    assert!(!is_open_question("Deployment target is Friday"));
}

// -----------------------------------------------------------------------
// MeetingHandoff::from_session
// -----------------------------------------------------------------------

#[test]
fn from_session_populates_all_fields() {
    let session = make_session(
        "Sprint planning",
        vec!["note 1", "What is the timeline for phase 9?"],
        vec![sample_decision()],
        vec![sample_action()],
        vec!["alice", "bob"],
    );

    let handoff = MeetingHandoff::from_session(&session);

    assert_eq!(handoff.topic, "Sprint planning");
    assert!(!handoff.started_at.is_empty());
    assert!(!handoff.closed_at.is_empty());
    assert_eq!(handoff.decisions.len(), 1);
    assert_eq!(handoff.action_items.len(), 1);
    assert_eq!(
        handoff.open_questions,
        vec![OpenQuestion {
            text: "What is the timeline for phase 9?".to_string(),
            explicit: false,
        }]
    );
    assert!(!handoff.processed);
    assert!(handoff.duration_secs.is_some());
    assert_eq!(handoff.transcript.len(), 2);
    // alice (session) + bob (session) — alice is already in session.participants
    // alice also appears in decision.participants but should not be duplicated.
    assert!(handoff.participants.contains(&"alice".to_string()));
    assert!(handoff.participants.contains(&"bob".to_string()));
}

#[test]
fn from_session_collects_unique_participants() {
    let session = make_session(
        "Dedup check",
        vec![],
        vec![MeetingDecision {
            description: "d".to_string(),
            rationale: "r".to_string(),
            participants: vec!["alice".to_string(), "charlie".to_string()],
        }],
        vec![ActionItem {
            description: "a".to_string(),
            owner: "alice".to_string(),
            priority: 1,
            due_description: None,
        }],
        vec!["alice", "bob"],
    );

    let handoff = MeetingHandoff::from_session(&session);
    // alice appears in session, decision, and action but should appear once.
    assert_eq!(
        handoff
            .participants
            .iter()
            .filter(|p| *p == "alice")
            .count(),
        1
    );
    // charlie from the decision participant list should be added.
    assert!(handoff.participants.contains(&"charlie".to_string()));
}

#[test]
fn from_session_empty_session() {
    let session = make_session("Empty", vec![], vec![], vec![], vec![]);
    let handoff = MeetingHandoff::from_session(&session);

    assert_eq!(handoff.topic, "Empty");
    assert!(handoff.decisions.is_empty());
    assert!(handoff.action_items.is_empty());
    assert!(handoff.open_questions.is_empty());
    assert!(handoff.transcript.is_empty());
    assert!(handoff.participants.is_empty());
}

#[test]
fn from_session_only_rhetorical_questions() {
    let session = make_session(
        "Rhetorical",
        vec![
            "Why not?",
            "Right?",
            "Looks good, don't you think?",
            "Plain note without question",
        ],
        vec![],
        vec![],
        vec![],
    );
    let handoff = MeetingHandoff::from_session(&session);
    assert!(
        handoff.open_questions.is_empty(),
        "Rhetorical questions should not appear in open_questions: {:?}",
        handoff.open_questions
    );
}

#[test]
fn from_session_explicit_markers_no_question_mark() {
    let session = make_session(
        "Markers",
        vec![
            "OPEN: decide on migration strategy",
            "TODO: finalize API contract",
            "Regular note",
        ],
        vec![],
        vec![],
        vec![],
    );
    let handoff = MeetingHandoff::from_session(&session);
    assert_eq!(handoff.open_questions.len(), 2);
    assert!(handoff.open_questions[0].text.starts_with("OPEN:"));
    assert!(!handoff.open_questions[0].explicit);
    assert!(handoff.open_questions[1].text.starts_with("TODO:"));
    assert!(!handoff.open_questions[1].explicit);
}

#[test]
fn from_session_populates_themes_from_notes() {
    let session = make_session(
        "Theme test",
        vec![
            "We discussed testing strategies.",
            "Testing coverage needs improvement.",
            "More testing will help quality.",
            "Deployment pipelines look good.",
        ],
        vec![],
        vec![],
        vec![],
    );
    let handoff = MeetingHandoff::from_session(&session);
    assert!(
        handoff.themes.contains(&"testing".to_string()),
        "Expected 'testing' theme from recurring notes: {:?}",
        handoff.themes
    );
}

#[test]
fn from_session_empty_notes_no_themes() {
    let session = make_session("No themes", vec![], vec![], vec![], vec![]);
    let handoff = MeetingHandoff::from_session(&session);
    assert!(handoff.themes.is_empty());
}

#[test]
fn from_session_explicit_themes_come_first() {
    let mut session = make_session(
        "Explicit theme test",
        vec![
            "We discussed testing strategies.",
            "Testing coverage needs improvement.",
            "More testing will help quality.",
        ],
        vec![],
        vec![],
        vec![],
    );
    session.themes = vec!["performance".to_string(), "reliability".to_string()];
    let handoff = MeetingHandoff::from_session(&session);
    // Explicit themes must appear before inferred ones
    assert_eq!(handoff.themes[0], "performance");
    assert_eq!(handoff.themes[1], "reliability");
    // Inferred "testing" still present (not a duplicate)
    assert!(
        handoff.themes.contains(&"testing".to_string()),
        "inferred theme should also appear: {:?}",
        handoff.themes
    );
}

#[test]
fn from_session_explicit_themes_deduplicated() {
    let mut session = make_session("Dedup test", vec![], vec![], vec![], vec![]);
    session.themes = vec!["Performance".to_string()];
    // Inferred would also produce "performance" if it appeared in notes
    // Just verify no duplicate casing issues in round-trip
    let handoff = MeetingHandoff::from_session(&session);
    let count = handoff
        .themes
        .iter()
        .filter(|t| t.to_lowercase() == "performance")
        .count();
    assert_eq!(
        count, 1,
        "no duplicate performance theme: {:?}",
        handoff.themes
    );
}

// -----------------------------------------------------------------------
// write / load / mark_processed round-trip (filesystem)
// -----------------------------------------------------------------------

#[test]
fn write_and_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let session = make_session(
        "Roundtrip",
        vec!["What is the deadline for the release?"],
        vec![sample_decision()],
        vec![sample_action()],
        vec!["alice"],
    );
    let handoff = MeetingHandoff::from_session(&session);
    write_meeting_handoff(dir.path(), &handoff).unwrap();

    let loaded = load_meeting_handoff(dir.path()).unwrap().unwrap();
    assert_eq!(loaded.topic, "Roundtrip");
    assert_eq!(loaded.decisions.len(), 1);
    assert_eq!(loaded.action_items.len(), 1);
    assert_eq!(loaded.open_questions.len(), 1);
    assert!(!loaded.processed);
}

#[test]
fn load_from_empty_dir_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let result = load_meeting_handoff(dir.path()).unwrap();
    assert!(result.is_none());
}

#[test]
fn mark_processed_persists() {
    let dir = tempfile::tempdir().unwrap();
    let session = make_session("Mark test", vec![], vec![], vec![], vec![]);
    let handoff = MeetingHandoff::from_session(&session);
    write_meeting_handoff(dir.path(), &handoff).unwrap();

    mark_meeting_handoff_processed(dir.path()).unwrap();

    let loaded = load_meeting_handoff(dir.path()).unwrap().unwrap();
    assert!(loaded.processed);
}

#[test]
fn mark_processed_noop_on_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    // Should succeed without error even when no handoff exists.
    mark_meeting_handoff_processed(dir.path()).unwrap();
}

#[test]
fn mark_processed_in_place_persists() {
    let dir = tempfile::tempdir().unwrap();
    let session = make_session("In-place", vec![], vec![], vec![], vec![]);
    let mut handoff = MeetingHandoff::from_session(&session);
    write_meeting_handoff(dir.path(), &handoff).unwrap();

    mark_handoff_processed_in_place(dir.path(), &mut handoff).unwrap();
    assert!(handoff.processed);

    let loaded = load_meeting_handoff(dir.path()).unwrap().unwrap();
    assert!(loaded.processed);
}
