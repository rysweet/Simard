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
                    u8::try_from(plan.goals.len() + 1).unwrap_or(u8::MAX),
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
mod tests;
