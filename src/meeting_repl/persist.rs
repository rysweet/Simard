//! Post-meeting persistence — handoff artifacts and memory storage.

use std::io::Write;

use crate::meeting_facilitator::MeetingSession;
use crate::memory_bridge::CognitiveMemoryBridge;

pub(super) fn write_meeting_handoff_artifact<W: Write>(closed: &MeetingSession, output: &mut W) {
    use crate::meeting_facilitator::{MeetingHandoff, default_handoff_dir, write_meeting_handoff};
    let handoff = MeetingHandoff::from_session(closed);
    let handoff_dir = default_handoff_dir();
    if let Err(e) = write_meeting_handoff(&handoff_dir, &handoff) {
        writeln!(output, "[warn] Failed to write meeting handoff: {e}").ok();
    } else {
        writeln!(
            output,
            "Meeting handoff written ({} decisions, {} actions). Run `simard act-on-decisions` to create issues.",
            handoff.decisions.len(),
            handoff.action_items.len(),
        ).ok();
    }
}

pub(super) fn persist_meeting_to_memory<W: Write>(
    closed: &MeetingSession,
    bridge: &CognitiveMemoryBridge,
    output: &mut W,
) {
    if !closed.notes.is_empty() {
        let transcript_text = closed.notes.join("\n");
        let episode_content = format!(
            "Meeting transcript — topic: {}\n\n{}",
            closed.topic, transcript_text
        );
        if let Err(e) = bridge.store_episode(
            &episode_content,
            "meeting-repl-transcript",
            Some(&serde_json::json!({
                "topic": closed.topic,
                "type": "transcript",
                "decisions": closed.decisions.len(),
                "action_items": closed.action_items.len(),
            })),
        ) {
            writeln!(output, "[warn] Failed to persist transcript: {e}").ok();
        }
    }

    for decision in &closed.decisions {
        let tags = vec![
            "meeting".to_string(),
            "decision".to_string(),
            closed.topic.clone(),
        ];
        if let Err(e) = bridge.store_fact(
            &format!("decision:{}", decision.description),
            &format!(
                "Decision: {} — Rationale: {}",
                decision.description, decision.rationale
            ),
            0.9,
            &tags,
            "meeting-repl",
        ) {
            writeln!(
                output,
                "[warn] Failed to persist decision '{}': {e}",
                decision.description
            )
            .ok();
        }
    }

    for item in &closed.action_items {
        if let Err(e) = bridge.store_prospective(
            &format!("Meeting action: {}", item.description),
            &format!("owner={} begins related work", item.owner),
            &format!(
                "Remind {} to complete: {} (priority {})",
                item.owner, item.description, item.priority
            ),
            i64::from(item.priority),
        ) {
            writeln!(
                output,
                "[warn] Failed to persist action '{}': {e}",
                item.description
            )
            .ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_facilitator::{
        ActionItem, MeetingDecision, MeetingSession, MeetingSessionStatus,
    };

    fn sample_empty_session() -> MeetingSession {
        MeetingSession {
            topic: "test topic".to_string(),
            decisions: vec![],
            action_items: vec![],
            notes: vec![],
            status: MeetingSessionStatus::Closed,
            started_at: "2024-01-01T00:00:00Z".to_string(),
            participants: vec!["alice".to_string()],
            explicit_questions: vec![],
        }
    }

    fn sample_session_with_data() -> MeetingSession {
        MeetingSession {
            topic: "architecture review".to_string(),
            decisions: vec![MeetingDecision {
                description: "use microservices".to_string(),
                rationale: "scalability".to_string(),
                participants: vec!["alice".to_string()],
            }],
            action_items: vec![ActionItem {
                description: "create RFC".to_string(),
                owner: "bob".to_string(),
                priority: 1,
                due_description: None,
            }],
            notes: vec!["discussed trade-offs".to_string()],
            status: MeetingSessionStatus::Closed,
            started_at: "2024-01-01T00:00:00Z".to_string(),
            participants: vec!["alice".to_string(), "bob".to_string()],
            explicit_questions: vec![],
        }
    }

    #[test]
    fn write_meeting_handoff_artifact_empty_session() {
        let session = sample_empty_session();
        let mut output = Vec::new();
        write_meeting_handoff_artifact(&session, &mut output);
        let text = String::from_utf8(output).unwrap();
        // Should produce output (success or warn message)
        assert!(!text.is_empty());
    }

    #[test]
    fn write_meeting_handoff_artifact_with_data() {
        let session = sample_session_with_data();
        let mut output = Vec::new();
        write_meeting_handoff_artifact(&session, &mut output);
        let text = String::from_utf8(output).unwrap();
        assert!(!text.is_empty());
    }
}
