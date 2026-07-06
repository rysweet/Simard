//! The Overseer's `Intervention` set — one variant per action it can take, each
//! mapped to an EXISTING Simard capability trait in `capabilities.rs`.
//! Interventions are proposed by Decide and (in M2+) executed by Act; HIGH-RISK
//! variants are gated by `guardrails::classify`.

use serde::{Deserialize, Serialize};

use crate::overseer::capabilities::{AuditScope, GoalBrief, OrchestratorRunBrief, RecipeBrief};
use crate::overseer::signal::GapItem;
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
    /// SELF-HEAL a false-parked standing/perpetual goal: auto-unblock + reactivate
    /// it — the exact operation `simard goal unblock` performs — so a perpetual
    /// goal wrongly hard-blocked by the no-progress safeguard re-enters the OODA
    /// spawn path. Deduped so it never fights itself; optionally followed by an
    /// advisory whisper steering Simard to carve a bounded shippable sub-goal.
    /// Capability: `GoalCurator::unblock` (+ optional `whisper_ops::WhisperSink`).
    UnblockGoal { goal_id: String, reason: String },
    /// ESCALATE a genuinely-blocked goal carrying a "needs human review" marker to
    /// the operator (email + Signal) with the goal id + reason + the root-cause
    /// **WHY**, so the marker AND its analysis actually reach a human — closing the
    /// silent-failure gap.
    /// Capability: `notify::OperatorNotifier`.
    EscalateBlockedGoal {
        goal_id: String,
        reason: String,
        /// The root-cause analysis (one-line WHY) carried into the operator
        /// notification so the human sees *why*, not just the bare symptom.
        why: String,
    },
    /// FLAG the backlog-coverage gaps the recurring gap-scan found — important
    /// work with no active workstream (uncovered high-priority goals, high-signal
    /// issues with no PR, live anomalies with no fix in flight). Acts through the
    /// SAME plumbing goal-health / M1 use: notify the operator on BOTH channels
    /// (email + Signal) with the specifics AND file one deduped issue per gap.
    /// Deduped per gap signature so a recurring gap notifies/files at most once.
    /// Capability: `notify::OperatorNotifier` + `IssueFiler::file`.
    FlagWorkstreamGaps { gaps: Vec<GapItem> },
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
            Self::UnblockGoal { .. } => "unblock_goal",
            Self::EscalateBlockedGoal { .. } => "escalate_blocked_goal",
            Self::FlagWorkstreamGaps { .. } => "flag_workstream_gaps",
        }
    }
}

/// How a chosen action relates to the problem's ROOT CAUSE (issue #2635). Every
/// planned intervention is classified so a symptom-only patch can NEVER be
/// applied silently: it is explicitly labelled and its unaddressed cause is
/// surfaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemediationClass {
    /// The action targets the root cause (self-heal, launch a fix, escalate the
    /// systemic defect for a fix, deliver/merge, steer). The cause is addressed.
    RootCause,
    /// The action only mitigates the SYMPTOM (e.g. hand budget pressure to the
    /// operator) — the underlying cause stays live and MUST be surfaced.
    SymptomMitigation,
    /// The "problem" is a deliberate/intentional state (an operator or dependency
    /// block) — acknowledged and surfaced, with nothing to fix. Counts as
    /// addressed (it never cries wolf).
    Acknowledged,
}

/// The root-cause classification attached to a [`PlannedIntervention`]: whether
/// the action addressed the ROOT CAUSE, and — when it did not — the surfaced note
/// recording that the cause remains unaddressed. Enforces the invariant that a
/// [`RemediationClass::SymptomMitigation`] always records the cause as
/// unaddressed AND carries a note (never a silent patch).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remediation {
    pub class: RemediationClass,
    /// True when the root cause was addressed (root-cause action or acknowledged
    /// deliberate block); false for a symptom-only mitigation.
    pub root_cause_addressed: bool,
    /// The surfaced "root cause remains unaddressed" note. `Some` iff `class` is
    /// [`RemediationClass::SymptomMitigation`].
    pub unaddressed_note: Option<String>,
}

impl Remediation {
    /// A root-cause-addressing remediation (self-heal, fix launch, escalate the
    /// systemic defect, deliver/merge, steer).
    pub fn root_cause() -> Self {
        Self {
            class: RemediationClass::RootCause,
            root_cause_addressed: true,
            unaddressed_note: None,
        }
    }

    /// An acknowledged deliberate/intentional block — addressed, nothing to fix.
    pub fn acknowledged() -> Self {
        Self {
            class: RemediationClass::Acknowledged,
            root_cause_addressed: true,
            unaddressed_note: None,
        }
    }

    /// A symptom-only mitigation: the root cause stays live and is surfaced via
    /// `note` (never silently patched).
    pub fn symptom(note: impl Into<String>) -> Self {
        Self {
            class: RemediationClass::SymptomMitigation,
            root_cause_addressed: false,
            unaddressed_note: Some(note.into()),
        }
    }

    /// Short, stable label for logs/feeds.
    pub fn class_label(&self) -> &'static str {
        match self.class {
            RemediationClass::RootCause => "root-cause",
            RemediationClass::SymptomMitigation => "symptom-mitigation",
            RemediationClass::Acknowledged => "acknowledged",
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
    /// The root-cause classification of this action (issue #2635): whether it
    /// targets the root cause or only mitigates the symptom.
    pub remediation: Remediation,
}
