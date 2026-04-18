//! GitHub activity polling for tracked developer watches.
//!
//! Uses the `gh` CLI to fetch recent public events (commits, PRs, issues,
//! discussions) for each [`DeveloperWatch`] and stores noteworthy items as
//! semantic facts in cognitive memory.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::types::DeveloperWatch;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single activity event fetched from GitHub.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitHubActivityEvent {
    pub event_type: String,
    pub repo: String,
    pub title: String,
    pub created_at: String,
}

impl GitHubActivityEvent {
    /// One-line summary suitable for storage as a fact content string.
    pub fn summary(&self) -> String {
        format!(
            "type={}; repo={}; title={}; created_at={}",
            self.event_type, self.repo, self.title, self.created_at,
        )
    }
}

/// Result of polling a single developer's activity.
#[derive(Clone, Debug)]
pub struct PollResult {
    pub github_id: String,
    pub events: Vec<GitHubActivityEvent>,
    pub stored_count: usize,
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// Fetch recent activity for a GitHub user via the `gh` CLI.
///
/// Returns up to `limit` events. If the `gh` CLI is unavailable or the call
/// fails, returns an empty vec rather than propagating the error (honest
/// degradation — Pillar 11).
pub fn fetch_activity(github_id: &str, limit: u32) -> Vec<GitHubActivityEvent> {
    let mut events = Vec::new();

    // PRs authored by the user.
    if let Some(prs) = fetch_prs(github_id, limit) {
        events.extend(prs);
    }

    // Issues authored by the user.
    if let Some(issues) = fetch_issues(github_id, limit) {
        events.extend(issues);
    }

    // Truncate to requested limit.
    events.truncate(limit as usize);
    events
}

/// Fetch recent PRs authored by `github_id` across GitHub.
fn fetch_prs(github_id: &str, limit: u32) -> Option<Vec<GitHubActivityEvent>> {
    let output = Command::new("gh")
        .args([
            "search",
            "prs",
            "--author",
            github_id,
            "--sort",
            "created",
            "--order",
            "desc",
            "--limit",
            &limit.to_string(),
            "--json",
            "title,repository,createdAt",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_search_results(&output.stdout, "pull_request")
}

/// Fetch recent issues authored by `github_id` across GitHub.
fn fetch_issues(github_id: &str, limit: u32) -> Option<Vec<GitHubActivityEvent>> {
    let output = Command::new("gh")
        .args([
            "search",
            "issues",
            "--author",
            github_id,
            "--sort",
            "created",
            "--order",
            "desc",
            "--limit",
            &limit.to_string(),
            "--json",
            "title,repository,createdAt",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_search_results(&output.stdout, "issue")
}

/// Parse JSON output from `gh search prs/issues --json title,repository,createdAt`.
fn parse_search_results(raw: &[u8], event_type: &str) -> Option<Vec<GitHubActivityEvent>> {
    let items: Vec<serde_json::Value> = serde_json::from_slice(raw).ok()?;
    let events = items
        .into_iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            let repo = item
                .get("repository")
                .and_then(|r| r.get("nameWithOwner"))
                .or_else(|| item.get("repository").and_then(|r| r.get("name")))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let created_at = item
                .get("createdAt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(GitHubActivityEvent {
                event_type: event_type.to_string(),
                repo,
                title,
                created_at,
            })
        })
        .collect();
    Some(events)
}

// ---------------------------------------------------------------------------
// Storing
// ---------------------------------------------------------------------------

/// Store a batch of activity events as semantic facts in cognitive memory.
///
/// Each event becomes a fact keyed by `"dev-activity:{github_id}:{index}"`.
/// An episode entry is also recorded summarising the poll.
pub fn store_activity_events(
    github_id: &str,
    events: &[GitHubActivityEvent],
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<usize> {
    if events.is_empty() {
        return Ok(0);
    }

    let mut stored = 0;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for (i, event) in events.iter().enumerate() {
        let concept = format!("dev-activity:{github_id}:{now}:{i}");
        let content = event.summary();
        let tags = vec![
            "developer-activity".to_string(),
            format!("dev:{github_id}"),
            event.event_type.clone(),
        ];
        memory.store_fact(&concept, &content, 0.6, &tags, "activity-poller")?;
        stored += 1;
    }

    memory.store_episode(
        &format!("Polled {stored} activity event(s) for developer {github_id}",),
        "activity-poller",
        Some(&json!({
            "github_id": github_id,
            "event_count": stored,
        })),
    )?;

    Ok(stored)
}

// ---------------------------------------------------------------------------
// Poll orchestration
// ---------------------------------------------------------------------------

/// Poll activity for a single developer watch, fetch events and store them.
///
/// Returns a [`PollResult`] with the events found and how many were stored.
pub fn poll_developer_activity(
    watch: &DeveloperWatch,
    memory: &dyn CognitiveMemoryOps,
    limit: u32,
) -> SimardResult<PollResult> {
    if watch.github_id.trim().is_empty() {
        return Err(SimardError::InvalidResearchRecord {
            field: "developer_watch.github_id".to_string(),
            reason: "github_id cannot be empty".to_string(),
        });
    }

    let events = fetch_activity(&watch.github_id, limit);
    let stored_count = store_activity_events(&watch.github_id, &events, memory)?;

    Ok(PollResult {
        github_id: watch.github_id.clone(),
        events,
        stored_count,
    })
}

/// Poll activity for all developer watches and store results.
///
/// Returns one [`PollResult`] per watch. Individual watch failures are logged
/// to stderr but do not abort the batch (Pillar 11: honest degradation).
pub fn poll_all_developer_activity(
    watches: &[DeveloperWatch],
    memory: &dyn CognitiveMemoryOps,
    limit_per_dev: u32,
) -> Vec<PollResult> {
    let mut results = Vec::with_capacity(watches.len());
    for watch in watches {
        match poll_developer_activity(watch, memory, limit_per_dev) {
            Ok(result) => results.push(result),
            Err(e) => {
                eprintln!("[simard] activity poll failed for {}: {e}", watch.github_id);
            }
        }
    }
    results
}

/// Format a human-readable summary of poll results.
pub fn summarize_poll_results(results: &[PollResult]) -> String {
    if results.is_empty() {
        return "no developer activity polled".to_string();
    }
    let summaries: Vec<String> = results
        .iter()
        .map(|r| format!("{}:{} events", r.github_id, r.events.len()))
        .collect();
    format!(
        "polled {} developer(s): {}",
        results.len(),
        summaries.join(", ")
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;

    fn mock_memory() -> Box<dyn CognitiveMemoryOps> {
        Box::new(CognitiveMemoryBridge::new(Box::new(
            InMemoryBridgeTransport::new("test-activity", |method, _params| match method {
                "memory.store_fact" => Ok(serde_json::json!({"id": "sem_act_1"})),
                "memory.store_episode" => Ok(serde_json::json!({"id": "epi_act_1"})),
                "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            }),
        )))
    }

    #[test]
    fn event_summary_format() {
        let event = GitHubActivityEvent {
            event_type: "pull_request".to_string(),
            repo: "octocat/hello-world".to_string(),
            title: "Add README".to_string(),
            created_at: "2024-01-15T10:00:00Z".to_string(),
        };
        let summary = event.summary();
        assert!(summary.contains("type=pull_request"));
        assert!(summary.contains("repo=octocat/hello-world"));
        assert!(summary.contains("title=Add README"));
        assert!(summary.contains("created_at=2024-01-15T10:00:00Z"));
    }

    #[test]
    fn store_activity_events_empty_is_noop() {
        let memory = mock_memory();
        let stored = store_activity_events("octocat", &[], &*memory).unwrap();
        assert_eq!(stored, 0);
    }

    #[test]
    fn store_activity_events_stores_facts() {
        let memory = mock_memory();
        let events = vec![
            GitHubActivityEvent {
                event_type: "pull_request".to_string(),
                repo: "octocat/hello".to_string(),
                title: "Fix bug".to_string(),
                created_at: "2024-01-15T10:00:00Z".to_string(),
            },
            GitHubActivityEvent {
                event_type: "issue".to_string(),
                repo: "octocat/world".to_string(),
                title: "Feature request".to_string(),
                created_at: "2024-01-15T11:00:00Z".to_string(),
            },
        ];
        let stored = store_activity_events("octocat", &events, &*memory).unwrap();
        assert_eq!(stored, 2);
    }

    #[test]
    fn poll_developer_rejects_empty_github_id() {
        let memory = mock_memory();
        let watch = DeveloperWatch {
            github_id: "".to_string(),
            focus_areas: vec!["testing".to_string()],
            last_checked: None,
        };
        let err = poll_developer_activity(&watch, &*memory, 5).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn poll_developer_returns_result_structure() {
        let memory = mock_memory();
        let watch = DeveloperWatch {
            github_id: "octocat".to_string(),
            focus_areas: vec!["testing".to_string()],
            last_checked: None,
        };
        // gh CLI may not be configured for this user, so events may be empty.
        // The function should still succeed structurally.
        let result = poll_developer_activity(&watch, &*memory, 5).unwrap();
        assert_eq!(result.github_id, "octocat");
        assert_eq!(result.stored_count, result.events.len());
    }

    #[test]
    fn poll_all_handles_batch() {
        let memory = mock_memory();
        let watches = vec![
            DeveloperWatch {
                github_id: "user-a".to_string(),
                focus_areas: vec!["rust".to_string()],
                last_checked: None,
            },
            DeveloperWatch {
                github_id: "user-b".to_string(),
                focus_areas: vec!["python".to_string()],
                last_checked: None,
            },
        ];
        let results = poll_all_developer_activity(&watches, &*memory, 3);
        // Both should succeed structurally (even if no events are found).
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn parse_search_results_valid_json() {
        let raw = br#"[
            {"title": "Add feature", "repository": {"nameWithOwner": "org/repo"}, "createdAt": "2024-01-15T10:00:00Z"},
            {"title": "Fix bug", "repository": {"nameWithOwner": "org/repo2"}, "createdAt": "2024-01-16T10:00:00Z"}
        ]"#;
        let events = parse_search_results(raw, "pull_request").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "pull_request");
        assert_eq!(events[0].repo, "org/repo");
        assert_eq!(events[0].title, "Add feature");
        assert_eq!(events[1].repo, "org/repo2");
    }

    #[test]
    fn parse_search_results_empty_array() {
        let raw = b"[]";
        let events = parse_search_results(raw, "issue").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn parse_search_results_invalid_json() {
        let raw = b"not json";
        assert!(parse_search_results(raw, "issue").is_none());
    }

    #[test]
    fn parse_search_results_missing_fields() {
        let raw = br#"[{"unrelated": true}]"#;
        let events = parse_search_results(raw, "issue").unwrap();
        // Items missing "title" are filtered out.
        assert!(events.is_empty());
    }

    #[test]
    fn summarize_poll_results_empty() {
        let summary = summarize_poll_results(&[]);
        assert_eq!(summary, "no developer activity polled");
    }

    #[test]
    fn summarize_poll_results_with_data() {
        let results = vec![
            PollResult {
                github_id: "alice".to_string(),
                events: vec![GitHubActivityEvent {
                    event_type: "pull_request".to_string(),
                    repo: "org/repo".to_string(),
                    title: "PR".to_string(),
                    created_at: "2024-01-15T10:00:00Z".to_string(),
                }],
                stored_count: 1,
            },
            PollResult {
                github_id: "bob".to_string(),
                events: vec![],
                stored_count: 0,
            },
        ];
        let summary = summarize_poll_results(&results);
        assert!(summary.contains("polled 2 developer(s)"));
        assert!(summary.contains("alice:1 events"));
        assert!(summary.contains("bob:0 events"));
    }

    #[test]
    fn fetch_activity_unknown_user_returns_empty_or_results() {
        // Should not panic regardless of gh CLI availability.
        let events = fetch_activity("__nonexistent_user_test__", 3);
        // Events may be empty (gh not authed) or contain results — both valid.
        let _ = events;
    }
}
