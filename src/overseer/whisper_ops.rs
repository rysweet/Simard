//! The **Simard Whisperer** — the Overseer's lightweight steering channel.
//!
//! A *whisper* is a short, advisory steering note the Overseer injects into
//! Simard's OODA loop when it observes her looping without progress or drifting
//! from a goal's intent. Unlike the meeting/handoff escalation path (which hands
//! a goal to Simard), a whisper is **advisory context only**: it rides onto the
//! SAME `meeting_handoffs` inbox `src/ooda_loop/observe.rs` already scans, shaped
//! so Simard's curate step can never promote it into a goal or backlog item
//! (empty `decisions` + empty `action_items`). Simard's reasoners still decide;
//! the whisper is only additional input folded into the next cycle's context.
//!
//! Delivery reuses [`crate::meeting_facilitator::write_meeting_handoff`] — no new
//! parallel channel is introduced. The note is carried in `open_questions` (a
//! non-promoting field), tagged with the [`WHISPER_THEME`] so the OODA ingest
//! ([`crate::ooda_loop::drain_overseer_whispers`]) recognises it and so the
//! Overseer never whispers about its own whisper, and authored under the
//! Overseer's DISTINCT steward identity ([`crate::overseer::config::overseer_author_login`]).

use std::path::PathBuf;

use crate::meeting_facilitator::{MeetingHandoff, OpenQuestion, write_meeting_handoff};
use crate::overseer::capabilities::{ObservedState, OverseerError};
use crate::overseer::signal::{Problem, ProblemKind, Signal};

/// Theme tag stamped on every whisper handoff. The OODA whisper-drain scans for
/// it, and Observe/`signals_from` ignore handoffs carrying it so the Overseer
/// never re-triggers on (whispers about) its own whisper.
pub const WHISPER_THEME: &str = "overseer-whisper";

/// How urgently a whisper should be treated. `High` feeds the escalation
/// decision (a repeated/urgent condition escalates to a full meeting instead of
/// a lightweight whisper); `Normal` is the default steering note.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WhisperUrgency {
    Low,
    #[default]
    Normal,
    High,
}

impl WhisperUrgency {
    /// Stable lower-case label for tracing/notification fields.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

/// A single advisory whisper, as delivered to a [`WhisperSink`]. Carries the
/// note plus enough metadata for transparent tracing and dedup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhisperRecord {
    /// The advisory steering note folded into Simard's next-cycle context.
    pub note: String,
    pub urgency: WhisperUrgency,
    /// The observed problem that triggered the whisper.
    pub problem: ProblemKind,
    /// The goal being steered, if known.
    pub goal_id: Option<String>,
    /// The Overseer's DISTINCT steward login (never the operator's).
    pub author: String,
    /// Stable dedup signature (see [`whisper_signature`]).
    pub signature: String,
}

/// The delivery seam for a whisper. Production writes a `handoff-*.json` onto the
/// shared meeting-handoff inbox; tests inject a fake that records/fails/panics.
pub trait WhisperSink: Send + Sync {
    /// Deliver one whisper, returning the path it was written to. A failure is
    /// surfaced as an [`OverseerError`] (counted by the isolated tick, never
    /// fatal); a panic is caught by the panic-isolated tick.
    fn deliver(&self, rec: &WhisperRecord) -> Result<PathBuf, OverseerError>;
}

/// Production [`WhisperSink`]: writes the whisper as an advisory `MeetingHandoff`
/// onto the SAME `meeting_handoffs` directory `observe.rs` scans, via the reused
/// [`write_meeting_handoff`]. The handoff has empty `decisions`/`action_items`
/// (so curate can never promote it to a goal), the note in `open_questions`, the
/// [`WHISPER_THEME`] tag, and the Overseer author in `participants`.
pub struct MeetingHandoffWhisperSink {
    dir: PathBuf,
}

impl MeetingHandoffWhisperSink {
    /// Deliver whispers into `dir` (the `<state_root>/meeting_handoffs` inbox).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

impl WhisperSink for MeetingHandoffWhisperSink {
    fn deliver(&self, rec: &WhisperRecord) -> Result<PathBuf, OverseerError> {
        let handoff = whisper_handoff(rec);
        write_meeting_handoff(&self.dir, &handoff).map_err(|e| OverseerError::Capability {
            what: "whisper.deliver",
            detail: format!("writing whisper handoff: {e}"),
        })?;
        // `write_meeting_handoff` derives its filename deterministically from
        // `closed_at`; mirror that derivation so we can return the exact path.
        let ts = handoff.closed_at.replace(':', "-").replace('+', "_");
        Ok(self.dir.join(format!("handoff-{ts}.json")))
    }
}

/// Build the advisory handoff that carries a whisper. Empty `decisions` and
/// `action_items` guarantee `check_meeting_handoffs` fast-marks it processed
/// without ever creating a goal or backlog item — the note is pure context.
fn whisper_handoff(rec: &WhisperRecord) -> MeetingHandoff {
    let now = chrono::Utc::now().to_rfc3339();
    MeetingHandoff {
        schema_version: 2,
        meeting_id: String::new(),
        topic: format!("overseer whisper ({})", rec.problem_label()),
        started_at: now.clone(),
        closed_at: now,
        // Advisory-only: no decisions, no action items ⇒ never promoted.
        decisions: Vec::new(),
        action_items: Vec::new(),
        // The steering note rides in a non-promoting field.
        open_questions: vec![OpenQuestion {
            text: rec.note.clone(),
            explicit: true,
        }],
        // Delivered UNPROCESSED so the OODA inbox scan folds it into context.
        processed: false,
        duration_secs: None,
        transcript: Vec::new(),
        transcript_path: None,
        // Authored under the Overseer's DISTINCT steward identity.
        participants: vec![rec.author.clone()],
        // Tagged for recognition + self-whisper skip.
        themes: vec![WHISPER_THEME.to_string()],
        next_owner: Some("ooda-observe".to_string()),
        artifacts: Vec::new(),
        goal: rec.goal_id.clone(),
        next_actor: None,
        applied_templates: Vec::new(),
        history_truncated_count: 0,
        partial_reason: None,
        risks: Vec::new(),
        disagreements: Vec::new(),
    }
}

impl WhisperRecord {
    fn problem_label(&self) -> &'static str {
        match self.problem {
            ProblemKind::LoopDetected => "loop_detected",
            ProblemKind::DriftCorrection => "drift_correction",
            _ => "steering",
        }
    }
}

/// A deterministic, pure steering note for a problem. References the goal being
/// steered so the note is actionable in Simard's context. Never performs I/O.
pub fn compose_whisper_note(problem: &Problem, state: &ObservedState) -> String {
    let goal = whisper_goal_id(problem)
        .or_else(|| state.active_goal_id.clone())
        .unwrap_or_else(|| "the active goal".to_string());
    match problem.kind {
        ProblemKind::LoopDetected => format!(
            "Overseer steering note for goal {goal}: this goal appears to be looping \
             without progress ({}). Re-read its stated intent, then take the single \
             smallest next step; if it is already satisfied, close it.",
            problem.summary
        ),
        ProblemKind::DriftCorrection => format!(
            "Overseer steering note for goal {goal}: work appears to be drifting from \
             the goal's intent ({}). Refocus on the stated outcome before broadening \
             scope.",
            problem.summary
        ),
        _ => format!(
            "Overseer steering note for goal {goal}: {}",
            problem.summary
        ),
    }
}

/// Pull the goal id out of a whisper-triggering problem's evidence.
fn whisper_goal_id(problem: &Problem) -> Option<String> {
    problem.evidence.iter().find_map(|s| match s {
        Signal::LoopDetected { goal_id, .. } | Signal::DriftCorrection { goal_id, .. } => {
            Some(goal_id.clone())
        }
        _ => None,
    })
}

/// Stable dedup signature for a whisper. Case- and whitespace-insensitive on the
/// note (so trivially-different phrasings collapse to one signature) and
/// discriminated by problem kind + goal so a loop and a drift on the same goal,
/// or the same problem on two goals, are distinct whispers.
pub fn whisper_signature(kind: ProblemKind, goal_id: Option<&str>, note: &str) -> String {
    format!(
        "{:?}|{}|{}",
        kind,
        goal_id.unwrap_or("-"),
        normalize_note(note)
    )
}

/// Note-only dedup signature used by [`crate::overseer::Overseer::act`], which
/// receives an `Intervention::Whisper { note, .. }` without the originating
/// problem. Identical notes collapse to one signature so the same whisper is not
/// re-injected every cycle.
pub fn note_signature(note: &str) -> String {
    normalize_note(note)
}

/// Collapse whitespace runs and lower-case so trivially-different notes dedup.
fn normalize_note(note: &str) -> String {
    note.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
