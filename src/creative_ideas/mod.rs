//! Creative Ideas subsystem (issue #2647) — an idea-generation subsystem
//! that primes a pool of candidate self-improvement ideas inside the single
//! Simard brain.
//!
//! **Status: live, default-ON opt-out.** This module owns the reviewer-pipeline
//! trait and adapters ([`reviewers`]), the synthesis step ([`synthesis`]), the
//! routing functions ([`routing`]), the dedup/portfolio/budget helpers
//! ([`dedup`]), and the config flag ([`CreativeIdeasConfig`]). The `CreativeIdea`
//! prospective type lives in [`crate::cognitive_memory::creative_idea`] and the
//! generator thread in [`crate::cognitive_threads::threads::creative_ideas`],
//! which the OODA daemon registers on startup — default-ON, opt-out via
//! `SIMARD_CREATIVE_IDEAS_ENABLED`, consistent with the Overseer/Journal threads
//! and independent of the generic `SIMARD_COGNITIVE_THREADS_ENABLED` switch.
//!
//! No type or module is a transport shim — this is one brain with
//! cognitive threads and reviewers, not a separate service.
#![allow(dead_code)]

pub mod dedup;
pub mod dedup_gate;
pub mod pipeline;
pub mod prompt;
pub mod reviewers;
pub mod routing;
pub mod source;
pub mod synthesis;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_visibility_2896;

/// Master switch env var. When falsey/unset the thread never ticks and nothing
/// is generated or routed.
pub const ENABLED_ENV: &str = "SIMARD_CREATIVE_IDEAS_ENABLED";
/// Generator cadence env var (seconds); a large (>= 24h) observation window.
pub const INTERVAL_SECS_ENV: &str = "SIMARD_CREATIVE_IDEAS_INTERVAL_SECS";
/// Ideas targeted per run env var.
pub const BATCH_ENV: &str = "SIMARD_CREATIVE_IDEAS_BATCH";

/// Default cadence: 24 hours.
pub const DEFAULT_INTERVAL_SECS: u64 = 86_400;
/// Default batch: ten ideas per run (the design's fixed batch).
pub const DEFAULT_BATCH: usize = 10;

/// Issue label applied to a human-review issue minted from an idea.
pub const CREATIVE_IDEA_ISSUE_LABEL: &str = "creative-idea";
/// Merge-blocking PR label for a creative-idea PR (human-review gate).
pub const CREATIVE_IDEA_PR_LABEL: &str = "creative-idea-needs-human-review";
/// Repo owner tagged as issue assignee / PR reviewer for the human gate.
pub const CREATIVE_IDEA_OWNER: &str = "rysweet";

/// Configuration + gating for the Creative Ideas subsystem.
///
/// A default-constructed config is **enabled** (default-ON, opt-out) —
/// consistent with the Overseer and Journal cognitive threads.
/// [`Self::from_env`] is the single source of truth for gating and cadence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreativeIdeasConfig {
    /// `SIMARD_CREATIVE_IDEAS_ENABLED` (default `true`; opt-out via a falsey value).
    pub enabled: bool,
    /// `SIMARD_CREATIVE_IDEAS_INTERVAL_SECS` (default `86_400`).
    pub interval_secs: u64,
    /// `SIMARD_CREATIVE_IDEAS_BATCH` (default `10`).
    pub batch: usize,
}

impl Default for CreativeIdeasConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: DEFAULT_INTERVAL_SECS,
            batch: DEFAULT_BATCH,
        }
    }
}

impl CreativeIdeasConfig {
    /// Parse the config from the real process environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Parse from an arbitrary env resolver (test seam).
    ///
    /// The gate mirrors the Overseer/Journal pattern: the subsystem is
    /// **default-ON** and only an explicit *falsey* value opts out; unset/empty
    /// leaves it enabled.
    #[must_use]
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let enabled = !lookup(ENABLED_ENV)
            .as_deref()
            .map(str::trim)
            .is_some_and(is_falsey);
        let interval_secs = lookup(INTERVAL_SECS_ENV)
            .as_deref()
            .map(str::trim)
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_INTERVAL_SECS);
        let batch = lookup(BATCH_ENV)
            .as_deref()
            .map(str::trim)
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_BATCH);
        Self {
            enabled,
            interval_secs,
            batch,
        }
    }

    /// True unless the master switch was explicitly set to a falsey value.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// Recognise an explicit falsey env value (case-insensitive; trimmed). Anything
/// else — including unset/empty and truthy values — leaves the subsystem ON.
fn is_falsey(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}
