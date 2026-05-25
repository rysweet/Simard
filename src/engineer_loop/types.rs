use std::time::Duration;

use crate::goals::GoalRecord;
use crate::terminal_engineer_bridge::TerminalBridgeContext;

use std::path::PathBuf;

/// Serialize/deserialize Duration as u64 milliseconds for JSON IPC between
/// recipe steps. Lossy below 1ms, but engineer-loop phases are coarse-grained.
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis().min(u128::from(u64::MAX)) as u64)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let ms = u64::deserialize(d)?;
        Ok(Duration::from_millis(ms))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RepoInspection {
    pub workspace_root: PathBuf,
    pub repo_root: PathBuf,
    pub branch: String,
    pub head: String,
    pub worktree_dirty: bool,
    pub changed_files: Vec<String>,
    pub active_goals: Vec<GoalRecord>,
    pub carried_meeting_decisions: Vec<String>,
    pub architecture_gap_summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StructuredEditRequest {
    pub relative_path: String,
    pub search: String,
    pub replacement: String,
    pub verify_contains: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateFileRequest {
    pub relative_path: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppendToFileRequest {
    pub relative_path: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShellCommandRequest {
    pub argv: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GitCommitRequest {
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpenIssueRequest {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineerActionKind {
    ReadOnlyScan,
    StructuredTextReplace(StructuredEditRequest),
    CargoTest,
    CargoCheck,
    CreateFile(CreateFileRequest),
    AppendToFile(AppendToFileRequest),
    RunShellCommand(ShellCommandRequest),
    GitCommit(GitCommitRequest),
    OpenIssue(OpenIssueRequest),
    AgentSession { outcome_summary: String },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectedEngineerAction {
    pub label: String,
    pub rationale: String,
    pub argv: Vec<String>,
    pub plan_summary: String,
    pub verification_steps: Vec<String>,
    pub expected_changed_files: Vec<String>,
    pub kind: EngineerActionKind,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExecutedEngineerAction {
    pub selected: SelectedEngineerAction,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub changed_files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerificationReport {
    pub status: String,
    pub summary: String,
    pub checks: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PhaseOutcome {
    Success,
    Failed(String),
    Skipped(String),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PhaseTrace {
    pub name: String,
    #[serde(with = "duration_millis")]
    pub duration: Duration,
    pub outcome: PhaseOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EngineerLoopRun {
    pub state_root: PathBuf,
    pub execution_scope: String,
    pub inspection: RepoInspection,
    pub action: ExecutedEngineerAction,
    pub verification: VerificationReport,
    pub terminal_bridge_context: Option<TerminalBridgeContext>,
    #[serde(with = "duration_millis")]
    pub elapsed_duration: Duration,
    pub phase_traces: Vec<PhaseTrace>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzedAction {
    CreateFile,
    AppendToFile,
    RunShellCommand,
    GitCommit,
    OpenIssue,
    StructuredTextReplace,
    CargoTest,
    ReadOnlyScan,
}

/// Classify an objective string into an action category using keyword matching.
/// Case-insensitive. More specific compound patterns are checked before single
/// keywords so that "run tests" maps to `CargoTest` rather than `RunShellCommand`.
pub fn analyze_objective(objective: &str) -> AnalyzedAction {
    let lower = objective.to_lowercase();

    // Issue/bug patterns before "create" — "create a feature request" is an issue, not a file
    if lower.contains("issue") || lower.contains("bug report") || lower.contains("feature request")
    {
        AnalyzedAction::OpenIssue
    } else if lower.contains("new file") || lower.contains("create") || lower.contains("add file") {
        AnalyzedAction::CreateFile
    } else if lower.contains("append") || lower.contains("add to") {
        AnalyzedAction::AppendToFile
    } else if lower.contains("commit") || lower.contains("save changes") {
        AnalyzedAction::GitCommit
    } else if lower.contains("cargo test")
        || lower.contains("run tests")
        || lower.contains("test suite")
        || lower.contains("run the tests")
    {
        AnalyzedAction::CargoTest
    } else if lower.contains("run") || lower.contains("execute") || lower.contains("check") {
        AnalyzedAction::RunShellCommand
    } else if lower.contains("fix")
        || lower.contains("change")
        || lower.contains("update")
        || lower.contains("replace")
    {
        AnalyzedAction::StructuredTextReplace
    } else if lower.contains("test") {
        AnalyzedAction::CargoTest
    } else {
        AnalyzedAction::ReadOnlyScan
    }
}

impl AnalyzedAction {
    /// Returns `true` if this action category implies repository mutations
    /// (file edits, commits, issue creation). Read-only scans, tests, and
    /// checks are non-mutating.
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            AnalyzedAction::CreateFile
                | AnalyzedAction::AppendToFile
                | AnalyzedAction::GitCommit
                | AnalyzedAction::OpenIssue
                | AnalyzedAction::StructuredTextReplace
        )
    }
}

/// Minimal reflection record produced when a session fails. Captures the
/// error context so failures are not silently lost (issue #2088). The spec
/// requires reflection on every session outcome including errors.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionErrorReflection {
    /// The original objective that was being pursued.
    pub objective: String,
    /// Which phase the session was in when the error occurred.
    pub failed_phase: String,
    /// Human-readable error description.
    pub error_message: String,
    /// Phase traces collected up to the point of failure.
    pub phase_traces: Vec<PhaseTrace>,
}
