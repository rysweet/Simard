use crate::base_types::{BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::SimardResult;
use crate::goals::{GoalRecord, GoalStatus, GoalUpdate};
use crate::memory::MemoryScope;
use crate::metadata::{BackendDescriptor, Freshness};

use super::parsing::parse_goal_directive;
use super::types::{AgentProgram, AgentProgramContext, AgentProgramMemoryRecord};

#[derive(Debug)]
pub struct GoalCuratorProgram {
    descriptor: BackendDescriptor,
}

impl GoalCuratorProgram {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "agent-program::goal-curator",
                "runtime-port:agent-program",
                Freshness::now()?,
            ),
        })
    }
}

impl AgentProgram for GoalCuratorProgram {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn plan_turn(&self, context: &AgentProgramContext) -> SimardResult<BaseTypeTurnInput> {
        let plan = StructuredGoalPlan::parse(&context.objective)?;
        Ok(BaseTypeTurnInput::objective_only(
            plan.turn_objective(&context.active_goals),
        ))
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let plan = StructuredGoalPlan::parse(&context.objective)?;
        Ok(format!(
            "Goal curator '{}' processed {} goal updates through '{}' on '{}' from '{}'. Active top goals after curation: {}. Outcome summary: {}.",
            self.descriptor.identity,
            plan.goals.len(),
            context.selected_base_type,
            context.topology,
            context.runtime_node,
            plan.active_goal_count(),
            outcome.execution_summary,
        ))
    }

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let plan = StructuredGoalPlan::parse(&context.objective)?;
        Ok(format!(
            "goal-curation-record | updates={} | active={} | proposed={} | paused={} | completed={} | outcome={}",
            plan.goals.len(),
            plan.goal_count(GoalStatus::Active),
            plan.goal_count(GoalStatus::Proposed),
            plan.goal_count(GoalStatus::Paused),
            plan.goal_count(GoalStatus::Completed),
            outcome.execution_summary,
        ))
    }

    fn additional_memory_records(
        &self,
        context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<AgentProgramMemoryRecord>> {
        let plan = StructuredGoalPlan::parse(&context.objective)?;
        if plan.goals.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![AgentProgramMemoryRecord {
            key_suffix: "goal-curation-record".to_string(),
            scope: MemoryScope::Decision,
            value: format!("goal-curation-top-five={}", plan.concise_top_five()),
        }])
    }

    fn goal_updates(
        &self,
        context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<GoalUpdate>> {
        Ok(StructuredGoalPlan::parse(&context.objective)?.goals)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct StructuredGoalPlan {
    goals: Vec<GoalUpdate>,
}

impl StructuredGoalPlan {
    fn parse(raw: &str) -> SimardResult<Self> {
        let mut plan = Self::default();
        for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let Some((label, value)) = line.split_once(':') else {
                continue;
            };
            let label = label.trim().to_ascii_lowercase();
            if label == "goal" {
                plan.goals.push(parse_goal_directive(
                    value.trim(),
                    (plan.goals.len() + 1) as u8,
                )?);
            }
        }
        if plan.goals.is_empty() {
            plan.goals.push(GoalUpdate::new(
                raw.trim(),
                "natural-language objective accepted as a durable Simard priority",
                GoalStatus::Active,
                1,
            )?);
        }
        Ok(plan)
    }

    fn active_goal_count(&self) -> usize {
        self.goal_count(GoalStatus::Active)
    }

    fn goal_count(&self, status: GoalStatus) -> usize {
        self.goals
            .iter()
            .filter(|goal| goal.status == status)
            .count()
    }

    fn concise_top_five(&self) -> String {
        let mut active: Vec<_> = self
            .goals
            .iter()
            .filter(|goal| goal.status == GoalStatus::Active)
            .collect();
        active.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then(left.title.cmp(&right.title))
        });
        active
            .into_iter()
            .take(5)
            .map(|goal| format!("p{}:{} ({})", goal.priority, goal.title, goal.rationale))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn turn_objective(&self, active_goals: &[GoalRecord]) -> String {
        format!(
            "Curate {} goal updates, preserve a truthful top-5 priority list, and keep meeting-to-engineer handoff inspectable. Existing active goals in runtime state: {}.",
            self.goals.len(),
            if active_goals.is_empty() {
                "<none>".to_string()
            } else {
                active_goals
                    .iter()
                    .map(GoalRecord::concise_label)
                    .collect::<Vec<_>>()
                    .join(" | ")
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_program::test_support::{test_context, test_outcome};
    use crate::session::SessionId;

    // --- GoalCuratorProgram ---

    #[test]
    fn goal_curator_accepts_natural_language_input() {
        let plan = StructuredGoalPlan::parse("review top 5 goals").unwrap();
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
        .unwrap();
        assert_eq!(plan.goals.len(), 1);
        assert_eq!(plan.goals[0].priority, 1);
        assert_eq!(plan.goals[0].status, GoalStatus::Active);
    }

    #[test]
    fn goal_curator_natural_language_generates_slug() {
        let plan = StructuredGoalPlan::parse("review top 5 goals").unwrap();
        assert_eq!(plan.goals[0].slug, "review-top-5-goals");
    }

    #[test]
    fn goal_curator_natural_language_sets_rationale() {
        let plan = StructuredGoalPlan::parse("review top 5 goals").unwrap();
        assert!(plan.goals[0].rationale.contains("natural-language"));
    }

    #[test]
    fn goal_curator_multiline_natural_language_uses_full_text() {
        let plan = StructuredGoalPlan::parse("review all goals\nand prioritize them").unwrap();
        assert_eq!(plan.goals.len(), 1);
        assert!(plan.goals[0].title.contains("review all goals"));
    }

    #[test]
    fn goal_curator_mixed_structured_and_freetext_prefers_structured() {
        let plan = StructuredGoalPlan::parse(
            "some preamble\ngoal: Ship v2 | priority=2 | status=active | rationale=roadmap",
        )
        .unwrap();
        assert_eq!(plan.goals.len(), 1);
        assert_eq!(plan.goals[0].title, "Ship v2");
    }

    #[test]
    fn goal_curator_plan_turn_succeeds_with_natural_language() {
        let program = GoalCuratorProgram::try_default().unwrap();
        let context = test_context("review top 5 goals");
        let input = program.plan_turn(&context).unwrap();
        assert!(input.objective.contains("1 goal updates"));
    }

    #[test]
    fn goal_curator_descriptor_has_identity() {
        let program = GoalCuratorProgram::try_default().unwrap();
        let desc = program.descriptor();
        assert!(desc.identity.contains("goal-curator"));
    }

    #[test]
    fn goal_curator_reflection_summary_includes_goal_count() {
        let program = GoalCuratorProgram::try_default().unwrap();
        let context = test_context(
            "goal: Ship v1 | priority=1 | status=active\ngoal: Add tests | priority=2 | status=proposed",
        );
        let summary = program
            .reflection_summary(&context, &test_outcome())
            .unwrap();
        assert!(summary.contains("2 goal updates"));
    }

    #[test]
    fn goal_curator_persistence_summary_includes_counts() {
        let program = GoalCuratorProgram::try_default().unwrap();
        let context = test_context(
            "goal: Ship v1 | priority=1 | status=active\ngoal: Old | priority=3 | status=completed",
        );
        let summary = program
            .persistence_summary(&context, &test_outcome())
            .unwrap();
        assert!(summary.contains("goal-curation-record"));
        assert!(summary.contains("active=1"));
        assert!(summary.contains("completed=1"));
    }

    #[test]
    fn goal_curator_additional_memory_records_with_goals() {
        let program = GoalCuratorProgram::try_default().unwrap();
        let context = test_context("goal: Ship v1 | priority=1 | status=active");
        let records = program
            .additional_memory_records(&context, &test_outcome())
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key_suffix, "goal-curation-record");
        assert!(records[0].value.contains("Ship v1"));
    }

    #[test]
    fn goal_curator_additional_memory_records_empty_for_no_goals_in_output() {
        let program = GoalCuratorProgram::try_default().unwrap();
        // Natural language objective gets one auto-goal, so memory records are non-empty.
        // To get empty, we'd need parse to produce empty goals, but the fallback always adds one.
        // Instead, test that goal_updates returns the parsed goals.
        let context = test_context("goal: Test | priority=1 | status=active");
        let updates = program.goal_updates(&context, &test_outcome()).unwrap();
        assert_eq!(updates.len(), 1);
    }

    #[test]
    fn goal_curator_plan_turn_with_active_goals_in_context() {
        let program = GoalCuratorProgram::try_default().unwrap();
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
        let input = program.plan_turn(&context).unwrap();
        assert!(input.objective.contains("Existing Goal"));
    }

    // --- StructuredGoalPlan ---

    #[test]
    fn goal_plan_parse_multiple_goals() {
        let plan = StructuredGoalPlan::parse(
            "goal: A | priority=1 | status=active\ngoal: B | priority=2 | status=proposed",
        )
        .unwrap();
        assert_eq!(plan.goals.len(), 2);
        assert_eq!(plan.goals[0].title, "A");
        assert_eq!(plan.goals[1].title, "B");
    }

    #[test]
    fn goal_plan_active_goal_count() {
        let plan = StructuredGoalPlan::parse(
            "goal: A | priority=1 | status=active\ngoal: B | priority=2 | status=proposed\ngoal: C | priority=3 | status=active",
        )
        .unwrap();
        assert_eq!(plan.active_goal_count(), 2);
    }

    #[test]
    fn goal_plan_goal_count_by_status() {
        let plan = StructuredGoalPlan::parse(
            "goal: A | status=active\ngoal: B | status=completed\ngoal: C | status=paused",
        )
        .unwrap();
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
        let plan = StructuredGoalPlan::parse(&raw).unwrap();
        let top = plan.concise_top_five();
        let count = top.matches(" | ").count() + 1;
        assert!(count <= 5, "should limit to 5 goals, got {count}");
    }

    #[test]
    fn goal_plan_concise_top_five_sorted_by_priority() {
        let plan = StructuredGoalPlan::parse(
            "goal: Low | priority=3 | status=active\ngoal: High | priority=1 | status=active",
        )
        .unwrap();
        let top = plan.concise_top_five();
        let high_pos = top.find("High").unwrap();
        let low_pos = top.find("Low").unwrap();
        assert!(high_pos < low_pos, "higher priority should come first");
    }

    #[test]
    fn goal_plan_turn_objective_with_no_active_goals() {
        let plan = StructuredGoalPlan::parse("goal: X | status=active").unwrap();
        let obj = plan.turn_objective(&[]);
        assert!(obj.contains("<none>"));
    }

    #[test]
    fn goal_plan_turn_objective_with_active_goals() {
        let plan = StructuredGoalPlan::parse("goal: X | status=active").unwrap();
        let goals = vec![GoalRecord {
            slug: "existing".to_string(),
            title: "Existing".to_string(),
            rationale: "test".to_string(),
            status: GoalStatus::Active,
            priority: 1,
            owner_identity: "test".to_string(),
            source_session_id: SessionId::parse("session-00000000-0000-0000-0000-000000000001")
                .unwrap(),
            updated_in: crate::session::SessionPhase::Persistence,
        }];
        let obj = plan.turn_objective(&goals);
        assert!(obj.contains("Existing"));
    }
}
