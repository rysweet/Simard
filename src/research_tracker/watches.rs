//! Default developer watch list and seeding helpers.

use crate::memory_bridge::CognitiveMemoryBridge;

use super::operations::track_developer;
use super::types::DeveloperWatch;

// ---------------------------------------------------------------------------
// Default developer watch list
// ---------------------------------------------------------------------------

/// Developers whose public activity Simard tracks by default.
/// Each tuple: (github_id, focus_areas).
pub const DEFAULT_DEVELOPER_WATCHES: [(&str, &[&str]); 5] = [
    (
        "ramparte",
        &["agentic-coding", "agent-frameworks", "developer-tools"],
    ),
    (
        "simonw",
        &["llm-tooling", "sqlite", "datasette", "prompt-engineering"],
    ),
    (
        "steveyegge",
        &["ai-coding", "developer-experience", "platform-engineering"],
    ),
    (
        "bkrabach",
        &["multi-agent-systems", "azure-ai", "agent-orchestration"],
    ),
    (
        "robotdad",
        &[
            "rust-tooling",
            "systems-programming",
            "developer-productivity",
        ],
    ),
];

/// Build the default developer watch list from the compile-time constant.
pub fn default_developer_watches() -> Vec<DeveloperWatch> {
    DEFAULT_DEVELOPER_WATCHES
        .iter()
        .map(|(github_id, areas)| DeveloperWatch {
            github_id: (*github_id).to_string(),
            focus_areas: areas.iter().map(|a| (*a).to_string()).collect(),
            last_checked: None,
        })
        .collect()
}

/// Seed the default developer watches into cognitive memory if not already
/// tracked. Returns the number of watches stored.
pub fn seed_developer_watches(bridge: &CognitiveMemoryBridge) -> usize {
    let mut seeded = 0;
    for watch in default_developer_watches() {
        if track_developer(watch, bridge).is_ok() {
            seeded += 1;
        }
    }
    seeded
}
