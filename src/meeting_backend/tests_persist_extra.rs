use super::persist::*;
use super::types::{ConversationMessage, Role};
use super::*;
use crate::meeting_facilitator::MeetingHandoff;
use serial_test::serial;

fn make_msg(role: Role, content: &str) -> ConversationMessage {
    ConversationMessage {
        role,
        content: content.to_string(),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
    }
}
#[test]
fn extract_open_questions_explicit_markers() {
    let messages = vec![
        make_msg(Role::User, "OPEN: What database should we use?"),
        make_msg(Role::Assistant, "Question: Who owns the rollback plan?"),
    ];
    let questions = extract_open_questions(&messages);
    assert_eq!(questions.len(), 2);
    assert!(questions[0].explicit);
    assert!(questions[1].explicit);
}

#[test]
fn extract_open_questions_genuine_question() {
    let messages = vec![make_msg(
        Role::User,
        "How should we handle backward compatibility for the API?",
    )];
    let questions = extract_open_questions(&messages);
    assert_eq!(questions.len(), 1);
    assert!(!questions[0].explicit);
    assert!(questions[0].text.contains("backward compatibility"));
}

#[test]
fn extract_open_questions_skips_short() {
    let messages = vec![make_msg(Role::User, "Why not?")];
    let questions = extract_open_questions(&messages);
    assert!(
        questions.is_empty(),
        "Short rhetorical-like questions should be filtered"
    );
}

#[test]
fn extract_open_questions_empty_messages() {
    let questions = extract_open_questions(&[]);
    assert!(questions.is_empty());
}

// ── Theme extraction tests ──────────────────────────────────────

#[test]
fn extract_themes_recurring_words() {
    let messages = vec![
        make_msg(Role::User, "We need to improve testing coverage."),
        make_msg(Role::Assistant, "Testing is important for quality."),
        make_msg(Role::User, "Let's add more testing to the pipeline."),
    ];
    let themes = extract_themes(&messages);
    assert!(
        themes.contains(&"testing".to_string()),
        "Expected 'testing' in themes: {themes:?}"
    );
}

#[test]
fn extract_themes_empty_messages() {
    let themes = extract_themes(&[]);
    assert!(themes.is_empty());
}

#[test]
fn extract_themes_skips_system_messages() {
    let messages = vec![
        make_msg(
            Role::System,
            "System prompt with repeated system words system.",
        ),
        make_msg(Role::User, "Hello"),
    ];
    let themes = extract_themes(&messages);
    // "system" only appeared in system messages, which are skipped.
    assert!(!themes.contains(&"system".to_string()));
}

// ── write_handoff completeness test ─────────────────────────────

#[test]
#[serial]
fn write_handoff_includes_structured_data() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("SIMARD_HANDOFF_DIR", dir.path().as_os_str());
    }

    let messages = vec![
        make_msg(Role::User, "We need better testing."),
        make_msg(
            Role::Assistant,
            "Decision: We will adopt TDD. OPEN: Who will lead the effort?",
        ),
    ];
    let action_items = vec![HandoffActionItem {
        description: "Set up CI pipeline".to_string(),
        assignee: Some("alice".to_string()),
        deadline: Some("Friday".to_string()),
        linked_goal: None,
        priority: None,
    }];
    let decisions = vec!["We will adopt TDD".to_string()];

    let result = write_handoff(
        "Sprint planning",
        "Good meeting",
        &messages,
        &action_items,
        &decisions,
    );
    assert!(result.is_ok(), "write_handoff failed: {result:?}");

    // Read the written handoff file.
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!entries.is_empty(), "No handoff file written");

    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let handoff: MeetingHandoff = serde_json::from_str(&content).unwrap();

    // Decisions are populated.
    assert_eq!(handoff.decisions.len(), 1);
    assert!(handoff.decisions[0].description.contains("TDD"));

    // Action items are populated.
    assert_eq!(handoff.action_items.len(), 1);
    assert_eq!(handoff.action_items[0].description, "Set up CI pipeline");
    assert_eq!(handoff.action_items[0].owner, "alice");

    // Open questions are extracted from messages.
    assert!(
        !handoff.open_questions.is_empty(),
        "Expected open questions from message content"
    );

    // Participants include roles from messages and assignees.
    assert!(handoff.participants.contains(&"operator".to_string()));
    assert!(handoff.participants.contains(&"alice".to_string()));

    // Transcript contains summary.
    assert!(handoff.transcript.contains(&"Good meeting".to_string()));

    unsafe {
        std::env::remove_var("SIMARD_HANDOFF_DIR");
    }
}

#[test]
#[serial]
fn write_handoff_empty_data_uses_defaults() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("SIMARD_HANDOFF_DIR", dir.path().as_os_str());
    }

    let result = write_handoff("Empty meeting", "No notes", &[], &[], &[]);
    assert!(result.is_ok());

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let handoff: MeetingHandoff = serde_json::from_str(&content).unwrap();

    assert!(handoff.decisions.is_empty());
    assert!(handoff.action_items.is_empty());
    assert!(handoff.open_questions.is_empty());
    assert!(handoff.participants.is_empty());
    assert!(handoff.themes.is_empty());

    unsafe {
        std::env::remove_var("SIMARD_HANDOFF_DIR");
    }
}

// ── JSON sibling artifact (issue #1646 — TDD red phase) ──────────────────
//
// The markdown handoff writer emits a structured JSON sibling artifact next
// to the markdown report (same dir, same basename, `.json` extension). The
// JSON shape is documented in `docs/operations/meeting-handoffs.md` and
// includes: schema_version, participants, decisions, action_items
// (each with title/owner/acceptance_criteria), open_questions, transcript_ref.
//
// Tests below set SIMARD_MEETINGS_DIR to a tempdir to redirect output.
// The env override mirrors the SIMARD_HANDOFF_DIR idiom in
// meeting_facilitator::default_handoff_dir().

use crate::meeting_backend::persist::{
    JsonHandoffActionItem, JsonHandoffSibling, write_handoff_markdown_report,
};
use crate::meeting_facilitator::MeetingDecision;

fn populated_messages() -> Vec<ConversationMessage> {
    vec![
        make_msg(Role::User, "We need to ship the migration."),
        make_msg(
            Role::Assistant,
            "Decision: We will adopt TDD. OPEN: Who owns the rollback plan for the migration?",
        ),
    ]
}

fn populated_action_items() -> Vec<HandoffActionItem> {
    vec![
        HandoffActionItem {
            description: "Set up CI pipeline".to_string(),
            assignee: Some("alice".to_string()),
            deadline: Some("Friday".to_string()),
            linked_goal: None,
            priority: Some(1),
        },
        HandoffActionItem {
            description: "Document migration steps".to_string(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        },
    ]
}

fn populated_decisions() -> Vec<MeetingDecision> {
    vec![MeetingDecision {
        description: "We will adopt TDD".to_string(),
        rationale: "Better quality bar.".to_string(),
        participants: vec!["operator".to_string(), "simard".to_string()],
    }]
}

/// The markdown handoff writer must emit a JSON sibling file with the same
/// basename and a `.json` extension, in the same directory as the markdown
/// report. Markdown remains the canonical artifact; JSON is a side-effect.
#[test]
#[serial]
fn markdown_handoff_writes_json_sibling_with_same_basename() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("SIMARD_MEETINGS_DIR", dir.path().as_os_str());
    }

    let md_path = write_handoff_markdown_report(
        "Sprint planning",
        "2025-01-01T00:00:00Z",
        "Good meeting summary.",
        &populated_messages(),
        &populated_action_items(),
        &populated_decisions(),
        &[],
    )
    .expect("markdown handoff write should succeed under SIMARD_MEETINGS_DIR override");

    // Sibling lives at the same path with `.json` extension.
    let json_path = md_path.with_extension("json");
    assert!(
        json_path.exists(),
        "expected JSON sibling at {} alongside markdown {}",
        json_path.display(),
        md_path.display(),
    );

    // Same parent directory as the markdown report.
    assert_eq!(
        json_path.parent(),
        md_path.parent(),
        "JSON sibling must live in the same directory as the markdown report",
    );

    // Same basename (stem) — only the extension differs.
    assert_eq!(
        json_path.file_stem(),
        md_path.file_stem(),
        "JSON sibling must share the markdown report's basename",
    );

    unsafe {
        std::env::remove_var("SIMARD_MEETINGS_DIR");
    }
}

/// JSON sibling deserializes into the `JsonHandoffSibling` DTO with all
/// expected fields populated. This is the contract consumers depend on:
/// - schema_version (string, "v1")
/// - participants (Vec<String>)
/// - decisions (Vec<String>)
/// - action_items: Vec<JsonHandoffActionItem { title, owner, acceptance_criteria }>
/// - open_questions (Vec<String>)
/// - transcript_ref (String)
#[test]
#[serial]
fn json_sibling_round_trips_all_fields_via_serde_json() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("SIMARD_MEETINGS_DIR", dir.path().as_os_str());
    }

    let md_path = write_handoff_markdown_report(
        "Sprint planning",
        "2025-01-01T00:00:00Z",
        "Good summary.",
        &populated_messages(),
        &populated_action_items(),
        &populated_decisions(),
        &[],
    )
    .unwrap();

    let json_path = md_path.with_extension("json");
    let json_text = std::fs::read_to_string(&json_path).unwrap_or_else(|e| {
        panic!(
            "failed to read JSON sibling at {}: {e}",
            json_path.display()
        )
    });

    // Must deserialize into the canonical DTO.
    let sibling: JsonHandoffSibling = serde_json::from_str(&json_text)
        .unwrap_or_else(|e| panic!("JSON sibling failed to deserialize into JsonHandoffSibling: {e}\n--- json ---\n{json_text}"));

    // schema_version is "v1" so consumers can branch on future schema changes.
    assert_eq!(
        sibling.schema_version, "v1",
        "schema_version must be 'v1' for the initial schema",
    );

    // Participants must include the operator role at minimum (derived from
    // the user message in populated_messages()).
    assert!(
        sibling.participants.iter().any(|p| p == "operator"),
        "participants should include 'operator' from messages: {:?}",
        sibling.participants,
    );

    // Decisions vector contains the structured decision descriptions.
    assert!(
        sibling
            .decisions
            .iter()
            .any(|d| d.contains("adopt TDD") || d.contains("TDD")),
        "decisions should include the TDD decision: {:?}",
        sibling.decisions,
    );

    // Action items map title := description, owner := assignee.
    assert_eq!(
        sibling.action_items.len(),
        2,
        "expected 2 action items in sibling: {:?}",
        sibling.action_items,
    );
    let ci = sibling
        .action_items
        .iter()
        .find(|a| a.title == "Set up CI pipeline")
        .unwrap_or_else(|| {
            panic!(
                "missing 'Set up CI pipeline' action item: {:?}",
                sibling.action_items
            )
        });
    assert_eq!(ci.owner.as_deref(), Some("alice"));

    // Open questions are extracted from the transcript via existing
    // extract_open_questions helper (no duplicate parsing).
    assert!(
        !sibling.open_questions.is_empty(),
        "open_questions should be populated from message extraction: {:?}",
        sibling.open_questions,
    );

    // transcript_ref locates the markdown file. Per the design, it is the
    // markdown report's basename (privacy: no full local paths leak into the
    // handoff artifact).
    let md_basename = md_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap()
        .to_string();
    assert_eq!(
        sibling.transcript_ref, md_basename,
        "transcript_ref should be the markdown report's basename, not a full path",
    );

    // Round-trip through serde_json::to_string preserves equality.
    let reserialized = serde_json::to_string(&sibling).unwrap();
    let sibling2: JsonHandoffSibling = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(sibling, sibling2);

    unsafe {
        std::env::remove_var("SIMARD_MEETINGS_DIR");
    }
}

/// Empty extraction must serialize as JSON empty arrays (`[]`), NOT as
/// `null`. Consumers (dashboards, downstream tools) iterate over these
/// arrays and would crash on `null`.
#[test]
#[serial]
fn json_sibling_with_empty_extraction_serializes_empty_arrays_not_null() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("SIMARD_MEETINGS_DIR", dir.path().as_os_str());
    }

    let md_path = write_handoff_markdown_report(
        "Empty meeting",
        "2025-01-01T00:00:00Z",
        "Nothing was discussed.",
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();

    let json_path = md_path.with_extension("json");
    let json_text = std::fs::read_to_string(&json_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json_text).unwrap();

    for field in &[
        "participants",
        "decisions",
        "action_items",
        "open_questions",
    ] {
        let val = &v[field];
        assert!(
            val.is_array(),
            "field `{field}` must serialize as a JSON array (not null/missing): {json_text}"
        );
        assert!(
            val.as_array().unwrap().is_empty(),
            "field `{field}` should be an empty array on empty extraction: {json_text}"
        );
    }

    // schema_version still present even on empty meeting.
    assert_eq!(
        v["schema_version"],
        serde_json::Value::String("v1".to_string())
    );

    // transcript_ref is always a non-empty string (the markdown basename).
    assert!(
        v["transcript_ref"].is_string(),
        "transcript_ref must be a string: {json_text}"
    );
    assert!(
        !v["transcript_ref"].as_str().unwrap().is_empty(),
        "transcript_ref must not be empty: {json_text}"
    );

    unsafe {
        std::env::remove_var("SIMARD_MEETINGS_DIR");
    }
}

/// Security S2: JSON sibling must be world-unreadable (mode 0o600) on Unix
/// to mirror the markdown report and transcript privacy guarantees. This
/// prevents meeting content leaking via shared `~` directories or backups
/// readable by other users on the same host.
#[cfg(unix)]
#[test]
#[serial]
fn json_sibling_has_owner_only_permissions_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("SIMARD_MEETINGS_DIR", dir.path().as_os_str());
    }

    let md_path = write_handoff_markdown_report(
        "Permission test",
        "2025-01-01T00:00:00Z",
        "Some summary.",
        &populated_messages(),
        &populated_action_items(),
        &populated_decisions(),
        &[],
    )
    .unwrap();

    let json_path = md_path.with_extension("json");
    let meta = std::fs::metadata(&json_path).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode,
        0o600,
        "JSON sibling at {} must be 0o600, got {:o}",
        json_path.display(),
        mode,
    );

    unsafe {
        std::env::remove_var("SIMARD_MEETINGS_DIR");
    }
}

/// `JsonHandoffActionItem::from(&HandoffActionItem)` maps fields exactly as
/// specified: `title := description`, `owner := assignee`,
/// `acceptance_criteria := None` (reserved slot for future extractor work).
#[test]
fn json_handoff_action_item_from_handoff_action_item_maps_fields() {
    let src = HandoffActionItem {
        description: "Ship v1.0".to_string(),
        assignee: Some("bob".to_string()),
        deadline: Some("EOD".to_string()),
        linked_goal: Some("release-train".to_string()),
        priority: Some(1),
    };
    let dst: JsonHandoffActionItem = (&src).into();
    assert_eq!(dst.title, "Ship v1.0");
    assert_eq!(dst.owner.as_deref(), Some("bob"));
    assert_eq!(
        dst.acceptance_criteria, None,
        "acceptance_criteria is a reserved slot; current extractor never populates it",
    );

    // Unassigned action item maps to owner=None (NOT empty string, NOT 'unassigned').
    let unassigned = HandoffActionItem {
        description: "Triage backlog".to_string(),
        assignee: None,
        deadline: None,
        linked_goal: None,
        priority: None,
    };
    let dst2: JsonHandoffActionItem = (&unassigned).into();
    assert_eq!(dst2.title, "Triage backlog");
    assert_eq!(dst2.owner, None);
}

/// Per design A2 (resolve to explicit `null`), `Option<String>` fields on
/// `JsonHandoffActionItem` must serialize as JSON `null` when None — NOT
/// be omitted via `skip_serializing_if`. This is a contract consumers can
/// rely on (every action_item entry has the same shape).
#[test]
fn json_handoff_action_item_none_fields_serialize_as_explicit_null() {
    let item = JsonHandoffActionItem {
        title: "Triage backlog".to_string(),
        owner: None,
        acceptance_criteria: None,
    };
    let json = serde_json::to_string(&item).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v["owner"],
        serde_json::Value::Null,
        "owner=None must serialize as explicit JSON null: {json}"
    );
    assert_eq!(
        v["acceptance_criteria"],
        serde_json::Value::Null,
        "acceptance_criteria=None must serialize as explicit JSON null: {json}"
    );
    // The `title` field is always present.
    assert_eq!(
        v["title"],
        serde_json::Value::String("Triage backlog".to_string())
    );
}

// ────────────────────────────────────────────────────────────────────────────
// #1906: meeting REPL must honor SIMARD_STATE_ROOT
//
// `meetings_dir()` is the resolver used by `write_transcript` and
// `write_auto_save`. The audit (issue #1906) found that operators who set
// SIMARD_STATE_ROOT saw their autosaves silently written to
// `~/.simard/meetings/_autosave_<topic>.json` regardless. The resolver now
// mirrors `goal-curation read`:
//   1. SIMARD_MEETINGS_DIR (narrow override; wins)
//   2. SIMARD_STATE_ROOT    -> $SIMARD_STATE_ROOT/meetings
//   3. $HOME/.simard/meetings (default)
// ────────────────────────────────────────────────────────────────────────────

/// End-to-end: with only SIMARD_STATE_ROOT set, `write_auto_save` must
/// land the autosave file under `$SIMARD_STATE_ROOT/meetings/` and must
/// NOT touch `$HOME/.simard/meetings/`. This is the outside-in assertion
/// the #1906 audit reproduction specifies.
#[test]
#[serial]
fn write_auto_save_lands_under_simard_state_root() {
    use std::path::PathBuf;

    // Belt-and-braces: clear both narrow overrides before setting STATE_ROOT.
    // SAFETY: serialized via `#[serial]` (default lock group) across this
    // test binary; matches the existing tests in this file that mutate
    // SIMARD_MEETINGS_DIR.
    unsafe {
        std::env::remove_var("SIMARD_MEETINGS_DIR");
        std::env::remove_var("SIMARD_STATE_ROOT");
    }

    let state_root = tempfile::tempdir().expect("create state-root tempdir");
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", state_root.path().as_os_str());
    }

    let transcript = MeetingTranscript {
        topic: "audit_state_root_probe".to_string(),
        started_at: "2026-05-19T00:00:00Z".to_string(),
        closed_at: String::new(),
        duration_secs: 0,
        summary: "[in-progress — meeting still open]".to_string(),
        messages: vec![
            make_msg(Role::User, "hello"),
            make_msg(Role::User, "/close"),
        ],
    };

    let written = write_auto_save(&transcript).expect("autosave should succeed");

    let expected_parent = state_root.path().join("meetings");
    assert_eq!(
        written.parent(),
        Some(expected_parent.as_path()),
        "autosave parent must be $SIMARD_STATE_ROOT/meetings (got {})",
        written.display(),
    );
    assert_eq!(
        written.file_name().and_then(|s| s.to_str()),
        Some("_autosave_audit_state_root_probe.json"),
        "autosave filename contract preserved (got {})",
        written.display(),
    );
    assert!(written.is_file(), "autosave file must exist on disk");

    // Critical #1906 assertion (on the returned write path, not on stale disk
    // state): the resolver must route writes to $SIMARD_STATE_ROOT/meetings,
    // not silently fall through to $HOME/.simard/meetings. Developer machines
    // already have stale artifacts under $HOME/.simard/meetings/ from prior
    // operator probes (which is exactly how #1906 was discovered), so we can
    // only test fix-correctness via the returned path.
    let home_meetings =
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".simard/meetings");
    assert!(
        !written.starts_with(&home_meetings),
        "audit #1906: write_auto_save must NOT write under $HOME/.simard/meetings \
         when SIMARD_STATE_ROOT is set. Got: {}",
        written.display(),
    );

    unsafe {
        std::env::remove_var("SIMARD_STATE_ROOT");
    }
}

/// SIMARD_MEETINGS_DIR is the narrow override and must win over
/// SIMARD_STATE_ROOT when both are set. Preserves the existing operator/test
/// contract that lets callers redirect only the meeting artifact dir.
#[test]
#[serial]
fn meetings_dir_narrow_override_wins_over_state_root() {
    let narrow = tempfile::tempdir().expect("narrow tempdir");
    let state = tempfile::tempdir().expect("state tempdir");
    unsafe {
        std::env::set_var("SIMARD_MEETINGS_DIR", narrow.path().as_os_str());
        std::env::set_var("SIMARD_STATE_ROOT", state.path().as_os_str());
    }

    let transcript = MeetingTranscript {
        topic: "narrow_wins".to_string(),
        started_at: "2026-05-19T00:00:00Z".to_string(),
        closed_at: String::new(),
        duration_secs: 0,
        summary: String::new(),
        messages: vec![make_msg(Role::User, "hi")],
    };

    let written = write_auto_save(&transcript).expect("autosave should succeed");

    assert_eq!(
        written.parent(),
        Some(narrow.path()),
        "SIMARD_MEETINGS_DIR must win over SIMARD_STATE_ROOT (preserves narrow-override contract)"
    );

    unsafe {
        std::env::remove_var("SIMARD_MEETINGS_DIR");
        std::env::remove_var("SIMARD_STATE_ROOT");
    }
}
