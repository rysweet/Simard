use crate::base_types::{BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::SimardResult;
use crate::goals::GoalUpdate;
use crate::memory::MemoryScope;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::sanitization::objective_metadata;

use super::parsing::{format_goal_items, format_items, parse_goal_directive};
use super::types::{AgentProgram, AgentProgramContext, AgentProgramMemoryRecord};

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
        Ok(BaseTypeTurnInput::objective_only(notes.turn_objective()))
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
                    notes.next_steps.push(value.to_string());
                }
                "open-question" | "open_question" | "question" => {
                    notes.open_questions.push(value.to_string());
                }
                "goal" => notes.goals.push(parse_goal_directive(
                    value,
                    u8::try_from(notes.goals.len() + 1).unwrap_or(u8::MAX),
                )?),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_program::test_support::{test_context, test_outcome};

    // --- MeetingFacilitatorProgram ---

    #[test]
    fn meeting_facilitator_parses_structured_notes() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let context = test_context("agenda: Sprint planning\ndecision: Ship by Friday");
        let input = program
            .plan_turn(&context)
            .expect("plan_turn should succeed");
        assert!(input.objective.contains("Sprint planning"));
        assert!(input.objective.contains("decisions=1"));
    }

    #[test]
    fn meeting_facilitator_reflection_includes_counts() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let context = test_context("agenda: Retro\nrisk: Scope creep\nnext-step: Write tests");
        let summary = program
            .reflection_summary(&context, &test_outcome())
            .expect("reflection_summary should succeed");
        assert!(summary.contains("1 risks"));
        assert!(summary.contains("1 next steps"));
    }

    #[test]
    fn meeting_facilitator_descriptor_has_identity() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let desc = program.descriptor();
        assert!(desc.identity.contains("meeting-facilitator"));
    }

    #[test]
    fn meeting_facilitator_parses_multiple_note_types() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let context = test_context(
            "agenda: Sprint 42\nupdate: PR merged\ndecision: Ship Friday\nrisk: Scope creep\nnext-step: Write tests\nopen-question: Deploy strategy?",
        );
        let input = program
            .plan_turn(&context)
            .expect("plan_turn should succeed");
        assert!(input.objective.contains("Sprint 42"));
        assert!(input.objective.contains("updates=1"));
        assert!(input.objective.contains("decisions=1"));
        assert!(input.objective.contains("risks=1"));
        assert!(input.objective.contains("next_steps=1"));
        assert!(input.objective.contains("open_questions=1"));
    }

    #[test]
    fn meeting_facilitator_persistence_summary_includes_meeting_record() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let context = test_context("agenda: Retro\ndecision: Move to Rust");
        let summary = program
            .persistence_summary(&context, &test_outcome())
            .expect("persistence_summary should succeed");
        assert!(summary.contains("meeting-record"));
        assert!(summary.contains("Retro"));
    }

    #[test]
    fn meeting_facilitator_additional_memory_records_with_outputs() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let context = test_context("agenda: Standup\ndecision: Deploy v2");
        let records = program
            .additional_memory_records(&context, &test_outcome())
            .expect("additional_memory_records should succeed");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key_suffix, "decision-record");
        assert_eq!(records[0].scope, MemoryScope::Decision);
        assert!(records[0].value.contains("Standup"));
    }

    #[test]
    fn meeting_facilitator_additional_memory_records_empty_when_no_outputs() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let context = test_context("just some freetext");
        let records = program
            .additional_memory_records(&context, &test_outcome())
            .expect("additional_memory_records should succeed");
        assert!(records.is_empty());
    }

    #[test]
    fn meeting_facilitator_goal_updates_from_structured() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let context = test_context("agenda: Planning\ngoal: Ship v3 | priority=1 | status=active");
        let updates = program
            .goal_updates(&context, &test_outcome())
            .expect("goal_updates should succeed");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].title, "Ship v3");
    }

    #[test]
    fn meeting_facilitator_goal_updates_empty_when_no_goals() {
        let program = MeetingFacilitatorProgram::try_default().expect("create test program");
        let context = test_context("agenda: Quick sync\nupdate: All good");
        let updates = program
            .goal_updates(&context, &test_outcome())
            .expect("goal_updates should succeed");
        assert!(updates.is_empty());
    }

    // --- StructuredMeetingNotes parsing ---

    #[test]
    fn meeting_notes_parse_multiple_agendas_concatenated() {
        let notes = StructuredMeetingNotes::parse("agenda: Topic A\nagenda: Topic B")
            .expect("parse test meeting notes");
        assert!(notes.agenda.contains("Topic A"));
        assert!(notes.agenda.contains("Topic B"));
        assert!(notes.agenda.contains("/"));
    }

    #[test]
    fn meeting_notes_parse_topic_alias_for_agenda() {
        let notes =
            StructuredMeetingNotes::parse("topic: My Topic").expect("parse test meeting notes");
        assert_eq!(notes.agenda, "My Topic");
    }

    #[test]
    fn meeting_notes_parse_status_alias_for_update() {
        let notes =
            StructuredMeetingNotes::parse("status: All green").expect("parse test meeting notes");
        assert_eq!(notes.updates, vec!["All green"]);
    }

    #[test]
    fn meeting_notes_parse_action_item_aliases() {
        let notes =
            StructuredMeetingNotes::parse("next_step: Do A\naction: Do B\naction-item: Do C")
                .expect("test operation should succeed");
        assert_eq!(notes.next_steps.len(), 3);
    }

    #[test]
    fn meeting_notes_parse_question_aliases() {
        let notes =
            StructuredMeetingNotes::parse("open-question: Q1\nopen_question: Q2\nquestion: Q3")
                .expect("test operation should succeed");
        assert_eq!(notes.open_questions.len(), 3);
    }

    #[test]
    fn meeting_notes_parse_empty_value_skipped() {
        let notes = StructuredMeetingNotes::parse("decision:\nrisk: Real risk")
            .expect("parse test meeting notes");
        assert!(notes.decisions.is_empty());
        assert_eq!(notes.risks.len(), 1);
    }

    #[test]
    fn meeting_notes_parse_freetext_lines_become_agenda() {
        let notes = StructuredMeetingNotes::parse("Some freetext line\nAnother line")
            .expect("parse test meeting notes");
        assert!(notes.agenda.contains("Some freetext line"));
        assert!(notes.agenda.contains("Another line"));
    }

    #[test]
    fn meeting_notes_has_persistable_outputs_false_when_empty() {
        let notes =
            StructuredMeetingNotes::parse("just freetext").expect("parse test meeting notes");
        assert!(!notes.has_persistable_outputs());
    }

    #[test]
    fn meeting_notes_has_persistable_outputs_true_with_decisions() {
        let notes =
            StructuredMeetingNotes::parse("decision: Ship it").expect("parse test meeting notes");
        assert!(notes.has_persistable_outputs());
    }

    #[test]
    fn meeting_notes_concise_record_format() {
        let notes = StructuredMeetingNotes::parse("agenda: Sprint\ndecision: Yes\nrisk: Maybe")
            .expect("parse test meeting notes");
        let record = notes.concise_record();
        assert!(record.contains("agenda=Sprint"));
        assert!(record.contains("decisions=[Yes]"));
        assert!(record.contains("risks=[Maybe]"));
    }
}
