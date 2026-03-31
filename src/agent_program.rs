use crate::base_types::{BaseTypeId, BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::{SimardError, SimardResult};
use crate::goals::{GoalRecord, GoalStatus, GoalUpdate};
use crate::identity::OperatingMode;
use crate::improvements::ImprovementPromotionPlan;
use crate::memory::MemoryScope;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::sanitization::objective_metadata;
use crate::session::SessionId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProgramContext {
    pub session_id: SessionId,
    pub identity_name: String,
    pub mode: OperatingMode,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
    pub runtime_node: RuntimeNodeId,
    pub mailbox_address: RuntimeAddress,
    pub objective: String,
    pub active_goals: Vec<GoalRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProgramMemoryRecord {
    pub key_suffix: String,
    pub scope: MemoryScope,
    pub value: String,
}

pub trait AgentProgram: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn plan_turn(&self, context: &AgentProgramContext) -> SimardResult<BaseTypeTurnInput>;

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String>;

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String>;

    fn additional_memory_records(
        &self,
        _context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<AgentProgramMemoryRecord>> {
        Ok(Vec::new())
    }

    fn goal_updates(
        &self,
        _context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<GoalUpdate>> {
        Ok(Vec::new())
    }
}

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
        Ok(BaseTypeTurnInput { objective })
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let objective_summary = objective_metadata(&context.objective);
        Ok(format!(
            "Agent program '{}' completed '{}' through '{}' on '{}' from '{}' with {} and {} active top goals in scope.",
            self.descriptor.identity,
            context.mode,
            context.selected_base_type,
            context.topology,
            context.runtime_node,
            objective_summary,
            context.active_goals.len(),
        ) + &format!(" Outcome summary: {}.", outcome.execution_summary))
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

#[derive(Debug)]
pub struct MeetingFacilitatorProgram {
    descriptor: BackendDescriptor,
}

impl MeetingFacilitatorProgram {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "agent-program::meeting-facilitator",
                "runtime-port:agent-program",
                Freshness::now()?,
            ),
        })
    }
}

impl AgentProgram for MeetingFacilitatorProgram {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn plan_turn(&self, context: &AgentProgramContext) -> SimardResult<BaseTypeTurnInput> {
        let notes = StructuredMeetingNotes::parse(&context.objective)?;
        Ok(BaseTypeTurnInput {
            objective: notes.turn_objective(),
        })
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let notes = StructuredMeetingNotes::parse(&context.objective)?;
        Ok(format!(
            "Meeting facilitator '{}' captured {} decisions, {} risks, {} next steps, and {} open questions for agenda '{}' through '{}' on '{}' from '{}'. Goal updates captured: {}. Outcome summary: {}.",
            self.descriptor.identity,
            notes.decisions.len(),
            notes.risks.len(),
            notes.next_steps.len(),
            notes.open_questions.len(),
            notes.agenda,
            context.selected_base_type,
            context.topology,
            context.runtime_node,
            notes.goals.len(),
            outcome.execution_summary,
        ))
    }

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let notes = StructuredMeetingNotes::parse(&context.objective)?;
        Ok(format!(
            "meeting-record | {} | selected-base-type={} | topology={} | goal-updates={} | outcome={}",
            notes.concise_record(),
            context.selected_base_type,
            context.topology,
            notes.goals.len(),
            outcome.execution_summary,
        ))
    }

    fn additional_memory_records(
        &self,
        context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<AgentProgramMemoryRecord>> {
        let notes = StructuredMeetingNotes::parse(&context.objective)?;
        if !notes.has_persistable_outputs() {
            return Ok(Vec::new());
        }

        Ok(vec![AgentProgramMemoryRecord {
            key_suffix: "decision-record".to_string(),
            scope: MemoryScope::Decision,
            value: notes.concise_record(),
        }])
    }

    fn goal_updates(
        &self,
        context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<GoalUpdate>> {
        let notes = StructuredMeetingNotes::parse(&context.objective)?;
        Ok(notes.goals)
    }
}

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
        Ok(BaseTypeTurnInput {
            objective: plan.turn_objective(&context.active_goals),
        })
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
        Ok(BaseTypeTurnInput {
            objective: format!(
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
            ),
        })
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct StructuredMeetingNotes {
    agenda: String,
    updates: Vec<String>,
    decisions: Vec<String>,
    risks: Vec<String>,
    next_steps: Vec<String>,
    open_questions: Vec<String>,
    goals: Vec<GoalUpdate>,
}

impl StructuredMeetingNotes {
    fn parse(raw: &str) -> SimardResult<Self> {
        let mut notes = Self::default();
        let mut agenda_fragments = Vec::new();

        for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let Some((label, value)) = line.split_once(':') else {
                agenda_fragments.push(line.to_string());
                continue;
            };

            let label = label.trim().to_ascii_lowercase();
            let value = value.trim();
            if value.is_empty() {
                continue;
            }

            match label.as_str() {
                "agenda" | "topic" | "goal" if !value.contains('|') => {
                    if notes.agenda.is_empty() {
                        notes.agenda = value.to_string();
                    } else {
                        notes.agenda.push_str(" / ");
                        notes.agenda.push_str(value);
                    }
                }
                "update" | "status" => notes.updates.push(value.to_string()),
                "decision" => notes.decisions.push(value.to_string()),
                "risk" => notes.risks.push(value.to_string()),
                "next-step" | "next_step" | "action" | "action-item" => {
                    notes.next_steps.push(value.to_string())
                }
                "open-question" | "open_question" | "question" => {
                    notes.open_questions.push(value.to_string())
                }
                "goal" => notes
                    .goals
                    .push(parse_goal_directive(value, (notes.goals.len() + 1) as u8)?),
                _ => agenda_fragments.push(line.to_string()),
            }
        }

        if notes.agenda.is_empty() {
            notes.agenda = if agenda_fragments.is_empty() {
                objective_metadata(raw)
            } else {
                agenda_fragments.join(" / ")
            };
        }

        Ok(notes)
    }

    fn has_persistable_outputs(&self) -> bool {
        !self.updates.is_empty()
            || !self.decisions.is_empty()
            || !self.risks.is_empty()
            || !self.next_steps.is_empty()
            || !self.open_questions.is_empty()
            || !self.goals.is_empty()
    }

    fn turn_objective(&self) -> String {
        format!(
            "Facilitate meeting agenda '{}' and preserve concise updates={}, decisions={}, risks={}, next_steps={}, open_questions={}, goal_updates={}.",
            self.agenda,
            self.updates.len(),
            self.decisions.len(),
            self.risks.len(),
            self.next_steps.len(),
            self.open_questions.len(),
            self.goals.len(),
        )
    }

    fn concise_record(&self) -> String {
        format!(
            "agenda={}; updates={}; decisions={}; risks={}; next_steps={}; open_questions={}; goals={}",
            self.agenda,
            format_items(&self.updates),
            format_items(&self.decisions),
            format_items(&self.risks),
            format_items(&self.next_steps),
            format_items(&self.open_questions),
            format_goal_items(&self.goals),
        )
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
            return Err(SimardError::InvalidGoalRecord {
                field: "goal".to_string(),
                reason: "at least one structured 'goal:' line is required".to_string(),
            });
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
        let mut goals = self.goals.clone();
        goals.sort_by(|left, right| {
            left.status
                .cmp(&right.status)
                .then(left.priority.cmp(&right.priority))
                .then(left.title.cmp(&right.title))
        });
        goals
            .into_iter()
            .filter(|goal| goal.status == GoalStatus::Active)
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

fn parse_goal_directive(raw: &str, default_priority: u8) -> SimardResult<GoalUpdate> {
    let mut segments = raw
        .split('|')
        .map(str::trim)
        .filter(|segment| !segment.is_empty());
    let title = segments
        .next()
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "goal".to_string(),
            reason: "goal entries must include a title before any attributes".to_string(),
        })?;
    let mut priority = default_priority.max(1);
    let mut status = GoalStatus::Active;
    let mut rationale = "captured as a durable Simard priority".to_string();

    for segment in segments {
        let (key, value) =
            segment
                .split_once('=')
                .ok_or_else(|| SimardError::InvalidGoalRecord {
                    field: "goal".to_string(),
                    reason: format!("goal attribute '{segment}' must look like key=value"),
                })?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            return Err(SimardError::InvalidGoalRecord {
                field: key,
                reason: "goal attribute values cannot be empty".to_string(),
            });
        }
        match key.as_str() {
            "priority" => {
                priority = value
                    .parse::<u8>()
                    .map_err(|_| SimardError::InvalidGoalRecord {
                        field: "priority".to_string(),
                        reason: format!("goal priority '{value}' is not a valid integer"),
                    })?;
            }
            "status" => status = parse_goal_status(value)?,
            "rationale" => rationale = value.to_string(),
            other => {
                return Err(SimardError::InvalidGoalRecord {
                    field: other.to_string(),
                    reason: "supported goal attributes are priority=, status=, and rationale="
                        .to_string(),
                });
            }
        }
    }

    GoalUpdate::new(title, rationale, status, priority)
}

fn parse_goal_status(value: &str) -> SimardResult<GoalStatus> {
    match value.trim().to_ascii_lowercase().as_str() {
        "candidate" => Ok(GoalStatus::Proposed),
        "hold" | "holding" => Ok(GoalStatus::Paused),
        "done" => Ok(GoalStatus::Completed),
        other => GoalStatus::parse(other).ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "status".to_string(),
            reason: format!(
                "unsupported goal status '{other}'; expected active, proposed, paused, or completed"
            ),
        }),
    }
}

fn format_items(items: &[String]) -> String {
    if items.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", items.join(" | "))
    }
}

fn format_goal_items(items: &[GoalUpdate]) -> String {
    if items.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            items
                .iter()
                .map(|goal| format!(
                    "p{}:{}:{}:{}",
                    goal.priority, goal.status, goal.title, goal.rationale
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;

    fn test_context(objective: &str) -> AgentProgramContext {
        AgentProgramContext {
            session_id: SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap(),
            identity_name: "test-identity".to_string(),
            mode: OperatingMode::Engineer,
            selected_base_type: BaseTypeId::new("local-harness"),
            topology: RuntimeTopology::SingleProcess,
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
            objective: objective.to_string(),
            active_goals: vec![],
        }
    }

    fn test_outcome() -> BaseTypeOutcome {
        BaseTypeOutcome {
            plan: "test plan".to_string(),
            execution_summary: "executed successfully".to_string(),
            evidence: vec!["evidence-1".to_string()],
        }
    }

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
    fn meeting_facilitator_parses_structured_notes() {
        let program = MeetingFacilitatorProgram::try_default().unwrap();
        let context = test_context("agenda: Sprint planning\ndecision: Ship by Friday");
        let input = program.plan_turn(&context).unwrap();
        assert!(input.objective.contains("Sprint planning"));
        assert!(input.objective.contains("decisions=1"));
    }

    #[test]
    fn meeting_facilitator_reflection_includes_counts() {
        let program = MeetingFacilitatorProgram::try_default().unwrap();
        let context = test_context("agenda: Retro\nrisk: Scope creep\nnext-step: Write tests");
        let summary = program
            .reflection_summary(&context, &test_outcome())
            .unwrap();
        assert!(summary.contains("1 risks"));
        assert!(summary.contains("1 next steps"));
    }

    #[test]
    fn goal_curator_rejects_empty_input() {
        let err = StructuredGoalPlan::parse("no goal lines here").unwrap_err();
        assert!(matches!(err, SimardError::InvalidGoalRecord { .. }));
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
}
