//! Typed registry of synthetic OODA priority kinds.
//!
//! ## Motivation
//!
//! Before this module landed, the OODA loop encoded its synthetic (non-goal)
//! priorities as magic `goal_id` strings — `__memory__`, `__improvement__`,
//! `__poll_activity__`, `__extract_ideas__`, `__eval_watchdog__` — sprinkled
//! across ~30 sites in 6 files. The strings were the implicit "type" of a
//! priority: `priority.goal_id.starts_with("__")` was the test for
//! "is this a synthetic kind?" and a 4-arm string `match` in the
//! [`DeterministicFallbackDecideBrain`] was the routing table.
//!
//! Adding a new synthetic priority (e.g. issue #1868's `MergePr`) meant
//! touching every site, hoping nobody mistyped the underscore count, and
//! praying no real goal id ever started with `__`. The fix:
//!
//! - One enum, one source of truth.
//! - Every magic string in the code base is constructed from the enum
//!   via [`SyntheticPriorityKind::synthetic_id`].
//! - Every place that asked "is this synthetic?" or "what kind?" now goes
//!   through [`SyntheticPriorityKind::from_synthetic_id`].
//!
//! ## Why "synthetic"
//!
//! Most priorities map 1:1 to a real goal in the goal board — those carry
//! the goal's slug as their `goal_id`. **Synthetic** priorities are the
//! cross-cutting ones the Orient phase synthesizes from observation state
//! (memory pressure, gym health, watchdog tripped, etc.) that do not
//! correspond to any persisted goal. This enum enumerates exactly those.
//!
//! The serialization format is unchanged: priorities still serialize with
//! `goal_id: "__memory__"` etc. so the dashboard, cycle reports, and
//! persisted history keep working bit-for-bit. The enum is the **in-code**
//! type; the string is the **on-wire** projection.

use std::fmt;

/// One of the synthetic (non-goal) priority kinds the OODA loop knows about.
///
/// A real goal-based priority is *not* represented here — that's just a
/// `Priority { goal_id: "<slug>", … }` whose slug is a real goal id. Use
/// [`SyntheticPriorityKind::from_synthetic_id`] to detect synthetic kinds
/// in a typed way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SyntheticPriorityKind {
    /// Cross-session episodic memory consolidation. Synthesized when the
    /// episodic store has grown past the consolidation threshold.
    ConsolidateMemory,
    /// Gym-driven self-improvement cycle. Synthesized when gym overall
    /// score is below the target threshold with at least one scenario.
    RunImprovement,
    /// Poll developer activity / ingest signals. Periodic synthetic cycle.
    PollDeveloperActivity,
    /// Mine recent activity for new research ideas. Periodic synthetic cycle.
    ExtractIdeas,
    /// The eval watchdog tripped in the Observe phase. Highest-urgency
    /// synthetic — preempts ordinary work until an operator investigates.
    EvalWatchdog,
    /// Brain-orchestrated safe self-update. Synthesized when the running
    /// binary is behind `origin/main` by at least `min_commits_since_build`
    /// commits and the four-part triggering doctrine is satisfied.
    SafeUpdate,
}

impl SyntheticPriorityKind {
    /// The on-wire goal_id string used to serialize this synthetic kind.
    ///
    /// **Do not** spell these literals out anywhere else in the code base —
    /// always go through this method. That is the whole point of this enum.
    pub const fn synthetic_id(self) -> &'static str {
        match self {
            Self::ConsolidateMemory => "__memory__",
            Self::RunImprovement => "__improvement__",
            Self::PollDeveloperActivity => "__poll_activity__",
            Self::ExtractIdeas => "__extract_ideas__",
            Self::EvalWatchdog => "__eval_watchdog__",
            Self::SafeUpdate => "__safe_update__",
        }
    }

    /// Inverse of [`synthetic_id`]. Returns `None` for any string that is
    /// not a known synthetic id — including real goal slugs and unknown
    /// `__foo__` strings (an unknown synthetic must be a typo, never a
    /// silent fallback).
    pub fn from_synthetic_id(id: &str) -> Option<Self> {
        Some(match id {
            "__memory__" => Self::ConsolidateMemory,
            "__improvement__" => Self::RunImprovement,
            "__poll_activity__" => Self::PollDeveloperActivity,
            "__extract_ideas__" => Self::ExtractIdeas,
            "__eval_watchdog__" => Self::EvalWatchdog,
            "__safe_update__" => Self::SafeUpdate,
            _ => return None,
        })
    }

    /// Iterate over every known synthetic kind. Used by tests and audits to
    /// guarantee that adding a variant here will surface in coverage that
    /// enumerates all kinds.
    pub fn all() -> &'static [Self] {
        &[
            Self::ConsolidateMemory,
            Self::RunImprovement,
            Self::PollDeveloperActivity,
            Self::ExtractIdeas,
            Self::EvalWatchdog,
            Self::SafeUpdate,
        ]
    }
}

impl fmt::Display for SyntheticPriorityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.synthetic_id())
    }
}

/// Convenience: is this `goal_id` string a recognized synthetic priority?
///
/// Replaces the previous `goal_id.starts_with("__")` heuristic, which had
/// two problems: (a) it accepted any unknown `__foo__` as synthetic, and
/// (b) it made the test for "synthetic" implicit in lexical shape rather
/// than in enumeration membership.
pub fn is_synthetic_id(goal_id: &str) -> bool {
    SyntheticPriorityKind::from_synthetic_id(goal_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip every variant. Catches the next person who adds a variant
    /// to `all()` but forgets to map it in `synthetic_id` /
    /// `from_synthetic_id`.
    #[test]
    fn synthetic_id_roundtrips_for_every_variant() {
        for kind in SyntheticPriorityKind::all() {
            let id = kind.synthetic_id();
            assert_eq!(
                SyntheticPriorityKind::from_synthetic_id(id),
                Some(*kind),
                "round-trip failed for {kind:?}"
            );
        }
    }

    #[test]
    fn synthetic_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for kind in SyntheticPriorityKind::all() {
            assert!(
                seen.insert(kind.synthetic_id()),
                "duplicate synthetic id for {kind:?}: {}",
                kind.synthetic_id()
            );
        }
    }

    #[test]
    fn is_synthetic_id_rejects_real_goal_slugs() {
        assert!(!is_synthetic_id("ship-v1"));
        assert!(!is_synthetic_id("improve-memory-persistence"));
        assert!(!is_synthetic_id(""));
    }

    #[test]
    fn is_synthetic_id_rejects_unknown_double_underscore_strings() {
        // The old `starts_with("__")` heuristic would accept this. We
        // explicitly do not — an unknown `__foo__` is a typo, not a kind.
        assert!(!is_synthetic_id("__unknown__"));
        assert!(!is_synthetic_id("__memory"));
        assert!(!is_synthetic_id("memory__"));
        assert!(!is_synthetic_id("__"));
    }

    #[test]
    fn is_synthetic_id_accepts_every_known_kind() {
        for kind in SyntheticPriorityKind::all() {
            assert!(
                is_synthetic_id(kind.synthetic_id()),
                "is_synthetic_id failed for {kind:?}"
            );
        }
    }

    #[test]
    fn display_matches_synthetic_id() {
        for kind in SyntheticPriorityKind::all() {
            assert_eq!(format!("{kind}"), kind.synthetic_id());
        }
    }

    /// Pin the on-wire string format so a future "rename for clarity" PR
    /// can't silently break the dashboard / persisted cycle reports. If
    /// you really want to rename one of these, do it intentionally and
    /// add a migration; do not let the test catch you by surprise.
    #[test]
    fn synthetic_ids_pinned_for_serialization_compat() {
        assert_eq!(
            SyntheticPriorityKind::ConsolidateMemory.synthetic_id(),
            "__memory__"
        );
        assert_eq!(
            SyntheticPriorityKind::RunImprovement.synthetic_id(),
            "__improvement__"
        );
        assert_eq!(
            SyntheticPriorityKind::PollDeveloperActivity.synthetic_id(),
            "__poll_activity__"
        );
        assert_eq!(
            SyntheticPriorityKind::ExtractIdeas.synthetic_id(),
            "__extract_ideas__"
        );
        assert_eq!(
            SyntheticPriorityKind::EvalWatchdog.synthetic_id(),
            "__eval_watchdog__"
        );
        assert_eq!(
            SyntheticPriorityKind::SafeUpdate.synthetic_id(),
            "__safe_update__"
        );
    }
}
