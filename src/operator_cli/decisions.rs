use crate::meeting_facilitator::{
    default_handoff_dir, load_meeting_handoff, mark_meeting_handoff_processed,
};

/// Run `gh issue create` and print the result. Returns `true` on success.
fn gh_create_issue(title: &str, body: &str, label: &str) -> bool {
    match std::process::Command::new("gh")
        .args(["issue", "create", "--title", title, "--body", body])
        .output()
    {
        Ok(output) if output.status.success() => {
            let url = String::from_utf8_lossy(&output.stdout);
            println!("  Created issue: {label} → {}", url.trim());
            true
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "  [warn] gh issue create failed for '{label}': {}",
                stderr.trim()
            );
            false
        }
        Err(e) => {
            eprintln!("  [warn] Failed to run gh: {e}");
            false
        }
    }
}

/// Read the latest meeting handoff and create GitHub issues for each
/// decision and action item via `gh issue create`.
pub(super) fn dispatch_act_on_decisions() -> Result<(), Box<dyn std::error::Error>> {
    let dir = default_handoff_dir();
    let handoff = load_meeting_handoff(&dir)?;

    let Some(handoff) = handoff else {
        println!("No meeting handoff found in {}", dir.display());
        return Ok(());
    };

    if handoff.processed {
        println!(
            "Meeting handoff already processed (topic: {})",
            handoff.topic
        );
        return Ok(());
    }

    println!(
        "Processing meeting handoff: {} (closed {})",
        handoff.topic, handoff.closed_at
    );

    let mut created = 0u32;

    for decision in &handoff.decisions {
        let title = format!("Decision: {}", decision.description);
        let body = format!(
            "**Rationale:** {}\n**Participants:** {}\n\n_From meeting: {}_",
            decision.rationale,
            if decision.participants.is_empty() {
                "(none)".to_string()
            } else {
                decision.participants.join(", ")
            },
            handoff.topic,
        );
        if gh_create_issue(&title, &body, &decision.description) {
            created += 1;
        }
    }

    for item in &handoff.action_items {
        let title = format!("Action: {}", item.description);
        let due = item.due_description.as_deref().unwrap_or("(unspecified)");
        let body = format!(
            "**Owner:** {}\n**Priority:** {}\n**Due:** {}\n\n_From meeting: {}_",
            item.owner, item.priority, due, handoff.topic,
        );
        if gh_create_issue(&title, &body, &item.description) {
            created += 1;
        }
    }

    if !handoff.open_questions.is_empty() {
        println!("\nOpen questions (not filed as issues):");
        for q in &handoff.open_questions {
            let tag = if q.explicit { "explicit" } else { "inferred" };
            println!("  - [{tag}] {}", q.text);
        }
    }

    mark_meeting_handoff_processed(&dir)?;
    println!("\nDone. Created {created} issue(s). Handoff marked as processed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_facilitator::{ActionItem, MeetingDecision, OpenQuestion};
    use crate::meeting_facilitator::{
        MeetingHandoff, load_meeting_handoff, mark_meeting_handoff_processed, write_meeting_handoff,
    };
    use tempfile::tempdir;

    fn sample_handoff(processed: bool) -> MeetingHandoff {
        MeetingHandoff {
            topic: "Sprint Review".to_string(),
            started_at: "2025-01-01T00:00:00Z".to_string(),
            closed_at: "2025-01-01T01:00:00Z".to_string(),
            decisions: vec![MeetingDecision {
                description: "Adopt Rust".to_string(),
                rationale: "Safety".to_string(),
                participants: vec!["alice".to_string()],
            }],
            action_items: vec![ActionItem {
                description: "Write tests".to_string(),
                owner: "bob".to_string(),
                priority: 1,
                due_description: Some("next week".to_string()),
            }],
            open_questions: vec![OpenQuestion {
                text: "What about Python?".to_string(),
                explicit: true,
            }],
            processed,
            duration_secs: Some(3600),
            transcript: vec![],
            participants: vec!["alice".to_string(), "bob".to_string()],
            themes: Vec::new(),
        }
    }

    #[test]
    fn gh_create_issue_handles_missing_binary() {
        let result = gh_create_issue("test-only", "body", "label");
        let _ = result;
    }

    #[test]
    fn dispatch_fn_exists() {
        let _fn_ref: fn() -> Result<(), Box<dyn std::error::Error>> = dispatch_act_on_decisions;
    }

    #[test]
    fn handoff_serde_round_trip() {
        let h = sample_handoff(false);
        let json = serde_json::to_string(&h).unwrap();
        let h2: MeetingHandoff = serde_json::from_str(&json).unwrap();
        assert_eq!(h.topic, h2.topic);
        assert_eq!(h.decisions.len(), h2.decisions.len());
        assert_eq!(h.action_items.len(), h2.action_items.len());
        assert_eq!(h.processed, h2.processed);
    }

    #[test]
    fn handoff_processed_flag_default() {
        let json = r#"{"topic":"t","started_at":"","closed_at":"","decisions":[],"action_items":[],"open_questions":[]}"#;
        let h: MeetingHandoff = serde_json::from_str(json).unwrap();
        assert!(!h.processed);
    }

    #[test]
    fn handoff_processed_true() {
        let h = sample_handoff(true);
        assert!(h.processed);
    }

    #[test]
    fn handoff_open_questions_preserved() {
        let h = sample_handoff(false);
        assert_eq!(h.open_questions.len(), 1);
        assert!(h.open_questions[0].explicit);
        assert!(h.open_questions[0].text.contains("Python"));
    }

    #[test]
    fn handoff_empty_decisions_and_actions() {
        let h = MeetingHandoff {
            topic: "empty".to_string(),
            started_at: String::new(),
            closed_at: String::new(),
            decisions: vec![],
            action_items: vec![],
            open_questions: vec![],
            processed: false,
            duration_secs: None,
            transcript: vec![],
            participants: vec![],
            themes: Vec::new(),
        };
        let json = serde_json::to_string(&h).unwrap();
        let h2: MeetingHandoff = serde_json::from_str(&json).unwrap();
        assert!(h2.decisions.is_empty());
        assert!(h2.action_items.is_empty());
    }

    #[test]
    fn write_and_load_handoff_round_trip() {
        let tmp = tempdir().unwrap();
        let h = sample_handoff(false);
        write_meeting_handoff(tmp.path(), &h).unwrap();
        let loaded = load_meeting_handoff(tmp.path()).unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.topic, "Sprint Review");
        assert!(!loaded.processed);
    }

    #[test]
    fn mark_handoff_processed_updates_flag() {
        let tmp = tempdir().unwrap();
        let h = sample_handoff(false);
        write_meeting_handoff(tmp.path(), &h).unwrap();
        mark_meeting_handoff_processed(tmp.path()).unwrap();
        let loaded = load_meeting_handoff(tmp.path()).unwrap().unwrap();
        assert!(loaded.processed);
    }
}
