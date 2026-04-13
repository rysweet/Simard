//! Default developer watch list and seeding helpers.

use crate::cognitive_memory::CognitiveMemoryOps;

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
pub fn seed_developer_watches(bridge: &dyn CognitiveMemoryOps) -> usize {
    let mut seeded = 0;
    for watch in default_developer_watches() {
        if track_developer(watch, bridge).is_ok() {
            seeded += 1;
        }
    }
    seeded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_developer_watches_count() {
        let watches = default_developer_watches();
        assert_eq!(watches.len(), DEFAULT_DEVELOPER_WATCHES.len());
    }

    #[test]
    fn default_developer_watches_have_nonempty_fields() {
        for watch in default_developer_watches() {
            assert!(!watch.github_id.is_empty());
            assert!(!watch.focus_areas.is_empty());
            assert!(watch.last_checked.is_none());
        }
    }

    #[test]
    fn default_developer_watches_contains_known_ids() {
        let watches = default_developer_watches();
        let ids: Vec<_> = watches.iter().map(|w| w.github_id.as_str()).collect();
        assert!(ids.contains(&"ramparte"));
        assert!(ids.contains(&"simonw"));
    }

    #[test]
    fn default_developer_watches_constant_length() {
        assert_eq!(DEFAULT_DEVELOPER_WATCHES.len(), 5);
    }

    #[test]
    fn default_watches_focus_areas_are_nonempty_strings() {
        for (_, areas) in &DEFAULT_DEVELOPER_WATCHES {
            for area in *areas {
                assert!(!area.is_empty());
            }
        }
    }
}
