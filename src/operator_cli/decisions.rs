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
            println!("  - {q}");
        }
    }

    mark_meeting_handoff_processed(&dir)?;
    println!("\nDone. Created {created} issue(s). Handoff marked as processed.");
    Ok(())
}
