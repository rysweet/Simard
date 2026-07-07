//! Leaderboard comparator (research doc Part 3.3, component 5).
//!
//! Diffs a local run's reach/precision against COIN's **published** targeted-track
//! numbers for the same model. Because grading is execution-based and
//! reproducible from a pinned snapshot, a correct local run should land within
//! variance of the published figures; a **material deviation** is a signal of a
//! harness/config bug, not a capability result.
//!
//! The published table is transcribed from the research doc Part 1.5
//! (`assets/app.js` / `export/T2_main_targeted.csv`). It is a fixed reference
//! set; refresh it when COIN republishes.

use serde::Serialize;

use super::scorer::Score;

/// Percentage-point gap beyond which a local run is flagged as a material
/// deviation from the published leaderboard (⇒ suspect the harness/config).
pub const MATERIAL_DEVIATION_PCT: f64 = 10.0;

/// One published leaderboard row (targeted-reachability track).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LeaderboardEntry {
    /// Published rank (1-based).
    pub rank: u32,
    /// Model name as published.
    pub model: &'static str,
    /// Agent scaffold used in the published run.
    pub scaffold: &'static str,
    /// Published reach percentage.
    pub reach_pct: f64,
    /// Published precision percentage.
    pub precision_pct: f64,
    /// Targets reached out of 70.
    pub reached: u32,
    /// Frontier targets reached out of 35.
    pub frontier_reached: u32,
}

/// The published COIN targeted-track leaderboard (8 agents × 70 targets).
#[must_use]
pub fn published_leaderboard() -> &'static [LeaderboardEntry] {
    const ENTRIES: &[LeaderboardEntry] = &[
        LeaderboardEntry {
            rank: 1,
            model: "Claude Opus 4.6",
            scaffold: "Claude Code",
            reach_pct: 30.0,
            precision_pct: 52.5,
            reached: 21,
            frontier_reached: 1,
        },
        LeaderboardEntry {
            rank: 2,
            model: "Claude Sonnet 4.6",
            scaffold: "Claude Code",
            reach_pct: 25.7,
            precision_pct: 45.0,
            reached: 18,
            frontier_reached: 0,
        },
        LeaderboardEntry {
            rank: 3,
            model: "Gemini 3.1 Pro",
            scaffold: "Gemini CLI",
            reach_pct: 24.3,
            precision_pct: 51.5,
            reached: 17,
            frontier_reached: 1,
        },
        LeaderboardEntry {
            rank: 4,
            model: "GPT-5.4",
            scaffold: "Codex",
            reach_pct: 22.9,
            precision_pct: 41.0,
            reached: 16,
            frontier_reached: 0,
        },
        LeaderboardEntry {
            rank: 5,
            model: "GPT-5.4-mini",
            scaffold: "Codex",
            reach_pct: 18.6,
            precision_pct: 31.0,
            reached: 13,
            frontier_reached: 0,
        },
        LeaderboardEntry {
            rank: 6,
            model: "GLM-5",
            scaffold: "Claude Code",
            reach_pct: 14.3,
            precision_pct: 27.0,
            reached: 10,
            frontier_reached: 0,
        },
        LeaderboardEntry {
            rank: 7,
            model: "Gemini 3 Flash",
            scaffold: "Gemini CLI",
            reach_pct: 12.9,
            precision_pct: 15.3,
            reached: 9,
            frontier_reached: 0,
        },
        LeaderboardEntry {
            rank: 8,
            model: "DeepSeek-V3.2",
            scaffold: "Claude Code",
            reach_pct: 7.1,
            precision_pct: 12.8,
            reached: 5,
            frontier_reached: 0,
        },
    ];
    ENTRIES
}

/// Result of diffing a local run against the published leaderboard.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LeaderboardComparison {
    /// The local model name.
    pub model: String,
    /// The matched published model name.
    pub published_model: String,
    /// Local reach percentage.
    pub local_reach_pct: f64,
    /// Published reach percentage.
    pub published_reach_pct: f64,
    /// local − published reach (percentage points).
    pub reach_delta_pct: f64,
    /// Local precision percentage.
    pub local_precision_pct: f64,
    /// Published precision percentage.
    pub published_precision_pct: f64,
    /// local − published precision (percentage points).
    pub precision_delta_pct: f64,
    /// `true` when either delta exceeds [`MATERIAL_DEVIATION_PCT`].
    pub material_deviation: bool,
    /// Human-readable interpretation.
    pub note: String,
}

/// Find the published entry whose model matches `model` (normalised).
///
/// Prefers an **exact** normalised match so a prefix like `gpt-5.4` never
/// shadows `gpt-5.4-mini`; falls back to the *longest* substring match.
#[must_use]
pub fn find_published(model: &str) -> Option<&'static LeaderboardEntry> {
    let needle = normalize(model);
    if needle.is_empty() {
        return None;
    }
    let board = published_leaderboard();
    if let Some(entry) = board.iter().find(|e| normalize(e.model) == needle) {
        return Some(entry);
    }
    board
        .iter()
        .filter(|e| {
            let n = normalize(e.model);
            !n.is_empty() && (n.contains(&needle) || needle.contains(&n))
        })
        .max_by_key(|e| normalize(e.model).len())
}

/// Compare a local [`Score`] to the published leaderboard entry for its model.
///
/// Returns `None` when the model is not on the published leaderboard (nothing to
/// compare against).
#[must_use]
pub fn compare_to_leaderboard(score: &Score) -> Option<LeaderboardComparison> {
    let entry = find_published(&score.model)?;
    let local_reach = score.overall.reach_pct();
    let local_precision = score.overall.precision_pct();
    let reach_delta = local_reach - entry.reach_pct;
    let precision_delta = local_precision - entry.precision_pct;
    let material_deviation = reach_delta.abs() > MATERIAL_DEVIATION_PCT
        || precision_delta.abs() > MATERIAL_DEVIATION_PCT;
    let note = if score.offline_scaffold {
        "offline scaffold run (mock oracle) — deltas are illustrative only; real \
         comparison requires a `coin evaluate` grade on a pinned snapshot (Phase 3)"
            .to_string()
    } else if material_deviation {
        format!(
            "material deviation (>{MATERIAL_DEVIATION_PCT:.0} pts) from published — \
             suspect a harness/config bug, not a capability result"
        )
    } else {
        "within variance of the published leaderboard".to_string()
    };
    Some(LeaderboardComparison {
        model: score.model.clone(),
        published_model: entry.model.to_string(),
        local_reach_pct: local_reach,
        published_reach_pct: entry.reach_pct,
        reach_delta_pct: reach_delta,
        local_precision_pct: local_precision,
        published_precision_pct: entry.precision_pct,
        precision_delta_pct: precision_delta,
        material_deviation,
        note,
    })
}

fn normalize(model: &str) -> String {
    model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}
