//! Two-pass journal generation (issue #2606).
//!
//! Generation is deliberately two-pass so the jargon-free guarantee is
//! structural, not incidental:
//!
//! 1. A [`JournalDrafter`] assembles a professional, third-person engineering
//!    **report** draft **largely from episodic memories**, augmented by the
//!    day's code-change proposals, goals, live-system updates, Overseer
//!    activity, memory growth, prepared-context substance, and notable events.
//! 2. A [`JournalReviewer`] **always** runs over that draft to remove or
//!    explain jargon and rewrite it for a layperson.
//!
//! [`JournalGenerator::generate`] wires the two together (plus an unconditional
//! secret-redaction post-pass) so neither the review nor the redaction can be
//! skipped. Both passes are pluggable: the default drafter is a deterministic
//! report assembler (offline-testable) and the default reviewer is the glossary
//! scrubber, while [`JournalGenerator::for_repo`] prefers a language-model
//! (recipe-runner) drafter and reviewer when available (guideline G3).

use std::fmt::Write as _;
use std::path::Path;

use chrono::Utc;

use crate::journal::jargon::{scrub_jargon, scrub_secrets};
use crate::journal::providers::episode_time_label;
use crate::journal::recipe::{RecipeDrafter, RecipeReviewer};
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

/// The default drafter: a deterministic, template-based **report** assembler.
///
/// It writes a professional, third-person engineering-and-research report — an
/// `## Overview` paragraph followed by clearly delineated `##` sections
/// (engineering work, research and findings, key observations) and a
/// chronological, timestamped list of the day's remembered moments (episodic
/// memories, the primary source). The prepared-context substance (the facts,
/// triggers, and procedures) is summarised in full rather than as a bare count.
/// A quiet day yields an honest "quiet day" report rather than an empty or
/// invented narrative. It is **not** a first-person "Dear diary".
#[derive(Debug, Clone, Copy, Default)]
pub struct TemplateDrafter;

impl JournalDrafter for TemplateDrafter {
    fn draft(&self, day: &DayContext) -> String {
        if day.is_quiet() {
            return quiet_day_report(day);
        }

        let mut blocks: Vec<String> = Vec::new();

        // ── Overview ────────────────────────────────────────────────────
        blocks.push("## Overview".to_string());
        blocks.push(overview_paragraph(day));

        // ── Engineering work ────────────────────────────────────────────
        blocks.push("## Engineering work".to_string());
        if !day.deploys.is_empty() {
            blocks.push(lead_and_bullets(
                "Simard shipped the following updates to the live system:",
                &day.deploys,
            ));
        }
        if !day.goals.is_empty() {
            blocks.push(lead_and_bullets(
                "Work advanced toward these goals:",
                &day.goals,
            ));
        }
        blocks.push(pr_paragraph(day));

        // ── Research and findings ───────────────────────────────────────
        if !day.facts.is_empty() || !day.notable.is_empty() {
            blocks.push("## Research and findings".to_string());
            if !day.facts.is_empty() {
                blocks.push(lead_and_bullets(
                    "Simard recorded these new findings (the facts it learned):",
                    &day.facts,
                ));
            }
            if !day.notable.is_empty() {
                blocks.push(lead_and_bullets(
                    "Other noteworthy observations from the day:",
                    &day.notable,
                ));
            }
        }

        // ── Key observations ────────────────────────────────────────────
        if !day.triggers.is_empty()
            || !day.procedures.is_empty()
            || day.memory_growth.is_some()
            || !day.overseer_events.is_empty()
        {
            blocks.push("## Key observations".to_string());
            if !day.triggers.is_empty() {
                blocks.push(lead_and_bullets(
                    "Reminders that came due during the day:",
                    &day.triggers,
                ));
            }
            if !day.procedures.is_empty() {
                blocks.push(lead_and_bullets(
                    "Know-how (step-by-step procedures) that was applied or refined:",
                    &day.procedures,
                ));
            }
            if let Some(mg) = day.memory_growth {
                blocks.push(format!(
                    "Simard's memory grew by {} new facts and {} new episodes.",
                    mg.facts_added, mg.episodes_added
                ));
            }
            if !day.overseer_events.is_empty() {
                blocks.push(lead_and_bullets(
                    "The steward, the Overseer, was active as well:",
                    &day.overseer_events,
                ));
            }
        }

        // ── Remembered moments (episodic memories, chronological) ───────
        if !day.episodes.is_empty() {
            blocks.push("## Remembered moments".to_string());
            // Oldest-to-newest so the report reads as a timeline, and each moment
            // shows when it occurred (issue #2606).
            let mut moments: Vec<_> = day.episodes.iter().collect();
            moments.sort_by_key(|e| e.temporal_index);
            let mut body = String::from(
                "These are the day's episodic memories, listed in the order they occurred:",
            );
            for ep in moments {
                let _ = write!(
                    body,
                    "\n- [{}] {}",
                    episode_time_label(ep.temporal_index),
                    ep.content.trim()
                );
            }
            blocks.push(body);
        }

        blocks.join("\n\n")
    }
}

/// A professional, third-person one-paragraph summary of the day's activity.
fn overview_paragraph(day: &DayContext) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !day.episodes.is_empty() {
        parts.push(format!(
            "recorded {} remembered {}",
            day.episodes.len(),
            plural(day.episodes.len(), "moment", "moments")
        ));
    }
    if !day.goals.is_empty() {
        parts.push(format!(
            "pursued {} {}",
            day.goals.len(),
            plural(day.goals.len(), "goal", "goals")
        ));
    }
    if !day.deploys.is_empty() {
        parts.push(format!(
            "shipped {} {} to the live system",
            day.deploys.len(),
            plural(day.deploys.len(), "update", "updates")
        ));
    }
    if !day.prs.is_empty() {
        parts.push(format!(
            "reviewed {} code-change {}",
            day.prs.len(),
            plural(day.prs.len(), "proposal", "proposals")
        ));
    }
    if !day.facts.is_empty() {
        parts.push(format!(
            "captured {} new {}",
            day.facts.len(),
            plural(day.facts.len(), "finding", "findings")
        ));
    }
    let activity = if parts.is_empty() {
        "kept watch over the system".to_string()
    } else {
        join_with_and(&parts)
    };
    format!(
        "On {}, Simard — an autonomous software-engineering and research system — and its steward, \
         the Overseer, {}.",
        day.date.format("%Y-%m-%d"),
        activity
    )
}

/// The code-change-proposal lead-in for the Engineering work section (or an
/// honest "none opened" note). Always plain-language and never a bare acronym.
fn pr_paragraph(day: &DayContext) -> String {
    if day.prs.is_empty() {
        return "No code-change proposals (pull requests) were opened during the day.".to_string();
    }
    let merged = day
        .prs
        .iter()
        .filter(|p| p.outcome.eq_ignore_ascii_case("merged"))
        .count();
    format!(
        "The day's code-change proposals (pull requests) are summarised in the table below: \
         {} in total, of which {} {} combined into the main code.",
        day.prs.len(),
        merged,
        if merged == 1 { "was" } else { "were" }
    )
}

/// An honest, report-style narrative for a day on which nothing notable
/// happened. Third-person and free of bullet points (there is nothing to list).
fn quiet_day_report(day: &DayContext) -> String {
    format!(
        "## Overview\n\nOn {}, Simard and its steward, the Overseer, kept watch over a quiet day. \
         No goals advanced, no code-change proposals were opened, and nothing notable occurred \
         — a calm, quiet day.",
        day.date.format("%Y-%m-%d")
    )
}

/// Build a "lead-in line + bullet list" block from `items`.
fn lead_and_bullets(lead: &str, items: &[String]) -> String {
    let mut b = String::from(lead);
    for item in items {
        let _ = write!(b, "\n- {}", item.trim());
    }
    b
}

/// `singular` when `n == 1`, else `plural_form`.
fn plural<'a>(n: usize, singular: &'a str, plural_form: &'a str) -> &'a str {
    if n == 1 { singular } else { plural_form }
}

/// Join `parts` into an English list: `"a"`, `"a and b"`, `"a, b, and c"`.
fn join_with_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [only] => only.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {}", rest.join(", "), last),
    }
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
    ///
    /// This is the deterministic, offline path — the honest fallback whenever
    /// the prompt-first recipe path (see [`for_repo`](Self::for_repo)) is
    /// unavailable.
    pub fn default_pipeline() -> Self {
        Self::new(Box::new(TemplateDrafter), Box::new(GlossaryReviewer))
    }

    /// Build the **prompt-first** generator for `repo_root` (issue #2606,
    /// guideline G3: agentic over brittle parsing).
    ///
    /// When the journal recipe assets and the recipe runner are available for a
    /// real repository, both passes are language-model-backed (a report drafter
    /// and a plain-language de-jargon reviewer) — the preferred production path.
    /// Each recipe pass degrades per-call to its deterministic equivalent on any
    /// failure, and the whole constructor falls back to
    /// [`default_pipeline`](Self::default_pipeline) when the assets/runner are
    /// absent (offline, tests, or a non-repo path), so generation is always
    /// available and never blocks.
    pub fn for_repo(repo_root: &Path) -> Self {
        let recipe_pair = if repo_root.is_dir() {
            Option::zip(
                RecipeDrafter::for_repo(repo_root),
                RecipeReviewer::for_repo(repo_root),
            )
        } else {
            None
        };
        match recipe_pair {
            Some((drafter, reviewer)) => Self::new(Box::new(drafter), Box::new(reviewer)),
            None => Self::default_pipeline(),
        }
    }

    /// Generate the reviewed [`JournalEntry`] for `day`.
    ///
    /// Runs the drafter, then **always** runs the reviewer over the draft, then
    /// applies an **unconditional** [`scrub_secrets`] redaction post-pass over
    /// the reviewed text — so a credential never reaches the durable narrative
    /// even if a language-model reviewer failed to strip it. Stores both the raw
    /// draft (for provenance) and the reviewed, secret-free narrative.
    pub fn generate(&self, day: &DayContext) -> JournalEntry {
        let draft = self.drafter.draft(day);
        // Mandatory review pass — the jargon-free guarantee lives here.
        let reviewed = self.reviewer.review(&draft);
        // Unconditional secret-redaction post-pass over the stored narrative.
        let narrative = scrub_secrets(&reviewed);
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
