//! Action-item / decision / theme extraction from meeting messages.

use crate::meeting_facilitator::OpenQuestion;

use crate::meeting_backend::types::{ConversationMessage, HandoffActionItem};

const ACTION_SIGNALS: &[&str] = &[
    "action item:",
    "todo:",
    "to-do:",
    "task:",
    "ai:",
    " will ",
    " should ",
    " needs to ",
    " need to ",
    " must ",
    "let's ",
    "let\u{2019}s ",
    "follow up",
    "follow-up",
];

const DEADLINE_SIGNALS: &[&str] = &[
    "by friday",
    "by monday",
    "by tuesday",
    "by wednesday",
    "by thursday",
    "by saturday",
    "by sunday",
    "by tomorrow",
    "by end of day",
    "by eod",
    "by end of week",
    "by eow",
    "by next week",
    "by next sprint",
    "next sprint",
    "this week",
    "this sprint",
    "asap",
    "immediately",
    "today",
    "tonight",
];

/// Extract structured action items from a conversation transcript.
///
/// Uses heuristic signal phrases to identify action items from both user and
/// assistant messages. This is a best-effort extraction — the LLM summary
/// provides the authoritative narrative.
pub fn extract_action_items(messages: &[ConversationMessage]) -> Vec<HandoffActionItem> {
    let mut items = Vec::new();
    for msg in messages {
        let lower = msg.content.to_lowercase();
        let is_action = ACTION_SIGNALS.iter().any(|s| lower.contains(s));
        if !is_action {
            continue;
        }

        for sentence in split_sentences(&msg.content) {
            let sent_lower = sentence.to_lowercase();
            let has_signal = ACTION_SIGNALS.iter().any(|s| sent_lower.contains(s));
            if !has_signal {
                continue;
            }

            let description = clean_action_description(&sentence);
            if description.len() < 5 {
                continue;
            }

            let assignee = extract_assignee(&sentence);
            let deadline = extract_deadline(&sent_lower);

            items.push(HandoffActionItem {
                description,
                assignee,
                deadline,
                linked_goal: None,
                priority: None,
            });
        }
    }
    items
}

/// Try to extract an assignee from a sentence.
pub(crate) fn extract_assignee(sentence: &str) -> Option<String> {
    let verbs = [" will ", " should ", " needs to ", " need to ", " must "];
    for verb in &verbs {
        if let Some(idx) = sentence.to_lowercase().find(verb) {
            let prefix = sentence[..idx].trim();
            if let Some(name) = prefix.split_whitespace().last() {
                let clean = name.trim_matches(|c: char| !c.is_alphanumeric());
                if !clean.is_empty()
                    && clean.len() >= 2
                    && clean.chars().next().is_some_and(|c| c.is_uppercase())
                {
                    return Some(clean.to_string());
                }
            }
        }
    }
    if let Some(idx) = sentence.to_lowercase().find("assigned to ") {
        let after = &sentence[idx + "assigned to ".len()..];
        if let Some(name) = after.split_whitespace().next() {
            let clean = name.trim_matches(|c: char| !c.is_alphanumeric());
            if !clean.is_empty() && clean.len() >= 2 {
                return Some(clean.to_string());
            }
        }
    }
    None
}

/// Extract a deadline phrase if present.
pub(crate) fn extract_deadline(lower_sentence: &str) -> Option<String> {
    for signal in DEADLINE_SIGNALS {
        if lower_sentence.contains(signal) {
            return Some(signal.trim().to_string());
        }
    }
    None
}

/// Clean up an action item description — strip leading signal labels.
pub(crate) fn clean_action_description(sentence: &str) -> String {
    let mut s = sentence.trim().to_string();
    let prefixes = [
        "action item:",
        "Action item:",
        "ACTION ITEM:",
        "todo:",
        "TODO:",
        "To-do:",
        "to-do:",
        "task:",
        "Task:",
        "TASK:",
        "ai:",
        "AI:",
    ];
    for prefix in &prefixes {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
            break;
        }
    }
    s
}

/// Split text into sentences (simple heuristic).
pub(crate) fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if ch == '.' || ch == '!' || ch == '?' || ch == '\n' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current.clear();
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        sentences.push(trimmed);
    }
    sentences
}

// ── Goal linkage ────────────────────────────────────────────────────────

/// Match extracted action items against active goals by keyword overlap.
pub fn link_action_items_to_goals(
    items: &mut [HandoffActionItem],
    goal_titles: &[(String, String)],
) {
    for item in items.iter_mut() {
        let item_words: Vec<String> = item
            .description
            .to_lowercase()
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .map(|w| w.to_string())
            .collect();

        let mut best_match: Option<(&str, usize)> = None;

        for (slug, title) in goal_titles {
            let goal_words: Vec<String> = title
                .to_lowercase()
                .split_whitespace()
                .filter(|w| w.len() > 2)
                .map(|w| w.to_string())
                .collect();

            let overlap = item_words.iter().filter(|w| goal_words.contains(w)).count();

            let threshold = if goal_words.len() <= 2 { 1 } else { 2 };
            if overlap >= threshold && (best_match.is_none() || overlap > best_match.unwrap().1) {
                best_match = Some((slug.as_str(), overlap));
            }
        }

        if let Some((slug, _)) = best_match {
            item.linked_goal = Some(slug.to_string());
        }
    }
}

/// Extract decision statements from transcript messages.
pub fn extract_decisions(messages: &[ConversationMessage]) -> Vec<String> {
    let decision_signals = [
        "decision:",
        "decided:",
        "we decided",
        "we agreed",
        "the decision is",
        "agreed to",
        "conclusion:",
    ];
    let mut decisions = Vec::new();
    for msg in messages {
        for sentence in split_sentences(&msg.content) {
            let lower = sentence.to_lowercase();
            if decision_signals.iter().any(|s| lower.contains(s)) {
                let clean = sentence.trim().to_string();
                if clean.len() >= 5 && !decisions.contains(&clean) {
                    decisions.push(clean);
                }
            }
        }
    }
    decisions
}

/// Extract open questions from transcript messages.
///
/// Looks for explicit question markers (`OPEN:`, `QUESTION:`, `TBD:`, etc.) and
/// genuine questions (sentences containing `?` that aren't too short/rhetorical).
pub fn extract_open_questions(messages: &[ConversationMessage]) -> Vec<OpenQuestion> {
    let explicit_prefixes = ["open:", "question:", "tbd:", "unresolved:"];
    let mut questions: Vec<OpenQuestion> = Vec::new();

    for msg in messages {
        for sentence in split_sentences(&msg.content) {
            let lower = sentence.trim().to_lowercase();

            // Check explicit markers first.
            let is_explicit = explicit_prefixes.iter().any(|p| lower.starts_with(p));
            if is_explicit {
                let text = sentence.trim().to_string();
                if !questions.iter().any(|q| q.text == text) {
                    questions.push(OpenQuestion {
                        text,
                        explicit: true,
                    });
                }
                continue;
            }

            // Genuine questions: contains `?`, long enough to not be rhetorical.
            if sentence.contains('?') && sentence.trim().len() >= 15 {
                let text = sentence.trim().to_string();
                if !questions.iter().any(|q| q.text == text) {
                    questions.push(OpenQuestion {
                        text,
                        explicit: false,
                    });
                }
            }
        }
    }
    questions
}

/// Extract high-level themes from transcript messages by frequency analysis.
///
/// Identifies recurring topic keywords (nouns/phrases that appear across multiple
/// messages) and returns them as theme strings.
pub fn extract_themes(messages: &[ConversationMessage]) -> Vec<String> {
    use std::collections::HashMap;

    // Common stop words to ignore.
    const STOP_WORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with", "by",
        "is", "it", "that", "this", "was", "are", "be", "has", "have", "had", "not", "we", "they",
        "you", "will", "can", "should", "would", "could", "do", "does", "did", "from", "about",
        "into", "out", "if", "then", "so", "up", "one", "all", "been", "just", "also", "than",
        "like", "more", "some", "what", "when", "how", "who", "which", "there", "their", "our",
        "i", "my", "me", "your", "its",
    ];

    let mut word_freq: HashMap<String, usize> = HashMap::new();
    for msg in messages {
        // Only count user and assistant messages, skip system.
        if matches!(msg.role, crate::meeting_backend::types::Role::System) {
            continue;
        }
        let words: Vec<String> = msg
            .content
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric() && c != '-')
            .filter(|w| w.len() > 3 && !STOP_WORDS.contains(w))
            .map(String::from)
            .collect();
        // Count unique words per message to avoid single-message spam.
        let mut seen = std::collections::HashSet::new();
        for w in words {
            if seen.insert(w.clone()) {
                *word_freq.entry(w).or_insert(0) += 1;
            }
        }
    }

    // Themes are words appearing in at least 2 messages.
    let min_freq = 2;
    let mut themes: Vec<(String, usize)> = word_freq
        .into_iter()
        .filter(|(_, count)| *count >= min_freq)
        .collect();
    themes.sort_by_key(|a| std::cmp::Reverse(a.1));
    themes.truncate(10);
    themes.into_iter().map(|(word, _)| word).collect()
}

/// Extract rationale context for a decision from surrounding messages.
///
/// Looks for the message containing the decision text and checks the preceding
/// message for context that explains *why* the decision was made.
fn extract_decision_rationale(decision: &str, messages: &[ConversationMessage]) -> String {
    let decision_lower = decision.to_lowercase();
    for (i, msg) in messages.iter().enumerate() {
        if msg.content.to_lowercase().contains(&decision_lower) {
            // Check the preceding message for rationale context.
            if i > 0 {
                let prev = &messages[i - 1].content;
                // Truncate long rationale to keep handoff concise.
                if prev.len() > 300 {
                    return format!("{}…", &prev[..297]);
                }
                return prev.clone();
            }
        }
    }
    String::new()
}

/// Public wrapper for extracting rationale — used by the backend on close.
pub fn extract_decision_rationale_pub(decision: &str, messages: &[ConversationMessage]) -> String {
    extract_decision_rationale(decision, messages)
}

/// Extract participant roles involved in a decision from the message that
/// contains it and the preceding message.
fn extract_decision_participants(decision: &str, messages: &[ConversationMessage]) -> Vec<String> {
    let decision_lower = decision.to_lowercase();
    let mut participants = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg.content.to_lowercase().contains(&decision_lower) {
            let role = match msg.role {
                crate::meeting_backend::types::Role::User => "operator",
                crate::meeting_backend::types::Role::Assistant => "simard",
                crate::meeting_backend::types::Role::System => "system",
            };
            participants.push(role.to_string());
            // Include the role from the preceding message if it contributed.
            if i > 0 {
                let prev_role = match messages[i - 1].role {
                    crate::meeting_backend::types::Role::User => "operator",
                    crate::meeting_backend::types::Role::Assistant => "simard",
                    crate::meeting_backend::types::Role::System => "system",
                };
                if !participants.contains(&prev_role.to_string()) {
                    participants.push(prev_role.to_string());
                }
            }
            break;
        }
    }
    participants
}

/// Public wrapper for extracting decision participants — used by the backend on close.
pub fn extract_decision_participants_pub(
    decision: &str,
    messages: &[ConversationMessage],
) -> Vec<String> {
    extract_decision_participants(decision, messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_backend::types::{ConversationMessage, HandoffActionItem, Role};

    fn msg(role: Role, content: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn split_sentences_basic() {
        let result = split_sentences("Hello world. Goodbye world!");
        assert_eq!(result, vec!["Hello world.", "Goodbye world!"]);
    }

    #[test]
    fn split_sentences_newline_delimiter() {
        let result = split_sentences("Line one\nLine two");
        assert_eq!(result, vec!["Line one", "Line two"]);
    }

    #[test]
    fn split_sentences_question_mark() {
        let result = split_sentences("Is this a test? Yes it is.");
        assert_eq!(result, vec!["Is this a test?", "Yes it is."]);
    }

    #[test]
    fn split_sentences_empty() {
        assert!(split_sentences("").is_empty());
    }

    #[test]
    fn split_sentences_no_terminator() {
        let result = split_sentences("No terminator here");
        assert_eq!(result, vec!["No terminator here"]);
    }

    #[test]
    fn split_sentences_trailing_whitespace() {
        let result = split_sentences("  Hello.   ");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "Hello.");
    }

    #[test]
    fn clean_strips_action_item_prefix() {
        assert_eq!(
            clean_action_description("action item: fix the bug"),
            "fix the bug"
        );
    }

    #[test]
    fn clean_strips_todo_prefix() {
        assert_eq!(clean_action_description("TODO: write tests"), "write tests");
    }

    #[test]
    fn clean_strips_task_prefix() {
        assert_eq!(
            clean_action_description("Task: deploy the service"),
            "deploy the service"
        );
    }

    #[test]
    fn clean_preserves_normal_text() {
        assert_eq!(
            clean_action_description("Alice will fix it"),
            "Alice will fix it"
        );
    }

    #[test]
    fn clean_strips_ai_prefix() {
        assert_eq!(clean_action_description("AI: review PR"), "review PR");
    }

    #[test]
    fn assignee_from_will_verb() {
        assert_eq!(
            extract_assignee("Alice will fix the tests"),
            Some("Alice".to_string())
        );
    }

    #[test]
    fn assignee_from_should_verb() {
        assert_eq!(
            extract_assignee("Bob should review the PR"),
            Some("Bob".to_string())
        );
    }

    #[test]
    fn assignee_from_needs_to_verb() {
        assert_eq!(
            extract_assignee("Charlie needs to update docs"),
            Some("Charlie".to_string())
        );
    }

    #[test]
    fn assignee_from_assigned_to() {
        assert_eq!(
            extract_assignee("This is assigned to Dave immediately"),
            Some("Dave".to_string())
        );
    }

    #[test]
    fn assignee_none_when_lowercase_prefix() {
        assert_eq!(extract_assignee("we will do it"), None);
    }

    #[test]
    fn assignee_none_for_short_name() {
        assert_eq!(extract_assignee("I will do it"), None);
    }

    #[test]
    fn deadline_by_friday() {
        assert_eq!(
            extract_deadline("finish this by friday"),
            Some("by friday".to_string())
        );
    }

    #[test]
    fn deadline_asap() {
        assert_eq!(
            extract_deadline("we need this asap"),
            Some("asap".to_string())
        );
    }

    #[test]
    fn deadline_none_when_absent() {
        assert_eq!(extract_deadline("no deadline mentioned"), None);
    }

    #[test]
    fn deadline_by_eod() {
        assert_eq!(
            extract_deadline("please complete by eod"),
            Some("by eod".to_string())
        );
    }

    #[test]
    fn deadline_next_sprint() {
        assert_eq!(
            extract_deadline("ship it next sprint"),
            Some("next sprint".to_string())
        );
    }

    #[test]
    fn extract_action_items_basic() {
        let messages = vec![msg(Role::User, "Alice will fix the login bug by friday.")];
        let items = extract_action_items(&messages);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].assignee, Some("Alice".to_string()));
        assert_eq!(items[0].deadline, Some("by friday".to_string()));
        assert!(
            items[0]
                .description
                .contains("Alice will fix the login bug")
        );
    }

    #[test]
    fn extract_action_items_todo_prefix() {
        let messages = vec![msg(Role::Assistant, "TODO: Write unit tests for persist.")];
        let items = extract_action_items(&messages);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].description, "Write unit tests for persist.");
    }

    #[test]
    fn extract_action_items_skips_short_desc() {
        let messages = vec![msg(Role::User, "todo: ok")];
        let items = extract_action_items(&messages);
        assert!(items.is_empty());
    }

    #[test]
    fn extract_action_items_empty_messages() {
        let items = extract_action_items(&[]);
        assert!(items.is_empty());
    }

    #[test]
    fn extract_action_items_no_signals() {
        let messages = vec![msg(Role::User, "The weather is nice today.")];
        let items = extract_action_items(&messages);
        assert!(items.is_empty());
    }

    #[test]
    fn extract_action_items_multiple_sentences() {
        let messages = vec![msg(
            Role::User,
            "Alice will fix the bug. Bob should review the PR.",
        )];
        let items = extract_action_items(&messages);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn extract_action_items_follow_up_signal() {
        let messages = vec![msg(Role::User, "We need a follow-up meeting by tomorrow.")];
        let items = extract_action_items(&messages);
        assert!(!items.is_empty(), "follow-up should trigger action signal");
        assert_eq!(items[0].deadline, Some("by tomorrow".to_string()));
    }

    #[test]
    fn link_action_items_matches_goal() {
        let mut items = vec![HandoffActionItem {
            description: "Improve the search performance significantly".to_string(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        let goals = vec![(
            "perf-sprint".to_string(),
            "Improve search performance".to_string(),
        )];
        link_action_items_to_goals(&mut items, &goals);
        assert_eq!(items[0].linked_goal, Some("perf-sprint".to_string()));
    }

    #[test]
    fn link_action_items_no_match() {
        let mut items = vec![HandoffActionItem {
            description: "Fix login page".to_string(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        let goals = vec![("perf".to_string(), "Improve search performance".to_string())];
        link_action_items_to_goals(&mut items, &goals);
        assert_eq!(items[0].linked_goal, None);
    }

    #[test]
    fn link_action_items_picks_best_match() {
        let mut items = vec![HandoffActionItem {
            description: "Improve the search performance and indexing".to_string(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        let goals = vec![
            ("auth".to_string(), "Authentication overhaul".to_string()),
            (
                "perf".to_string(),
                "Improve search performance indexing".to_string(),
            ),
        ];
        link_action_items_to_goals(&mut items, &goals);
        assert_eq!(items[0].linked_goal, Some("perf".to_string()));
    }

    #[test]
    fn extract_decisions_basic() {
        let messages = vec![msg(
            Role::User,
            "Decision: We will use Rust for the rewrite.",
        )];
        let decisions = extract_decisions(&messages);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].contains("We will use Rust"));
    }

    #[test]
    fn extract_decisions_agreed_to() {
        let messages = vec![msg(Role::Assistant, "We agreed to ship on Monday.")];
        let decisions = extract_decisions(&messages);
        assert_eq!(decisions.len(), 1);
    }

    #[test]
    fn extract_decisions_deduplicates() {
        let messages = vec![
            msg(Role::User, "Decision: Use Rust."),
            msg(Role::Assistant, "Decision: Use Rust."),
        ];
        let decisions = extract_decisions(&messages);
        assert_eq!(decisions.len(), 1);
    }

    #[test]
    fn extract_decisions_empty() {
        let decisions = extract_decisions(&[]);
        assert!(decisions.is_empty());
    }

    #[test]
    fn extract_decisions_no_signals() {
        let messages = vec![msg(Role::User, "The weather is fine.")];
        let decisions = extract_decisions(&messages);
        assert!(decisions.is_empty());
    }

    #[test]
    fn extract_open_questions_explicit_prefix() {
        let messages = vec![msg(Role::User, "OPEN: Who will lead the migration?")];
        let questions = extract_open_questions(&messages);
        assert_eq!(questions.len(), 1);
        assert!(questions[0].explicit);
    }

    #[test]
    fn extract_open_questions_question_mark() {
        let messages = vec![msg(
            Role::User,
            "Should we migrate to the new framework entirely?",
        )];
        let questions = extract_open_questions(&messages);
        assert_eq!(questions.len(), 1);
        assert!(!questions[0].explicit);
    }

    #[test]
    fn extract_open_questions_short_question_skipped() {
        let messages = vec![msg(Role::User, "Right?")];
        let questions = extract_open_questions(&messages);
        assert!(questions.is_empty());
    }

    #[test]
    fn extract_open_questions_deduplicates() {
        let messages = vec![
            msg(Role::User, "Should we deploy this to production now?"),
            msg(Role::Assistant, "Should we deploy this to production now?"),
        ];
        let questions = extract_open_questions(&messages);
        assert_eq!(questions.len(), 1);
    }

    #[test]
    fn extract_open_questions_tbd_prefix() {
        let messages = vec![msg(Role::User, "TBD: resource allocation for Q3")];
        let questions = extract_open_questions(&messages);
        assert_eq!(questions.len(), 1);
        assert!(questions[0].explicit);
    }

    #[test]
    fn extract_themes_recurring_words() {
        let messages = vec![
            msg(Role::User, "We need better performance for search."),
            msg(
                Role::Assistant,
                "Performance improvements will help search speed.",
            ),
        ];
        let themes = extract_themes(&messages);
        assert!(
            themes.contains(&"performance".to_string()) || themes.contains(&"search".to_string()),
            "Expected recurring words in themes: {themes:?}"
        );
    }

    #[test]
    fn extract_themes_skips_system_messages() {
        let messages = vec![
            msg(Role::System, "performance performance performance"),
            msg(Role::User, "Hello, let's begin."),
        ];
        let themes = extract_themes(&messages);
        assert!(
            !themes.contains(&"performance".to_string()),
            "System messages should be skipped"
        );
    }

    #[test]
    fn extract_themes_empty_messages() {
        let themes = extract_themes(&[]);
        assert!(themes.is_empty());
    }

    #[test]
    fn extract_themes_single_message_no_themes() {
        let messages = vec![msg(Role::User, "Unique words in a single message only.")];
        let themes = extract_themes(&messages);
        assert!(themes.is_empty());
    }

    #[test]
    fn extract_themes_caps_at_ten() {
        let many_words: Vec<String> = (0..20).map(|i| format!("keyword{i}")).collect();
        let content = many_words.join(" ");
        let messages = vec![msg(Role::User, &content), msg(Role::Assistant, &content)];
        let themes = extract_themes(&messages);
        assert!(themes.len() <= 10);
    }

    #[test]
    fn rationale_from_preceding_message() {
        let messages = vec![
            msg(Role::User, "Rust gives us memory safety."),
            msg(Role::Assistant, "We decided to adopt Rust for the rewrite."),
        ];
        let rationale =
            extract_decision_rationale_pub("We decided to adopt Rust for the rewrite.", &messages);
        assert!(rationale.contains("memory safety"));
    }

    #[test]
    fn rationale_empty_when_no_preceding() {
        let messages = vec![msg(Role::User, "We decided to use Rust.")];
        let rationale = extract_decision_rationale_pub("We decided to use Rust.", &messages);
        assert!(rationale.is_empty());
    }

    #[test]
    fn rationale_truncates_long_preceding() {
        let long_msg = "x".repeat(400);
        let messages = vec![
            msg(Role::User, &long_msg),
            msg(Role::Assistant, "We decided to proceed."),
        ];
        let rationale = extract_decision_rationale_pub("We decided to proceed.", &messages);
        assert!(rationale.len() <= 300, "rationale should be truncated");
        assert!(rationale.ends_with('…'));
    }

    #[test]
    fn participants_from_decision_message() {
        let messages = vec![
            msg(Role::User, "We should adopt this."),
            msg(Role::Assistant, "We decided to use Rust."),
        ];
        let parts = extract_decision_participants_pub("We decided to use Rust.", &messages);
        assert!(parts.contains(&"simard".to_string()));
        assert!(parts.contains(&"operator".to_string()));
    }

    #[test]
    fn participants_empty_when_decision_not_found() {
        let messages = vec![msg(Role::User, "Nothing relevant.")];
        let parts = extract_decision_participants_pub("Nonexistent decision", &messages);
        assert!(parts.is_empty());
    }

    #[test]
    fn participants_single_message_no_predecessor() {
        let messages = vec![msg(Role::User, "We decided to ship it.")];
        let parts = extract_decision_participants_pub("We decided to ship it.", &messages);
        assert_eq!(parts, vec!["operator".to_string()]);
    }
}
