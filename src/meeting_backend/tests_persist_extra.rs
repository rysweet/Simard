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
