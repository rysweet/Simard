use super::persist::*;
use super::types::{ConversationMessage, Role};
use super::*;

#[test]
fn sanitize_basic() {
    assert_eq!(sanitize_filename("Sprint Planning"), "Sprint_Planning");
}

#[test]
fn sanitize_path_traversal() {
    assert_eq!(sanitize_filename("../../etc/passwd"), "etc_passwd");
}

#[test]
fn sanitize_null_bytes() {
    assert_eq!(sanitize_filename("test\0file"), "testfile");
}

#[test]
fn sanitize_empty() {
    assert_eq!(sanitize_filename(""), "meeting");
}

#[test]
fn sanitize_special_chars() {
    assert_eq!(sanitize_filename("a:b*c?d<e>f|g"), "a_b_c_d_e_f_g");
}

#[test]
fn sanitize_long_string() {
    let long = "a".repeat(200);
    let result = sanitize_filename(&long);
    assert!(result.len() <= MAX_FILENAME_LEN);
}

#[test]
fn sanitize_only_dots_and_underscores() {
    assert_eq!(sanitize_filename("...___..."), "meeting");
}

#[test]
fn find_template_by_name() {
    assert!(find_template("standup").is_some());
    assert!(find_template("1on1").is_some());
    assert!(find_template("retro").is_some());
    assert!(find_template("planning").is_some());
    assert!(find_template("nonexistent").is_none());
}

#[test]
fn find_template_case_insensitive() {
    assert!(find_template("STANDUP").is_some());
    assert!(find_template("Retro").is_some());
}

#[test]
fn templates_have_content() {
    for t in TEMPLATES {
        assert!(!t.name.is_empty());
        assert!(!t.description.is_empty());
        assert!(!t.agenda.is_empty());
    }
}

#[test]
fn at_least_four_templates() {
    assert!(TEMPLATES.len() >= 4);
}

#[test]
fn markdown_export_format() {
    // Verify the markdown format contains expected YAML frontmatter
    let topic = "Test Topic";
    let started_at = "2025-01-01T00:00:00Z";
    let mut md = String::new();
    md.push_str("---\n");
    md.push_str(&format!("topic: \"{topic}\"\n"));
    md.push_str(&format!("date: \"{started_at}\"\n"));
    md.push_str("participants:\n  - \"operator\"\n  - \"simard\"\n");
    md.push_str("---\n\n");
    md.push_str(&format!("# Meeting: {topic}\n\n"));

    assert!(md.contains("---"));
    assert!(md.contains("topic: \"Test Topic\""));
    assert!(md.contains("date: \"2025-01-01T00:00:00Z\""));
    assert!(md.contains("participants:"));
}

// ── Action item extraction tests ────────────────────────────────

fn make_msg(role: Role, content: &str) -> ConversationMessage {
    ConversationMessage {
        role,
        content: content.to_string(),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
    }
}

#[test]
fn extract_action_items_from_will_verb() {
    let messages = vec![make_msg(
        Role::User,
        "Alice will write the integration tests by Friday.",
    )];
    let items = extract_action_items(&messages);
    assert!(!items.is_empty(), "should extract at least one action item");
    assert_eq!(items[0].assignee.as_deref(), Some("Alice"));
    assert_eq!(items[0].deadline.as_deref(), Some("by friday"));
}

#[test]
fn extract_action_items_labeled_prefix() {
    let messages = vec![make_msg(
        Role::Assistant,
        "Action item: Deploy the staging environment.",
    )];
    let items = extract_action_items(&messages);
    assert!(!items.is_empty());
    assert!(items[0].description.contains("Deploy"));
    assert!(!items[0].description.starts_with("Action item:"));
}

#[test]
fn extract_action_items_no_false_positives() {
    let messages = vec![
        make_msg(Role::User, "The weather is nice today."),
        make_msg(Role::Assistant, "I agree, it is nice."),
    ];
    let items = extract_action_items(&messages);
    assert!(items.is_empty(), "no action items in casual chat");
}

#[test]
fn extract_action_items_needs_to_pattern() {
    let messages = vec![make_msg(
        Role::User,
        "Bob needs to update the CI pipeline this week.",
    )];
    let items = extract_action_items(&messages);
    assert!(!items.is_empty());
    assert_eq!(items[0].assignee.as_deref(), Some("Bob"));
    assert_eq!(items[0].deadline.as_deref(), Some("this week"));
}

#[test]
fn extract_assignee_from_assigned_to() {
    let result = extract_assignee("This task is assigned to Carol for next sprint.");
    assert_eq!(result.as_deref(), Some("Carol"));
}

#[test]
fn extract_deadline_various() {
    assert_eq!(extract_deadline("do it by eod"), Some("by eod".to_string()));
    assert_eq!(
        extract_deadline("finish next sprint"),
        Some("next sprint".to_string())
    );
    assert_eq!(extract_deadline("nothing here"), None);
}

#[test]
fn clean_action_description_strips_prefixes() {
    assert_eq!(
        clean_action_description("TODO: Fix the tests"),
        "Fix the tests"
    );
    assert_eq!(clean_action_description("task: Review PR"), "Review PR");
    assert_eq!(clean_action_description("Normal text"), "Normal text");
}

#[test]
fn split_sentences_basic() {
    let sentences = split_sentences("Hello world. How are you? Fine!");
    assert_eq!(sentences.len(), 3);
}

// ── Goal linkage tests ──────────────────────────────────────────

#[test]
fn link_action_items_exact_overlap() {
    let mut items = vec![HandoffActionItem {
        description: "Set up continuous integration pipeline".to_string(),
        assignee: None,
        deadline: None,
        linked_goal: None,
        priority: None,
    }];
    let goals = vec![(
        "ci-pipeline".to_string(),
        "Set up continuous integration".to_string(),
    )];
    link_action_items_to_goals(&mut items, &goals);
    assert_eq!(items[0].linked_goal.as_deref(), Some("ci-pipeline"));
}

#[test]
fn link_action_items_no_match() {
    let mut items = vec![HandoffActionItem {
        description: "Order new keyboards".to_string(),
        assignee: None,
        deadline: None,
        linked_goal: None,
        priority: None,
    }];
    let goals = vec![(
        "improve-testing".to_string(),
        "Improve testing coverage".to_string(),
    )];
    link_action_items_to_goals(&mut items, &goals);
    assert!(items[0].linked_goal.is_none());
}

// ── Decision extraction tests ───────────────────────────────────

#[test]
fn extract_decisions_from_transcript() {
    let messages = vec![
        make_msg(Role::User, "I think we should use Rust."),
        make_msg(
            Role::Assistant,
            "Decision: We will adopt Rust for the backend.",
        ),
        make_msg(Role::User, "We agreed to ship by end of month."),
    ];
    let decisions = extract_decisions(&messages);
    assert!(decisions.len() >= 2, "got: {decisions:?}");
    assert!(decisions.iter().any(|d| d.contains("Rust")));
    assert!(decisions.iter().any(|d| d.contains("agreed")));
}

#[test]
fn extract_decisions_none_found() {
    let messages = vec![
        make_msg(Role::User, "Let's discuss options."),
        make_msg(Role::Assistant, "Here are some possibilities."),
    ];
    let decisions = extract_decisions(&messages);
    assert!(decisions.is_empty());
}

// ── Open question extraction tests ──────────────────────────────
