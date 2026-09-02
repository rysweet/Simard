use super::session::*;
use super::types::{ActionItem, MeetingDecision, MeetingSessionStatus};
use crate::memory_client::CognitiveMemoryClient;
use crate::rpc_transport::InMemoryRpcTransport;
use serde_json::json;

fn mock_memory() -> CognitiveMemoryClient {
    let transport = InMemoryRpcTransport::new("test-meeting", |method, _params| match method {
        "memory.record_sensory" => Ok(json!({"id": "sen_m1"})),
        "memory.store_episode" => Ok(json!({"id": "epi_m1"})),
        "memory.store_fact" => Ok(json!({"id": "sem_m1"})),
        "memory.store_prospective" => Ok(json!({"id": "pro_m1"})),
        _ => Err(crate::rpc::RpcErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryClient::new(Box::new(transport))
}

#[test]
fn start_and_close_meeting_round_trip() {
    let memory = mock_memory();
    let mut session = start_meeting("Sprint planning", &memory).unwrap();
    assert_eq!(session.status, MeetingSessionStatus::Open);

    record_decision(
        &mut session,
        MeetingDecision {
            description: "Ship phase 8".to_string(),
            rationale: "Unblocks goal curation".to_string(),
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
            due_description: Some("end of sprint".to_string()),
            linked_issue: None,
        },
    )
    .unwrap();

    let closed = close_meeting(session, &memory).unwrap();
    assert_eq!(closed.status, MeetingSessionStatus::Closed);
    assert_eq!(closed.decisions.len(), 1);
    assert_eq!(closed.action_items.len(), 1);
}

#[test]
fn cannot_add_to_closed_meeting() {
    let memory = mock_memory();
    let session = start_meeting("Retro", &memory).unwrap();
    let mut closed = close_meeting(session, &memory).unwrap();

    let err = record_decision(
        &mut closed,
        MeetingDecision {
            description: "late".to_string(),
            rationale: "oops".to_string(),
            participants: vec![],
        },
    )
    .unwrap_err();

    assert!(err.to_string().contains("closed meeting"));
}

#[test]
fn rejects_empty_topic() {
    let memory = mock_memory();
    let err = start_meeting("", &memory).unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn rejects_zero_priority_action_item() {
    let memory = mock_memory();
    let mut session = start_meeting("Check", &memory).unwrap();
    let err = record_action_item(
        &mut session,
        ActionItem {
            description: "task".to_string(),
            owner: "me".to_string(),
            priority: 0,
            due_description: None,
            linked_issue: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("priority"));
}

#[test]
fn edit_decision_description() {
    let memory = mock_memory();
    let mut session = start_meeting("Edit test", &memory).unwrap();
    record_decision(
        &mut session,
        MeetingDecision {
            description: "Original".to_string(),
            rationale: "reason".to_string(),
            participants: vec![],
        },
    )
    .unwrap();
    edit_item(&mut session, "decision", 0, "Updated").unwrap();
    assert_eq!(session.decisions[0].description, "Updated");
}

#[test]
fn edit_action_item_description() {
    let memory = mock_memory();
    let mut session = start_meeting("Edit test", &memory).unwrap();
    record_action_item(
        &mut session,
        ActionItem {
            description: "Old task".to_string(),
            owner: "alice".to_string(),
            priority: 1,
            due_description: None,
            linked_issue: None,
        },
    )
    .unwrap();
    edit_item(&mut session, "action", 0, "New task").unwrap();
    assert_eq!(session.action_items[0].description, "New task");
    assert_eq!(session.action_items[0].owner, "alice");
}

#[test]
fn edit_note() {
    let memory = mock_memory();
    let mut session = start_meeting("Edit test", &memory).unwrap();
    add_note(&mut session, "old note").unwrap();
    edit_item(&mut session, "note", 0, "new note").unwrap();
    assert_eq!(session.notes[0], "new note");
}

#[test]
fn edit_out_of_bounds_returns_error() {
    let memory = mock_memory();
    let mut session = start_meeting("Edit test", &memory).unwrap();
    let err = edit_item(&mut session, "decision", 0, "text").unwrap_err();
    assert!(err.to_string().contains("out of range"));
}

#[test]
fn edit_unknown_type_returns_error() {
    let memory = mock_memory();
    let mut session = start_meeting("Edit test", &memory).unwrap();
    let err = edit_item(&mut session, "bogus", 0, "text").unwrap_err();
    assert!(err.to_string().contains("unknown item type"));
}

#[test]
fn remove_decision() {
    let memory = mock_memory();
    let mut session = start_meeting("Delete test", &memory).unwrap();
    record_decision(
        &mut session,
        MeetingDecision {
            description: "D1".to_string(),
            rationale: "r".to_string(),
            participants: vec![],
        },
    )
    .unwrap();
    record_decision(
        &mut session,
        MeetingDecision {
            description: "D2".to_string(),
            rationale: "r".to_string(),
            participants: vec![],
        },
    )
    .unwrap();
    remove_item(&mut session, "decision", 0).unwrap();
    assert_eq!(session.decisions.len(), 1);
    assert_eq!(session.decisions[0].description, "D2");
}

#[test]
fn remove_action_item() {
    let memory = mock_memory();
    let mut session = start_meeting("Delete test", &memory).unwrap();
    record_action_item(
        &mut session,
        ActionItem {
            description: "task".to_string(),
            owner: "me".to_string(),
            priority: 1,
            due_description: None,
            linked_issue: None,
        },
    )
    .unwrap();
    remove_item(&mut session, "action", 0).unwrap();
    assert!(session.action_items.is_empty());
}

#[test]
fn remove_note() {
    let memory = mock_memory();
    let mut session = start_meeting("Delete test", &memory).unwrap();
    add_note(&mut session, "keep").unwrap();
    add_note(&mut session, "remove").unwrap();
    remove_item(&mut session, "note", 1).unwrap();
    assert_eq!(session.notes, vec!["keep"]);
}

#[test]
fn remove_out_of_bounds_returns_error() {
    let memory = mock_memory();
    let mut session = start_meeting("Delete test", &memory).unwrap();
    let err = remove_item(&mut session, "action", 0).unwrap_err();
    assert!(err.to_string().contains("out of range"));
}

#[test]
fn remove_unknown_type_returns_error() {
    let memory = mock_memory();
    let mut session = start_meeting("Delete test", &memory).unwrap();
    let err = remove_item(&mut session, "bogus", 0).unwrap_err();
    assert!(err.to_string().contains("unknown item type"));
}

#[test]
fn add_question_to_session() {
    let memory = mock_memory();
    let mut session = start_meeting("Question test", &memory).unwrap();
    add_question(&mut session, "What is the release timeline?").unwrap();
    assert_eq!(session.explicit_questions.len(), 1);
    assert_eq!(
        session.explicit_questions[0],
        "What is the release timeline?"
    );
}

#[test]
fn add_question_rejects_empty() {
    let memory = mock_memory();
    let mut session = start_meeting("Question test", &memory).unwrap();
    let err = add_question(&mut session, "").unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn add_question_rejects_closed_meeting() {
    let memory = mock_memory();
    let session = start_meeting("Q", &memory).unwrap();
    let mut closed = close_meeting(session, &memory).unwrap();
    let err = add_question(&mut closed, "Late question").unwrap_err();
    assert!(err.to_string().contains("closed meeting"));
}

#[test]
fn edit_question() {
    let memory = mock_memory();
    let mut session = start_meeting("Edit Q", &memory).unwrap();
    add_question(&mut session, "Original question").unwrap();
    edit_item(&mut session, "question", 0, "Updated question").unwrap();
    assert_eq!(session.explicit_questions[0], "Updated question");
}

#[test]
fn remove_question() {
    let memory = mock_memory();
    let mut session = start_meeting("Delete Q", &memory).unwrap();
    add_question(&mut session, "Q1").unwrap();
    add_question(&mut session, "Q2").unwrap();
    remove_item(&mut session, "question", 0).unwrap();
    assert_eq!(session.explicit_questions, vec!["Q2"]);
}
