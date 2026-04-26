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
