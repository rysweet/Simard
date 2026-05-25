//! Tests for `meeting_decisions::load_carried_meeting_decisions` covering
//! the bundle-present path (issue #1985) and the legacy bundle-absent path.

use std::fs;

use tempfile::TempDir;

use crate::meeting_facilitator::{MeetingHandoff, derive_meeting_id, write_meeting_handoff};

use super::meeting_decisions::load_carried_meeting_decisions;

/// Helper: write a minimal `memory_records.json` so the memory store opens
/// cleanly without contributing any carried entries.
fn write_empty_memory(state_dir: &std::path::Path) {
    fs::write(state_dir.join("memory_records.json"), "[]").unwrap();
}

/// Helper: build a minimal unprocessed `MeetingHandoff` with one decision
/// and one action item.
fn sample_handoff(topic: &str, meeting_id: &str) -> MeetingHandoff {
    use crate::meeting_facilitator::{ActionItem, MeetingDecision};
    MeetingHandoff {
        schema_version: 2,
        meeting_id: meeting_id.to_string(),
        topic: topic.to_string(),
        started_at: "2026-05-20T10:00:00Z".to_string(),
        closed_at: "2026-05-20T11:00:00Z".to_string(),
        decisions: vec![MeetingDecision {
            description: "Use bundle consumer".to_string(),
            rationale: "Rich context".to_string(),
            participants: vec!["dev".to_string()],
        }],
        action_items: vec![ActionItem {
            description: "Ship #1985".to_string(),
            owner: "engineer".to_string(),
            priority: 1,
            due_description: None,
            linked_issue: Some("#1985".to_string()),
        }],
        open_questions: vec![],
        processed: false,
        duration_secs: Some(3600),
        transcript: vec!["summary".to_string()],
        transcript_path: None,
        participants: vec!["dev".to_string()],
        themes: vec![],
        next_owner: Some("engineer".to_string()),
        artifacts: vec![],
        goal: None,
        next_actor: None,
        applied_templates: vec![],
        history_truncated_count: 0,
        partial_reason: None,
    }
}

/// Bundle-present path: when a per-meeting bundle exists on disk, the
/// carried entries include the bundle-path line and a transcript-lines
/// count line.
#[test]
fn bundle_present_path_includes_bundle_line() {
    let state_dir = TempDir::new().unwrap();
    let handoff_dir = TempDir::new().unwrap();
    let meetings_dir = TempDir::new().unwrap();

    write_empty_memory(state_dir.path());

    let topic = "architecture review";
    let meeting_id = derive_meeting_id("2026-05-20T10:00:00Z", topic);

    // Write the handoff to the queue directory.
    let mut handoff = sample_handoff(topic, &meeting_id);
    write_meeting_handoff(handoff_dir.path(), &handoff).unwrap();

    // Write a per-meeting bundle (transcript + markdown).
    let bundle_dir = meetings_dir.path().join(&meeting_id);
    fs::create_dir_all(&bundle_dir).unwrap();

    let transcript = serde_json::json!({
        "meeting_id": meeting_id,
        "topic": topic,
        "started_at": "2026-05-20T10:00:00Z",
        "closed_at": "2026-05-20T11:00:00Z",
        "lines": [
            {"role": "operator", "content": "Let's decide", "timestamp": "2026-05-20T10:01:00Z"},
            {"role": "simard", "content": "Agreed", "timestamp": "2026-05-20T10:02:00Z"}
        ]
    });
    fs::write(
        bundle_dir.join("transcript.json"),
        serde_json::to_string_pretty(&transcript).unwrap(),
    )
    .unwrap();
    fs::write(
        bundle_dir.join("meeting_handoff.md"),
        "# Meeting handoff: architecture review\n",
    )
    .unwrap();
    // Also need the bundle handoff JSON so load_meeting_bundle finds it.
    handoff.meeting_id = meeting_id.clone();
    fs::write(
        bundle_dir.join("meeting_handoff.json"),
        serde_json::to_string_pretty(&handoff).unwrap(),
    )
    .unwrap();

    // Point env vars to our temp dirs.
    unsafe {
        std::env::set_var("SIMARD_HANDOFF_DIR", handoff_dir.path().as_os_str());
        std::env::set_var("SIMARD_MEETINGS_ROOT", meetings_dir.path().as_os_str());
    }

    let result = load_carried_meeting_decisions(state_dir.path());

    unsafe {
        std::env::remove_var("SIMARD_HANDOFF_DIR");
        std::env::remove_var("SIMARD_MEETINGS_ROOT");
    }

    let carried = result.unwrap();

    // MAX_CARRIED_MEETING_DECISIONS is 3, so only the last 3 entries survive
    // truncation. The bundle path + transcript count are appended last, so
    // they should be present. Decision/action text may be truncated.
    assert!(
        carried.iter().any(|s| s.contains("bundle:")),
        "expected a 'bundle:' line in carried entries, got: {carried:?}"
    );
    assert!(
        carried.iter().any(|s| s.contains("transcript lines: 2")),
        "expected transcript line count in carried entries, got: {carried:?}"
    );
    // Verify total count respects the cap.
    assert!(
        carried.len() <= 3,
        "carried entries should be capped at MAX_CARRIED_MEETING_DECISIONS, got: {}",
        carried.len()
    );
}

/// Bundle-absent path: a legacy v1 handoff with no meeting_id field and
/// no bundle dir still produces the basic carried entries (no regression).
#[test]
fn legacy_handoff_without_bundle_still_carries_decisions() {
    let state_dir = TempDir::new().unwrap();
    let handoff_dir = TempDir::new().unwrap();
    let meetings_dir = TempDir::new().unwrap();

    write_empty_memory(state_dir.path());

    // Legacy v1 handoff: empty meeting_id.
    let handoff = sample_handoff("legacy sync", "");
    write_meeting_handoff(handoff_dir.path(), &handoff).unwrap();

    unsafe {
        std::env::set_var("SIMARD_HANDOFF_DIR", handoff_dir.path().as_os_str());
        // Point meetings root to an empty dir — no bundle exists.
        std::env::set_var("SIMARD_MEETINGS_ROOT", meetings_dir.path().as_os_str());
    }

    let result = load_carried_meeting_decisions(state_dir.path());

    unsafe {
        std::env::remove_var("SIMARD_HANDOFF_DIR");
        std::env::remove_var("SIMARD_MEETINGS_ROOT");
    }

    let carried = result.unwrap();

    // Decision and action item should still be present.
    assert!(
        carried.iter().any(|s| s.contains("Use bundle consumer")),
        "expected decision text in carried entries, got: {carried:?}"
    );
    assert!(
        carried.iter().any(|s| s.contains("Ship #1985")),
        "expected action item text in carried entries, got: {carried:?}"
    );
    // No bundle line should appear.
    assert!(
        !carried.iter().any(|s| s.contains("bundle:")),
        "should not have a bundle line for legacy handoff, got: {carried:?}"
    );
}

/// find_oldest_unprocessed_handoff selects the oldest unprocessed handoff
/// over a newer one, matching the FIFO queue contract.
#[test]
fn oldest_unprocessed_handoff_wins_over_newer() {
    let handoff_dir = TempDir::new().unwrap();

    // Write an older handoff (unprocessed).
    let older = MeetingHandoff {
        topic: "older topic".to_string(),
        started_at: "2026-05-18T09:00:00Z".to_string(),
        closed_at: "2026-05-18T10:00:00Z".to_string(),
        processed: false,
        ..sample_handoff("older topic", "older-meeting")
    };
    // Manually write with an older timestamp filename.
    let older_json = serde_json::to_string_pretty(&older).unwrap();
    fs::write(
        handoff_dir.path().join("handoff-2026-05-18T09-00-00Z.json"),
        &older_json,
    )
    .unwrap();

    // Write a newer handoff (also unprocessed).
    let newer = MeetingHandoff {
        topic: "newer topic".to_string(),
        started_at: "2026-05-20T10:00:00Z".to_string(),
        closed_at: "2026-05-20T11:00:00Z".to_string(),
        processed: false,
        ..sample_handoff("newer topic", "newer-meeting")
    };
    let newer_json = serde_json::to_string_pretty(&newer).unwrap();
    fs::write(
        handoff_dir.path().join("handoff-2026-05-20T10-00-00Z.json"),
        &newer_json,
    )
    .unwrap();

    let result =
        crate::meeting_facilitator::find_oldest_unprocessed_handoff(handoff_dir.path()).unwrap();
    let path = result.expect("should find an unprocessed handoff");
    let name = path.file_name().unwrap().to_string_lossy();
    assert!(
        name.contains("2026-05-18"),
        "expected the older handoff to be selected, got: {name}"
    );
}
