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
