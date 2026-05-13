//! Integration test for the structured meeting handoff bundle.
//!
//! Drives a small scripted meeting (constructing a `MeetingSession` directly
//! to avoid pulling in the cognitive-memory bridge) and asserts the on-disk
//! shape of the per-meeting bundle that downstream goals/engineers consume.
//!
//! Test isolation: `SIMARD_MEETINGS_ROOT` is set to a per-test temp directory
//! so we never touch a real `~/.simard/meetings/`.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use simard::meeting_facilitator::{
    ActionItem, BundleTranscriptLine, MeetingDecision, MeetingHandoff, MeetingSession,
    MeetingSessionStatus, derive_meeting_id, write_meeting_bundle,
};

const BUNDLE_HANDOFF_JSON: &str = "meeting_handoff.json";
const BUNDLE_HANDOFF_MD: &str = "meeting_handoff.md";
const BUNDLE_TRANSCRIPT_JSON: &str = "transcript.json";

/// Per-test scratch directory; the fixture sets `SIMARD_MEETINGS_ROOT`
/// to this path so the bundle writer never touches the real home dir.
struct ScopedMeetingsRoot {
    dir: PathBuf,
}

impl ScopedMeetingsRoot {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("meeting-bundle-{label}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        // SAFETY: each test uses a unique label; the env var is reset on Drop.
        unsafe {
            std::env::set_var("SIMARD_MEETINGS_ROOT", &dir);
        }
        Self { dir }
    }
}

impl Drop for ScopedMeetingsRoot {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("SIMARD_MEETINGS_ROOT");
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn scripted_meeting_emits_structured_handoff_bundle() {
    let root = ScopedMeetingsRoot::new("scripted");

    // ---- Build a scripted meeting session ----

    let started_at = "2026-05-13T07:00:00Z".to_string();
    let session = MeetingSession {
        topic: "Phase 9 kickoff".to_string(),
        decisions: vec![MeetingDecision {
            description: "Adopt structured handoff bundles".to_string(),
            rationale: "Downstream engineer loop needs a stable shape".to_string(),
            participants: vec!["alice".to_string(), "bob".to_string()],
        }],
        action_items: vec![ActionItem {
            description: "Wire bundle writer into close() flow".to_string(),
            owner: "bob".to_string(),
            priority: 1,
            due_description: Some("by friday".to_string()),
            linked_issue: Some("rysweet/Simard#1730".to_string()),
        }],
        notes: vec!["Should we keep the legacy handoff dir for OODA?".to_string()],
        status: MeetingSessionStatus::Closed,
        started_at: started_at.clone(),
        participants: vec!["alice".to_string(), "bob".to_string()],
        explicit_questions: Vec::new(),
        themes: Vec::new(),
    };

    let mut handoff = MeetingHandoff::from_session(&session);

    // ---- Write the per-meeting bundle ----

    let lines = vec![
        BundleTranscriptLine {
            role: "operator".to_string(),
            content: "Let's plan the handoff.".to_string(),
            timestamp: handoff.started_at.clone(),
        },
        BundleTranscriptLine {
            role: "simard".to_string(),
            content: "Acknowledged — recording decisions and actions.".to_string(),
            timestamp: handoff.started_at.clone(),
        },
    ];

    let bundle_dir = write_meeting_bundle(&mut handoff, &lines).expect("write_meeting_bundle");

    // ---- Assert the on-disk shape ----

    assert!(
        bundle_dir.starts_with(&root.dir),
        "bundle dir {} should be inside test root {}",
        bundle_dir.display(),
        root.dir.display(),
    );
    let expected_meeting_id = derive_meeting_id(&started_at, "Phase 9 kickoff");
    assert_eq!(
        handoff.meeting_id, expected_meeting_id,
        "meeting_id should be auto-derived from (started_at, topic)"
    );
    assert_eq!(
        bundle_dir.file_name().unwrap().to_string_lossy(),
        expected_meeting_id,
        "bundle dir basename must equal the meeting_id"
    );

    let handoff_json_path = bundle_dir.join(BUNDLE_HANDOFF_JSON);
    let handoff_md_path = bundle_dir.join(BUNDLE_HANDOFF_MD);
    let transcript_json_path = bundle_dir.join(BUNDLE_TRANSCRIPT_JSON);
    for p in [&handoff_json_path, &handoff_md_path, &transcript_json_path] {
        assert!(p.exists(), "expected bundle file missing: {}", p.display());
    }

    // Validate the structured handoff JSON has the contract shape.
    let raw = fs::read_to_string(&handoff_json_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(v["meeting_id"], expected_meeting_id);
    assert_eq!(v["started_at"], started_at);
    assert!(
        v["closed_at"].is_string() && !v["closed_at"].as_str().unwrap().is_empty(),
        "closed_at must be a non-empty timestamp string"
    );
    let participants: Vec<&str> = v["participants"]
        .as_array()
        .expect("participants array")
        .iter()
        .filter_map(|p| p.as_str())
        .collect();
    assert!(
        participants.contains(&"alice") && participants.contains(&"bob"),
        "participants should include alice and bob, got {participants:?}"
    );

    let decisions = v["decisions"].as_array().expect("decisions array");
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0]["description"],
        "Adopt structured handoff bundles"
    );

    let actions = v["action_items"].as_array().expect("action_items array");
    assert_eq!(actions.len(), 1);
    let act = &actions[0];
    assert_eq!(act["owner"], "bob");
    assert_eq!(act["description"], "Wire bundle writer into close() flow");
    assert_eq!(act["linked_issue"], "rysweet/Simard#1730");

    assert!(
        v["open_questions"].is_array(),
        "open_questions must be an array (may be empty)"
    );

    // transcript_path should point at the bundle's transcript.json.
    let tp = v["transcript_path"]
        .as_str()
        .expect("transcript_path string");
    assert_eq!(
        PathBuf::from(tp),
        transcript_json_path,
        "transcript_path in JSON should equal the on-disk transcript.json path"
    );

    // Validate the markdown is non-empty and references the meeting id + topic.
    let md = fs::read_to_string(&handoff_md_path).unwrap();
    assert!(md.contains("Phase 9 kickoff"), "md must mention topic");
    assert!(
        md.contains(&expected_meeting_id),
        "md must mention meeting_id"
    );

    // Validate the transcript artifact is valid JSON with our scripted lines.
    let traw = fs::read_to_string(&transcript_json_path).unwrap();
    let tv: serde_json::Value = serde_json::from_str(&traw).unwrap();
    assert_eq!(tv["meeting_id"], expected_meeting_id);
    let tlines = tv["lines"].as_array().expect("lines array");
    assert_eq!(tlines.len(), 2);
    assert_eq!(tlines[0]["role"], "operator");
    assert_eq!(tlines[1]["role"], "simard");
}

#[test]
fn empty_transcript_still_produces_well_formed_bundle() {
    let _root = ScopedMeetingsRoot::new("empty-transcript");

    let mut handoff = MeetingHandoff {
        meeting_id: String::new(), // exercise auto-fill
        topic: "Quick sync".to_string(),
        started_at: "2026-05-13T07:00:00Z".to_string(),
        closed_at: "2026-05-13T07:05:00Z".to_string(),
        decisions: vec![],
        action_items: vec![],
        open_questions: vec![],
        processed: false,
        duration_secs: Some(300),
        transcript: vec![],
        participants: vec![],
        themes: vec![],
        transcript_path: None,
    };

    let dir = write_meeting_bundle(&mut handoff, &[]).expect("write_meeting_bundle");
    assert!(dir.join(BUNDLE_HANDOFF_JSON).exists());
    assert!(dir.join(BUNDLE_HANDOFF_MD).exists());
    assert!(dir.join(BUNDLE_TRANSCRIPT_JSON).exists());

    // Re-deserialize to confirm the JSON round-trips through MeetingHandoff.
    let raw = fs::read_to_string(dir.join(BUNDLE_HANDOFF_JSON)).unwrap();
    let parsed: MeetingHandoff = serde_json::from_str(&raw).unwrap();
    assert!(
        !parsed.meeting_id.is_empty(),
        "meeting_id should be filled in"
    );
    assert_eq!(parsed.topic, "Quick sync");
    assert!(parsed.transcript_path.is_some());
}
