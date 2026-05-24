//! Meeting handoff artifacts — written when a meeting closes, consumed by
//! the engineer loop and the `act-on-decisions` CLI subcommand.

use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::types::{ActionItem, MeetingDecision, MeetingSession, OpenQuestion};

/// Well-known filename for meeting handoff artifacts.
pub const MEETING_HANDOFF_FILENAME: &str = "meeting_handoff.json";

/// Well-known filename for the work-in-progress session snapshot.
pub const MEETING_SESSION_WIP_FILENAME: &str = "meeting_session_wip.json";

/// Default directory for meeting handoff artifacts.
///
/// Precedence ladder (issue #1906):
/// 1. `SIMARD_HANDOFF_DIR` — narrow override (preserves backward compat
///    with the legacy idiom used across `tests/ooda_loop` and the
///    `meeting_backend::tests_persist_extra` suite).
/// 2. `SIMARD_STATE_ROOT/meeting_handoffs` — broad override via the shared
///    [`crate::state_root`] helper.
/// 3. `~/.simard/meeting_handoffs/` — default.
///
/// `CARGO_MANIFEST_DIR` is no longer consulted at runtime; previously
/// `default_handoff_dir()` baked the manifest dir into release binaries.
pub fn default_handoff_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SIMARD_HANDOFF_DIR") {
        let s = p.to_string_lossy();
        if !s.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    crate::state_root::resolve_subdir("meeting_handoffs")
}

/// Well-known artifact-kind tag for a meeting transcript file.
pub const ARTIFACT_KIND_TRANSCRIPT: &str = "transcript";
/// Well-known artifact-kind tag for a per-meeting bundle directory.
pub const ARTIFACT_KIND_BUNDLE: &str = "bundle";
/// Well-known artifact-kind tag for the human-readable markdown report.
pub const ARTIFACT_KIND_MARKDOWN_REPORT: &str = "markdown_report";
/// Well-known artifact-kind tag for an applied template-agenda file.
pub const ARTIFACT_KIND_TEMPLATE_AGENDA: &str = "template_agenda";
/// Catch-all artifact-kind tag for anything not covered by the well-known set.
pub const ARTIFACT_KIND_OTHER: &str = "other";

/// A single artifact pointer carried in the meeting handoff payload.
///
/// Lets downstream consumers (engineer loop, dashboard chat, `act-on-decisions`)
/// link directly to artifacts produced by the close pipeline (transcript,
/// bundle directory, markdown report, applied template agendas) instead of
/// re-deriving paths from `meeting_id`, `topic`, and `started_at`.
///
/// `kind` is one of the well-known [`ARTIFACT_KIND_TRANSCRIPT`],
/// [`ARTIFACT_KIND_BUNDLE`], [`ARTIFACT_KIND_MARKDOWN_REPORT`],
/// [`ARTIFACT_KIND_TEMPLATE_AGENDA`], or [`ARTIFACT_KIND_OTHER`]. Custom
/// kinds are permitted but consumers may only render the well-known ones
/// specially; unknown kinds fall through to a generic listing.
///
/// `path` is the URI-or-path field named in issue #1954. It is a string
/// so artifacts can refer to absolute filesystem paths, bundle-relative
/// paths, or remote URIs (e.g. `https://…/meeting_handoff.md`) without
/// inventing a separate enum.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HandoffArtifact {
    /// Well-known kind tag — see the `ARTIFACT_KIND_*` constants.
    pub kind: String,
    /// Filesystem path or remote URI pointing at the artifact.
    ///
    /// Field name matches the schema in issue #1954. The `path` alias is
    /// accepted on deserialize for tooling that wrote the prior shape.
    #[serde(alias = "path")]
    pub uri_or_path: String,
    /// Optional human-readable description of what the artifact contains.
    #[serde(default)]
    pub description: Option<String>,
}

/// Default handoff schema version for new writes.
const DEFAULT_HANDOFF_SCHEMA_VERSION: u32 = 2;

/// Default version when deserializing v1 handoffs that lack the field.
fn default_handoff_schema_version_v1() -> u32 {
    1
}

/// A handoff artifact produced when a meeting closes. Contains decisions,
/// action items, open questions, the next responsible owner, and a list
/// of linked artifacts.
///
/// All fields added after the initial schema use `#[serde(default)]` so
/// legacy handoffs (written before the field existed) deserialize cleanly
/// with empty defaults. No on-disk migration step is required.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingHandoff {
    /// Schema version. v2 writes carry `2`; older v1 handoffs missing
    /// this field default to `1` on read.
    #[serde(default = "default_handoff_schema_version_v1")]
    pub schema_version: u32,
    /// Stable, sortable identifier for this meeting. Derived from
    /// `started_at` plus a slug of the topic. Empty in legacy artifacts —
    /// callers reading old handoffs should fall back to
    /// [`derive_meeting_id`] for a synthesized id.
    #[serde(default)]
    pub meeting_id: String,
    pub topic: String,
    pub started_at: String,
    /// Time the meeting ended, RFC3339. Accepts the legacy field name
    /// `closed_at` on read so older handoffs deserialize cleanly.
    #[serde(alias = "closed_at")]
    pub closed_at: String,
    pub decisions: Vec<MeetingDecision>,
    pub action_items: Vec<ActionItem>,
    pub open_questions: Vec<OpenQuestion>,
    #[serde(default)]
    pub processed: bool,
    #[serde(default)]
    pub duration_secs: Option<u64>,
    #[serde(default)]
    pub transcript: Vec<String>,
    /// Filesystem path to the full transcript artifact (e.g. the
    /// `transcript.json` inside the per-meeting bundle directory). Empty
    /// for legacy handoffs that inlined only a summary into `transcript`.
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub participants: Vec<String>,
    /// High-level themes or recurring topics identified during the meeting.
    #[serde(default)]
    pub themes: Vec<String>,
    /// Names the agent, persona, or human expected to action this handoff
    /// (e.g. `"engineer"`, `"ooda-curate"`, `"act-on-decisions"`, or a
    /// GitHub handle). Producer derives from the explicit `/owner <name>`
    /// slash command and otherwise falls back to the most-frequent
    /// `action_items[].owner`. Added in issue #1954; legacy handoffs
    /// deserialize as `None`.
    #[serde(default)]
    pub next_owner: Option<String>,
    /// Direct pointers to the artifacts produced by the close pipeline
    /// (transcript file, bundle directory, markdown report, applied
    /// template agendas, …). Lets consumers link without re-deriving
    /// paths from `meeting_id`. Added in issue #1954; legacy handoffs
    /// deserialize as `[]`.
    #[serde(default)]
    pub artifacts: Vec<HandoffArtifact>,
    /// The meeting's overarching objective, distinct from the short
    /// `topic`. Set by `/goal <text>` at the REPL; falls back to the
    /// first user message if unset. Added in issue #1987.
    #[serde(default)]
    pub goal: Option<String>,
    /// Structured routing hint: which actor should consume this handoff
    /// next. Complements the free-form `next_owner` string. Added in
    /// issue #1987.
    #[serde(default)]
    pub next_actor: Option<super::types::NextActor>,
    /// Templates applied during the meeting (via `/template <name>`).
    /// Already present on `MeetingSummary`; promoted to the handoff
    /// so downstream consumers see them without parsing the bundle.
    /// Added in issue #1987.
    #[serde(default)]
    pub applied_templates: Vec<crate::meeting_backend::AppliedTemplate>,
    /// Number of history messages dropped due to the `MAX_HISTORY` cap
    /// (currently 500). Lets consumers gauge transcript completeness.
    /// Added in issue #1987.
    #[serde(default)]
    pub history_truncated_count: usize,
    /// Wire-string form of `PartialReason` from the close pipeline.
    /// `Some("summary_timeout")` etc. when the close was partial;
    /// `None` for a clean close. Added in issue #1987.
    #[serde(default)]
    pub partial_reason: Option<String>,
}

/// Build a stable, sortable meeting id from a started-at timestamp and a
/// topic. Format is `YYYYMMDDTHHMMSSZ-<slug>` so ids sort by time and are
/// safe as a directory name.
///
/// Falls back to the current UTC time when `started_at` is empty or not
/// parseable — never panics.
pub fn derive_meeting_id(started_at: &str, topic: &str) -> String {
    let ts = chrono::DateTime::parse_from_rfc3339(started_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
        .format("%Y%m%dT%H%M%SZ")
        .to_string();
    let slug = slugify_topic(topic);
    if slug.is_empty() {
        ts
    } else {
        format!("{ts}-{slug}")
    }
}

/// Lower-case slug of a topic suitable for a filesystem path component.
/// Keeps `[a-z0-9-]`, collapses runs of separators, and caps length.
fn slugify_topic(topic: &str) -> String {
    let mut out = String::with_capacity(topic.len());
    let mut prev_dash = false;
    for c in topic.chars() {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > 64 {
        out.truncate(64);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// Check whether a note looks like a rhetorical question (short, common
/// filler phrases) so we can filter it out of open questions.
fn is_rhetorical(note: &str) -> bool {
    let trimmed = note.trim().trim_end_matches('?').trim();
    // Very short questions are usually rhetorical ("Why not?", "Right?").
    if trimmed.len() < 15 {
        return true;
    }
    let lower = note.trim().to_lowercase();
    let rhetorical_patterns = [
        "right?",
        "isn't it?",
        "aren't they?",
        "don't you think?",
        "wouldn't you say?",
        "isn't that so?",
        "why not?",
        "who knows?",
        "who cares?",
        "what else?",
        "so what?",
        "how about that?",
    ];
    rhetorical_patterns
        .iter()
        .any(|p| lower == *p || lower.ends_with(&format!(" {p}")))
}

/// Prefixes (case-insensitive) that mark a note as an explicit open question
/// even when it does not contain a `?`.
const OPEN_QUESTION_PREFIXES: &[&str] = &["open:", "todo:", "question:", "tbd:", "unresolved:"];

/// Returns `true` if `note` should be extracted as an open question.
pub(super) fn is_open_question(note: &str) -> bool {
    let lower = note.trim().to_lowercase();

    // Explicit markers always count.
    for prefix in OPEN_QUESTION_PREFIXES {
        if lower.starts_with(prefix) {
            return true;
        }
    }

    // Notes with `?` count unless they look rhetorical.
    if note.contains('?') && !is_rhetorical(note) {
        return true;
    }

    false
}

impl MeetingHandoff {
    /// Create a handoff from a closed meeting session.
    ///
    /// Open questions are extracted from two sources:
    /// 1. **Explicit** — questions added via `/question` during the meeting.
    /// 2. **Inferred** — notes containing `?` (unless rhetorical) or notes
    ///    starting with explicit markers (`OPEN:`, `TODO:`, `QUESTION:`,
    ///    `TBD:`, `UNRESOLVED:`).
    pub fn from_session(session: &MeetingSession) -> Self {
        // Explicit questions from /question command.
        let mut open_questions: Vec<OpenQuestion> = session
            .explicit_questions
            .iter()
            .map(|q| OpenQuestion {
                text: q.clone(),
                explicit: true,
            })
            .collect();

        // Inferred questions from notes heuristics.
        let inferred: Vec<OpenQuestion> = session
            .notes
            .iter()
            .filter(|n| is_open_question(n))
            .map(|n| OpenQuestion {
                text: n.clone(),
                explicit: false,
            })
            .collect();
        open_questions.extend(inferred);

        let duration_secs = chrono::DateTime::parse_from_rfc3339(&session.started_at)
            .ok()
            .map(|start| Utc::now().signed_duration_since(start).num_seconds().max(0) as u64);

        let transcript = session.notes.clone();

        // Collect unique participants from session.participants, decision participants, and action owners.
        let mut all_participants: Vec<String> = session.participants.clone();
        for d in &session.decisions {
            for p in &d.participants {
                if !all_participants.contains(p) {
                    all_participants.push(p.clone());
                }
            }
        }
        for a in &session.action_items {
            if !all_participants.contains(&a.owner) {
                all_participants.push(a.owner.clone());
            }
        }

        // Extract themes from notes; use decision/action text if notes
        // are empty (common in the backend code path which uses messages, not notes).
        // Explicit /theme entries from session always take priority.
        let inferred: Vec<String> = {
            let mut t = Self::extract_themes_from_notes(&session.notes);
            if t.is_empty() {
                let fallback_texts: Vec<String> = session
                    .decisions
                    .iter()
                    .map(|d| d.description.clone())
                    .chain(session.action_items.iter().map(|a| a.description.clone()))
                    .collect();
                t = Self::extract_themes_from_notes(&fallback_texts);
            }
            t
        };
        let mut themes: Vec<String> = session.themes.clone();
        for t in inferred {
            let lower = t.to_lowercase();
            if !themes.iter().any(|e| e.to_lowercase() == lower) {
                themes.push(t);
            }
        }

        Self {
            schema_version: DEFAULT_HANDOFF_SCHEMA_VERSION,
            meeting_id: derive_meeting_id(&session.started_at, &session.topic),
            topic: session.topic.clone(),
            started_at: session.started_at.clone(),
            closed_at: Utc::now().to_rfc3339(),
            decisions: session.decisions.clone(),
            action_items: session.action_items.clone(),
            open_questions,
            processed: false,
            duration_secs,
            transcript,
            participants: all_participants,
            themes,
            transcript_path: None,
            next_owner: session.next_owner.clone(),
            artifacts: Vec::new(),
            goal: session.goal.clone(),
            next_actor: None,
            applied_templates: Vec::new(),
            history_truncated_count: 0,
            partial_reason: None,
        }
    }

    /// Extract recurring theme keywords from meeting notes.
    fn extract_themes_from_notes(notes: &[String]) -> Vec<String> {
        use std::collections::HashMap;

        const STOP_WORDS: &[&str] = &[
            "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with",
            "by", "is", "it", "that", "this", "was", "are", "be", "has", "have", "had", "not",
            "we", "they", "you", "will", "can", "should", "would", "could", "do", "does", "did",
            "from", "about", "into", "out", "if", "then", "so", "up", "one", "all", "been", "just",
            "also", "than", "like", "more", "some", "what", "when", "how", "who", "which", "there",
            "their", "our", "i", "my", "me", "your", "its",
        ];

        let mut word_freq: HashMap<String, usize> = HashMap::new();
        for note in notes {
            let mut seen = std::collections::HashSet::new();
            let words: Vec<String> = note
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric() && c != '-')
                .filter(|w| w.len() > 3 && !STOP_WORDS.contains(w))
                .map(String::from)
                .collect();
            for w in words {
                if seen.insert(w.clone()) {
                    *word_freq.entry(w).or_insert(0) += 1;
                }
            }
        }

        let min_freq = 2;
        let mut themes: Vec<(String, usize)> = word_freq
            .into_iter()
            .filter(|(_, count)| *count >= min_freq)
            .collect();
        themes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        themes.truncate(10);
        themes.into_iter().map(|(word, _)| word).collect()
    }
}

/// Write a meeting handoff artifact to a directory.
mod persistence;
// `find_newest_handoff` is consumed by cfg(test) module `tests_handoff_extra`;
// clippy flags it as unused in non-test compilation. Keep the re-export stable.
#[allow(unused_imports)]
pub use persistence::{
    BundleTranscriptLine, bundle_handoff_path, bundle_markdown_path, bundle_transcript_path,
    default_bundle_root, find_newest_handoff, find_oldest_unprocessed_handoff,
    load_meeting_handoff, load_session_wip, mark_handoff_processed_in_place,
    mark_meeting_handoff_processed, meeting_bundle_dir, remove_session_wip, save_session_wip,
    write_meeting_bundle, write_meeting_handoff,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_facilitator::types::{
        ActionItem, MeetingDecision, MeetingSession, MeetingSessionStatus,
    };

    fn empty_session() -> MeetingSession {
        MeetingSession {
            topic: String::new(),
            decisions: vec![],
            action_items: vec![],
            notes: vec![],
            status: MeetingSessionStatus::Open,
            started_at: String::new(),
            participants: vec![],
            explicit_questions: vec![],
            themes: vec![],
            next_owner: None,
            goal: None,
        }
    }

    fn populated_session() -> MeetingSession {
        MeetingSession {
            topic: "Sprint planning".to_string(),
            decisions: vec![MeetingDecision {
                description: "Adopt TDD".to_string(),
                rationale: "Better quality".to_string(),
                participants: vec!["alice".to_string()],
            }],
            action_items: vec![ActionItem {
                description: "Set up CI".to_string(),
                owner: "bob".to_string(),
                priority: 1,
                due_description: Some("Friday".to_string()),
                linked_issue: None,
            }],
            notes: vec![
                "Good discussion about testing".to_string(),
                "OPEN: what about the timeline?".to_string(),
                "We need more testing coverage".to_string(),
                "testing should be a priority".to_string(),
            ],
            status: MeetingSessionStatus::Open,
            started_at: "2026-01-15T10:00:00Z".to_string(),
            participants: vec!["alice".to_string(), "bob".to_string()],
            explicit_questions: vec!["What about testing?".to_string()],
            themes: vec!["performance".to_string()],
            next_owner: Some("engineer".to_string()),
            goal: None,
        }
    }

    // ── default_handoff_dir ─────────────────────────────────────────

    #[test]
    fn default_handoff_dir_returns_path() {
        let dir = default_handoff_dir();
        let dir_str = dir.to_string_lossy();
        // May resolve via SIMARD_STATE_ROOT or SIMARD_HANDOFF_DIR env override.
        assert!(
            dir_str.contains("meeting_handoffs") || dir_str.contains("simard"),
            "default dir should be under simard state: {dir_str}"
        );
    }

    // ── derive_meeting_id ───────────────────────────────────────────

    #[test]
    fn derive_meeting_id_valid_timestamp() {
        let id = derive_meeting_id("2026-01-15T10:00:00Z", "Sprint Planning");
        assert!(id.starts_with("20260115T100000Z-"));
        assert!(id.contains("sprint"));
    }

    #[test]
    fn derive_meeting_id_idempotent() {
        let a = derive_meeting_id("2026-01-15T10:00:00Z", "topic");
        let b = derive_meeting_id("2026-01-15T10:00:00Z", "topic");
        assert_eq!(a, b);
    }

    #[test]
    fn derive_meeting_id_invalid_timestamp_does_not_panic() {
        let id = derive_meeting_id("not-a-date", "topic");
        assert!(id.contains("topic"));
    }

    #[test]
    fn derive_meeting_id_empty_topic() {
        let id = derive_meeting_id("2026-01-15T10:00:00Z", "");
        assert!(id.starts_with("20260115T100000Z"));
        assert!(
            !id.contains('-'),
            "empty topic should produce no slug suffix"
        );
    }

    #[test]
    fn derive_meeting_id_topic_punctuation_stable() {
        let a = derive_meeting_id("2026-01-15T10:00:00Z", "Sprint Planning!");
        let b = derive_meeting_id("2026-01-15T10:00:00Z", "Sprint Planning!");
        assert_eq!(a, b, "punctuation handling should be deterministic");
    }

    // ── is_open_question ────────────────────────────────────────────

    #[test]
    fn is_open_question_explicit_prefix() {
        assert!(is_open_question("OPEN: who will lead?"));
        assert!(is_open_question("question: what about coverage?"));
        assert!(is_open_question("TBD: deployment strategy"));
    }

    #[test]
    fn is_open_question_genuine_question() {
        assert!(is_open_question(
            "What about the deployment strategy going forward?"
        ));
    }

    #[test]
    fn is_open_question_rhetorical_rejected() {
        assert!(!is_open_question("Right?"));
        assert!(!is_open_question("Why not?"));
    }

    #[test]
    fn is_open_question_no_question_no_prefix() {
        assert!(!is_open_question("This is a statement about testing"));
    }

    // ── MeetingHandoff::from_session ────────────────────────────────

    #[test]
    fn from_session_empty_no_panic() {
        let session = empty_session();
        let handoff = MeetingHandoff::from_session(&session);
        assert!(handoff.decisions.is_empty());
        assert!(handoff.action_items.is_empty());
        assert!(handoff.open_questions.is_empty());
        assert!(handoff.themes.is_empty());
        assert!(handoff.next_owner.is_none());
        assert!(!handoff.processed);
    }

    #[test]
    fn from_session_populated_carries_decisions() {
        let handoff = MeetingHandoff::from_session(&populated_session());
        assert_eq!(handoff.decisions.len(), 1);
        assert_eq!(handoff.decisions[0].description, "Adopt TDD");
    }

    #[test]
    fn from_session_populated_carries_action_items() {
        let handoff = MeetingHandoff::from_session(&populated_session());
        assert_eq!(handoff.action_items.len(), 1);
        assert_eq!(handoff.action_items[0].owner, "bob");
    }

    #[test]
    fn from_session_explicit_questions_marked_explicit() {
        let handoff = MeetingHandoff::from_session(&populated_session());
        let explicit: Vec<_> = handoff
            .open_questions
            .iter()
            .filter(|q| q.explicit)
            .collect();
        assert!(!explicit.is_empty(), "explicit questions should be flagged");
        assert!(explicit.iter().any(|q| q.text.contains("testing")));
    }

    #[test]
    fn from_session_inferred_questions_from_notes() {
        let handoff = MeetingHandoff::from_session(&populated_session());
        let inferred: Vec<_> = handoff
            .open_questions
            .iter()
            .filter(|q| !q.explicit)
            .collect();
        // "OPEN: what about the timeline?" should be inferred from notes.
        assert!(
            !inferred.is_empty() || handoff.open_questions.len() >= 2,
            "should have inferred questions from notes"
        );
    }

    #[test]
    fn from_session_collects_all_participants() {
        let handoff = MeetingHandoff::from_session(&populated_session());
        assert!(handoff.participants.contains(&"alice".to_string()));
        assert!(handoff.participants.contains(&"bob".to_string()));
    }

    #[test]
    fn from_session_preserves_next_owner() {
        let handoff = MeetingHandoff::from_session(&populated_session());
        assert_eq!(handoff.next_owner.as_deref(), Some("engineer"));
    }

    #[test]
    fn from_session_topic_and_meeting_id() {
        let handoff = MeetingHandoff::from_session(&populated_session());
        assert_eq!(handoff.topic, "Sprint planning");
        assert!(!handoff.meeting_id.is_empty());
        assert!(handoff.meeting_id.contains("sprint"));
    }

    #[test]
    fn from_session_themes_include_explicit_and_inferred() {
        let handoff = MeetingHandoff::from_session(&populated_session());
        assert!(
            handoff.themes.contains(&"performance".to_string()),
            "explicit themes should be preserved"
        );
        // "testing" appears in 3+ notes → should be inferred as theme.
        assert!(
            handoff.themes.contains(&"testing".to_string()),
            "inferred theme 'testing' should appear: {:?}",
            handoff.themes
        );
    }

    // ── extract_themes_from_notes ───────────────────────────────────

    #[test]
    fn extract_themes_from_notes_empty() {
        let themes = MeetingHandoff::extract_themes_from_notes(&[]);
        assert!(themes.is_empty());
    }

    #[test]
    fn extract_themes_from_notes_dedup() {
        let notes = vec![
            "testing testing testing".to_string(),
            "testing is key to quality".to_string(),
        ];
        let themes = MeetingHandoff::extract_themes_from_notes(&notes);
        assert!(themes.contains(&"testing".to_string()));
        // No duplicates — the word appears once in the result.
        assert_eq!(themes.iter().filter(|t| t.as_str() == "testing").count(), 1);
    }

    #[test]
    fn extract_themes_from_notes_ordering_stable() {
        let notes = vec![
            "alpha bravo charlie delta".to_string(),
            "alpha bravo charlie delta".to_string(),
            "alpha bravo echo foxtrot".to_string(),
        ];
        let a = MeetingHandoff::extract_themes_from_notes(&notes);
        let b = MeetingHandoff::extract_themes_from_notes(&notes);
        assert_eq!(a, b, "theme extraction should be deterministic");
    }
}
