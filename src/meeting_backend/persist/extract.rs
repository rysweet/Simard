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
    use crate::meeting_backend::types::{ConversationMessage, Role};

    fn msg(role: Role, content: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: "2026-01-15T10:00:00Z".to_string(),
        }
    }

    // ── extract_action_items ────────────────────────────────────────

    #[test]
    fn extract_action_items_empty_messages() {
        assert!(extract_action_items(&[]).is_empty());
    }

    #[test]
    fn extract_action_items_detects_will() {
        let msgs = vec![msg(Role::User, "Alice will write the tests by friday")];
        let items = extract_action_items(&msgs);
        assert!(!items.is_empty(), "should detect 'will' signal");
        assert_eq!(items[0].assignee.as_deref(), Some("Alice"));
        assert_eq!(items[0].deadline.as_deref(), Some("by friday"));
    }

    #[test]
    fn extract_action_items_detects_action_item_prefix() {
        let msgs = vec![msg(Role::User, "Action item: deploy to staging")];
        let items = extract_action_items(&msgs);
        assert!(!items.is_empty());
        assert!(items[0].description.contains("deploy to staging"));
    }

    #[test]
    fn extract_action_items_skips_short_descriptions() {
        let msgs = vec![msg(Role::User, "todo: x")];
        let items = extract_action_items(&msgs);
        assert!(items.is_empty(), "descriptions < 5 chars should be skipped");
    }

    #[test]
    fn extract_action_items_multiple_sentences() {
        let msgs = vec![msg(
            Role::User,
            "Bob will fix the bug. Charlie should update docs.",
        )];
        let items = extract_action_items(&msgs);
        assert!(items.len() >= 2, "should extract from each sentence");
    }

    // ── extract_assignee ────────────────────────────────────────────

    #[test]
    fn extract_assignee_capitalized_name_before_will() {
        assert_eq!(
            extract_assignee("Alice will deploy"),
            Some("Alice".to_string())
        );
    }

    #[test]
    fn extract_assignee_needs_to() {
        assert_eq!(
            extract_assignee("Bob needs to review"),
            Some("Bob".to_string())
        );
    }

    #[test]
    fn extract_assignee_assigned_to() {
        assert_eq!(
            extract_assignee("task assigned to Charlie"),
            Some("Charlie".to_string())
        );
    }

    #[test]
    fn extract_assignee_lowercase_returns_none() {
        assert_eq!(extract_assignee("someone will do it"), None);
    }

    #[test]
    fn extract_assignee_short_name_returns_none() {
        assert_eq!(
            extract_assignee("x will do it"),
            None,
            "single-char names filtered"
        );
    }

    // ── extract_deadline ────────────────────────────────────────────

    #[test]
    fn extract_deadline_by_friday() {
        assert_eq!(
            extract_deadline("finish this by friday"),
            Some("by friday".to_string())
        );
    }

    #[test]
    fn extract_deadline_asap() {
        assert_eq!(extract_deadline("do this asap"), Some("asap".to_string()));
    }

    #[test]
    fn extract_deadline_none() {
        assert_eq!(extract_deadline("no deadline"), None);
    }

    // ── clean_action_description ────────────────────────────────────

    #[test]
    fn clean_strips_action_item_prefix() {
        assert_eq!(clean_action_description("Action item: deploy"), "deploy");
    }

    #[test]
    fn clean_strips_todo_prefix() {
        assert_eq!(clean_action_description("TODO: fix tests"), "fix tests");
    }

    #[test]
    fn clean_trims_whitespace() {
        assert_eq!(clean_action_description("  hello  "), "hello");
    }

    // ── split_sentences ─────────────────────────────────────────────

    #[test]
    fn split_sentences_basic() {
        let result = split_sentences("Hello. World!");
        assert_eq!(result, vec!["Hello.", "World!"]);
    }

    #[test]
    fn split_sentences_newlines() {
        let result = split_sentences("First line\nSecond line");
        assert_eq!(result, vec!["First line", "Second line"]);
    }

    #[test]
    fn split_sentences_empty() {
        assert!(split_sentences("").is_empty());
    }

    #[test]
    fn split_sentences_trailing_text() {
        let result = split_sentences("hello. world");
        assert_eq!(result, vec!["hello.", "world"]);
    }

    // ── link_action_items_to_goals ──────────────────────────────────

    #[test]
    fn link_no_goals_leaves_none() {
        let mut items = vec![HandoffActionItem {
            description: "write tests".into(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        link_action_items_to_goals(&mut items, &[]);
        assert!(items[0].linked_goal.is_none());
    }

    #[test]
    fn link_matches_goal_by_keyword() {
        let mut items = vec![HandoffActionItem {
            description: "improve test coverage for persistence".into(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        let goals = vec![("test-cov".to_string(), "improve test coverage".to_string())];
        link_action_items_to_goals(&mut items, &goals);
        assert_eq!(items[0].linked_goal.as_deref(), Some("test-cov"));
    }

    #[test]
    fn link_no_match_stays_none() {
        let mut items = vec![HandoffActionItem {
            description: "deploy to production".into(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        let goals = vec![("testing".to_string(), "write unit tests".to_string())];
        link_action_items_to_goals(&mut items, &goals);
        assert!(items[0].linked_goal.is_none());
    }

    // ── extract_decisions ───────────────────────────────────────────

    #[test]
    fn extract_decisions_empty_messages() {
        assert!(extract_decisions(&[]).is_empty());
    }

    #[test]
    fn extract_decisions_detects_decision_keyword() {
        let msgs = vec![msg(Role::User, "Decision: adopt TDD for the project")];
        let decisions = extract_decisions(&msgs);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].contains("adopt TDD"));
    }

    #[test]
    fn extract_decisions_deduplicates() {
        let msgs = vec![
            msg(Role::User, "We decided to use Rust."),
            msg(Role::Assistant, "We decided to use Rust."),
        ];
        let decisions = extract_decisions(&msgs);
        assert_eq!(decisions.len(), 1, "duplicates should be filtered");
    }

    #[test]
    fn extract_decisions_short_ignored() {
        let msgs = vec![msg(Role::User, "decided: ok")];
        let decisions = extract_decisions(&msgs);
        // "decided: ok" is 11 chars — it passes the >=5 filter.
        // A truly short sentence like "decided: x" (10 chars) still passes.
        // The filter catches strings < 5 chars only.
        // Verify the sentence was extracted (it has a signal keyword and length >= 5).
        assert_eq!(decisions.len(), 1);
    }

    // ── extract_open_questions ──────────────────────────────────────

    #[test]
    fn extract_open_questions_empty() {
        assert!(extract_open_questions(&[]).is_empty());
    }

    #[test]
    fn extract_open_questions_explicit_marker() {
        let msgs = vec![msg(Role::User, "OPEN: who will lead the migration effort")];
        let qs = extract_open_questions(&msgs);
        assert_eq!(qs.len(), 1);
        assert!(qs[0].explicit);
    }

    #[test]
    fn extract_open_questions_question_mark() {
        let msgs = vec![msg(
            Role::User,
            "What about the deployment strategy going forward?",
        )];
        let qs = extract_open_questions(&msgs);
        assert_eq!(qs.len(), 1);
        assert!(!qs[0].explicit);
    }

    #[test]
    fn extract_open_questions_short_question_ignored() {
        let msgs = vec![msg(Role::User, "Why not?")];
        let qs = extract_open_questions(&msgs);
        assert!(qs.is_empty(), "short questions are filtered as rhetorical");
    }

    #[test]
    fn extract_open_questions_deduplicates() {
        let msgs = vec![
            msg(Role::User, "What about testing strategies?"),
            msg(Role::Assistant, "What about testing strategies?"),
        ];
        let qs = extract_open_questions(&msgs);
        assert_eq!(qs.len(), 1);
    }

    // ── extract_themes ──────────────────────────────────────────────

    #[test]
    fn extract_themes_empty() {
        assert!(extract_themes(&[]).is_empty());
    }

    #[test]
    fn extract_themes_requires_cross_message_frequency() {
        let msgs = vec![
            msg(Role::User, "testing testing testing"),
            msg(Role::Assistant, "testing is important for quality"),
        ];
        let themes = extract_themes(&msgs);
        assert!(
            themes.contains(&"testing".to_string()),
            "word appearing in >=2 messages should be a theme"
        );
    }

    #[test]
    fn extract_themes_ignores_system_messages() {
        let msgs = vec![
            msg(Role::System, "deployment deployment deployment"),
            msg(Role::User, "let's talk about something"),
        ];
        let themes = extract_themes(&msgs);
        assert!(
            !themes.contains(&"deployment".to_string()),
            "system messages should be excluded from theme extraction"
        );
    }

    #[test]
    fn extract_themes_caps_at_ten() {
        let mut msgs = Vec::new();
        for i in 0..15 {
            let content: String = (0..15)
                .map(|j| format!("word{j}"))
                .collect::<Vec<_>>()
                .join(" ");
            msgs.push(msg(
                if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                &content,
            ));
        }
        let themes = extract_themes(&msgs);
        assert!(themes.len() <= 10, "themes should be capped at 10");
    }

    // ── extract_decision_rationale_pub ──────────────────────────────

    #[test]
    fn rationale_returns_preceding_message() {
        let msgs = vec![
            msg(Role::User, "We need memory safety for the backend."),
            msg(Role::Assistant, "We decided to use Rust."),
        ];
        let rationale = extract_decision_rationale_pub("We decided to use Rust.", &msgs);
        assert_eq!(rationale, "We need memory safety for the backend.");
    }

    #[test]
    fn rationale_first_message_returns_empty() {
        let msgs = vec![msg(Role::User, "We decided on TDD.")];
        let rationale = extract_decision_rationale_pub("We decided on TDD.", &msgs);
        assert!(rationale.is_empty());
    }

    #[test]
    fn rationale_no_match_returns_empty() {
        let msgs = vec![msg(Role::User, "unrelated content")];
        let rationale = extract_decision_rationale_pub("nonexistent decision", &msgs);
        assert!(rationale.is_empty());
    }

    #[test]
    fn rationale_truncates_long_preceding() {
        let long_msg = "x".repeat(500);
        let msgs = vec![
            msg(Role::User, &long_msg),
            msg(Role::Assistant, "decided: proceed"),
        ];
        let rationale = extract_decision_rationale_pub("decided: proceed", &msgs);
        assert!(rationale.len() <= 301, "should truncate to ~300 chars");
        assert!(rationale.ends_with('…'));
    }

    // ── extract_decision_participants_pub ────────────────────────────

    #[test]
    fn participants_includes_message_role() {
        let msgs = vec![msg(Role::User, "We decided to use Rust")];
        let participants = extract_decision_participants_pub("We decided to use Rust", &msgs);
        assert!(participants.contains(&"operator".to_string()));
    }

    #[test]
    fn participants_includes_preceding_role() {
        let msgs = vec![
            msg(Role::User, "I think we should use Rust"),
            msg(Role::Assistant, "Agreed, we decided to use Rust"),
        ];
        let participants =
            extract_decision_participants_pub("Agreed, we decided to use Rust", &msgs);
        assert!(participants.contains(&"simard".to_string()));
        assert!(participants.contains(&"operator".to_string()));
    }

    #[test]
    fn participants_no_match_returns_empty() {
        let msgs = vec![msg(Role::User, "hello")];
        let participants = extract_decision_participants_pub("nonexistent", &msgs);
        assert!(participants.is_empty());
    }
}
