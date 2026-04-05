//! Auto-detection of structured items (decisions, action items) from
//! natural conversation text.

use std::io::Write;

use crate::meeting_facilitator::{ActionItem, MeetingDecision, MeetingSession, record_action_item, record_decision};

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
                        "  [captured: decision \u{2014} {}]",
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
                    writeln!(
                        output,
                        "  [captured: action \u{2014} {}]",
                        short_label(desc, 60)
                    )
                    .ok();
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
