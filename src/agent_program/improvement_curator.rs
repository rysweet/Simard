use crate::base_types::{BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::SimardResult;
use crate::goals::{GoalRecord, GoalUpdate};
use crate::improvements::ImprovementPromotionPlan;
use crate::memory::MemoryScope;
use crate::metadata::{BackendDescriptor, Freshness};

use super::types::{AgentProgram, AgentProgramContext, AgentProgramMemoryRecord};

#[derive(Debug)]
pub struct ImprovementCuratorProgram {
    descriptor: BackendDescriptor,
}

impl ImprovementCuratorProgram {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "agent-program::improvement-curator",
                "runtime-port:agent-program",
                Freshness::now()?,
            ),
        })
    }
}

impl AgentProgram for ImprovementCuratorProgram {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn plan_turn(&self, context: &AgentProgramContext) -> SimardResult<BaseTypeTurnInput> {
        let plan = ImprovementPromotionPlan::parse(&context.objective)?;
        Ok(BaseTypeTurnInput::objective_only(format!(
            "Review '{}' for '{}' contains {} proposal(s). Approve {} proposal(s), defer {} proposal(s), keep the promotion loop operator-reviewable, and preserve truthful durable priorities. Existing active goals in runtime state: {}.",
            plan.review_id,
            if plan.review_target.trim().is_empty() {
                "unknown-target".to_string()
            } else {
                plan.review_target.clone()
            },
            plan.proposals.len(),
            plan.approvals.len(),
            plan.deferrals.len(),
            if context.active_goals.is_empty() {
                "<none>".to_string()
            } else {
                context
                    .active_goals
                    .iter()
                    .map(GoalRecord::concise_label)
                    .collect::<Vec<_>>()
                    .join(" | ")
            },
        )))
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let plan = ImprovementPromotionPlan::parse(&context.objective)?;
        Ok(format!(
            "Improvement curator '{}' reviewed '{}' for target '{}' through '{}' on '{}' from '{}'. Approved {} proposal(s), deferred {}, and preserved {} active runtime goals in scope. Outcome summary: {}.",
            self.descriptor.identity,
            plan.review_id,
            if plan.review_target.trim().is_empty() {
                "unknown-target".to_string()
            } else {
                plan.review_target.clone()
            },
            context.selected_base_type,
            context.topology,
            context.runtime_node,
            plan.approvals.len(),
            plan.deferrals.len(),
            context.active_goals.len(),
            outcome.execution_summary,
        ))
    }

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let plan = ImprovementPromotionPlan::parse(&context.objective)?;
        Ok(format!(
            "improvement-curation-record | review={} | target={} | approvals={} | deferrals={} | approved_goals=[{}] | deferred=[{}] | selected-base-type={} | topology={} | outcome={}",
            plan.review_id,
            if plan.review_target.trim().is_empty() {
                "unknown-target".to_string()
            } else {
                plan.review_target.clone()
            },
            plan.approvals.len(),
            plan.deferrals.len(),
            plan.approval_summaries().join(" | "),
            plan.deferral_summaries().join(" | "),
            context.selected_base_type,
            context.topology,
            outcome.execution_summary,
        ))
    }

    fn additional_memory_records(
        &self,
        context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<AgentProgramMemoryRecord>> {
        let plan = ImprovementPromotionPlan::parse(&context.objective)?;
        Ok(vec![AgentProgramMemoryRecord {
            key_suffix: "improvement-curation-record".to_string(),
            scope: MemoryScope::Decision,
            value: format!(
                "review={} target={} approvals=[{}] deferred=[{}]",
                plan.review_id,
                if plan.review_target.trim().is_empty() {
                    "unknown-target".to_string()
                } else {
                    plan.review_target.clone()
                },
                plan.approval_summaries().join(" | "),
                plan.deferral_summaries().join(" | "),
            ),
        }])
    }

    fn goal_updates(
        &self,
        context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<GoalUpdate>> {
        ImprovementPromotionPlan::parse(&context.objective)?.approved_goal_updates()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_program::test_support::test_context;
    use crate::goals::{GoalRecord, GoalStatus};

    #[test]
    fn improvement_curator_descriptor_has_identity() {
        let program = ImprovementCuratorProgram::try_default().unwrap();
        let desc = program.descriptor();
        assert!(desc.identity.contains("improvement-curator"));
    }

    #[test]
    fn improvement_curator_plan_turn_with_empty_review_target() {
        let program = ImprovementCuratorProgram::try_default().unwrap();
        let context = test_context(
            "review-id: review-001\nreview-target:   \nproposal: Fix flaky test | category=quality | rationale=stabilize CI | suggested_change=add retry logic\napprove: Fix flaky test | priority=2 | status=proposed | rationale=stabilize CI",
        );
        let input = program.plan_turn(&context).unwrap();
        assert!(input.objective.contains("unknown-target"));
    }

    #[test]
    fn improvement_curator_plan_turn_with_no_active_goals() {
        let program = ImprovementCuratorProgram::try_default().unwrap();
        let context = test_context(
            "review-id: review-001\nreview-target: prompt-system\nproposal: Improve prompt | category=quality | rationale=better output | suggested_change=rewrite system prompt\ndefer: Improve prompt | rationale=low priority",
        );
        let input = program.plan_turn(&context).unwrap();
        assert!(input.objective.contains("<none>"));
    }

    #[test]
    fn improvement_curator_plan_turn_with_active_goals() {
        let program = ImprovementCuratorProgram::try_default().unwrap();
        let mut context = test_context(
            "review-id: review-002\nreview-target: gym-eval\nproposal: Add scenario | category=coverage | rationale=more tests | suggested_change=write new gym scenarios\napprove: Add scenario | priority=1 | status=active | rationale=more tests",
        );
        context.active_goals = vec![GoalRecord {
            slug: "improve-scores".to_string(),
            title: "Improve Scores".to_string(),
            rationale: "low gym".to_string(),
            status: GoalStatus::Active,
            priority: 1,
            owner_identity: "test".to_string(),
            source_session_id: context.session_id.clone(),
            updated_in: crate::session::SessionPhase::Persistence,
        }];
        let input = program.plan_turn(&context).unwrap();
        assert!(input.objective.contains("Improve Scores"));
    }
}
