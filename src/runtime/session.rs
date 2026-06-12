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

/// Buffers memory and evidence writes for batched persistence after reflection
/// completes (spec line 581, issue #2093). Pre-persistence phases collect
/// records here instead of writing them to the stores immediately. All records
/// are flushed during the Persistence phase.
struct PendingWrites {
    memory_records: Vec<MemoryRecord>,
    evidence_records: Vec<EvidenceRecord>,
}

impl PendingWrites {
    fn new() -> Self {
        Self {
            memory_records: Vec::new(),
            evidence_records: Vec::new(),
        }
    }

    fn add_memory(&mut self, record: MemoryRecord) {
        self.memory_records.push(record);
    }

    fn add_evidence(&mut self, record: EvidenceRecord) {
        self.evidence_records.push(record);
    }
}

impl RuntimeKernel {
    pub(super) fn execute_session(&mut self, objective: String) -> SimardResult<SessionOutcome> {
        self.transition(RuntimeState::Active)?;

        let mut session = self.new_session(objective);

        // Issue #2093: buffer pre-persistence writes so they are flushed
        // atomically during the Persistence phase rather than written
        // incrementally during Preparation/Execution.
        let mut pending = PendingWrites::new();

        // --- Memory consolidation: intake at session start ---
        if let Some(bridge) = &self.cognitive_bridge {
            if let Err(e) = crate::memory_consolidation::intake_memory_operations(
                &session.objective,
                &session.id,
                &**bridge,
            ) {
                eprintln!("[simard] session consolidation: intake failed: {e}");
            }
            // Hydrate prior-session facts into working memory for cross-session recall.
            match crate::memory_consolidation::consolidation_intake(
                &session.id,
                &session.objective,
                &**bridge,
            ) {
                Ok(n) if n > 0 => {
                    eprintln!("[simard] session consolidation: hydrated {n} prior-session facts");
                }
                Err(e) => {
                    eprintln!(
                        "[simard] session consolidation: cross-session hydration failed: {e}"
                    );
                }
                _ => {}
            }
        }

        self.persist_session_scratch(&mut session, &mut pending)?;
        let outcome = self.run_selected_base_type_session(&mut session)?;
        self.record_execution_evidence(&mut session, &outcome, &mut pending)?;
        // Build context once and reuse for reflection + persistence phases.
        let context = self.agent_program_context(&session);
        let reflection = self.build_reflection(&mut session, &outcome, &context)?;
        self.persist_session_summary(&mut session, &outcome, &context, &mut pending)?;

        // --- Memory consolidation: persistence at session end ---

        // Flush any pending bridge writes before final persistence so that
        // records that fell back to the local file store get one last retry.
        let synced = self.ports.memory_store.flush_pending();
        if synced > 0 {
            eprintln!("[simard] session teardown: flushed {synced} pending memory records");
        }

        if let Some(bridge) = &self.cognitive_bridge {
            // Flush working memory to episodes before final persistence.
            crate::memory_consolidation::consolidation_persistence(&session.id, &**bridge)?;
            crate::memory_consolidation::persistence_memory_operations(&session.id, &**bridge)?;

            // Save a cognitive memory snapshot and prune to 10 most recent.
            if let Some(dir) = crate::memory_snapshot::snapshot_dir(None) {
                let path = crate::memory_snapshot::save_session_snapshot(
                    &**bridge,
                    &self.request.manifest.name,
                    &dir,
                )?;
                eprintln!("[simard] snapshot: saved {}", path.display());
                crate::memory_snapshot::prune_snapshots(&dir, 10);
            }
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

    fn persist_session_scratch(
        &mut self,
        session: &mut SessionRecord,
        pending: &mut PendingWrites,
    ) -> SimardResult<()> {
        session.advance(SessionPhase::Preparation)?;

        let scratch_key = format!("{}-scratch", session.id);
        // Issue #2093: buffer the write instead of persisting immediately.
        pending.add_memory(MemoryRecord {
            key: scratch_key.clone(),
            scope: MemoryScope::SessionScratch,
            value: objective_metadata(&session.objective),
            session_id: session.id.clone(),
            recorded_in: SessionPhase::Preparation,
            created_at: None,
        });
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
        pending: &mut PendingWrites,
    ) -> SimardResult<()> {
        session.advance(SessionPhase::Execution)?;

        let evidence_source = EvidenceSource::BaseType(self.request.selected_base_type.clone());
        for (index, detail) in outcome.evidence.iter().enumerate() {
            let evidence_id = format!("{}-evidence-{}", session.id, index + 1);
            // Issue #2093: buffer evidence instead of persisting immediately.
            pending.add_evidence(EvidenceRecord {
                id: evidence_id.clone(),
                session_id: session.id.clone(),
                phase: SessionPhase::Execution,
                detail: detail.clone(),
                source: evidence_source.clone(),
            });
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
        pending: &mut PendingWrites,
    ) -> SimardResult<()> {
        self.transition(RuntimeState::Persisting)?;
        session.advance(SessionPhase::Persistence)?;

        // Issue #2093: flush all buffered writes from pre-persistence phases
        // in a single batch now that reflection has completed.
        for record in pending.memory_records.drain(..) {
            self.ports.memory_store.put(record)?;
        }
        for record in pending.evidence_records.drain(..) {
            self.ports.evidence_store.record(record)?;
        }

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
mod tests_pending_writes {
    use super::PendingWrites;
    use crate::evidence::{EvidenceRecord, EvidenceSource};
    use crate::memory::{MemoryRecord, MemoryScope};
    use crate::session::SessionPhase;

    fn make_memory_record(key: &str) -> MemoryRecord {
        MemoryRecord {
            key: key.to_string(),
            scope: MemoryScope::SessionScratch,
            value: "test-value".to_string(),
            session_id: crate::session::SessionId::parse(
                "session-00000000-0000-0000-0000-000000000001",
            )
            .unwrap(),
            recorded_in: SessionPhase::Preparation,
            created_at: None,
        }
    }

    fn make_evidence_record(id: &str) -> EvidenceRecord {
        EvidenceRecord {
            id: id.to_string(),
            session_id: crate::session::SessionId::parse(
                "session-00000000-0000-0000-0000-000000000001",
            )
            .unwrap(),
            phase: SessionPhase::Execution,
            detail: "test-evidence".to_string(),
            source: EvidenceSource::Runtime,
        }
    }

    #[test]
    fn new_pending_writes_is_empty() {
        let pending = PendingWrites::new();
        assert!(pending.memory_records.is_empty());
        assert!(pending.evidence_records.is_empty());
    }

    #[test]
    fn add_memory_accumulates_records() {
        let mut pending = PendingWrites::new();
        pending.add_memory(make_memory_record("key-1"));
        pending.add_memory(make_memory_record("key-2"));
        assert_eq!(pending.memory_records.len(), 2);
        assert_eq!(pending.memory_records[0].key, "key-1");
        assert_eq!(pending.memory_records[1].key, "key-2");
    }

    #[test]
    fn add_evidence_accumulates_records() {
        let mut pending = PendingWrites::new();
        pending.add_evidence(make_evidence_record("ev-1"));
        pending.add_evidence(make_evidence_record("ev-2"));
        pending.add_evidence(make_evidence_record("ev-3"));
        assert_eq!(pending.evidence_records.len(), 3);
        assert_eq!(pending.evidence_records[0].id, "ev-1");
    }

    #[test]
    fn drain_clears_pending_records() {
        let mut pending = PendingWrites::new();
        pending.add_memory(make_memory_record("key-1"));
        pending.add_evidence(make_evidence_record("ev-1"));

        let mem: Vec<_> = pending.memory_records.drain(..).collect();
        let ev: Vec<_> = pending.evidence_records.drain(..).collect();

        assert_eq!(mem.len(), 1);
        assert_eq!(ev.len(), 1);
        assert!(pending.memory_records.is_empty());
        assert!(pending.evidence_records.is_empty());
    }

    #[test]
    fn mixed_records_maintain_insertion_order() {
        let mut pending = PendingWrites::new();
        pending.add_memory(make_memory_record("mem-a"));
        pending.add_evidence(make_evidence_record("ev-a"));
        pending.add_memory(make_memory_record("mem-b"));
        pending.add_evidence(make_evidence_record("ev-b"));

        assert_eq!(pending.memory_records[0].key, "mem-a");
        assert_eq!(pending.memory_records[1].key, "mem-b");
        assert_eq!(pending.evidence_records[0].id, "ev-a");
        assert_eq!(pending.evidence_records[1].id, "ev-b");
    }
}
