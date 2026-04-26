//! Cognitive memory writers for meeting summaries and handoffs.

use tracing::{debug, info, warn};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::meeting_facilitator::{ActionItem, MeetingDecision, MeetingHandoff, OpenQuestion};

use super::super::types::{ConversationMessage, HandoffActionItem, MeetingTranscript};

/// Store the meeting as an episodic memory via the cognitive bridge.
pub fn store_cognitive_memory(
    bridge: &dyn CognitiveMemoryOps,
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
) {
    // Store full transcript as episodic memory
    if !messages.is_empty() {
        let transcript_text: String = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    crate::meeting_backend::types::Role::User => "operator",
                    crate::meeting_backend::types::Role::Assistant => "simard",
                    crate::meeting_backend::types::Role::System => "system",
                };
                format!("{}: {}", role, m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let episode_content = format!(
            "Meeting transcript — topic: {topic}\n\n{transcript_text}\n\nSummary: {summary}"
        );
        if let Err(e) = bridge.store_episode(
            &episode_content,
            "meeting-backend-transcript",
            Some(&serde_json::json!({
                "topic": topic,
                "type": "transcript",
                "message_count": messages.len(),
            })),
        ) {
            warn!("Failed to persist meeting episode: {e}");
        } else {
            debug!("Meeting episode stored");
        }
    }

    // Store summary as a semantic fact
    if !summary.is_empty() {
        let tags = vec![
            "meeting".to_string(),
            "summary".to_string(),
            topic.to_string(),
        ];
        if let Err(e) = bridge.store_fact(
            &format!("meeting:{topic}"),
            summary,
            0.85,
            &tags,
            "meeting-backend",
        ) {
            warn!("Failed to persist meeting summary fact: {e}");
        } else {
            debug!("Meeting summary fact stored");
        }
    }
}

/// Write a markdown export of the current meeting to `~/.simard/meetings/`.
///
/// The file includes YAML frontmatter (topic, date, participants) and the
/// conversation transcript formatted as markdown.

/// Store enriched meeting data (with action items) into episodic memory.
pub fn store_enriched_cognitive_memory(
    bridge: &dyn CognitiveMemoryOps,
    topic: &str,
    summary: &str,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[String],
) {
    store_cognitive_memory(bridge, topic, summary, messages);

    if !action_items.is_empty() {
        let action_text: String = action_items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let mut line = format!("{}. {}", i + 1, item.description);
                if let Some(ref who) = item.assignee {
                    line.push_str(&format!(" [assignee: {who}]"));
                }
                if let Some(ref when) = item.deadline {
                    line.push_str(&format!(" [deadline: {when}]"));
                }
                if let Some(ref goal) = item.linked_goal {
                    line.push_str(&format!(" [goal: {goal}]"));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");

        let episode = format!("Action items from meeting \"{topic}\":\n{action_text}");
        if let Err(e) = bridge.store_episode(
            &episode,
            "meeting-action-items",
            Some(&serde_json::json!({
                "topic": topic,
                "type": "action-items",
                "count": action_items.len(),
            })),
        ) {
            warn!("Failed to persist meeting action-items episode: {e}");
        } else {
            debug!("Meeting action-items episode stored");
        }
    }

    if !decisions.is_empty() {
        let decision_text = decisions
            .iter()
            .enumerate()
            .map(|(i, d)| format!("{}. {}", i + 1, d))
            .collect::<Vec<_>>()
            .join("\n");

        let episode = format!("Decisions from meeting \"{topic}\":\n{decision_text}");
        if let Err(e) = bridge.store_episode(
            &episode,
            "meeting-decisions",
            Some(&serde_json::json!({
                "topic": topic,
                "type": "decisions",
                "count": decisions.len(),
            })),
        ) {
            warn!("Failed to persist meeting decisions episode: {e}");
        } else {
            debug!("Meeting decisions episode stored");
        }
    }
}
