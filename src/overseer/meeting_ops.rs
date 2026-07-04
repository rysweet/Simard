//! M3 — the [`MeetingHost`] adapter: transfer a goal to Simard via the meeting
//! handoff surface, without running the interactive REPL.
//!
//! Reuse (design doc §capability table): `meetings::build_persisted_meeting_record_value`
//! (`src/meetings/mod.rs:135`) — the non-interactive path that renders the same
//! on-wire meeting record the REPL persists (`meeting_repl::run_meeting_repl` is
//! the interactive equivalent). The record is written to a durable handoff file
//! under the shared state root, which Simard's OODA reads to adopt the goal.
//!
//! The writer is an injectable [`HandoffSink`] so the transfer is unit-tested
//! with a fake (no filesystem, no REPL). The real sink writes under
//! `<state_root>/meeting_handoffs/` — **never** `~/.simard/worktrees`.

use std::path::PathBuf;

use crate::meetings::build_persisted_meeting_record_value;
use crate::overseer::capabilities::{GoalBrief, MeetingHost, OverseerError};

/// Durable sink for a rendered meeting/goal-handoff record. Injectable so the
/// transfer is testable without touching the filesystem.
pub trait HandoffSink {
    /// Persist `record` for `topic`, returning the artifact path.
    fn record(&self, topic: &str, record: &str) -> Result<PathBuf, OverseerError>;
}

/// Real sink: writes a timestamped handoff file under `dir` (default
/// `<state_root>/meeting_handoffs/`). Creates `dir` if missing. Never writes to
/// `~/.simard/worktrees`.
pub struct FileHandoffSink {
    pub dir: PathBuf,
}

impl FileHandoffSink {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Default handoff dir: `<state_root>/meeting_handoffs/`.
    pub fn from_env() -> Self {
        Self::new(crate::state_root::simard_state_root().join("meeting_handoffs"))
    }
}

impl HandoffSink for FileHandoffSink {
    fn record(&self, topic: &str, record: &str) -> Result<PathBuf, OverseerError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| OverseerError::Capability {
            what: "handoff.mkdir",
            detail: e.to_string(),
        })?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = self
            .dir
            .join(format!("overseer-goal-{ts}-{}.txt", slug(topic)));
        std::fs::write(&path, record).map_err(|e| OverseerError::Capability {
            what: "handoff.write",
            detail: e.to_string(),
        })?;
        Ok(path)
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// The [`MeetingHost`] adapter. Renders the goal as a meeting record and writes
/// it to the handoff sink — the durable "transfer this goal to Simard" artifact.
pub struct MeetingGoalTransfer {
    sink: Box<dyn HandoffSink>,
}

impl MeetingGoalTransfer {
    pub fn new(sink: Box<dyn HandoffSink>) -> Self {
        Self { sink }
    }

    /// Production adapter: write handoffs under `<state_root>/meeting_handoffs/`.
    pub fn from_env() -> Self {
        Self::new(Box::new(FileHandoffSink::from_env()))
    }

    /// Render a goal as the on-wire meeting record (public for tests).
    pub fn render_goal_record(goal: &GoalBrief) -> String {
        let decisions = vec![format!(
            "Transfer goal to Simard: {} — {}",
            goal.title, goal.rationale
        )];
        let action_items = vec![format!(
            "Advance goal in {} (priority {})",
            goal.target_repo, goal.priority
        )];
        build_persisted_meeting_record_value(&goal.title, &decisions, &action_items, &[])
    }
}

impl MeetingHost for MeetingGoalTransfer {
    fn transfer_goal(&self, goal: &GoalBrief) -> Result<(), OverseerError> {
        let record = Self::render_goal_record(goal);
        self.sink.record(&goal.title, &record)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn goal() -> GoalBrief {
        GoalBrief {
            title: "Reduce distillation parse-failure rate".to_string(),
            rationale: "banner pollution is breaking distillation".to_string(),
            priority: 2,
            target_repo: "rysweet/Simard".to_string(),
        }
    }

    #[derive(Default)]
    struct FakeSink {
        recorded: Mutex<Vec<(String, String)>>,
    }
    impl HandoffSink for FakeSink {
        fn record(&self, topic: &str, record: &str) -> Result<PathBuf, OverseerError> {
            self.recorded
                .lock()
                .unwrap()
                .push((topic.to_string(), record.to_string()));
            Ok(PathBuf::from("/tmp/fake-handoff.txt"))
        }
    }

    #[test]
    fn transfer_records_the_goal_round_trip() {
        let sink = std::sync::Arc::new(FakeSink::default());
        let host = MeetingGoalTransfer::new(Box::new(SinkRef(sink.clone())));
        host.transfer_goal(&goal()).expect("transfer");

        let recorded = sink.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        let (topic, record) = &recorded[0];
        assert!(topic.contains("distillation"));
        // The record carries the goal title, rationale, and target repo.
        assert!(record.contains("Reduce distillation parse-failure rate"));
        assert!(record.contains("banner pollution"));
        assert!(record.contains("rysweet/Simard"));
    }

    /// Share a FakeSink across the adapter and the assertion.
    struct SinkRef(std::sync::Arc<FakeSink>);
    impl HandoffSink for SinkRef {
        fn record(&self, topic: &str, record: &str) -> Result<PathBuf, OverseerError> {
            self.0.record(topic, record)
        }
    }

    #[test]
    fn file_sink_writes_under_configured_dir_not_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("meeting_handoffs");
        let sink = FileHandoffSink::new(dir.clone());
        let path = sink.record("t", "some record").expect("write");
        assert!(
            path.starts_with(&dir),
            "handoff written under the configured dir"
        );
        assert!(
            !path.to_string_lossy().contains("worktrees"),
            "must never write into worktrees"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "some record");
    }
}
