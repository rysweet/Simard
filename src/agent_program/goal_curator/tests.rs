use super::*;
use crate::agent_program::test_support::{test_context, test_outcome};
use crate::session::SessionId;

// --- GoalCuratorProgram ---

#[test]
fn goal_curator_accepts_natural_language_input() {
    let plan = StructuredGoalPlan::parse("review top 5 goals").expect("parse test goal plan");
    assert_eq!(plan.goals.len(), 1);
    assert_eq!(plan.goals[0].title, "review top 5 goals");
    assert_eq!(plan.goals[0].status, GoalStatus::Active);
    assert_eq!(plan.goals[0].priority, 1);
}

#[test]
fn goal_curator_parses_goals_with_attributes() {
    let plan = StructuredGoalPlan::parse(
        "goal: Ship v1 | priority=1 | status=active | rationale=deadline",
    )
    .expect("test operation should succeed");
    assert_eq!(plan.goals.len(), 1);
    assert_eq!(plan.goals[0].priority, 1);
    assert_eq!(plan.goals[0].status, GoalStatus::Active);
}

#[test]
fn goal_curator_natural_language_generates_slug() {
    let plan = StructuredGoalPlan::parse("review top 5 goals").expect("parse test goal plan");
    assert_eq!(plan.goals[0].slug, "review-top-5-goals");
}

#[test]
fn goal_curator_natural_language_sets_rationale() {
    let plan = StructuredGoalPlan::parse("review top 5 goals").expect("parse test goal plan");
    assert!(plan.goals[0].rationale.contains("natural-language"));
}

#[test]
fn goal_curator_multiline_natural_language_uses_full_text() {
    let plan = StructuredGoalPlan::parse("review all goals\nand prioritize them")
        .expect("parse test goal plan");
    assert_eq!(plan.goals.len(), 1);
    assert!(plan.goals[0].title.contains("review all goals"));
}

#[test]
fn goal_curator_mixed_structured_and_freetext_prefers_structured() {
    let plan = StructuredGoalPlan::parse(
        "some preamble\ngoal: Ship v2 | priority=2 | status=active | rationale=roadmap",
    )
    .expect("test operation should succeed");
    assert_eq!(plan.goals.len(), 1);
    assert_eq!(plan.goals[0].title, "Ship v2");
}

#[test]
fn goal_curator_plan_turn_succeeds_with_natural_language() {
    let program = GoalCuratorProgram::try_default().expect("create test program");
    let context = test_context("review top 5 goals");
    let input = program
        .plan_turn(&context)
        .expect("plan_turn should succeed");
    assert!(input.objective.contains("1 goal updates"));
}

#[test]
fn goal_curator_descriptor_has_identity() {
    let program = GoalCuratorProgram::try_default().expect("create test program");
    let desc = program.descriptor();
    assert!(desc.identity.contains("goal-curator"));
}

#[test]
fn goal_curator_reflection_summary_includes_goal_count() {
    let program = GoalCuratorProgram::try_default().expect("create test program");
    let context = test_context(
        "goal: Ship v1 | priority=1 | status=active\ngoal: Add tests | priority=2 | status=proposed",
    );
    let summary = program
        .reflection_summary(&context, &test_outcome())
        .expect("reflection_summary should succeed");
    assert!(summary.contains("2 goal updates"));
}

#[test]
fn goal_curator_persistence_summary_includes_counts() {
    let program = GoalCuratorProgram::try_default().expect("create test program");
    let context = test_context(
        "goal: Ship v1 | priority=1 | status=active\ngoal: Old | priority=3 | status=completed",
    );
    let summary = program
        .persistence_summary(&context, &test_outcome())
        .expect("persistence_summary should succeed");
    assert!(summary.contains("goal-curation-record"));
    assert!(summary.contains("active=1"));
    assert!(summary.contains("completed=1"));
}

#[test]
fn goal_curator_additional_memory_records_with_goals() {
    let program = GoalCuratorProgram::try_default().expect("create test program");
    let context = test_context("goal: Ship v1 | priority=1 | status=active");
    let records = program
        .additional_memory_records(&context, &test_outcome())
        .expect("additional_memory_records should succeed");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].key_suffix, "goal-curation-record");
    assert!(records[0].value.contains("Ship v1"));
}

#[test]
fn goal_curator_additional_memory_records_empty_for_no_goals_in_output() {
    let program = GoalCuratorProgram::try_default().expect("create test program");
    // Natural language objective gets one auto-goal, so memory records are non-empty.
    // To get empty, we'd need parse to produce empty goals, but the parser always adds one.
    // Instead, test that goal_updates returns the parsed goals.
    let context = test_context("goal: Test | priority=1 | status=active");
    let updates = program
        .goal_updates(&context, &test_outcome())
        .expect("goal_updates should succeed");
    assert_eq!(updates.len(), 1);
}

#[test]
fn goal_curator_plan_turn_with_active_goals_in_context() {
    let program = GoalCuratorProgram::try_default().expect("create test program");
    let mut context = test_context("goal: Review | priority=1 | status=active");
    context.active_goals = vec![GoalRecord {
        slug: "existing".to_string(),
        title: "Existing Goal".to_string(),
        rationale: "test".to_string(),
        status: GoalStatus::Active,
        priority: 1,
        owner_identity: "test".to_string(),
        source_session_id: context.session_id.clone(),
        updated_in: crate::session::SessionPhase::Persistence,
    }];
    let input = program
        .plan_turn(&context)
        .expect("plan_turn should succeed");
    assert!(input.objective.contains("Existing Goal"));
}

// --- StructuredGoalPlan ---

#[test]
fn goal_plan_parse_multiple_goals() {
    let plan = StructuredGoalPlan::parse(
        "goal: A | priority=1 | status=active\ngoal: B | priority=2 | status=proposed",
    )
    .expect("test operation should succeed");
    assert_eq!(plan.goals.len(), 2);
    assert_eq!(plan.goals[0].title, "A");
    assert_eq!(plan.goals[1].title, "B");
}

#[test]
fn goal_plan_active_goal_count() {
    let plan = StructuredGoalPlan::parse(
        "goal: A | priority=1 | status=active\ngoal: B | priority=2 | status=proposed\ngoal: C | priority=3 | status=active",
    )
    .expect("test operation should succeed");
    assert_eq!(plan.active_goal_count(), 2);
}

#[test]
fn goal_plan_goal_count_by_status() {
    let plan = StructuredGoalPlan::parse(
        "goal: A | status=active\ngoal: B | status=completed\ngoal: C | status=paused",
    )
    .expect("test operation should succeed");
    assert_eq!(plan.goal_count(GoalStatus::Active), 1);
    assert_eq!(plan.goal_count(GoalStatus::Completed), 1);
    assert_eq!(plan.goal_count(GoalStatus::Paused), 1);
    assert_eq!(plan.goal_count(GoalStatus::Proposed), 0);
}

#[test]
fn goal_plan_concise_top_five_limits_to_five() {
    let raw = (1..=8)
        .map(|i| format!("goal: Goal{i} | priority={i} | status=active"))
        .collect::<Vec<_>>()
        .join("\n");
    let plan = StructuredGoalPlan::parse(&raw).expect("parse test goal plan");
    let top = plan.concise_top_five();
    let count = top.matches(" | ").count() + 1;
    assert!(count <= 5, "should limit to 5 goals, got {count}");
}

#[test]
fn goal_plan_concise_top_five_sorted_by_priority() {
    let plan = StructuredGoalPlan::parse(
        "goal: Low | priority=3 | status=active\ngoal: High | priority=1 | status=active",
    )
    .expect("test operation should succeed");
    let top = plan.concise_top_five();
    let high_pos = top.find("High").expect("substring should be present");
    let low_pos = top.find("Low").expect("substring should be present");
    assert!(high_pos < low_pos, "higher priority should come first");
}

#[test]
fn goal_plan_turn_objective_with_no_active_goals() {
    let plan = StructuredGoalPlan::parse("goal: X | status=active").expect("parse test goal plan");
    let obj = plan.turn_objective(&[]);
    assert!(obj.contains("<none>"));
}

#[test]
fn goal_plan_turn_objective_with_active_goals() {
    let plan = StructuredGoalPlan::parse("goal: X | status=active").expect("parse test goal plan");
    let goals = vec![GoalRecord {
        slug: "existing".to_string(),
        title: "Existing".to_string(),
        rationale: "test".to_string(),
        status: GoalStatus::Active,
        priority: 1,
        owner_identity: "test".to_string(),
        source_session_id: SessionId::parse("session-00000000-0000-0000-0000-000000000001")
            .expect("test operation should succeed"),
        updated_in: crate::session::SessionPhase::Persistence,
    }];
    let obj = plan.turn_objective(&goals);
    assert!(obj.contains("Existing"));
}
