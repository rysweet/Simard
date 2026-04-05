use crate::base_types::{BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::SimardResult;

use crate::metadata::{BackendDescriptor, Freshness};
use crate::sanitization::objective_metadata;

use super::types::{AgentProgram, AgentProgramContext};

#[derive(Debug)]
pub struct ObjectiveRelayProgram {
    descriptor: BackendDescriptor,
}

impl ObjectiveRelayProgram {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "agent-program::objective-relay",
                "runtime-port:agent-program",
                Freshness::now()?,
            ),
        })
    }
}

impl AgentProgram for ObjectiveRelayProgram {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn plan_turn(&self, context: &AgentProgramContext) -> SimardResult<BaseTypeTurnInput> {
        let mut objective = context.objective.clone();
        if !context.active_goals.is_empty() {
            objective.push_str("\n\nActive top goals:\n");
            for goal in &context.active_goals {
                objective.push_str("- ");
                objective.push_str(&goal.concise_label());
                objective.push('\n');
            }
        }
        Ok(BaseTypeTurnInput::objective_only(objective))
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let objective_summary = objective_metadata(&context.objective);
        Ok(format!(
            "Agent program '{}' completed '{}' through '{}' on '{}' from '{}' with {} and {} active top goals in scope. Outcome summary: {}.",
            self.descriptor.identity,
            context.mode,
            context.selected_base_type,
            context.topology,
            context.runtime_node,
            objective_summary,
            context.active_goals.len(),
            outcome.execution_summary,
        ))
    }

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        Ok(format!(
            "{} | active-goals={} | {} | {}",
            objective_metadata(&context.objective),
            context.active_goals.len(),
            outcome.plan,
            outcome.execution_summary,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_program::test_support::{test_context, test_outcome};
    use crate::goals::{GoalRecord, GoalStatus};

    #[test]
    fn objective_relay_plan_turn_passes_objective_through() {
        let program = ObjectiveRelayProgram::try_default().unwrap();
        let context = test_context("build the widget");
        let input = program.plan_turn(&context).unwrap();
        assert!(input.objective.contains("build the widget"));
    }

    #[test]
    fn objective_relay_appends_active_goals_to_objective() {
        let program = ObjectiveRelayProgram::try_default().unwrap();
        let mut context = test_context("build it");
        context.active_goals = vec![GoalRecord {
            slug: "ship-v1".to_string(),
            title: "Ship v1".to_string(),
            rationale: "deadline".to_string(),
            status: GoalStatus::Active,
            priority: 1,
            owner_identity: "test".to_string(),
            source_session_id: context.session_id.clone(),
            updated_in: crate::session::SessionPhase::Persistence,
        }];
        let input = program.plan_turn(&context).unwrap();
        assert!(input.objective.contains("Active top goals:"));
        assert!(input.objective.contains("Ship v1"));
    }

    #[test]
    fn objective_relay_reflection_summary_includes_identity() {
        let program = ObjectiveRelayProgram::try_default().unwrap();
        let context = test_context("test objective");
        let summary = program
            .reflection_summary(&context, &test_outcome())
            .unwrap();
        assert!(summary.contains(&program.descriptor().identity));
        assert!(summary.contains("Outcome summary:"));
    }

    #[test]
    fn objective_relay_persistence_summary_includes_metadata() {
        let program = ObjectiveRelayProgram::try_default().unwrap();
        let context = test_context("test objective");
        let summary = program
            .persistence_summary(&context, &test_outcome())
            .unwrap();
        assert!(summary.contains("objective-metadata("));
        assert!(summary.contains("test plan"));
    }

    #[test]
    fn objective_relay_descriptor_has_identity() {
        let program = ObjectiveRelayProgram::try_default().unwrap();
        let desc = program.descriptor();
        assert!(
            desc.identity.contains("objective-relay"),
            "descriptor identity should mention objective-relay"
        );
    }

    #[test]
    fn objective_relay_plan_turn_empty_goals_no_goals_section() {
        let program = ObjectiveRelayProgram::try_default().unwrap();
        let context = test_context("simple task");
        let input = program.plan_turn(&context).unwrap();
        assert!(!input.objective.contains("Active top goals:"));
        assert_eq!(input.objective, "simple task");
    }

    #[test]
    fn objective_relay_additional_memory_records_default_empty() {
        let program = ObjectiveRelayProgram::try_default().unwrap();
        let context = test_context("test");
        let records = program
            .additional_memory_records(&context, &test_outcome())
            .unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn objective_relay_goal_updates_default_empty() {
        let program = ObjectiveRelayProgram::try_default().unwrap();
        let context = test_context("test");
        let updates = program.goal_updates(&context, &test_outcome()).unwrap();
        assert!(updates.is_empty());
    }
}
