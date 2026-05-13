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
/// Respects `SIMARD_HANDOFF_DIR` when set, otherwise falls back to
/// `$CARGO_MANIFEST_DIR/target/meeting_handoffs`.
pub fn default_handoff_dir() -> PathBuf {
    std::env::var_os("SIMARD_HANDOFF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/meeting_handoffs")
        })
}

/// A handoff artifact produced when a meeting closes. Contains decisions,
/// action items, and open questions extracted from the meeting session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingHandoff {
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
        themes.sort_by_key(|a| std::cmp::Reverse(a.1));
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
