use crate::agent_program::AgentProgramContext;
use crate::base_types::{BaseTypeOutcome, BaseTypeSessionRequest};
use crate::error::SimardResult;
use crate::evidence::{EvidenceRecord, EvidenceSource};
use crate::goals::GoalRecord;
use crate::memory::{MemoryRecord, MemoryScope};
use crate::metadata::{Freshness, FreshnessState};
use crate::reflection::{ReflectionReport, ReflectionSnapshot, ReflectiveRuntime};
use crate::sanitization::objective_metadata;
use crate::session::{SessionPhase, SessionRecord};

use super::RuntimeKernel;
use super::types::{RuntimeState, SessionOutcome};

impl RuntimeKernel {
    pub(super) fn execute_session(&mut self, objective: String) -> SimardResult<SessionOutcome> {
        self.transition(RuntimeState::Active)?;

        let mut session = self.new_session(objective);

        // --- Memory consolidation: intake at session start ---
        if let Some(bridge) = &self.cognitive_bridge
            && let Err(e) = crate::memory_consolidation::intake_memory_operations(
                &session.objective,
                &session.id,
                bridge,
            )
        {
            eprintln!("[simard] session consolidation: intake failed: {e}");
        }

        self.persist_session_scratch(&mut session)?;
        let outcome = self.run_selected_base_type_session(&mut session)?;
        self.record_execution_evidence(&mut session, &outcome)?;
        // Build context once and reuse for reflection + persistence phases.
        let context = self.agent_program_context(&session);
        let reflection = self.build_reflection(&mut session, &outcome, &context)?;
        self.persist_session_summary(&mut session, &outcome, &context)?;

        // --- Memory consolidation: persistence at session end ---
        if let Some(bridge) = &self.cognitive_bridge
            && let Err(e) =
                crate::memory_consolidation::persistence_memory_operations(&session.id, bridge)
        {
            eprintln!("[simard] session consolidation: persistence failed: {e}");
        }

        // --- Retry any failed bridge writes before teardown ---
        let synced = self.ports.memory_store.flush_pending();
        if synced > 0 {
            eprintln!("[simard] session end: flushed {synced} pending bridge writes");
        }

        self.complete_session(session, outcome, reflection)
    }

    fn new_session(&mut self, objective: String) -> SessionRecord {
        let session = SessionRecord::new(
            self.request.manifest.default_mode,
            objective,
            self.request.selected_base_type.clone(),
            self.ports.session_ids.as_ref(),
        );
        self.remember_session(&session);
        session
    }

    fn persist_session_scratch(&mut self, session: &mut SessionRecord) -> SimardResult<()> {
        session.advance(SessionPhase::Preparation)?;

        let scratch_key = format!("{}-scratch", session.id);
        self.ports.memory_store.put(MemoryRecord {
            key: scratch_key.clone(),
            scope: MemoryScope::SessionScratch,
            value: objective_metadata(&session.objective),
            session_id: session.id.clone(),
            recorded_in: SessionPhase::Preparation,
            created_at: None,
        })?;
        session.attach_memory(scratch_key);
        self.remember_session(session);

        Ok(())
    }

    fn run_selected_base_type_session(
        &mut self,
        session: &mut SessionRecord,
    ) -> SimardResult<BaseTypeOutcome> {
        session.advance(SessionPhase::Planning)?;

        let context = self.agent_program_context(session);
        let turn_input = self.ports.agent_program.plan_turn(&context)?;

        let mut base_type_session = self.factory.open_session(BaseTypeSessionRequest {
            session_id: session.id.clone(),
            mode: session.mode,
            topology: self.request.topology,
            prompt_assets: self.prompt_assets.clone(),
            runtime_node: self.runtime_node.clone(),
            mailbox_address: self.mailbox_address.clone(),
        })?;
        base_type_session.open()?;
        let outcome = base_type_session.run_turn(turn_input);
        let close_result = base_type_session.close();

        match (outcome, close_result) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(close_error)) => Err(close_error),
            (Err(error), Err(close_error)) => {
                Err(crate::error::SimardError::BaseTypeSessionCleanupFailed {
                    base_type: self.request.selected_base_type.to_string(),
                    action: "run_turn".to_string(),
                    reason: error.to_string(),
                    cleanup_reason: close_error.to_string(),
                })
            }
        }
    }

    fn record_execution_evidence(
        &mut self,
        session: &mut SessionRecord,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<()> {
        session.advance(SessionPhase::Execution)?;

        let evidence_source = EvidenceSource::BaseType(self.request.selected_base_type.clone());
        for (index, detail) in outcome.evidence.iter().enumerate() {
            let evidence_id = format!("{}-evidence-{}", session.id, index + 1);
            self.ports.evidence_store.record(EvidenceRecord {
                id: evidence_id.clone(),
                session_id: session.id.clone(),
                phase: SessionPhase::Execution,
                detail: detail.clone(),
                source: evidence_source.clone(),
            })?;
            session.attach_evidence(evidence_id);
        }
        self.remember_session(session);

        Ok(())
    }

    fn build_reflection(
        &mut self,
        session: &mut SessionRecord,
        outcome: &BaseTypeOutcome,
        context: &AgentProgramContext,
    ) -> SimardResult<ReflectionReport> {
        self.transition(RuntimeState::Reflecting)?;
        session.advance(SessionPhase::Reflection)?;

        Ok(ReflectionReport {
            summary: self
                .ports
                .agent_program
                .reflection_summary(context, outcome)?,
            snapshot: self.snapshot_for(Some(session))?,
        })
    }

    fn persist_session_summary(
        &mut self,
        session: &mut SessionRecord,
        outcome: &BaseTypeOutcome,
        context: &AgentProgramContext,
    ) -> SimardResult<()> {
        self.transition(RuntimeState::Persisting)?;
        session.advance(SessionPhase::Persistence)?;

        let summary_key = format!("{}-summary", session.id);
        self.ports.memory_store.put(MemoryRecord {
            key: summary_key.clone(),
            scope: self.request.manifest.memory_policy.summary_scope,
            value: self
                .ports
                .agent_program
                .persistence_summary(context, outcome)?,
            session_id: session.id.clone(),
            recorded_in: SessionPhase::Persistence,
            created_at: None,
        })?;
        session.attach_memory(summary_key);

        for record in self
            .ports
            .agent_program
            .additional_memory_records(context, outcome)?
        {
            let key = format!("{}-{}", session.id, record.key_suffix);
            self.ports.memory_store.put(MemoryRecord {
                key: key.clone(),
                scope: record.scope,
                value: record.value,
                session_id: session.id.clone(),
                recorded_in: SessionPhase::Persistence,
                created_at: None,
            })?;
            session.attach_memory(key);
        }
        for update in self.ports.agent_program.goal_updates(context, outcome)? {
            self.ports.goal_store.put(GoalRecord::from_update(
                update,
                self.request.manifest.name.clone(),
                session.id.clone(),
                SessionPhase::Persistence,
            )?)?;
        }
        self.remember_session(session);

        Ok(())
    }

    fn complete_session(
        &mut self,
        mut session: SessionRecord,
        outcome: BaseTypeOutcome,
        reflection: ReflectionReport,
    ) -> SimardResult<SessionOutcome> {
        session.advance(SessionPhase::Complete)?;
        self.remember_session(&session);
        self.transition(RuntimeState::Ready)?;

        Ok(SessionOutcome {
            session,
            plan: outcome.plan,
            execution_summary: outcome.execution_summary,
            reflection,
        })
    }

    pub(super) fn mark_last_session_failed(&mut self) {
        if let Some(session) = self.last_session.as_mut()
            && session.phase != SessionPhase::Failed
        {
            session.phase = SessionPhase::Failed;
        }
    }

    fn agent_program_context(&self, session: &SessionRecord) -> AgentProgramContext {
        AgentProgramContext {
            session_id: session.id.clone(),
            identity_name: self.request.manifest.name.clone(),
            mode: session.mode,
            selected_base_type: self.request.selected_base_type.clone(),
            topology: self.request.topology,
            runtime_node: self.runtime_node.clone(),
            mailbox_address: self.mailbox_address.clone(),
            objective: session.objective.clone(),
            active_goals: self
                .ports
                .goal_store
                .active_top_goals(5)
                .unwrap_or_default(),
        }
    }

    pub(super) fn snapshot_for(
        &self,
        session: Option<&SessionRecord>,
    ) -> SimardResult<ReflectionSnapshot> {
        let adapter_desc = self.factory.descriptor();
        let evidence_records = match session {
            Some(active_session) => self
                .ports
                .evidence_store
                .count_for_session(&active_session.id)?,
            None => 0,
        };
        let memory_records = match session {
            Some(active_session) => self
                .ports
                .memory_store
                .count_for_session(&active_session.id)?,
            None => 0,
        };
        let active_goals = self.ports.goal_store.active_top_goals(5)?;
        let proposed_goals = self
            .ports
            .goal_store
            .top_goals_by_status(crate::goals::GoalStatus::Proposed, 5)?;
        let manifest_freshness = match self.state {
            RuntimeState::Stopped | RuntimeState::Failed => {
                Freshness::observed(FreshnessState::Stale)?
            }
            _ => Freshness::observed(FreshnessState::Current)?,
        };

        Ok(ReflectionSnapshot {
            identity_name: self.request.manifest.name.clone(),
            identity_components: self.request.manifest.components.clone(),
            selected_base_type: self.request.selected_base_type.clone(),
            topology: self.request.topology,
            runtime_state: self.state,
            runtime_node: self.runtime_node.clone(),
            mailbox_address: self.mailbox_address.clone(),
            session_phase: session.map(|active_session| active_session.phase),
            prompt_assets: self
                .prompt_assets
                .iter()
                .map(|asset| asset.id.clone())
                .collect(),
            manifest_contract: self
                .request
                .manifest
                .contract
                .with_freshness(manifest_freshness),
            evidence_records,
            memory_records,
            active_goal_count: active_goals.len(),
            active_goals: active_goals.iter().map(GoalRecord::concise_label).collect(),
            proposed_goal_count: proposed_goals.len(),
            proposed_goals: proposed_goals
                .iter()
                .map(GoalRecord::concise_label)
                .collect(),
            agent_program_backend: self.ports.agent_program.descriptor(),
            handoff_backend: self.ports.handoff_store.descriptor(),
            adapter_backend: adapter_desc.backend.clone(),
            adapter_capabilities: adapter_desc
                .capabilities
                .iter()
                .map(ToString::to_string)
                .collect(),
            adapter_supported_topologies: adapter_desc
                .supported_topologies
                .iter()
                .map(ToString::to_string)
                .collect(),
            topology_backend: self.ports.topology_driver.descriptor(),
            transport_backend: self.ports.transport.descriptor(),
            supervisor_backend: self.ports.supervisor.descriptor(),
            memory_backend: self.ports.memory_store.descriptor(),
            evidence_backend: self.ports.evidence_store.descriptor(),
            goal_backend: self.ports.goal_store.descriptor(),
        })
    }
}

impl ReflectiveRuntime for RuntimeKernel {
    fn snapshot(&self) -> SimardResult<ReflectionSnapshot> {
        self.snapshot_for(self.last_session.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use crate::base_types::BaseTypeId;
    use crate::identity::OperatingMode;
    use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};

    #[test]
    fn session_record_starts_at_intake() {
        let session = SessionRecord::new(
            OperatingMode::Engineer,
            "test-objective",
            BaseTypeId::new("local-harness"),
            &UuidSessionIdGenerator,
        );
        assert_eq!(session.phase, SessionPhase::Intake);
        assert!(session.evidence_ids.is_empty());
        assert!(session.memory_keys.is_empty());
    }

    #[test]
    fn session_advance_through_full_lifecycle() {
        let mut session = SessionRecord::new(
            OperatingMode::Engineer,
            "test",
            BaseTypeId::new("local-harness"),
            &UuidSessionIdGenerator,
        );
        let phases = [
            SessionPhase::Preparation,
            SessionPhase::Planning,
            SessionPhase::Execution,
            SessionPhase::Reflection,
            SessionPhase::Persistence,
            SessionPhase::Complete,
        ];
        for phase in phases {
            session.advance(phase).unwrap();
            assert_eq!(session.phase, phase);
        }
    }

    #[test]
    fn session_advance_invalid_transition_errors() {
        let mut session = SessionRecord::new(
            OperatingMode::Engineer,
            "test",
            BaseTypeId::new("local-harness"),
            &UuidSessionIdGenerator,
        );
        // Intake -> Complete should fail (must go through intermediate phases)
        let result = session.advance(SessionPhase::Complete);
        assert!(result.is_err());
    }

    #[test]
    fn session_advance_to_failed_from_any_phase() {
        for start_phase in [
            SessionPhase::Intake,
            SessionPhase::Preparation,
            SessionPhase::Planning,
            SessionPhase::Execution,
        ] {
            let mut session = SessionRecord::new(
                OperatingMode::Engineer,
                "test",
                BaseTypeId::new("local-harness"),
                &UuidSessionIdGenerator,
            );
            // Advance to the starting phase first
            let intermediates: Vec<SessionPhase> = match start_phase {
                SessionPhase::Intake => vec![],
                SessionPhase::Preparation => vec![SessionPhase::Preparation],
                SessionPhase::Planning => {
                    vec![SessionPhase::Preparation, SessionPhase::Planning]
                }
                SessionPhase::Execution => vec![
                    SessionPhase::Preparation,
                    SessionPhase::Planning,
                    SessionPhase::Execution,
                ],
                _ => vec![],
            };
            for phase in intermediates {
                session.advance(phase).unwrap();
            }
            // Failed should always be reachable
            session.advance(SessionPhase::Failed).unwrap();
            assert_eq!(session.phase, SessionPhase::Failed);
        }
    }

    #[test]
    fn session_attach_evidence_and_memory() {
        let mut session = SessionRecord::new(
            OperatingMode::Engineer,
            "test",
            BaseTypeId::new("local-harness"),
            &UuidSessionIdGenerator,
        );
        session.attach_evidence("ev-1");
        session.attach_evidence("ev-2");
        session.attach_memory("mem-1");
        assert_eq!(session.evidence_ids.len(), 2);
        assert_eq!(session.memory_keys.len(), 1);
    }

    #[test]
    fn session_redacted_for_handoff_changes_objective() {
        let session = SessionRecord::new(
            OperatingMode::Engineer,
            "secret objective text",
            BaseTypeId::new("local-harness"),
            &UuidSessionIdGenerator,
        );
        let redacted = session.redacted_for_handoff();
        assert_ne!(redacted.objective, "secret objective text");
        assert!(
            redacted.objective.starts_with("objective-metadata("),
            "redacted objective should start with 'objective-metadata(', got: {}",
            redacted.objective
        );
    }

    #[test]
    fn session_phase_display() {
        assert_eq!(SessionPhase::Intake.to_string(), "intake");
        assert_eq!(SessionPhase::Complete.to_string(), "complete");
        assert_eq!(SessionPhase::Failed.to_string(), "failed");
    }
}
