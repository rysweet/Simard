//! Auto-detection of structured items (decisions, action items) from
//! natural conversation text.

use std::io::Write;

use crate::meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingSession, record_action_item, record_decision,
};

/// A structured item auto-detected from natural conversation.
#[derive(Clone, Debug)]
pub(super) enum AutoCaptured {
    Decision(String),
    Action(String),
}

/// Scan user and agent text for implicit decisions and action items.
///
/// Returns a list of `AutoCaptured` items found via simple keyword heuristics.
/// This does NOT replace explicit `/decision` or `/action` commands — it
/// supplements them by catching things that happen in natural conversation.
pub(super) fn auto_detect_structured_items(user_text: &str, agent_text: &str) -> Vec<AutoCaptured> {
    let mut items = Vec::new();

    // Decision patterns — things the agent completed or confirmed.
    let decision_indicators: &[&str] = &[
        "\u{2705}", // ✅
        "Closed", "closed", "Created", "created", "Merged", "merged", "Done", "Shipped", "shipped",
        "Approved", "approved", "Resolved", "resolved",
    ];

    // Action patterns — things the agent committed to doing.
    let action_indicators: &[&str] = &[
        "I'll ",
        "I will ",
        "Let me ",
        "I'll ", // curly apostrophe
        "Next step",
        "next step",
        "TODO:",
        "Will do",
        "will do",
    ];

    // Skip lines that are clearly table rows, headings, or formatting.
    let is_structural = |line: &str| -> bool {
        let t = line.trim();
        t.starts_with('|')
            || t.starts_with('#')
            || t.starts_with("---")
            || t.starts_with("===")
            || t.starts_with("```")
            || t.starts_with("**")
            || t.chars().filter(|&c| c == '|').count() >= 2
    };

    // Scan agent text for decisions — only prose lines, not tables/formatting.
    for line in agent_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.len() < 15 || is_structural(trimmed) {
            continue;
        }
        for indicator in decision_indicators {
            if trimmed.contains(indicator) {
                let desc = truncate_for_capture(trimmed, 120);
                items.push(AutoCaptured::Decision(desc));
                break;
            }
        }
    }

    // Scan agent text for action commitments — only prose lines.
    for line in agent_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.len() < 15 || is_structural(trimmed) {
            continue;
        }
        let dominated = decision_indicators.iter().any(|ind| trimmed.contains(ind));
        if dominated {
            continue;
        }
        for indicator in action_indicators {
            if trimmed.contains(indicator) {
                let desc = truncate_for_capture(trimmed, 120);
                items.push(AutoCaptured::Action(desc));
                break;
            }
        }
    }

    // Also scan user text for explicit priorities stated conversationally.
    // e.g. "Let's prioritize X" or "We decided to Y"
    let user_decision_phrases: &[&str] = &[
        "we decided",
        "let's go with",
        "decision:",
        "agreed:",
        "final answer:",
    ];
    for line in user_text.lines() {
        let lower = line.to_lowercase();
        let trimmed = line.trim();
        if trimmed.len() < 5 {
            continue;
        }
        for phrase in user_decision_phrases {
            if lower.contains(phrase) {
                let desc = truncate_for_capture(trimmed, 120);
                items.push(AutoCaptured::Decision(desc));
                break;
            }
        }
    }

    items
}

/// Record auto-captured items into the meeting session and print notifications.
pub(super) fn auto_capture_structured_items<W: Write>(
    session: &mut MeetingSession,
    user_text: &str,
    agent_text: &str,
    output: &mut W,
) {
    let items = auto_detect_structured_items(user_text, agent_text);
    for item in items {
        match item {
            AutoCaptured::Decision(ref desc) => {
                let decision = MeetingDecision {
                    description: desc.clone(),
                    rationale: "auto-detected from conversation".to_string(),
                    participants: Vec::new(),
                };
                if record_decision(session, decision).is_ok() {
                    writeln!(
                        output,
                        "  Auto-captured decision: {}",
                        short_label(desc, 60)
                    )
                    .ok();
                }
            }
            AutoCaptured::Action(ref desc) => {
                let action = ActionItem {
                    description: desc.clone(),
                    owner: "simard".to_string(),
                    priority: 1,
                    due_description: None,
                };
                if record_action_item(session, action).is_ok() {
                    writeln!(output, "  Auto-captured action: {}", short_label(desc, 60)).ok();
                }
            }
        }
    }
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate_for_capture(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Short label for notification output.
fn short_label(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_facilitator::{MeetingSession, MeetingSessionStatus};

    fn open_session() -> MeetingSession {
        MeetingSession {
            topic: "test meeting".to_string(),
            decisions: Vec::new(),
            action_items: Vec::new(),
            notes: Vec::new(),
            status: MeetingSessionStatus::Open,
            started_at: "2024-01-01T00:00:00Z".to_string(),
            participants: vec!["simard".to_string()],
            explicit_questions: Vec::new(),
        }
    }

    #[test]
    fn test_truncate_for_capture_short_string() {
        let result = truncate_for_capture("short", 120);
        assert_eq!(result, "short");
    }

    #[test]
    fn test_truncate_for_capture_exact_length() {
        let s = "a".repeat(120);
        let result = truncate_for_capture(&s, 120);
        assert_eq!(result, s);
    }

    #[test]
    fn test_truncate_for_capture_long_string() {
        let s = "a".repeat(200);
        let result = truncate_for_capture(&s, 120);
        assert_eq!(result.len(), 123); // 120 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_short_label_short_string() {
        assert_eq!(short_label("hello", 60), "hello");
    }

    #[test]
    fn test_short_label_long_string() {
        let s = "a".repeat(100);
        let result = short_label(&s, 60);
        assert_eq!(result.len(), 60);
    }

    #[test]
    fn test_auto_detect_empty_texts() {
        let items = auto_detect_structured_items("", "");
        assert!(items.is_empty());
    }

    #[test]
    fn test_auto_detect_decision_from_agent_text() {
        let agent = "✅ Closed the issue and verified the fix works correctly";
        let items = auto_detect_structured_items("", agent);
        assert!(!items.is_empty());
        assert!(matches!(items[0], AutoCaptured::Decision(_)));
    }

    #[test]
    fn test_auto_detect_action_from_agent_text() {
        let agent = "I'll refactor the module to reduce coupling next sprint";
        let items = auto_detect_structured_items("", agent);
        assert!(!items.is_empty());
        assert!(matches!(items[0], AutoCaptured::Action(_)));
    }

    #[test]
    fn test_auto_detect_decision_from_user_text() {
        let user = "we decided to use PostgreSQL for the backend database";
        let items = auto_detect_structured_items(user, "");
        assert!(!items.is_empty());
        assert!(matches!(items[0], AutoCaptured::Decision(_)));
    }

    #[test]
    fn test_auto_detect_skips_short_lines() {
        let agent = "ok";
        let items = auto_detect_structured_items("", agent);
        assert!(items.is_empty());
    }

    #[test]
    fn test_auto_detect_skips_structural_lines() {
        let agent = "| Created | something | in table row format |";
        let items = auto_detect_structured_items("", agent);
        assert!(items.is_empty());
    }

    #[test]
    fn test_auto_detect_skips_heading_lines() {
        let agent = "# Created a new heading for the document";
        let items = auto_detect_structured_items("", agent);
        assert!(items.is_empty());
    }

    #[test]
    fn test_auto_capture_writes_to_output() {
        let mut session = open_session();
        let mut output = Vec::new();
        let agent = "✅ Created the new module and verified it compiles";
        auto_capture_structured_items(&mut session, "", agent, &mut output);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Auto-captured decision"));
    }

    #[test]
    fn test_auto_capture_records_action_items() {
        let mut session = open_session();
        let mut output = Vec::new();
        let agent = "I'll implement the caching layer for faster response times";
        auto_capture_structured_items(&mut session, "", agent, &mut output);
        assert!(!session.action_items.is_empty());
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Auto-captured action"));
    }

    #[test]
    fn test_auto_detect_decision_dominated_does_not_add_action() {
        // A line with both decision and action indicators should only be a decision
        let agent = "✅ I'll ship this fix and merge it immediately now";
        let items = auto_detect_structured_items("", agent);
        let decision_count = items
            .iter()
            .filter(|i| matches!(i, AutoCaptured::Decision(_)))
            .count();
        let action_count = items
            .iter()
            .filter(|i| matches!(i, AutoCaptured::Action(_)))
            .count();
        assert!(decision_count >= 1);
        // Action should be suppressed since the line also has a decision indicator
        assert_eq!(action_count, 0);
    }
}
