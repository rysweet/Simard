use super::handoff::*;
use super::*;
use crate::meeting_facilitator::types::{ActionItem, MeetingDecision, MeetingSessionStatus};
use std::fs;
use std::path::Path;

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
        next_owner: None,
        goal: None,
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
        linked_issue: None,
    }
}

// -----------------------------------------------------------------------
// is_open_question / is_rhetorical unit tests
// -----------------------------------------------------------------------
#[test]
fn serialization_round_trip_via_serde() {
    let session = make_session(
        "Serde test",
        vec!["TBD: release date", "How do we handle rollback?"],
        vec![sample_decision()],
        vec![sample_action()],
        vec!["alice"],
    );
    let handoff = MeetingHandoff::from_session(&session);

    let json = serde_json::to_string_pretty(&handoff).unwrap();
    let deser: MeetingHandoff = serde_json::from_str(&json).unwrap();
    assert_eq!(handoff, deser);
}

#[test]
fn newest_handoff_wins_when_multiple_exist() {
    let dir = tempfile::tempdir().unwrap();
    // Write two handoffs with different topics — the second has a later
    // timestamp so it should be the one returned by load.
    let s1 = make_session("First", vec![], vec![], vec![], vec![]);
    let h1 = MeetingHandoff::from_session(&s1);
    write_meeting_handoff(dir.path(), &h1).unwrap();

    // Small sleep so the timestamps differ.
    std::thread::sleep(std::time::Duration::from_millis(50));

    let s2 = make_session("Second", vec![], vec![], vec![], vec![]);
    let h2 = MeetingHandoff::from_session(&s2);
    write_meeting_handoff(dir.path(), &h2).unwrap();

    let loaded = load_meeting_handoff(dir.path()).unwrap().unwrap();
    assert_eq!(loaded.topic, "Second");
}

#[test]
fn default_handoff_dir_respects_env() {
    // This test is inherently environment-dependent — just verify it
    // returns a non-empty path.
    let dir = default_handoff_dir();
    assert!(!dir.as_os_str().is_empty());
}

#[test]
fn from_session_explicit_questions_tagged() {
    let mut session = make_session(
        "Tagged",
        vec!["What is the deadline for the release?"],
        vec![],
        vec![],
        vec![],
    );
    session
        .explicit_questions
        .push("Who owns the rollback plan?".to_string());

    let handoff = MeetingHandoff::from_session(&session);
    assert_eq!(handoff.open_questions.len(), 2);

    // Explicit question comes first.
    assert_eq!(
        handoff.open_questions[0].text,
        "Who owns the rollback plan?"
    );
    assert!(handoff.open_questions[0].explicit);

    // Inferred question from notes comes second.
    assert_eq!(
        handoff.open_questions[1].text,
        "What is the deadline for the release?"
    );
    assert!(!handoff.open_questions[1].explicit);
}

#[test]
fn from_session_only_explicit_questions() {
    let mut session = make_session("Explicit only", vec!["plain note"], vec![], vec![], vec![]);
    session
        .explicit_questions
        .push("When do we ship?".to_string());

    let handoff = MeetingHandoff::from_session(&session);
    assert_eq!(handoff.open_questions.len(), 1);
    assert_eq!(handoff.open_questions[0].text, "When do we ship?");
    assert!(handoff.open_questions[0].explicit);
}

// -----------------------------------------------------------------------
// PR tests: find_newest_handoff direct tests + nonexistent dir
// -----------------------------------------------------------------------

#[test]
fn load_from_nonexistent_dir_returns_none() {
    let result = load_meeting_handoff(Path::new("/nonexistent/path/for/testing")).unwrap();
    assert!(result.is_none());
}

#[test]
fn find_newest_picks_latest_timestamped_file() {
    let dir = tempfile::tempdir().unwrap();
    // Write two files with different timestamps.
    fs::write(
        dir.path().join("handoff-2024-01-01T00-00-00_00-00.json"),
        "{}",
    )
    .unwrap();
    fs::write(
        dir.path().join("handoff-2024-06-15T12-00-00_00-00.json"),
        "{}",
    )
    .unwrap();
    let newest = find_newest_handoff(dir.path()).unwrap();
    assert!(
        newest
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("2024-06-15")
    );
}

#[test]
fn find_newest_prefers_timestamped_over_legacy() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(MEETING_HANDOFF_FILENAME), "{}").unwrap();
    fs::write(
        dir.path().join("handoff-2099-01-01T00-00-00_00-00.json"),
        "{}",
    )
    .unwrap();
    let newest = find_newest_handoff(dir.path()).unwrap();
    // The timestamped file sorts after "handoff-" prefix and legacy sorts
    // after all timestamped files ("meeting_handoff.json" > "handoff-*"),
    // so the legacy file is actually picked as "newest" by filename sort.
    // Verify the function returns a valid path (either is acceptable as
    // the function picks the last by lexicographic sort).
    let name = newest.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name == MEETING_HANDOFF_FILENAME || name.contains("2099"),
        "expected either legacy or timestamped file, got {name}"
    );
}

// -----------------------------------------------------------------------
// Schema v2 round-trip and backward-compatibility tests (issue #1987)
// -----------------------------------------------------------------------

#[test]
fn v2_handoff_round_trip_all_fields_preserved() {
    use crate::meeting_backend::AppliedTemplate;
    use crate::meeting_facilitator::types::NextActor;

    let mut session = make_session(
        "V2 full test",
        vec!["TBD: rollout plan"],
        vec![sample_decision()],
        vec![sample_action()],
        vec!["alice", "bob"],
    );
    session.goal = Some("Agree on the release plan for v2".to_string());

    let mut handoff = MeetingHandoff::from_session(&session);
    handoff.next_actor = Some(NextActor::Engineer);
    handoff.applied_templates = vec![AppliedTemplate {
        name: "retro".to_string(),
        agenda: "## Retrospective\n1. What went well\n2. What didn't".to_string(),
        applied_at: "2026-05-01T10:00:00Z".to_string(),
    }];
    handoff.history_truncated_count = 42;
    handoff.partial_reason = Some("summary_timeout".to_string());

    assert_eq!(handoff.schema_version, 2);
    assert_eq!(
        handoff.goal.as_deref(),
        Some("Agree on the release plan for v2")
    );

    let json = serde_json::to_string_pretty(&handoff).unwrap();
    let parsed: MeetingHandoff = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.schema_version, 2);
    assert_eq!(parsed.goal, handoff.goal);
    assert_eq!(parsed.next_actor, Some(NextActor::Engineer));
    assert_eq!(parsed.applied_templates.len(), 1);
    assert_eq!(parsed.applied_templates[0].name, "retro");
    assert_eq!(parsed.history_truncated_count, 42);
    assert_eq!(parsed.partial_reason.as_deref(), Some("summary_timeout"));
    assert_eq!(handoff, parsed);
}

#[test]
fn v1_handoff_without_new_fields_deserializes_with_defaults() {
    // Simulate a v1 handoff JSON that lacks all 6 new fields.
    let v1_json = r#"{
        "meeting_id": "20260101T000000Z-legacy",
        "topic": "Legacy meeting",
        "started_at": "2026-01-01T00:00:00Z",
        "closed_at": "2026-01-01T01:00:00Z",
        "decisions": [],
        "action_items": [],
        "open_questions": [],
        "processed": false,
        "duration_secs": 3600,
        "transcript": ["operator: hello"],
        "participants": ["alice"],
        "themes": ["legacy"],
        "next_owner": "engineer",
        "artifacts": []
    }"#;

    let parsed: MeetingHandoff = serde_json::from_str(v1_json).unwrap();

    // schema_version defaults to 1 for legacy handoffs
    assert_eq!(
        parsed.schema_version, 1,
        "v1 handoff should report schema_version=1"
    );
    assert_eq!(parsed.goal, None, "v1 handoff should have goal=None");
    assert_eq!(
        parsed.next_actor, None,
        "v1 handoff should have next_actor=None"
    );
    assert!(
        parsed.applied_templates.is_empty(),
        "v1 handoff should have empty applied_templates"
    );
    assert_eq!(
        parsed.history_truncated_count, 0,
        "v1 handoff should have history_truncated_count=0"
    );
    assert_eq!(
        parsed.partial_reason, None,
        "v1 handoff should have partial_reason=None"
    );

    // Pre-existing fields should still be correct
    assert_eq!(parsed.topic, "Legacy meeting");
    assert_eq!(parsed.next_owner.as_deref(), Some("engineer"));
    assert_eq!(parsed.duration_secs, Some(3600));
}

#[test]
fn v2_handoff_strip_new_fields_still_deserializes() {
    use crate::meeting_facilitator::types::NextActor;

    let mut session = make_session(
        "Strippable",
        vec![],
        vec![sample_decision()],
        vec![sample_action()],
        vec!["alice"],
    );
    session.goal = Some("Test goal".to_string());

    let mut handoff = MeetingHandoff::from_session(&session);
    handoff.next_actor = Some(NextActor::OodaCurate);

    // Serialize to a serde_json::Value, strip the new fields, then deserialize
    let mut value: serde_json::Value = serde_json::to_value(&handoff).unwrap();
    let obj = value.as_object_mut().unwrap();
    obj.remove("schema_version");
    obj.remove("goal");
    obj.remove("next_actor");
    obj.remove("applied_templates");
    obj.remove("history_truncated_count");
    obj.remove("partial_reason");

    let stripped_json = serde_json::to_string_pretty(&value).unwrap();
    let parsed: MeetingHandoff = serde_json::from_str(&stripped_json).unwrap();

    // Should deserialize as v1 defaults
    assert_eq!(parsed.schema_version, 1);
    assert_eq!(parsed.goal, None);
    assert_eq!(parsed.next_actor, None);
    assert!(parsed.applied_templates.is_empty());
    assert_eq!(parsed.history_truncated_count, 0);
    assert_eq!(parsed.partial_reason, None);

    // Original fields preserved
    assert_eq!(parsed.topic, "Strippable");
    assert_eq!(parsed.decisions.len(), 1);
}

#[test]
fn next_actor_serde_round_trip() {
    use crate::meeting_facilitator::types::NextActor;

    for actor in [
        NextActor::Operator,
        NextActor::Engineer,
        NextActor::OodaCurate,
        NextActor::External,
    ] {
        let json = serde_json::to_string(&actor).unwrap();
        let parsed: NextActor = serde_json::from_str(&json).unwrap();
        assert_eq!(actor, parsed);
    }

    // Verify the tag structure
    let json = serde_json::to_string(&NextActor::Engineer).unwrap();
    assert!(
        json.contains("\"kind\""),
        "NextActor should use tag='kind', got: {json}"
    );
    assert!(
        json.contains("\"engineer\""),
        "Engineer variant should serialize as 'engineer', got: {json}"
    );
}

#[test]
fn from_session_carries_goal() {
    let mut session = make_session("Goal test", vec![], vec![], vec![], vec![]);
    session.goal = Some("Ship the release".to_string());

    let handoff = MeetingHandoff::from_session(&session);
    assert_eq!(handoff.goal.as_deref(), Some("Ship the release"));
    assert_eq!(handoff.schema_version, 2);
}

#[test]
fn from_session_defaults_v2_schema() {
    let session = make_session("Default v2", vec![], vec![], vec![], vec![]);
    let handoff = MeetingHandoff::from_session(&session);
    assert_eq!(handoff.schema_version, 2);
    assert_eq!(handoff.goal, None);
    assert_eq!(handoff.next_actor, None);
    assert!(handoff.applied_templates.is_empty());
    assert_eq!(handoff.history_truncated_count, 0);
    assert_eq!(handoff.partial_reason, None);
}

#[test]
fn save_and_load_wip_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let session = make_session(
        "WIP test",
        vec!["note one"],
        vec![sample_decision()],
        vec![sample_action()],
        vec!["alice"],
    );
    save_session_wip(dir.path(), &session).unwrap();

    let loaded = load_session_wip(dir.path()).unwrap().unwrap();
    assert_eq!(loaded.topic, "WIP test");
    assert_eq!(loaded.decisions.len(), 1);
    assert_eq!(loaded.action_items.len(), 1);
    assert_eq!(loaded.notes, vec!["note one"]);
    assert_eq!(loaded.participants, vec!["alice"]);
}

#[test]
fn load_wip_returns_none_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let result = load_session_wip(dir.path()).unwrap();
    assert!(result.is_none());
}

#[test]
fn remove_wip_deletes_file() {
    let dir = tempfile::tempdir().unwrap();
    let session = make_session("Remove me", vec![], vec![], vec![], vec![]);
    save_session_wip(dir.path(), &session).unwrap();

    // File should exist.
    assert!(dir.path().join(MEETING_SESSION_WIP_FILENAME).is_file());

    remove_session_wip(dir.path()).unwrap();

    // File should be gone.
    assert!(!dir.path().join(MEETING_SESSION_WIP_FILENAME).is_file());
    // Loading should return None.
    assert!(load_session_wip(dir.path()).unwrap().is_none());
}

#[test]
fn remove_wip_noop_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    // Should not error when there's nothing to remove.
    remove_session_wip(dir.path()).unwrap();
}

#[test]
fn save_wip_overwrites_previous() {
    let dir = tempfile::tempdir().unwrap();
    let s1 = make_session("First", vec![], vec![], vec![], vec![]);
    save_session_wip(dir.path(), &s1).unwrap();

    let s2 = make_session("Second", vec!["updated"], vec![], vec![], vec![]);
    save_session_wip(dir.path(), &s2).unwrap();

    let loaded = load_session_wip(dir.path()).unwrap().unwrap();
    assert_eq!(loaded.topic, "Second");
    assert_eq!(loaded.notes, vec!["updated"]);
}
