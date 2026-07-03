//! First-class guardrails for the Overseer: the autonomy boundary (routine vs
//! HIGH-RISK), anti-recursion (never act on its own work; never entangle with
//! Simard's OODA loop), budget caps, and conflict-avoidance sequencing.
//!
//! These mirror `docs/concepts/operational-autonomy-model.md` and are enforced
//! ON TOP of Simard's existing always-on floors
//! (`crate::git_guardrails`, `crate::ado_acl_guard`) — they never replace them.

use crate::overseer::capabilities::OverseerError;
use crate::overseer::intervention::Intervention;

/// Risk classification for an intervention. Routine interventions execute
/// autonomously (per the operator directive "for most operations she should not
/// need outside-party validation"); HIGH-RISK ones are gated and surface to the
/// human via `Intervention::Escalate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskClass {
    Routine,
    HighRisk,
}

/// Classify an intervention. HIGH-RISK maps to the five gated operations of the
/// autonomy model (deploy is the Overseer's one self-mutating action; conflict
/// resolution can involve force-adjacent pushes; escalation is inherently a
/// hand-off). Everything else is routine.
pub fn classify(iv: &Intervention) -> RiskClass {
    match iv {
        // Self-mutating binary swap of the live daemon.
        Intervention::Deploy { .. } => RiskClass::HighRisk,
        // Conflict resolution can touch shared/protected history; gate it so the
        // `--no-verify` push path is never taken unattended by default.
        Intervention::ResolveConflict { .. } => RiskClass::HighRisk,
        // Escalation is, by definition, a request for human sign-off.
        Intervention::Escalate { .. } => RiskClass::HighRisk,
        _ => RiskClass::Routine,
    }
}

/// Admits routine interventions; gates HIGH-RISK ones unless the operator has
/// explicitly opted in (default `false`). A gated intervention is not executed —
/// it is turned into an `Escalate` in the plan.
#[derive(Clone, Copy, Debug, Default)]
pub struct AutonomyGate {
    pub allow_high_risk: bool,
}

impl AutonomyGate {
    pub fn admit(&self, iv: &Intervention) -> Result<(), OverseerError> {
        match classify(iv) {
            RiskClass::Routine => Ok(()),
            RiskClass::HighRisk if self.allow_high_risk => Ok(()),
            RiskClass::HighRisk => Err(OverseerError::Gated {
                intervention: iv.label().to_string(),
                risk: "high",
            }),
        }
    }
}

/// A subject the Overseer might act on. Used by `RecursionGuard` to refuse the
/// Overseer's own artifacts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Pr {
        repo: String,
        pr: u32,
        author: String,
    },
    Commit {
        sha: String,
        author: String,
    },
    Branch {
        name: String,
    },
    Goal {
        id: String,
        source: String,
    },
}

/// Anti-recursion identity. The Overseer must never verify/merge/deploy its OWN
/// PRs/commits, never sweep branches it created, and never re-open goals it
/// filed. Combined with the architectural fact that the Overseer is a co-process
/// (NOT a `CognitiveThread`), this guarantees it neither schedules nor is
/// scheduled by Simard's OODA loop — the two loops never drive each other.
#[derive(Clone, Debug, Default)]
pub struct RecursionGuard {
    /// The GitHub login the Overseer's own workstreams author under.
    pub author_login: String,
    /// Branch prefix the Overseer's launched recipes use (e.g. `overseer/`).
    pub branch_prefix: String,
    /// Goal-source tag the Overseer stamps on goals it files (e.g. `overseer:`).
    pub goal_source_tag: String,
}

impl RecursionGuard {
    /// True if `subject` is the Overseer's own work and must not be acted on.
    pub fn is_own(&self, subject: &Subject) -> bool {
        match subject {
            Subject::Pr { author, .. } | Subject::Commit { author, .. } => {
                !self.author_login.is_empty() && author == &self.author_login
            }
            Subject::Branch { name } => {
                !self.branch_prefix.is_empty() && name.starts_with(&self.branch_prefix)
            }
            Subject::Goal { source, .. } => {
                !self.goal_source_tag.is_empty() && source.starts_with(&self.goal_source_tag)
            }
        }
    }

    /// Convenience: refuse acting on own work with a typed error.
    pub fn admit(&self, subject: &Subject) -> Result<(), OverseerError> {
        if self.is_own(subject) {
            Err(OverseerError::Recursion {
                subject: format!("{subject:?}"),
            })
        } else {
            Ok(())
        }
    }
}

/// Budget cap checked before any cost-bearing intervention (recipe launches,
/// audits). Reads today's spend vs the daily budget.
///
/// **Reuse:** `crate::cost_tracking::daily_summary` for spend;
/// `SIMARD_DAILY_BUDGET_USD` for the ceiling (the same knob the OODA loop uses).
#[derive(Clone, Copy, Debug)]
pub struct BudgetGate {
    pub daily_budget_usd: f64,
}

impl Default for BudgetGate {
    fn default() -> Self {
        // Mirrors the OODA loop's default daily budget.
        Self {
            daily_budget_usd: 500.0,
        }
    }
}

impl BudgetGate {
    pub fn admit(&self, spent_today_usd: f64) -> Result<(), OverseerError> {
        if self.daily_budget_usd > 0.0 && spent_today_usd >= self.daily_budget_usd {
            Err(OverseerError::Budget {
                spent_usd: spent_today_usd,
                budget_usd: self.daily_budget_usd,
            })
        } else {
            Ok(())
        }
    }
}

/// Conflict-avoidance sequencing. Feature recipes may run in parallel, but
/// mechanical sweeps that touch the same shared files (e.g. OODA-core renames,
/// `print!` purges) must run ONE-AT-A-TIME. The sequencer admits at most one
/// active sweep per `sequence_group`.
#[derive(Clone, Debug, Default)]
pub struct ConflictSequencer {
    active_groups: Vec<String>,
}

impl ConflictSequencer {
    /// Admit a workstream for `group` (None = unsequenced feature work, always
    /// admitted). A sequenced group is refused while one of its sweeps is active.
    pub fn admit(&mut self, group: Option<&str>) -> Result<(), OverseerError> {
        match group {
            None => Ok(()),
            Some(g) => {
                if self.active_groups.iter().any(|a| a == g) {
                    Err(OverseerError::Conflict {
                        with: g.to_string(),
                    })
                } else {
                    self.active_groups.push(g.to_string());
                    Ok(())
                }
            }
        }
    }

    /// Release a sequence group when its sweep completes.
    pub fn release(&mut self, group: &str) {
        self.active_groups.retain(|a| a != group);
    }
}
