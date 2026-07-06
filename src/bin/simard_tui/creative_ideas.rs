//! Creative Ideas reader for the TUI.
//!
//! Reads Simard's creative-idea prospective memories out of the *same*
//! cognitive memory the goal board and journal live in
//! (`<state_root>/cognitive_memory.ladybug`), opened **read-only** so it
//! coexists with the running daemon's writer — exactly like the goals and
//! journal panes.
//!
//! Each idea is a `ProspectiveMemory` node whose `trigger_condition` is the
//! `creative-idea` sentinel and whose `action_on_trigger` carries the versioned
//! JSON payload; this module enumerates those nodes, rebuilds a
//! [`CognitiveProspective`], and reuses the library-owned
//! [`CreativeIdea::from_prospective`] parser (so there is no duplicated payload
//! parsing) plus [`latest_revision_per_idea`] to collapse revisions identically
//! to the dashboard.
//!
//! Read failures (missing DB, unreadable store) degrade gracefully to an empty
//! list: the pane shows an honest "no ideas" message rather than crashing.

use std::path::Path;

use simard::cognitive_memory::creative_idea::{
    CREATIVE_IDEA_TRIGGER, CreativeIdea, IdeaStatus, latest_revision_per_idea,
};
use simard::memory_cognitive::CognitiveProspective;

use crate::goals::COGNITIVE_MEMORY_DB;

/// The library agent id Simard writes cognitive memory under.
const AGENT_ID: &str = "simard";
/// Reject absurd payloads defensively.
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

/// Read the current creative-idea pool (latest revision per idea, newest first).
///
/// Returns an empty vector on any failure so the pane degrades honestly.
#[must_use]
pub fn read_creative_ideas(state_root: &Path) -> Vec<CreativeIdea> {
    read_inner(state_root).unwrap_or_default()
}

fn read_inner(state_root: &Path) -> Option<Vec<CreativeIdea>> {
    let db_path = state_root.join(COGNITIVE_MEMORY_DB);
    if !db_path.exists() {
        return None;
    }
    let config = lbug::SystemConfig::default().read_only(true);
    let db = lbug::Database::new(&db_path, config).ok()?;
    let conn = lbug::Connection::new(&db).ok()?;
    let query = format!(
        "MATCH (p:ProspectiveMemory) WHERE p.agent_id = '{AGENT_ID}' \
         AND p.trigger_condition = '{CREATIVE_IDEA_TRIGGER}' \
         RETURN p.node_id, p.desc_text, p.action_on_trigger",
    );
    let result = conn.query(&query).ok()?;
    let rows: Vec<Vec<lbug::Value>> = result.collect();

    let mut ideas = Vec::new();
    for row in rows {
        let node_id = string_at(&row, 0);
        let description = string_at(&row, 1);
        let action = string_at(&row, 2);
        if action.len() > MAX_PAYLOAD_BYTES {
            continue;
        }
        let node = CognitiveProspective {
            node_id,
            description,
            trigger_condition: CREATIVE_IDEA_TRIGGER.to_string(),
            action_on_trigger: action,
            status: "pending".to_string(),
            priority: 0,
        };
        // Reuse the library-owned parser; a corrupt row is skipped, not fatal.
        if let Ok(idea) = CreativeIdea::from_prospective(&node) {
            ideas.push(idea);
        }
    }
    Some(latest_revision_per_idea(ideas))
}

fn string_at(row: &[lbug::Value], idx: usize) -> String {
    match row.get(idx) {
        Some(lbug::Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A short, layperson label for a status (for the pane).
#[must_use]
pub fn status_label(status: IdeaStatus) -> &'static str {
    match status {
        IdeaStatus::New => "new",
        IdeaStatus::NeedsRevision => "needs revision",
        IdeaStatus::NeedsHumanReview => "needs human review",
        IdeaStatus::AcceptedForImplementation => "accepted",
        IdeaStatus::Rejected => "rejected",
        IdeaStatus::Deferred => "deferred",
        IdeaStatus::ImplementationStarted => "in progress",
        IdeaStatus::ImplementationCompleted => "completed",
    }
}

/// Case-insensitive match of an idea against a free-text `query` (idea + rationale).
#[must_use]
pub fn idea_matches(idea: &CreativeIdea, query: &str) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    idea.idea.to_ascii_lowercase().contains(&q)
        || idea.context.rationale.to_ascii_lowercase().contains(&q)
        || status_label(idea.status).contains(&q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_on_text_and_status() {
        let mut idea = CreativeIdea::new("improve recall ranking", ctx(), 1);
        assert!(idea_matches(&idea, "recall"));
        assert!(idea_matches(&idea, "")); // empty matches all
        assert!(idea_matches(&idea, "new")); // status label
        assert!(!idea_matches(&idea, "zzz-not-present"));
        idea.status = IdeaStatus::Rejected;
        assert!(idea_matches(&idea, "rejected"));
    }

    fn ctx() -> simard::cognitive_memory::creative_idea::IdeaContext {
        simard::cognitive_memory::creative_idea::IdeaContext {
            source: "t".into(),
            goals_snapshot: vec![],
            observation_digest: String::new(),
            rationale: "grounded rationale".into(),
        }
    }
}
