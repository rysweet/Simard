//! Action-item / decision / theme extraction from meeting messages.

use crate::error::SimardResult;
use crate::meeting_facilitator::{ActionItem, MeetingDecision, MeetingHandoff, OpenQuestion};

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
