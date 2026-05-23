//! Structured JSON sibling artifact emitted alongside the markdown handoff
//! report (issue #1646).
//!
//! For every markdown handoff report written to `meetings_dir()/{stem}.md`,
//! we also emit `meetings_dir()/{stem}.json` containing the same content in
//! a stable, machine-readable shape so downstream tooling (dashboards, OODA
//! loops, third-party consumers) can ingest meeting outcomes without
//! re-parsing the markdown.
//!
//! Markdown remains the canonical artifact: a JSON write failure must NOT
//! abort the markdown write — it is logged at `warn!` level and skipped,
//! mirroring the resilience pattern in [`super::write_handoff`].
//!
//! Schema: [`JsonHandoffSibling`] — versioned via [`JsonHandoffSibling::schema_version`]
//! ("v1") so consumers can branch on future schema changes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::SimardResult;
use crate::meeting_backend::types::HandoffActionItem;

use super::extract::extract_open_questions;
use crate::meeting_backend::types::ConversationMessage;
use crate::meeting_facilitator::MeetingDecision;

/// Current sibling JSON schema version. Bump when adding non-additive changes.
const SCHEMA_VERSION: &str = "v1";

/// Structured JSON sibling artifact written next to the markdown handoff
/// report. Same directory, same basename, `.json` extension.
///
/// `Option<String>` fields serialize as explicit JSON `null` (NOT omitted)
/// so consumers see a uniform shape across every emitted file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonHandoffSibling {
    /// Schema version string ("v1"). Always present.
    pub schema_version: String,
    /// Unique participant labels derived from message roles + action item
    /// assignees. Order: insertion order from extraction (deterministic).
    pub participants: Vec<String>,
    /// Decision descriptions extracted from the meeting.
    pub decisions: Vec<String>,
    /// Action items in the canonical sibling shape (title/owner/acceptance_criteria).
    pub action_items: Vec<JsonHandoffActionItem>,
    /// Open questions extracted from the transcript via [`extract_open_questions`].
    pub open_questions: Vec<String>,
    /// Reference to the canonical markdown report — the markdown file's
    /// basename (NOT a full local path, to avoid leaking host paths into a
    /// portable artifact).
    pub transcript_ref: String,
}

/// Action item shape used inside the JSON sibling.
///
/// Field mapping from [`HandoffActionItem`] (resolved per design A3):
/// - `title`               := `description`
/// - `owner`               := `assignee` (`None` for unassigned, NOT empty/"unassigned")
/// - `acceptance_criteria` := always `None` until the extractor learns to populate it (A2)
///
/// `Option<String>` fields serialize as explicit JSON `null` so every
/// `action_items[i]` entry has the same shape — consumers can rely on
/// `obj.owner === null` rather than branching on missing keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonHandoffActionItem {
    pub title: String,
    pub owner: Option<String>,
    pub acceptance_criteria: Option<String>,
}

impl From<&HandoffActionItem> for JsonHandoffActionItem {
    fn from(src: &HandoffActionItem) -> Self {
        Self {
            title: src.description.clone(),
            owner: src.assignee.clone(),
            // `acceptance_criteria` is a reserved slot for future extractor
            // work (design A2): the current heuristic extractor produces no
            // such field, so we always emit `null` here. Schema-stable.
            acceptance_criteria: None,
        }
    }
}

/// Build the [`JsonHandoffSibling`] DTO from the same inputs as the markdown
/// writer. Pure function — performs no I/O.
///
/// Reuses [`extract_open_questions`] for the `open_questions` field so there
/// is no duplicate parsing logic (per task spec).
pub(super) fn build_sibling(
    md_path: &Path,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[MeetingDecision],
) -> JsonHandoffSibling {
    // Participants: same derivation as the markdown writer — message roles
    // plus action-item assignees, deduped, insertion-order preserved.
    let mut participants: Vec<String> = Vec::new();
    for msg in messages {
        let role_name = match msg.role {
            crate::meeting_backend::types::Role::User => "operator",
            crate::meeting_backend::types::Role::Assistant => "simard",
            crate::meeting_backend::types::Role::System => "system",
        };
        let s = role_name.to_string();
        if !participants.contains(&s) {
            participants.push(s);
        }
    }
    for a in action_items {
        if let Some(ref assignee) = a.assignee
            && !participants.contains(assignee)
        {
            participants.push(assignee.clone());
        }
    }

    // Decisions: flatten to descriptions for the JSON schema (rationale and
    // participants live in the markdown view, not the structured sibling).
    let decision_strings: Vec<String> = decisions.iter().map(|d| d.description.clone()).collect();

    // Open questions: reuse the existing extractor — no duplicate parsing.
    let open_questions: Vec<String> = extract_open_questions(messages)
        .into_iter()
        .map(|q| q.text)
        .collect();

    // Action items: convert via the documented field mapping.
    let action_items: Vec<JsonHandoffActionItem> = action_items
        .iter()
        .map(JsonHandoffActionItem::from)
        .collect();

    // transcript_ref is the markdown file's basename (privacy: never the
    // full local path — see design A4).
    let transcript_ref = md_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    JsonHandoffSibling {
        schema_version: SCHEMA_VERSION.to_string(),
        participants,
        decisions: decision_strings,
        action_items,
        open_questions,
        transcript_ref,
    }
}

/// Write the JSON sibling artifact next to `md_path`.
///
/// Sibling path is `md_path.with_extension("json")`. On Unix the file mode
/// is set to `0o600` to mirror the markdown report's privacy guarantee
/// (security S2). Errors are propagated so the caller can decide whether to
/// log-and-continue (resilience) or fail.
pub(super) fn write_json_sibling_for_markdown(
    md_path: &Path,
    messages: &[ConversationMessage],
    action_items: &[HandoffActionItem],
    decisions: &[MeetingDecision],
) -> SimardResult<PathBuf> {
    let sibling = build_sibling(md_path, messages, action_items, decisions);
    let json_path = md_path.with_extension("json");

    let json = serde_json::to_string_pretty(&sibling).map_err(|e| {
        crate::error::SimardError::ActionExecutionFailed {
            action: "serialize-json-sibling".to_string(),
            reason: e.to_string(),
        }
    })?;

    std::fs::write(&json_path, &json).map_err(|e| {
        crate::error::SimardError::ActionExecutionFailed {
            action: "write-json-sibling".to_string(),
            reason: e.to_string(),
        }
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&json_path, perms) {
            warn!("Failed to set JSON sibling permissions: {e}");
        }
    }

    Ok(json_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_backend::types::{ConversationMessage, HandoffActionItem, Role};
    use crate::meeting_facilitator::MeetingDecision;

    fn msg(role: Role, content: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: "2026-01-15T10:00:00Z".to_string(),
        }
    }

    #[test]
    fn json_action_item_from_handoff_preserves_fields() {
        let src = HandoffActionItem {
            description: "Deploy to staging".to_string(),
            assignee: Some("Alice".to_string()),
            deadline: Some("friday".to_string()),
            linked_goal: Some("deploy-goal".to_string()),
            priority: Some(1),
        };
        let json_item = JsonHandoffActionItem::from(&src);
        assert_eq!(json_item.title, "Deploy to staging");
        assert_eq!(json_item.owner, Some("Alice".to_string()));
        assert_eq!(json_item.acceptance_criteria, None);
    }

    #[test]
    fn json_action_item_from_no_assignee() {
        let src = HandoffActionItem {
            description: "Unassigned task".to_string(),
            assignee: None,
            deadline: None,
            linked_goal: None,
            priority: None,
        };
        let json_item = JsonHandoffActionItem::from(&src);
        assert_eq!(json_item.owner, None);
    }

    #[test]
    fn json_action_item_round_trip_serde() {
        let item = JsonHandoffActionItem {
            title: "Test".to_string(),
            owner: Some("Bob".to_string()),
            acceptance_criteria: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        let r: JsonHandoffActionItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item, r);
    }

    #[test]
    fn build_sibling_populates_all_fields() {
        let messages = vec![
            msg(Role::User, "Let's discuss the plan."),
            msg(
                Role::Assistant,
                "OPEN: what about the timeline going forward?",
            ),
        ];
        let items = vec![HandoffActionItem {
            description: "Write docs".to_string(),
            assignee: Some("Charlie".to_string()),
            deadline: None,
            linked_goal: None,
            priority: None,
        }];
        let decisions = vec![MeetingDecision {
            description: "Use Rust".to_string(),
            rationale: "Memory safety".to_string(),
            participants: vec!["operator".to_string()],
        }];
        let md_path = std::path::Path::new("/tmp/test_report.md");
        let sibling = build_sibling(md_path, &messages, &items, &decisions);

        assert_eq!(sibling.schema_version, "v1");
        assert!(sibling.participants.contains(&"operator".to_string()));
        assert!(sibling.participants.contains(&"simard".to_string()));
        assert!(sibling.participants.contains(&"Charlie".to_string()));
        assert_eq!(sibling.decisions, vec!["Use Rust".to_string()]);
        assert_eq!(sibling.action_items.len(), 1);
        assert_eq!(sibling.action_items[0].title, "Write docs");
        assert_eq!(sibling.transcript_ref, "test_report.md");
        assert!(!sibling.open_questions.is_empty());
    }

    #[test]
    fn build_sibling_empty_inputs() {
        let sibling = build_sibling(std::path::Path::new("/tmp/empty.md"), &[], &[], &[]);
        assert_eq!(sibling.schema_version, "v1");
        assert!(sibling.participants.is_empty());
        assert!(sibling.decisions.is_empty());
        assert!(sibling.action_items.is_empty());
        assert!(sibling.open_questions.is_empty());
    }

    #[test]
    fn sibling_serde_round_trip() {
        let sibling = JsonHandoffSibling {
            schema_version: "v1".to_string(),
            participants: vec!["operator".to_string()],
            decisions: vec!["Use TDD".to_string()],
            action_items: vec![JsonHandoffActionItem {
                title: "Deploy".to_string(),
                owner: Some("Dev".to_string()),
                acceptance_criteria: None,
            }],
            open_questions: vec!["When?".to_string()],
            transcript_ref: "report.md".to_string(),
        };
        let json = serde_json::to_string_pretty(&sibling).unwrap();
        let rt: JsonHandoffSibling = serde_json::from_str(&json).unwrap();
        assert_eq!(sibling, rt);
    }

    #[test]
    fn write_json_sibling_creates_file() {
        let dir = std::env::temp_dir().join(format!(
            "json-sibling-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let md_path = dir.join("report.md");
        std::fs::write(&md_path, "# Report").unwrap();

        let result = write_json_sibling_for_markdown(&md_path, &[], &[], &[]);
        assert!(result.is_ok());
        let json_path = result.unwrap();
        assert!(json_path.exists());
        assert_eq!(json_path.extension().unwrap(), "json");

        let content = std::fs::read_to_string(&json_path).unwrap();
        let parsed: JsonHandoffSibling = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.schema_version, "v1");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
