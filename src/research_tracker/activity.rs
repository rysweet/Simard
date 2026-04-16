//! Poll GitHub for tracked developer activity and surface research topics.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};

use super::types::{ResearchStatus, ResearchTopic, ResearchTracker};

// ---------------------------------------------------------------------------
// GitHub event types
// ---------------------------------------------------------------------------

/// Minimal representation of a GitHub public event.
#[derive(Clone, Debug, Deserialize)]
pub struct GitHubEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub repo: GitHubRepo,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GitHubRepo {
    pub name: String,
}

// ---------------------------------------------------------------------------
// Event fetcher trait (for testability)
// ---------------------------------------------------------------------------

/// Abstraction over GitHub event fetching so tests can inject a mock.
pub trait EventFetcher: Send + Sync {
    fn fetch_events(&self, github_id: &str) -> SimardResult<Vec<GitHubEvent>>;
}

/// Default fetcher that shells out to `gh api`.
pub struct GhCliFetcher;

impl EventFetcher for GhCliFetcher {
    fn fetch_events(&self, github_id: &str) -> SimardResult<Vec<GitHubEvent>> {
        let output = std::process::Command::new("gh")
            .args([
                "api",
                &format!("/users/{github_id}/events/public"),
                "--jq",
                "[.[] | {type, repo: {name: .repo.name}, created_at}]",
            ])
            .output()
            .map_err(|e| SimardError::InvalidResearchRecord {
                field: "event_fetch".to_string(),
                reason: format!("failed to run gh cli: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::InvalidResearchRecord {
                field: "event_fetch".to_string(),
                reason: format!("gh api failed for {github_id}: {stderr}"),
            });
        }

        let events: Vec<GitHubEvent> = serde_json::from_slice(&output.stdout).map_err(|e| {
            SimardError::InvalidResearchRecord {
                field: "event_parse".to_string(),
                reason: format!("failed to parse events for {github_id}: {e}"),
            }
        })?;

        Ok(events)
    }
}

// ---------------------------------------------------------------------------
// Core logic
// ---------------------------------------------------------------------------

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs()
}

/// Derive a stable topic ID from a developer event.
fn topic_id_for_event(github_id: &str, repo_name: &str) -> String {
    format!("dev-activity:{github_id}:{repo_name}")
}

/// Convert a developer's events into candidate research topics, filtering by
/// focus areas. Returns only unique topics (deduplicated by topic id).
pub(super) fn events_to_research_topics(
    github_id: &str,
    events: &[GitHubEvent],
    focus_areas: &[String],
) -> Vec<ResearchTopic> {
    let mut seen = HashSet::new();
    let mut topics = Vec::new();

    for event in events {
        let repo_lower = event.repo.name.to_lowercase();
        let is_relevant = focus_areas.iter().any(|area| {
            let area_lower = area.to_lowercase();
            repo_lower.contains(&area_lower) || area_lower.contains(&repo_lower)
        });
        if !is_relevant {
            continue;
        }

        let id = topic_id_for_event(github_id, &event.repo.name);
        if seen.contains(&id) {
            continue;
        }
        seen.insert(id.clone());

        topics.push(ResearchTopic {
            id,
            title: format!(
                "{} activity in {} ({})",
                github_id, event.repo.name, event.event_type
            ),
            source: format!("github:{github_id}"),
            priority: 3,
            status: ResearchStatus::Proposed,
        });
    }

    topics
}

/// Poll GitHub for each tracked developer, return new [`ResearchTopic`]s that
/// are not already present in the tracker. Updates `last_checked` on each
/// watch entry.
pub fn check_developer_activity(
    tracker: &mut ResearchTracker,
    fetcher: &dyn EventFetcher,
) -> SimardResult<Vec<ResearchTopic>> {
    let existing_ids: HashSet<String> = tracker.topics.iter().map(|t| t.id.clone()).collect();

    let mut new_topics: Vec<ResearchTopic> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let now = now_epoch_secs();

    for watch in &mut tracker.watches {
        let events = fetcher.fetch_events(&watch.github_id)?;
        let candidates = events_to_research_topics(&watch.github_id, &events, &watch.focus_areas);

        for topic in candidates {
            if !existing_ids.contains(&topic.id) && !seen_ids.contains(&topic.id) {
                seen_ids.insert(topic.id.clone());
                new_topics.push(topic);
            }
        }

        watch.last_checked = Some(now);
    }

    Ok(new_topics)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research_tracker::DeveloperWatch;

    /// Mock fetcher that returns canned events.
    struct MockFetcher {
        events: Vec<GitHubEvent>,
    }

    impl MockFetcher {
        fn new(events: Vec<GitHubEvent>) -> Self {
            Self { events }
        }

        fn empty() -> Self {
            Self { events: vec![] }
        }
    }

    impl EventFetcher for MockFetcher {
        fn fetch_events(&self, _github_id: &str) -> SimardResult<Vec<GitHubEvent>> {
            Ok(self.events.clone())
        }
    }

    /// Mock fetcher that always fails.
    struct FailingFetcher;

    impl EventFetcher for FailingFetcher {
        fn fetch_events(&self, github_id: &str) -> SimardResult<Vec<GitHubEvent>> {
            Err(SimardError::InvalidResearchRecord {
                field: "event_fetch".to_string(),
                reason: format!("mock failure for {github_id}"),
            })
        }
    }

    fn make_event(event_type: &str, repo_name: &str) -> GitHubEvent {
        GitHubEvent {
            event_type: event_type.to_string(),
            repo: GitHubRepo {
                name: repo_name.to_string(),
            },
            created_at: "2026-04-16T00:00:00Z".to_string(),
        }
    }

    fn make_watch(github_id: &str, areas: Vec<&str>) -> DeveloperWatch {
        DeveloperWatch {
            github_id: github_id.to_string(),
            focus_areas: areas.into_iter().map(String::from).collect(),
            last_checked: None,
        }
    }

    // -- events_to_research_topics --

    #[test]
    fn events_to_topics_filters_by_focus_area() {
        let events = vec![
            make_event("PushEvent", "user/agent-frameworks"),
            make_event("PushEvent", "user/unrelated-repo"),
        ];
        let areas = vec!["agent-frameworks".to_string()];
        let topics = events_to_research_topics("octocat", &events, &areas);
        assert_eq!(topics.len(), 1);
        assert!(topics[0].id.contains("agent-frameworks"));
        assert_eq!(topics[0].source, "github:octocat");
    }

    #[test]
    fn events_to_topics_deduplicates_same_repo() {
        let events = vec![
            make_event("PushEvent", "user/llm-tooling"),
            make_event("CreateEvent", "user/llm-tooling"),
        ];
        let areas = vec!["llm-tooling".to_string()];
        let topics = events_to_research_topics("dev", &events, &areas);
        assert_eq!(topics.len(), 1);
    }

    #[test]
    fn events_to_topics_empty_events() {
        let topics = events_to_research_topics("dev", &[], &["anything".to_string()]);
        assert!(topics.is_empty());
    }

    #[test]
    fn events_to_topics_no_matching_areas() {
        let events = vec![make_event("PushEvent", "user/cooking-recipes")];
        let areas = vec!["rust".to_string(), "llm".to_string()];
        let topics = events_to_research_topics("dev", &events, &areas);
        assert!(topics.is_empty());
    }

    #[test]
    fn events_to_topics_case_insensitive_matching() {
        let events = vec![make_event("PushEvent", "user/LLM-Tooling")];
        let areas = vec!["llm-tooling".to_string()];
        let topics = events_to_research_topics("dev", &events, &areas);
        assert_eq!(topics.len(), 1);
    }

    #[test]
    fn events_to_topics_sets_proposed_status() {
        let events = vec![make_event("PushEvent", "user/rust-tooling")];
        let areas = vec!["rust-tooling".to_string()];
        let topics = events_to_research_topics("dev", &events, &areas);
        assert_eq!(topics[0].status, ResearchStatus::Proposed);
        assert_eq!(topics[0].priority, 3);
    }

    // -- check_developer_activity --

    #[test]
    fn check_activity_returns_new_topics() {
        let fetcher = MockFetcher::new(vec![make_event("PushEvent", "user/agent-frameworks")]);
        let mut tracker = ResearchTracker {
            topics: vec![],
            watches: vec![make_watch("octocat", vec!["agent-frameworks"])],
        };

        let new = check_developer_activity(&mut tracker, &fetcher).unwrap();
        assert_eq!(new.len(), 1);
        assert!(new[0].id.contains("octocat"));
    }

    #[test]
    fn check_activity_updates_last_checked() {
        let fetcher = MockFetcher::empty();
        let mut tracker = ResearchTracker {
            topics: vec![],
            watches: vec![make_watch("dev1", vec!["rust"])],
        };

        assert!(tracker.watches[0].last_checked.is_none());
        check_developer_activity(&mut tracker, &fetcher).unwrap();
        assert!(tracker.watches[0].last_checked.is_some());
    }

    #[test]
    fn check_activity_skips_existing_topics() {
        let fetcher = MockFetcher::new(vec![make_event("PushEvent", "user/llm-tooling")]);
        let existing_id = topic_id_for_event("simonw", "user/llm-tooling");
        let mut tracker = ResearchTracker {
            topics: vec![ResearchTopic {
                id: existing_id,
                title: "Already tracked".to_string(),
                source: "github:simonw".to_string(),
                priority: 2,
                status: ResearchStatus::InProgress,
            }],
            watches: vec![make_watch("simonw", vec!["llm-tooling"])],
        };

        let new = check_developer_activity(&mut tracker, &fetcher).unwrap();
        assert!(new.is_empty());
    }

    #[test]
    fn check_activity_empty_watches_returns_empty() {
        let fetcher = MockFetcher::empty();
        let mut tracker = ResearchTracker::new();

        let new = check_developer_activity(&mut tracker, &fetcher).unwrap();
        assert!(new.is_empty());
    }

    #[test]
    fn check_activity_deduplicates_across_watches() {
        let fetcher = MockFetcher::new(vec![make_event("PushEvent", "user/shared-repo")]);
        let mut tracker = ResearchTracker {
            topics: vec![],
            watches: vec![
                make_watch("dev1", vec!["shared-repo"]),
                make_watch("dev2", vec!["shared-repo"]),
            ],
        };

        let new = check_developer_activity(&mut tracker, &fetcher).unwrap();
        // Each dev produces a different topic id (includes github_id)
        assert_eq!(new.len(), 2);
        assert_ne!(new[0].id, new[1].id);
    }

    #[test]
    fn check_activity_propagates_fetch_errors() {
        let fetcher = FailingFetcher;
        let mut tracker = ResearchTracker {
            topics: vec![],
            watches: vec![make_watch("fail-dev", vec!["anything"])],
        };

        let err = check_developer_activity(&mut tracker, &fetcher).unwrap_err();
        assert!(err.to_string().contains("mock failure"));
    }

    #[test]
    fn check_activity_updates_all_watches_last_checked() {
        let fetcher = MockFetcher::empty();
        let mut tracker = ResearchTracker {
            topics: vec![],
            watches: vec![
                make_watch("a", vec!["x"]),
                make_watch("b", vec!["y"]),
                make_watch("c", vec!["z"]),
            ],
        };

        check_developer_activity(&mut tracker, &fetcher).unwrap();
        for watch in &tracker.watches {
            assert!(
                watch.last_checked.is_some(),
                "{} should have last_checked set",
                watch.github_id
            );
        }
    }

    // -- topic_id_for_event --

    #[test]
    fn topic_id_is_stable_and_deterministic() {
        let id1 = topic_id_for_event("user", "repo/name");
        let id2 = topic_id_for_event("user", "repo/name");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("dev-activity:"));
    }

    #[test]
    fn topic_id_differs_for_different_users() {
        let id1 = topic_id_for_event("alice", "repo");
        let id2 = topic_id_for_event("bob", "repo");
        assert_ne!(id1, id2);
    }
}
