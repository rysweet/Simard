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
