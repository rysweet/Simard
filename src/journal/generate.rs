//! Two-pass journal generation (issue #2606).
//!
//! Generation is deliberately two-pass so the jargon-free guarantee is
//! structural, not incidental:
//!
//! 1. A [`JournalDrafter`] assembles a first-person-steward draft **largely
//!    from episodic memories**, augmented by the day's code-change proposals,
//!    goals, live-system updates, Overseer activity, memory growth, and notable
//!    events.
//! 2. A [`JournalReviewer`] **always** runs over that draft to remove or
//!    explain jargon and rewrite it for a layperson.
//!
//! [`JournalGenerator::generate`] wires the two together so the review pass can
//! never be skipped. Both passes are pluggable: the default drafter is
//! deterministic (offline-testable) and the default reviewer is the
//! glossary scrubber, but an LLM reasoner can be swapped in behind either trait.

use std::fmt::Write as _;

use chrono::Utc;

use crate::journal::jargon::scrub_jargon;
use crate::journal::types::{DayContext, JournalEntry};

/// First pass: assemble a raw narrative draft from a [`DayContext`].
///
/// Implementations may still use jargon freely — the mandatory
/// [`JournalReviewer`] pass is responsible for scrubbing it. The draft must be
/// grounded in the context (episodics first) and must never fabricate events
/// that are not present in the context.
pub trait JournalDrafter: Send + Sync {
    /// Produce the raw (pre-review) narrative for `day`.
    fn draft(&self, day: &DayContext) -> String;
}

/// Second pass: rewrite a draft into final, layperson-readable, jargon-free
/// prose. This pass is mandatory and always runs (see [`JournalGenerator`]).
pub trait JournalReviewer: Send + Sync {
    /// Rewrite `draft` for a layperson, removing or explaining jargon.
    fn review(&self, draft: &str) -> String;
}

/// The default reviewer: the deterministic glossary
/// [`scrub_jargon`](crate::journal::jargon::scrub_jargon) pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct GlossaryReviewer;

impl JournalReviewer for GlossaryReviewer {
    fn review(&self, draft: &str) -> String {
        scrub_jargon(draft)
    }
}

/// The default drafter: a deterministic, template-based assembler.
///
/// It leads with the day's episodic memories (the primary source), then folds
/// in goals, live-system updates, Overseer activity, memory growth, notable
/// events, and a one-line lead-in to the code-change-proposal table. A quiet
/// day yields an honest "quiet day" paragraph rather than an empty or invented
/// narrative.
#[derive(Debug, Clone, Copy, Default)]
pub struct TemplateDrafter;

impl JournalDrafter for TemplateDrafter {
    fn draft(&self, day: &DayContext) -> String {
        if day.is_quiet() {
            return quiet_day_draft(day);
        }

        let mut s = String::with_capacity(512);
        let _ = writeln!(
            s,
            "Dear diary — here is what I, Simard, got up to on {}.",
            day.date.format("%Y-%m-%d")
        );
        s.push('\n');

        if !day.episodes.is_empty() {
            s.push_str("Through the day these moments stayed with me (my episodic memories):\n");
            for ep in &day.episodes {
                let _ = writeln!(s, "- {}", ep.content.trim());
            }
            s.push('\n');
        }

        if !day.goals.is_empty() {
            s.push_str("I kept working toward these goals:\n");
            for g in &day.goals {
                let _ = writeln!(s, "- {}", g.trim());
            }
            s.push('\n');
        }

        if !day.deploys.is_empty() {
            s.push_str("I deployed these updates to the live system:\n");
            for d in &day.deploys {
                let _ = writeln!(s, "- {}", d.trim());
            }
            s.push('\n');
        }

        if !day.overseer_events.is_empty() {
            s.push_str("My steward, the Overseer, was busy too:\n");
            for e in &day.overseer_events {
                let _ = writeln!(s, "- {}", e.trim());
            }
            s.push('\n');
        }

        if let Some(mg) = day.memory_growth {
            let _ = writeln!(
                s,
                "My memory grew by {} new facts and {} new episodes.",
                mg.facts_added, mg.episodes_added
            );
            s.push('\n');
        }

        if !day.notable.is_empty() {
            s.push_str("A few other things stood out:\n");
            for note in &day.notable {
                let _ = writeln!(s, "- {}", note.trim());
            }
            s.push('\n');
        }

        if day.prs.is_empty() {
            s.push_str("No PRs were opened today.\n");
        } else {
            let merged = day
                .prs
                .iter()
                .filter(|p| p.outcome.eq_ignore_ascii_case("merged"))
                .count();
            let _ = writeln!(
                s,
                "Engineers opened {} PRs today; {} were merged. The table below explains each one in plain language.",
                day.prs.len(),
                merged
            );
        }

        s
    }
}

/// Honest narrative for a day on which nothing notable happened.
fn quiet_day_draft(day: &DayContext) -> String {
    format!(
        "Dear diary — {} was a quiet day. I, Simard, kept watch alongside my steward, \
         the Overseer, but there was little of note: no goals advanced, no PRs were \
         opened, and nothing remarkable happened. A calm, quiet day.",
        day.date.format("%Y-%m-%d")
    )
}

/// The two-pass generator: draft then mandatory review.
///
/// Holds boxed [`JournalDrafter`] / [`JournalReviewer`] so the drafter and
/// reviewer can be swapped independently (deterministic template + glossary
/// scrub by default; an LLM reasoner in production if available). The review
/// pass is invoked on every [`generate`](Self::generate) call — there is no
/// code path that returns an unreviewed narrative.
pub struct JournalGenerator {
    drafter: Box<dyn JournalDrafter>,
    reviewer: Box<dyn JournalReviewer>,
}

impl JournalGenerator {
    /// Build a generator from an explicit drafter and reviewer.
    pub fn new(drafter: Box<dyn JournalDrafter>, reviewer: Box<dyn JournalReviewer>) -> Self {
        Self { drafter, reviewer }
    }

    /// The default pipeline: [`TemplateDrafter`] + [`GlossaryReviewer`].
    pub fn default_pipeline() -> Self {
        Self::new(Box::new(TemplateDrafter), Box::new(GlossaryReviewer))
    }

    /// Generate the reviewed [`JournalEntry`] for `day`.
    ///
    /// Runs the drafter, then **always** runs the reviewer over the draft, and
    /// stores both the draft (for provenance) and the reviewed narrative.
    pub fn generate(&self, day: &DayContext) -> JournalEntry {
        let draft = self.drafter.draft(day);
        // Mandatory review pass — the jargon-free guarantee lives here.
        let narrative = self.reviewer.review(&draft);
        JournalEntry {
            date: day.date,
            generated_at: Utc::now(),
            narrative,
            draft,
            prs: day.prs.clone(),
            quiet_day: day.is_quiet(),
        }
    }
}
