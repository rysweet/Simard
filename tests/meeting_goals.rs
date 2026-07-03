//! Integration tests for Phase 8: meeting facilitator, goal curation,
//! research tracking, and dual identity.

use serde_json::json;

use simard::goal_curation::{
    ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS, add_active_goal,
    add_backlog_item, archive_completed, load_goal_board, persist_board, promote_to_active,
    update_goal_progress,
};
use simard::identity_auth::{
    AuthIdentity, DualIdentityConfig, env_for_identity, identity_for_operation,
    validate_identity_for_operation,
};
use simard::meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingSessionStatus, add_note, close_meeting, record_action_item,
    record_decision, start_meeting,
};
use simard::memory_adapter::CognitiveMemoryAdapter;
use simard::research_tracker::{
    DeveloperWatch, ResearchStatus, ResearchTopic, add_research_topic, track_developer,
    update_topic_status,
};
use simard::server_subprocess::InMemoryServerTransport;
use simard::server_transport::ServerErrorPayload;

fn mock_adapter() -> CognitiveMemoryAdapter {
    let transport =
        InMemoryServerTransport::new("test-integration", |method, _params| match method {
            "memory.record_sensory" => Ok(json!({"id": "sen_int"})),
            "memory.store_episode" => Ok(json!({"id": "epi_int"})),
            "memory.store_fact" => Ok(json!({"id": "sem_int"})),
            "memory.store_prospective" => Ok(json!({"id": "pro_int"})),
            "memory.search_facts" => Ok(json!({"facts": []})),
            _ => Err(ServerErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryAdapter::new(Box::new(transport))
}

fn sample_active(id: &str, priority: u32) -> ActiveGoal {
    ActiveGoal {
        parent_goal_id: None,
        repo: None,
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: Vec::new(),
        last_progress_update_at: None,
    }
}

#[test]
fn meeting_full_lifecycle() {
    let adapter = mock_adapter();
    let mut session = start_meeting("Phase 8 planning", &adapter).expect("meeting should start");
    assert_eq!(session.status, MeetingSessionStatus::Open);

    record_decision(
        &mut session,
        MeetingDecision {
            description: "Implement goal curation with max 5 active".to_string(),
            rationale: "Prevents goal sprawl".to_string(),
            participants: vec!["alice".to_string()],
        },
    )
    .unwrap();
    record_action_item(
        &mut session,
        ActionItem {
            description: "Write tests".to_string(),
            owner: "bob".to_string(),
            priority: 1,
            due_description: Some("before merge".to_string()),
            linked_issue: None,
        },
    )
    .unwrap();
    add_note(&mut session, "Research tracker also needed").unwrap();

    let closed = close_meeting(session, &adapter).expect("meeting should close");
    assert_eq!(closed.status, MeetingSessionStatus::Closed);
    assert_eq!(closed.decisions.len(), 1);
    assert_eq!(closed.action_items.len(), 1);
    assert_eq!(closed.notes.len(), 1);
    assert!(closed.durable_summary().contains("Phase 8 planning"));
}

#[test]
fn meeting_rejects_operations_on_closed_session_and_double_close() {
    let adapter = mock_adapter();
    let session = start_meeting("Retro", &adapter).unwrap();
    let mut closed = close_meeting(session, &adapter).unwrap();

    assert!(
        record_decision(
            &mut closed,
            MeetingDecision {
                description: "late".to_string(),
                rationale: "nope".to_string(),
                participants: vec![],
            }
        )
        .is_err()
    );
    assert!(
        record_action_item(
            &mut closed,
            ActionItem {
                description: "late".to_string(),
                owner: "x".to_string(),
                priority: 1,
                due_description: None,
                linked_issue: None,
            }
        )
        .is_err()
    );
    assert!(add_note(&mut closed, "late").is_err());
    assert!(close_meeting(closed, &adapter).is_err());
}

#[test]
fn goal_board_enforce_capacity_and_promote() {
    let mut board = GoalBoard::new();
    for i in 1..=MAX_ACTIVE_GOALS {
        add_active_goal(&mut board, sample_active(&format!("g{i}"), i as u32)).unwrap();
    }
    assert_eq!(board.active_slots_remaining(), 0);
    assert!(add_active_goal(&mut board, sample_active("overflow", 1)).is_err());

    add_backlog_item(
        &mut board,
        BacklogItem {
            id: "bl-1".to_string(),
            description: "Backlog item".to_string(),
            source: "meeting".to_string(),
            score: 0.9,
        },
    )
    .unwrap();
    assert!(promote_to_active(&mut board, "bl-1", 1, None).is_err());

    update_goal_progress(&mut board, "g1", GoalProgress::Completed).unwrap();
    let archived = archive_completed(&mut board);
    assert_eq!(archived.len(), 1);

    promote_to_active(&mut board, "bl-1", 1, Some("curator".to_string())).unwrap();
    assert_eq!(board.active.len(), MAX_ACTIVE_GOALS);
    assert!(board.backlog.is_empty());
}

#[test]
fn goal_board_progress_lifecycle() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, sample_active("g1", 1)).unwrap();

    update_goal_progress(&mut board, "g1", GoalProgress::InProgress { percent: 50 }).unwrap();
    assert!(matches!(
        board.active[0].status,
        GoalProgress::InProgress { percent: 50 }
    ));

    update_goal_progress(
        &mut board,
        "g1",
        GoalProgress::Blocked("waiting".to_string()),
    )
    .unwrap();
    assert!(matches!(board.active[0].status, GoalProgress::Blocked(_)));

    update_goal_progress(&mut board, "g1", GoalProgress::Completed).unwrap();
    assert_eq!(archive_completed(&mut board).len(), 1);
    assert!(board.active.is_empty());
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn goal_board_load_persist_and_duplicates() {
    // `persist_board` flows through `save_goal_board`, which acquires a
    // cross-process advisory lock under `<state_root>/state/` (issue #2511).
    // Pin a hermetic state root so that lock lands in a tempdir instead of the
    // operator's ~/.simard. `HermeticState` mutates process-global env vars,
    // so its contract requires `#[serial(cognitive_memory)]`.
    let _hermetic = simard::test_support::HermeticState::new();
    let adapter = mock_adapter();
    let board = load_goal_board(&adapter).unwrap();
    assert!(board.active.is_empty());
    persist_board(&board, &adapter).unwrap();

    let mut board2 = GoalBoard::new();
    add_active_goal(&mut board2, sample_active("g1", 1)).unwrap();
    assert!(
        add_active_goal(&mut board2, sample_active("g1", 1))
            .unwrap_err()
            .to_string()
            .contains("already active")
    );
}

#[test]
fn research_topic_and_developer_watch() {
    let adapter = mock_adapter();

    add_research_topic(
        ResearchTopic {
            id: "rt-1".to_string(),
            title: "Cognitive memory consolidation".to_string(),
            source: "meeting".to_string(),
            priority: 1,
            status: ResearchStatus::Proposed,
        },
        &adapter,
    )
    .unwrap();
    update_topic_status("rt-1", ResearchStatus::Completed, &adapter).unwrap();

    track_developer(
        DeveloperWatch {
            github_id: "octocat".to_string(),
            focus_areas: vec!["agent-sdk".to_string()],
            last_checked: None,
        },
        &adapter,
    )
    .unwrap();

    // Rejects empty fields.
    assert!(
        add_research_topic(
            ResearchTopic {
                id: "".to_string(),
                title: "x".to_string(),
                source: "y".to_string(),
                priority: 1,
                status: ResearchStatus::Proposed,
            },
            &adapter,
        )
        .is_err()
    );
    assert!(
        track_developer(
            DeveloperWatch {
                github_id: "".to_string(),
                focus_areas: vec!["a".to_string()],
                last_checked: None,
            },
            &adapter,
        )
        .is_err()
    );
}

#[test]
fn dual_identity_config_and_env() {
    let config =
        DualIdentityConfig::new("copilot-user", "commit-user", "commit@example.com").unwrap();
    assert_eq!(config.copilot_user, "copilot-user");
    assert!(DualIdentityConfig::new("", "user", "e@x.com").is_err());
    assert!(DualIdentityConfig::new("u", "u", "noemail").is_err());

    let copilot_env = env_for_identity(AuthIdentity::CopilotAuth, &config);
    assert_eq!(copilot_env.len(), 1);
    assert_eq!(copilot_env[0].0, "GITHUB_USER");

    let commit_env = env_for_identity(AuthIdentity::CommitAuth, &config);
    assert_eq!(commit_env.len(), 4);
}

#[test]
fn identity_operation_validation() {
    assert!(validate_identity_for_operation(AuthIdentity::CopilotAuth, "git-commit").is_err());
    assert!(validate_identity_for_operation(AuthIdentity::CommitAuth, "copilot-chat").is_err());
    assert!(validate_identity_for_operation(AuthIdentity::CopilotAuth, "copilot-chat").is_ok());
    assert!(validate_identity_for_operation(AuthIdentity::CommitAuth, "git-commit").is_ok());
    assert!(validate_identity_for_operation(AuthIdentity::CopilotAuth, "custom").is_ok());

    assert_eq!(
        identity_for_operation("copilot-submit"),
        Some(AuthIdentity::CopilotAuth)
    );
    assert_eq!(
        identity_for_operation("git-tag"),
        Some(AuthIdentity::CommitAuth)
    );
    assert_eq!(identity_for_operation("unknown"), None);
}

#[test]
fn meeting_decisions_feed_goal_board() {
    let adapter = mock_adapter();
    let mut session = start_meeting("Goal alignment", &adapter).unwrap();
    record_decision(
        &mut session,
        MeetingDecision {
            description: "Prioritize cognitive memory consolidation".to_string(),
            rationale: "Foundation for all features".to_string(),
            participants: vec!["team".to_string()],
        },
    )
    .unwrap();
    let closed = close_meeting(session, &adapter).unwrap();

    let mut board = GoalBoard::new();
    for (i, decision) in closed.decisions.iter().enumerate() {
        add_active_goal(
            &mut board,
            ActiveGoal {
                parent_goal_id: None,
                repo: None,
                id: format!("from-meeting-{i}"),
                description: decision.description.clone(),
                priority: (i + 1) as u32,
                status: GoalProgress::NotStarted,
                assigned_to: None,
                current_activity: None,
                wip_refs: Vec::new(),
                last_progress_update_at: None,
            },
        )
        .unwrap();
    }
    assert_eq!(board.active.len(), 1);
    assert!(board.active[0].description.contains("cognitive memory"));
}

#[test]
fn meeting_action_items_become_research_topics() {
    let adapter = mock_adapter();
    let mut session = start_meeting("Research planning", &adapter).unwrap();
    record_action_item(
        &mut session,
        ActionItem {
            description: "Investigate agent memory patterns".to_string(),
            owner: "researcher".to_string(),
            priority: 2,
            due_description: None,
            linked_issue: None,
        },
    )
    .unwrap();
    let closed = close_meeting(session, &adapter).unwrap();

    for (i, item) in closed.action_items.iter().enumerate() {
        add_research_topic(
            ResearchTopic {
                id: format!("from-action-{i}"),
                title: item.description.clone(),
                source: "meeting-action-item".to_string(),
                priority: item.priority,
                status: ResearchStatus::Proposed,
            },
            &adapter,
        )
        .unwrap();
    }
}
