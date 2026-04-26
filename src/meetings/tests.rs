use super::{PersistedMeetingGoalUpdate, PersistedMeetingRecord};
use crate::error::SimardError;
use crate::goals::GoalStatus;

#[test]
fn parses_persisted_meeting_record_for_readback() {
    let record = PersistedMeetingRecord::parse(
        "agenda=align the next Simard workstream; updates=[durable memory merged]; decisions=[preserve meeting-to-engineer continuity]; risks=[workflow routing is still unreliable]; next_steps=[keep durable priorities visible]; open_questions=[how aggressively should Simard reprioritize?]; goals=[p1:active:Preserve meeting handoff:meeting decisions must shape later work]",
    )
    .expect("persisted meeting record should parse");

    assert_eq!(record.agenda, "align the next Simard workstream");
    assert_eq!(record.updates, vec!["durable memory merged"]);
    assert_eq!(
        record.decisions,
        vec!["preserve meeting-to-engineer continuity"]
    );
    assert_eq!(record.risks, vec!["workflow routing is still unreliable"]);
    assert_eq!(record.next_steps, vec!["keep durable priorities visible"]);
    assert_eq!(
        record.open_questions,
        vec!["how aggressively should Simard reprioritize?"]
    );
    assert_eq!(
        record.goals,
        vec![PersistedMeetingGoalUpdate {
            priority: 1,
            status: GoalStatus::Active,
            title: "Preserve meeting handoff".to_string(),
            rationale: "meeting decisions must shape later work".to_string(),
        }]
    );
    assert_eq!(
        record.goals[0].concise_label(),
        "p1 [active] Preserve meeting handoff"
    );
}

#[test]
fn rejects_malformed_persisted_meeting_goal_update() {
    let error = PersistedMeetingRecord::parse(
        "agenda=align the next Simard workstream; updates=[]; decisions=[preserve meeting-to-engineer continuity]; risks=[]; next_steps=[]; open_questions=[]; goals=[p0:active:Preserve meeting handoff:meeting decisions must shape later work]",
    )
    .expect_err("malformed goal update should fail");

    assert_eq!(
        error,
        SimardError::InvalidMeetingRecord {
            field: "goals".to_string(),
            reason: "goal update 'p0:active:Preserve meeting handoff:meeting decisions must shape later work' must use priority 1 or greater".to_string(),
        }
    );
}

#[test]
fn parse_empty_record_fails() {
    let err = PersistedMeetingRecord::parse("").expect_err("empty should fail");
    assert_eq!(
        err,
        SimardError::InvalidMeetingRecord {
            field: "record".to_string(),
            reason: "persisted meeting record cannot be empty".to_string(),
        }
    );
}

#[test]
fn parse_whitespace_only_record_fails() {
    let err = PersistedMeetingRecord::parse("   \n  ").expect_err("whitespace should fail");
    assert_eq!(
        err,
        SimardError::InvalidMeetingRecord {
            field: "record".to_string(),
            reason: "persisted meeting record cannot be empty".to_string(),
        }
    );
}

#[test]
fn looks_like_persisted_meeting_record_positive() {
    let valid =
        "agenda=x; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[]";
    assert!(
        super::looks_like_persisted_meeting_record(valid),
        "should recognize valid meeting record format"
    );
}

#[test]
fn looks_like_persisted_meeting_record_negative() {
    assert!(!super::looks_like_persisted_meeting_record("random text"));
    assert!(!super::looks_like_persisted_meeting_record("agenda=x"));
    assert!(!super::looks_like_persisted_meeting_record(""));
}

#[test]
fn parse_record_with_all_empty_lists() {
    let record = PersistedMeetingRecord::parse(
        "agenda=standup; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[]",
    )
    .expect("all-empty-lists record should parse");
    assert_eq!(record.agenda, "standup");
    assert!(record.updates.is_empty());
    assert!(record.decisions.is_empty());
    assert!(record.risks.is_empty());
    assert!(record.next_steps.is_empty());
    assert!(record.open_questions.is_empty());
    assert!(record.goals.is_empty());
}

#[test]
fn parse_record_with_multiple_pipe_separated_items() {
    let record = PersistedMeetingRecord::parse(
        "agenda=review; updates=[item1 | item2 | item3]; decisions=[d1 | d2]; risks=[r1]; next_steps=[n1 | n2]; open_questions=[q1]; goals=[]",
    )
    .expect("pipe-separated items should parse");
    assert_eq!(record.updates, vec!["item1", "item2", "item3"]);
    assert_eq!(record.decisions, vec!["d1", "d2"]);
    assert_eq!(record.risks, vec!["r1"]);
    assert_eq!(record.next_steps, vec!["n1", "n2"]);
}

#[test]
fn parse_record_with_multiple_goals() {
    let record = PersistedMeetingRecord::parse(
        "agenda=planning; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[p1:active:Goal A:rationale A | p2:completed:Goal B:rationale B]",
    )
    .expect("multiple goals should parse");
    assert_eq!(record.goals.len(), 2);
    assert_eq!(record.goals[0].priority, 1);
    assert_eq!(record.goals[0].status, GoalStatus::Active);
    assert_eq!(record.goals[0].title, "Goal A");
    assert_eq!(record.goals[1].priority, 2);
    assert_eq!(record.goals[1].status, GoalStatus::Completed);
    assert_eq!(record.goals[1].title, "Goal B");
}

#[test]
fn parse_goal_invalid_status_fails() {
    let err = PersistedMeetingRecord::parse(
        "agenda=test; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[p1:nonsense:Title:Reason]",
    )
    .expect_err("invalid status should fail");
    match err {
        SimardError::InvalidMeetingRecord { field, reason } => {
            assert_eq!(field, "goals");
            assert!(reason.contains("unsupported status"));
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn parse_goal_missing_colons_fails() {
    let err = PersistedMeetingRecord::parse(
        "agenda=test; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[no-colons-here]",
    )
    .expect_err("goal without colons should fail");
    match err {
        SimardError::InvalidMeetingRecord { reason, .. } => {
            assert!(reason.contains("p<priority>:<status>:<title>:<rationale>"));
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn parse_missing_agenda_prefix_fails() {
    let err = PersistedMeetingRecord::parse("no-agenda-prefix here")
        .expect_err("missing agenda= should fail");
    match err {
        SimardError::InvalidMeetingRecord { field, .. } => {
            assert_eq!(field, "agenda");
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn concise_label_formats_correctly() {
    let goal = PersistedMeetingGoalUpdate {
        priority: 3,
        status: GoalStatus::Paused,
        title: "Review docs".to_string(),
        rationale: "waiting on feedback".to_string(),
    };
    assert_eq!(goal.concise_label(), "p3 [paused] Review docs");
}

#[test]
fn concise_label_all_statuses() {
    let make = |status: GoalStatus| PersistedMeetingGoalUpdate {
        priority: 1,
        status,
        title: "T".to_string(),
        rationale: "R".to_string(),
    };
    assert!(
        make(GoalStatus::Proposed)
            .concise_label()
            .contains("[proposed]")
    );
    assert!(
        make(GoalStatus::Active)
            .concise_label()
            .contains("[active]")
    );
    assert!(
        make(GoalStatus::Completed)
            .concise_label()
            .contains("[completed]")
    );
}

#[test]
fn parse_goal_non_numeric_priority_fails() {
    let err = PersistedMeetingRecord::parse(
        "agenda=test; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[pX:active:Title:Reason]",
    )
    .expect_err("non-numeric priority should fail");
    match err {
        SimardError::InvalidMeetingRecord { reason, .. } => {
            assert!(reason.contains("invalid priority"));
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn parse_goal_empty_title_fails() {
    let err = PersistedMeetingRecord::parse(
        "agenda=test; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[p1:active::Reason]",
    )
    .expect_err("empty title should fail");
    match err {
        SimardError::InvalidMeetingRecord { reason, .. } => {
            assert!(reason.contains("cannot be empty"));
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn parse_goal_empty_rationale_fails() {
    let err = PersistedMeetingRecord::parse(
        "agenda=test; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[p1:active:Title:]",
    )
    .expect_err("empty rationale should fail");
    match err {
        SimardError::InvalidMeetingRecord { reason, .. } => {
            assert!(reason.contains("cannot be empty"));
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn looks_like_partial_fields_returns_false() {
    // Has some but not all required fields
    assert!(!super::looks_like_persisted_meeting_record(
        "agenda=x; updates=[]; decisions=[]"
    ));
    assert!(!super::looks_like_persisted_meeting_record(
        "agenda=x; updates=[]; decisions=[]; risks=[]; next_steps=[]"
    ));
}

#[test]
fn parse_missing_decisions_field_fails() {
    let err = PersistedMeetingRecord::parse(
        "agenda=test; updates=[]; MISSING=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[]",
    )
    .expect_err("missing decisions= should fail");
    match err {
        SimardError::InvalidMeetingRecord { field, .. } => {
            assert_eq!(field, "updates");
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn parse_unbracketed_updates_fails() {
    let err = PersistedMeetingRecord::parse(
        "agenda=test; updates=no-brackets; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[]",
    )
    .expect_err("unbracketed value should fail");
    match err {
        SimardError::InvalidMeetingRecord { reason, .. } => {
            assert!(reason.contains("bracketed list syntax"));
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn parse_goal_without_p_prefix_fails() {
    let err = PersistedMeetingRecord::parse(
        "agenda=test; updates=[]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[1:active:Title:Reason]",
    )
    .expect_err("goal without p prefix should fail");
    match err {
        SimardError::InvalidMeetingRecord { reason, .. } => {
            assert!(reason.contains("p<priority>"));
        }
        other => panic!("expected InvalidMeetingRecord, got {other:?}"),
    }
}

#[test]
fn parse_record_with_whitespace_around_values() {
    let record = PersistedMeetingRecord::parse(
        "agenda=  spaced agenda  ; updates=[ item with spaces ]; decisions=[]; risks=[]; next_steps=[]; open_questions=[]; goals=[]",
    )
    .expect("whitespace in values should parse");
    assert_eq!(record.agenda, "spaced agenda");
    assert_eq!(record.updates, vec!["item with spaces"]);
}
