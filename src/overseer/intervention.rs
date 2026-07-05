//! The Overseer's `Intervention` set — one variant per action it can take, each
//! mapped to an EXISTING Simard capability trait in `capabilities.rs`.
//! Interventions are proposed by Decide and (in M2+) executed by Act; HIGH-RISK
//! variants are gated by `guardrails::classify`.

use crate::overseer::capabilities::{AuditScope, GoalBrief, OrchestratorRunBrief, RecipeBrief};
use crate::overseer::whisper_ops::WhisperUrgency;

/// A single action the Overseer can take. Each variant names the capability it
/// dispatches through; see the trait doc comments for the exact reused function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intervention {
    /// Launch an amplihack recipe workstream (smart-orchestrator → default-workflow).
    /// Capability: `RecipeLauncher`.
    LaunchRecipe { brief: RecipeBrief },
    /// Run the pr-verify checklist and merge if merge-ready.
    /// Capability: `PrOps::verify` + `PrOps::merge`.
    VerifyAndMergePr { repo: String, pr: u32 },
    /// Resolve merge conflicts on a PR (union-merge; `--no-verify` push).
    /// Capability: `PrOps::resolve_conflict` (under `git_guardrails`).
    ResolveConflict { repo: String, pr: u32 },
    /// Build+verify+hand over a new binary at `commit`. HIGH-RISK → gated.
    /// Capability: `Deployer::deploy`.
    Deploy { commit: String },
    /// File a deduplicated GitHub issue for a recurring failure.
    /// Capability: `IssueFiler::file`.
    FileIssue { run: OrchestratorRunBrief },
    /// Transfer/handoff a goal to Simard via the meeting REPL.
    /// Capability: `MeetingHost::transfer_goal`.
    TransferGoal { goal: GoalBrief },
    /// Emit a periodic status report (uptime, resources, tokens/cost, workstreams,
    /// completed work, telemetry anomalies, goals, self-improvement PRs).
    /// Capability: `StatusReader` (rendered).
    Report,
    /// Run a quality-audit loop (crusty-old-engineer-gated).
    /// Capability: `Auditor::run_audit`.
    RunAudit { scope: AuditScope },
    /// Surface a HIGH-RISK or low-confidence decision to the human operator.
    Escalate { reason: String },
    /// Inject a lightweight ADVISORY steering note ("whisper") into Simard's OODA
    /// loop — additional/corrective context she picks up at the start of her next
    /// cycle, WITHOUT the Overseer taking the action for her. Delivered onto the
    /// existing meeting-handoff inbox as an advisory (non-promoting) handoff.
    /// Capability: `whisper_ops::WhisperSink`.
    Whisper {
        note: String,
        urgency: WhisperUrgency,
    },
}

impl Intervention {
    /// Short, stable label used in gate messages, telemetry, and dedup.
    pub fn label(&self) -> &'static str {
        match self {
            Self::LaunchRecipe { .. } => "launch_recipe",
            Self::VerifyAndMergePr { .. } => "verify_and_merge_pr",
            Self::ResolveConflict { .. } => "resolve_conflict",
            Self::Deploy { .. } => "deploy",
            Self::FileIssue { .. } => "file_issue",
            Self::TransferGoal { .. } => "transfer_goal",
            Self::Report => "report",
            Self::RunAudit { .. } => "run_audit",
            Self::Escalate { .. } => "escalate",
            Self::Whisper { .. } => "whisper",
        }
    }
}

/// A planned intervention after gating: the chosen action plus whether the gates
/// admitted it for autonomous execution and, if not, why. `run_cycle` returns
/// these; Act (M2+) executes only the admitted ones.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedIntervention {
    pub intervention: Intervention,
    pub admitted: bool,
    /// Human-readable gate note (e.g. "HIGH-RISK: escalated", "budget exceeded",
    /// "own PR skipped", "deferred: overlaps sweep group ooda-core").
    pub note: String,
}
