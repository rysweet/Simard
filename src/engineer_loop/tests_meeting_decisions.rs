//! Tests for `load_carried_meeting_decisions` with per-meeting bundle support.
//!
//! Covers:
//! - Bundle-present path: a meeting with a populated bundle yields carried
//!   entries plus the bundle-path log line.
//! - Bundle-absent path: a legacy v1 handoff (no bundle dir) still produces
//!   carried entries — no regression.
//! - `find_oldest_unprocessed_handoff` selection: oldest unprocessed wins
//!   over newer ones.

use std::fs;
use std::path::Path;

use serial_test::serial;

use crate::meeting_facilitator::{
    BundleTranscriptLine, MeetingDecision, MeetingHandoff, derive_meeting_id, write_meeting_bundle,
};

/// Create a minimal handoff with decisions for testing.
fn test_handoff(topic: &str, started_at: &str) -> MeetingHandoff {
    MeetingHandoff {
        schema_version: 2,
        meeting_id: derive_meeting_id(started_at, topic),
        topic: topic.to_string(),
        started_at: started_at.to_string(),
        closed_at: "2026-01-15T11:00:00Z".to_string(),
        decisions: vec![MeetingDecision {
            description: "Use Rust".to_string(),
            rationale: "Safety first".to_string(),
            participants: vec!["dev".to_string()],
        }],
        action_items: vec![],
        open_questions: vec![],
        processed: false,
        duration_secs: Some(3600),
        transcript: vec!["Test transcript".to_string()],
        transcript_path: None,
        participants: vec!["dev".to_string()],
        themes: vec![],
        next_owner: None,
        artifacts: vec![],
        goal: Some("Ship handoff v2".to_string()),
        next_actor: None,
        applied_templates: vec![],
        history_truncated_count: 0,
        partial_reason: None,
    }
}

/// Write a handoff file directly into the handoff dir (legacy style).
fn write_handoff_to_dir(dir: &Path, handoff: &MeetingHandoff) {
    fs::create_dir_all(dir).unwrap();
    let ts = handoff.closed_at.replace(':', "-").replace('+', "_");
    let filename = format!("handoff-{ts}.json");
    let path = dir.join(&filename);
    let json = serde_json::to_string_pretty(handoff).unwrap();
    fs::write(&path, &json).unwrap();
}

/// Set up a state_root with a valid memory_records.json.
fn setup_state_root(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("memory_records.json"), "[]").unwrap();
}

#[test]
#[serial]
fn bundle_present_yields_bundle_path_line() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().join("state");
    setup_state_root(&state_root);

    let handoff_dir = tmp.path().join("handoffs");
    let bundle_root = tmp.path().join("meetings");

    let handoff = test_handoff("Bundle test", "2026-01-15T10:00:00Z");
    write_handoff_to_dir(&handoff_dir, &handoff);

    let mut bundle_handoff = handoff.clone();
    let transcript = vec![BundleTranscriptLine {
        role: "operator".to_string(),
        content: "Hello".to_string(),
        timestamp: "2026-01-15T10:01:00Z".to_string(),
    }];

    // SAFETY (Rust 2024): set_var requires unsafe — tests only, serialized.
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", handoff_dir.to_str().unwrap()) };
    unsafe { std::env::set_var("SIMARD_MEETINGS_ROOT", bundle_root.to_str().unwrap()) };

    write_meeting_bundle(&mut bundle_handoff, &transcript).unwrap();
    let carried = super::load_carried_meeting_decisions(&state_root).unwrap();

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
    unsafe { std::env::remove_var("SIMARD_MEETINGS_ROOT") };

    // The bundle path line must be present. Note: MAX_CARRIED_MEETING_DECISIONS=3
    // caps the carried list; when a bundle adds 3 metadata lines (bundle path,
    // transcript, markdown report) plus 1 decision = 4, the oldest entry (the
    // decision) is trimmed. This is expected: the bundle path is higher value
    // since it points the engineer to the full artifact.
    assert!(
        carried.iter().any(|s| s.contains("bundle:")),
        "expected bundle path line in carried; got: {carried:?}"
    );
    assert!(
        carried.iter().any(|s| s.contains("Bundle test")),
        "expected topic name in carried entries; got: {carried:?}"
    );
}

#[test]
#[serial]
fn legacy_handoff_without_bundle_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().join("state");
    setup_state_root(&state_root);

    let handoff_dir = tmp.path().join("handoffs");
    let bundle_root = tmp.path().join("meetings_empty");

    let handoff = test_handoff("Legacy test", "2026-01-15T10:00:00Z");
    write_handoff_to_dir(&handoff_dir, &handoff);

    // SAFETY (Rust 2024): set_var requires unsafe — tests only, serialized.
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", handoff_dir.to_str().unwrap()) };
    unsafe { std::env::set_var("SIMARD_MEETINGS_ROOT", bundle_root.to_str().unwrap()) };

    let carried = super::load_carried_meeting_decisions(&state_root).unwrap();

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
    unsafe { std::env::remove_var("SIMARD_MEETINGS_ROOT") };

    // Without a bundle, only 1 decision is carried — well within the cap.
    assert!(
        carried.iter().any(|s| s.contains("Use Rust")),
        "expected decision in carried even without bundle; got: {carried:?}"
    );
    assert!(
        !carried.iter().any(|s| s.contains("bundle:")),
        "should not have bundle line when bundle is absent; got: {carried:?}"
    );
}

#[test]
#[serial]
fn oldest_unprocessed_wins_over_newer() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().join("state");
    setup_state_root(&state_root);

    let handoff_dir = tmp.path().join("handoffs");
    let bundle_root = tmp.path().join("meetings");

    // Older handoff has a different closed_at so the filenames differ.
    let mut older = test_handoff("Older meeting", "2026-01-10T09:00:00Z");
    older.closed_at = "2026-01-10T10:00:00Z".to_string();
    older.decisions[0].description = "OlderDecision".to_string();
    write_handoff_to_dir(&handoff_dir, &older);

    let mut newer = test_handoff("Newer meeting", "2026-01-15T10:00:00Z");
    newer.closed_at = "2026-01-15T11:00:00Z".to_string();
    newer.decisions[0].description = "NewerDecision".to_string();
    write_handoff_to_dir(&handoff_dir, &newer);

    // SAFETY (Rust 2024): set_var requires unsafe — tests only, serialized.
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", handoff_dir.to_str().unwrap()) };
    unsafe { std::env::set_var("SIMARD_MEETINGS_ROOT", bundle_root.to_str().unwrap()) };

    let carried = super::load_carried_meeting_decisions(&state_root).unwrap();

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
    unsafe { std::env::remove_var("SIMARD_MEETINGS_ROOT") };

    // Oldest unprocessed should be selected (FIFO). No bundle dir exists
    // for this handoff, so only the decision line is carried.
    assert!(
        carried.iter().any(|s| s.contains("OlderDecision")),
        "expected oldest unprocessed handoff's decision; got: {carried:?}"
    );
    assert!(
        !carried.iter().any(|s| s.contains("NewerDecision")),
        "should not contain newer handoff's decision; got: {carried:?}"
    );
}

#[test]
#[serial]
fn load_meeting_bundle_returns_none_for_missing_dir() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY (Rust 2024): set_var requires unsafe — tests only, serialized.
    unsafe { std::env::set_var("SIMARD_MEETINGS_ROOT", tmp.path().to_str().unwrap()) };

    let result = crate::meeting_facilitator::load_meeting_bundle("nonexistent-meeting-id");

    unsafe { std::env::remove_var("SIMARD_MEETINGS_ROOT") };

    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
#[serial]
fn load_meeting_bundle_reads_all_files() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle_root = tmp.path().join("meetings");

    let mut handoff = test_handoff("Full bundle", "2026-01-15T10:00:00Z");
    let transcript = vec![
        BundleTranscriptLine {
            role: "operator".to_string(),
            content: "Hello".to_string(),
            timestamp: "2026-01-15T10:01:00Z".to_string(),
        },
        BundleTranscriptLine {
            role: "simard".to_string(),
            content: "Hi there".to_string(),
            timestamp: "2026-01-15T10:01:05Z".to_string(),
        },
    ];

    // SAFETY (Rust 2024): set_var requires unsafe — tests only, serialized.
    unsafe { std::env::set_var("SIMARD_MEETINGS_ROOT", bundle_root.to_str().unwrap()) };

    let meeting_id = handoff.meeting_id.clone();
    write_meeting_bundle(&mut handoff, &transcript).unwrap();

    let bundle = crate::meeting_facilitator::load_meeting_bundle(&meeting_id)
        .unwrap()
        .expect("bundle should exist");

    unsafe { std::env::remove_var("SIMARD_MEETINGS_ROOT") };

    assert_eq!(bundle.handoff.topic, "Full bundle");
    assert_eq!(bundle.transcript.len(), 2);
    assert_eq!(bundle.transcript[0].role, "operator");
    assert_eq!(bundle.transcript[1].content, "Hi there");
    assert!(
        bundle.markdown_report.is_some(),
        "markdown report should be present"
    );
    assert!(
        bundle
            .markdown_report
            .as_ref()
            .unwrap()
            .contains("Full bundle"),
        "markdown should mention the topic"
    );
}
