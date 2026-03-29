use crate::base_types::{BaseTypeId, BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::SimardResult;
use crate::identity::OperatingMode;
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
        Ok(BaseTypeTurnInput {
            objective: context.objective.clone(),
        })
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let objective_summary = objective_metadata(&context.objective);
        Ok(format!(
            "Agent program '{}' completed '{}' through '{}' on '{}' from '{}' with {}.",
            self.descriptor.identity,
            context.mode,
            context.selected_base_type,
            context.topology,
            context.runtime_node,
            objective_summary,
        ) + &format!(" Outcome summary: {}.", outcome.execution_summary))
    }

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        Ok(format!(
            "{} | {} | {}",
            objective_metadata(&context.objective),
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
        let notes = StructuredMeetingNotes::parse(&context.objective);
        Ok(BaseTypeTurnInput {
            objective: notes.turn_objective(),
        })
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let notes = StructuredMeetingNotes::parse(&context.objective);
        Ok(format!(
            "Meeting facilitator '{}' captured {} decisions, {} risks, {} next steps, and {} open questions for agenda '{}' through '{}' on '{}' from '{}'. Outcome summary: {}.",
            self.descriptor.identity,
            notes.decisions.len(),
            notes.risks.len(),
            notes.next_steps.len(),
            notes.open_questions.len(),
            notes.agenda,
            context.selected_base_type,
            context.topology,
            context.runtime_node,
            outcome.execution_summary,
        ))
    }

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let notes = StructuredMeetingNotes::parse(&context.objective);
        Ok(format!(
            "meeting-record | {} | selected-base-type={} | topology={} | outcome={}",
            notes.concise_record(),
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
        let notes = StructuredMeetingNotes::parse(&context.objective);
        if !notes.has_persistable_outputs() {
            return Ok(Vec::new());
        }

        Ok(vec![AgentProgramMemoryRecord {
            key_suffix: "decision-record".to_string(),
            scope: MemoryScope::Decision,
            value: notes.concise_record(),
        }])
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
}

impl StructuredMeetingNotes {
    fn parse(raw: &str) -> Self {
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
                "agenda" | "topic" | "goal" => {
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

        notes
    }

    fn has_persistable_outputs(&self) -> bool {
        !self.updates.is_empty()
            || !self.decisions.is_empty()
            || !self.risks.is_empty()
            || !self.next_steps.is_empty()
            || !self.open_questions.is_empty()
    }

    fn turn_objective(&self) -> String {
        format!(
            "Facilitate meeting agenda '{}' and preserve concise updates={}, decisions={}, risks={}, next_steps={}, open_questions={}.",
            self.agenda,
            self.updates.len(),
            self.decisions.len(),
            self.risks.len(),
            self.next_steps.len(),
            self.open_questions.len(),
        )
    }

    fn concise_record(&self) -> String {
        format!(
            "agenda={}; updates={}; decisions={}; risks={}; next_steps={}; open_questions={}",
            self.agenda,
            format_items(&self.updates),
            format_items(&self.decisions),
            format_items(&self.risks),
            format_items(&self.next_steps),
            format_items(&self.open_questions),
        )
    }
}

fn format_items(items: &[String]) -> String {
    if items.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", items.join(" | "))
    }
}
