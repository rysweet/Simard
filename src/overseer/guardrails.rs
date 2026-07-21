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
/// need outside-party validation"); `MergeAuthority` and `HighRisk` ones are
/// gated and surface to the human via `Intervention::Escalate` unless the
/// operator has explicitly opted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskClass {
    Routine,
    /// PR verify-and-merge. **Opt-in, NOT Routine on day one** (crusty review
    /// risk #1): autonomous-merge is a closed self-modification loop gated only
    /// by "CI green", which is the absence of one signal, not judgment. It stays
    /// human-in-the-loop until M1's signal quality is proven — autonomy is
    /// earned, not defaulted.
    MergeAuthority,
    HighRisk,
}

/// Classify an intervention. `HighRisk` maps to the gated operations of the
/// autonomy model (deploy is the Overseer's one self-mutating action; conflict
/// resolution can involve force-adjacent pushes; escalation is inherently a
/// hand-off). `VerifyAndMergePr` is `MergeAuthority` — deliberately opt-in
/// rather than Routine (crusty risk #1). Everything else is routine.
pub fn classify(iv: &Intervention) -> RiskClass {
    match iv {
        // Self-mutating binary swap of the live daemon.
        Intervention::Deploy { .. } => RiskClass::HighRisk,
        // Conflict resolution can touch shared/protected history; gate it so the
        // `--no-verify` push path is never taken unattended by default.
        Intervention::ResolveConflict { .. } => RiskClass::HighRisk,
        // Escalation is, by definition, a request for human sign-off.
        Intervention::Escalate { .. } => RiskClass::HighRisk,
        // Merge authority is opt-in until proven (crusty risk #1).
        Intervention::VerifyAndMergePr { .. } => RiskClass::MergeAuthority,
        // The agentic merge-queue hygiene actions (#4097) touch the merge queue
        // (comment on / close an open PR), so they share the SAME opt-in
        // MergeAuthority class as VerifyAndMergePr — notify-only until the
        // operator flips `allow_verify_merge`. Their argv can never carry
        // `--admin`/`--no-verify` (see intervention.rs argv builders).
        Intervention::FlagStalePr { .. } | Intervention::CloseDuplicatePr { .. } => {
            RiskClass::MergeAuthority
        }
        // A whisper takes NO action on Simard's behalf and spends no budget — it
        // is advisory context only. Routine; its own dedup/identity gates
        // ([`WhisperGate`] / [`RecursionGuard`]) apply in the act path.
        Intervention::Whisper { .. } => RiskClass::Routine,
        // Goal-board self-heal and escalation are routine stewardship: unblocking
        // a false-parked perpetual goal restores the shipped `simard goal unblock`
        // state, and escalation is a notification. Both are deduped and
        // identity-gated (fail-closed) in the act path, and spend no LLM budget.
        Intervention::UnblockGoal { .. } | Intervention::EscalateBlockedGoal { .. } => {
            RiskClass::Routine
        }
        // Flagging backlog-coverage gaps is routine stewardship: it notifies the
        // operator without creating recursive tracking work.
        // Its identity/dedup gates (fail-closed [`RecursionGuard`] + the per-gap
        // dedup gate) apply in the act path.
        Intervention::FlagWorkstreamGaps { .. } => RiskClass::Routine,
        _ => RiskClass::Routine,
    }
}

/// Admits routine interventions; gates `MergeAuthority` and `HighRisk` ones
/// unless the operator has explicitly opted in (both default `false`). A gated
/// intervention is not executed — it is turned into an `Escalate` in the plan.
#[derive(Clone, Copy, Debug, Default)]
pub struct AutonomyGate {
    /// Opt into autonomous HIGH-RISK execution (deploy / conflict-resolution).
    pub allow_high_risk: bool,
    /// Opt into autonomous PR verify-and-merge (crusty risk #1: default `false`
    /// — the operator must enable it, and only after M1 signals are proven).
    pub allow_verify_merge: bool,
}

impl AutonomyGate {
    pub fn admit(&self, iv: &Intervention) -> Result<(), OverseerError> {
        match classify(iv) {
            RiskClass::Routine => Ok(()),
            RiskClass::MergeAuthority if self.allow_verify_merge => Ok(()),
            RiskClass::HighRisk if self.allow_high_risk => Ok(()),
            RiskClass::MergeAuthority => Err(OverseerError::Gated {
                intervention: iv.label().to_string(),
                risk: "merge-authority",
            }),
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

    /// True only when every identity field is populated. A guard missing any
    /// field cannot reliably recognise its own artifacts, so `admit` must fail
    /// CLOSED (see below) rather than silently wave work through.
    pub fn is_configured(&self) -> bool {
        !self.author_login.is_empty()
            && !self.branch_prefix.is_empty()
            && !self.goal_source_tag.is_empty()
    }

    /// Gate acting on a subject. Fails **CLOSED** (crusty risk #3): when the
    /// identity needed to classify `subject` is unconfigured, REFUSE rather than
    /// allow — an anti-recursion guard that disables itself when misconfigured is
    /// worse than none. PR/commit subjects require `author_login`; branch
    /// subjects require `branch_prefix`; goal subjects require `goal_source_tag`.
    /// When configured, refuse only the Overseer's OWN work (`is_own`).
    ///
    /// The Overseer must run under a DISTINCT identity (never the human
    /// operator's login), so a correctly-configured guard admits the operator's
    /// PRs while still refusing its own.
    pub fn admit(&self, subject: &Subject) -> Result<(), OverseerError> {
        let unconfigured = match subject {
            Subject::Pr { .. } | Subject::Commit { .. } => self.author_login.is_empty(),
            Subject::Branch { .. } => self.branch_prefix.is_empty(),
            Subject::Goal { .. } => self.goal_source_tag.is_empty(),
        };
        if unconfigured {
            return Err(OverseerError::Recursion {
                subject: format!("unconfigured-identity: {subject:?}"),
            });
        }
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
        // Single-sourced from `SIMARD_DAILY_BUDGET_USD` (crusty risk #6): the
        // Overseer's ceiling can never drift from the OODA loop's. The 500.0
        // fallback lives in one place — `config::resolve_daily_budget_usd`.
        Self::from_env()
    }
}

impl BudgetGate {
    /// Construct from the single-sourced `SIMARD_DAILY_BUDGET_USD` env knob.
    pub fn from_env() -> Self {
        Self {
            daily_budget_usd: crate::overseer::config::daily_budget_usd(),
        }
    }

    /// Construct with an explicit ceiling (used by callers that already resolved
    /// the budget, and by tests).
    pub fn with_budget(daily_budget_usd: f64) -> Self {
        Self { daily_budget_usd }
    }

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

    /// Clear all active groups. The sequencer serialises the sweeps PLANNED
    /// within a single OODA cycle (like the per-cycle launch cap); cross-cycle
    /// dedup is the in-flight guard's + backoff gate's job. Reset at the top of
    /// each [`crate::overseer::Overseer::run_cycle`] so a group admitted in a
    /// prior cycle never permanently locks out later coverage of that group.
    pub fn reset(&mut self) {
        self.active_groups.clear();
    }
}

/// A whisper gate's decision: deliver the whisper, or suppress it as a duplicate
/// (same signature within the dedup window) or because the per-hour cap is spent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhisperDecision {
    Deliver,
    SuppressDuplicate,
    SuppressCapReached,
}

/// Dedup + rate-limit gate for whispers (mirrors the [`BudgetGate`] /
/// [`ConflictSequencer`] guardrail style). Two limits, both on an INJECTED clock
/// (`now_secs`) so the daemon uses wall-clock while tests drive a virtual clock:
///
/// 1. **Dedup window** — the same whisper signature is suppressed while it is
///    within its active window of its last delivery, so a persistent condition
///    is not re-injected every cycle. Under [`with_backoff`](WhisperGate::with_backoff)
///    this window grows exponentially per re-delivery (capped); the fixed-window
///    [`new`](WhisperGate::new) keeps it constant.
/// 2. **Per-hour cap** — at most `cap_per_hour` whispers are delivered within any
///    rolling hour, so a noisy Overseer cannot flood Simard's inbox.
///
/// Only DELIVERED whispers count toward either limit; a suppressed whisper is
/// never recorded. [`admit`](WhisperGate::admit) is the combined decide+commit
/// used in unit tests; the act path peeks then commits only on a successful
/// delivery so a failed/panicking sink does not consume the dedup slot.
#[derive(Clone, Debug)]
pub struct WhisperGate {
    /// First (strike-1) suppression window. Also the constant window in the
    /// fixed-window `new(..)` mode (where `base_secs == cap_secs`).
    base_secs: i64,
    /// Upper bound the per-signature window may grow to under backoff. Equal to
    /// `base_secs` in fixed-window mode, so the window never grows there.
    cap_secs: i64,
    cap_per_hour: usize,
    last_delivered: std::collections::HashMap<String, i64>,
    /// Per-signature delivery (strike) count. Drives the exponential backoff
    /// window; only committed deliveries advance it.
    strikes: std::collections::HashMap<String, u32>,
    deliveries: Vec<i64>,
}

impl WhisperGate {
    /// A gate with a fixed `window_secs` dedup window and a `cap_per_hour`
    /// rolling-hour delivery cap. Equivalent to a degenerate backoff whose
    /// window never grows (`base == cap`), preserving the original semantics.
    pub fn new(window_secs: i64, cap_per_hour: usize) -> Self {
        Self::with_backoff(window_secs, window_secs, cap_per_hour)
    }

    /// A gate whose per-signature suppression window grows EXPONENTIALLY on each
    /// re-delivery of the SAME signature: the first window is `base_secs`, then
    /// doubles per re-delivery, capped at `cap_secs`. The `cap_per_hour`
    /// rolling-hour delivery cap applies exactly as in [`new`](WhisperGate::new).
    ///
    /// Because the blocked-goal signature is per goal id (`escalate:{goal_id}` /
    /// `unblock:{goal_id}`), a change in the blocked-goal SET surfaces as a NEW
    /// signature that fires immediately (strike 0) — the "re-fires on state
    /// change" contract — while a persistently-blocked goal is escalated once
    /// and then suppressed for a widening cooldown instead of every tick.
    pub fn with_backoff(base_secs: i64, cap_secs: i64, cap_per_hour: usize) -> Self {
        Self {
            base_secs,
            cap_secs: cap_secs.max(base_secs),
            cap_per_hour,
            last_delivered: std::collections::HashMap::new(),
            strikes: std::collections::HashMap::new(),
            deliveries: Vec::new(),
        }
    }

    /// The active suppression window for `signature`, given how many times it has
    /// already been delivered: `min(base * 2^(strikes - 1), cap)`. A signature
    /// with no prior delivery uses the base window. Saturates instead of
    /// overflowing for very large strike counts.
    fn window_for(&self, signature: &str) -> i64 {
        let strikes = self.strikes.get(signature).copied().unwrap_or(0);
        if strikes <= 1 {
            return self.base_secs;
        }
        let exp = strikes - 1;
        let grown = if exp >= 62 {
            self.cap_secs
        } else {
            self.base_secs
                .checked_mul(1i64 << exp)
                .unwrap_or(self.cap_secs)
        };
        grown.min(self.cap_secs)
    }

    /// Decide WITHOUT recording — the act path uses this so it can commit only
    /// after a successful delivery.
    pub fn peek(&self, signature: &str, now_secs: i64) -> WhisperDecision {
        if let Some(&last) = self.last_delivered.get(signature)
            && now_secs - last < self.window_for(signature)
        {
            return WhisperDecision::SuppressDuplicate;
        }
        let hour_ago = now_secs - 3600;
        let recent = self.deliveries.iter().filter(|&&t| t > hour_ago).count();
        if recent >= self.cap_per_hour {
            return WhisperDecision::SuppressCapReached;
        }
        WhisperDecision::Deliver
    }

    /// Record a successful delivery of `signature` at `now_secs` (advances the
    /// per-signature strike/backoff window and the rolling-hour budget, pruning
    /// stale entries).
    pub fn commit(&mut self, signature: &str, now_secs: i64) {
        self.last_delivered.insert(signature.to_string(), now_secs);
        *self.strikes.entry(signature.to_string()).or_insert(0) += 1;
        self.deliveries.push(now_secs);
        let hour_ago = now_secs - 3600;
        self.deliveries.retain(|&t| t > hour_ago);
    }

    /// Decide and, on `Deliver`, record the delivery in one call.
    pub fn admit(&mut self, signature: &str, now_secs: i64) -> WhisperDecision {
        let decision = self.peek(signature, now_secs);
        if decision == WhisperDecision::Deliver {
            self.commit(signature, now_secs);
        }
        decision
    }
}

/// A [`BackoffGate`]'s decision for one key at one instant: surface the signal /
/// launch (`Admit`) or hold it as a duplicate within the current backoff window
/// (`Suppress`). Suppression can only ever REDUCE actions; on any ambiguity
/// (clock regression, overflow) the gate fails toward `Admit` — it never
/// permanently silences a real, recurring signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackoffDecision {
    Admit,
    Suppress,
}

/// Per-key exponential-backoff suppressor on an INJECTED `now_secs` clock
/// (Problem 1 / issue #4186; meta bugs #4255, #4126). It closes the
/// duplicate-issue hole where the Overseer re-launches an identical
/// `WorkstreamCoverage` recipe (or re-emits an identical recurring signature)
/// every tick once the prior covering workstream has COMPLETED — the exact
/// churn that spawned duplicate backlog issues
/// (#4186/#4190/#4191/#4198/#4201/#4203/#4206).
///
/// Contract (mirrors the [`WhisperGate`] peek/commit split so the act path can
/// commit only after a successful launch):
///
/// * [`new`](BackoffGate::new)`(base_window_secs, multiplier, max_window_secs)`.
/// * [`peek`](BackoffGate::peek) decides WITHOUT recording.
/// * [`commit`](BackoffGate::commit) records an admitted occurrence and GROWS
///   that key's window `× multiplier` (saturating, hard-capped at
///   `max_window_secs`); after a silence `>= 2 × current_window` the window
///   RESETS to `base_window_secs`, so a genuinely-recurring gap is never
///   permanently silenced.
/// * [`admit`](BackoffGate::admit) = peek-then-commit-on-`Admit`.
///
/// Every key tracks an independent window, so one noisy signature can never
/// starve an unrelated one. A non-monotonic clock (`now` before the last admit)
/// is treated as "window elapsed" and admits (fail toward surfacing).
#[derive(Clone, Debug)]
pub struct BackoffGate {
    base_window_secs: i64,
    multiplier: i64,
    max_window_secs: i64,
    /// Per key: `(last_admit_secs, current_window_secs)`.
    state: std::collections::HashMap<String, (i64, i64)>,
}

impl BackoffGate {
    /// A gate with a `base_window_secs` initial suppression window that grows
    /// `× multiplier` per admitted occurrence, hard-capped at `max_window_secs`.
    pub fn new(base_window_secs: i64, multiplier: i64, max_window_secs: i64) -> Self {
        Self {
            base_window_secs,
            multiplier,
            max_window_secs,
            state: std::collections::HashMap::new(),
        }
    }

    /// Decide WITHOUT recording — the act path uses this so it can commit only
    /// after a successful launch/emit (a failed launch must not consume the
    /// dedup slot). An unseen key, an elapsed window, or a backwards clock jump
    /// all admit; only a re-occurrence strictly INSIDE the current window is
    /// suppressed.
    pub fn peek(&self, key: &str, now_secs: i64) -> BackoffDecision {
        match self.state.get(key) {
            None => BackoffDecision::Admit,
            Some(&(last_admit, window)) => {
                let elapsed = now_secs - last_admit;
                // Clock regression ⇒ fail toward surfacing (never suppress on a
                // clock we cannot trust).
                if elapsed < 0 || elapsed >= window {
                    BackoffDecision::Admit
                } else {
                    BackoffDecision::Suppress
                }
            }
        }
    }

    /// Record an admitted occurrence of `key` at `now_secs`, arming (or growing)
    /// its suppression window. The first occurrence arms the base window; each
    /// subsequent one grows it `× multiplier` (saturating, capped), UNLESS the
    /// silence since the last admit was `>= 2 × current_window`, in which case
    /// the window resets to the base (so a real recurrence resurfaces promptly).
    pub fn commit(&mut self, key: &str, now_secs: i64) {
        let next_window = match self.state.get(key) {
            None => self.base_window_secs,
            Some(&(last_admit, window)) => {
                let elapsed = now_secs - last_admit;
                // A backwards clock or a long silence (>= 2× the current window)
                // resets to the base window rather than growing it.
                if elapsed < 0 || elapsed >= window.saturating_mul(2) {
                    self.base_window_secs
                } else {
                    window
                        .saturating_mul(self.multiplier)
                        .min(self.max_window_secs)
                }
            }
        };
        self.state.insert(key.to_string(), (now_secs, next_window));
    }

    /// Decide and, on `Admit`, record the occurrence in one call. Used by unit
    /// tests; the production act path peeks then commits on success.
    pub fn admit(&mut self, key: &str, now_secs: i64) -> BackoffDecision {
        let decision = self.peek(key, now_secs);
        if decision == BackoffDecision::Admit {
            self.commit(key, now_secs);
        }
        decision
    }

    /// Evict `key`'s suppression state entirely, so the next [`peek`] admits as
    /// if the key had never been seen. The act path calls this when a subject
    /// reaches a TERMINAL state (e.g. a PR merges or closes), so the per-key
    /// `state` map is not kept growing one permanent entry per subject ever
    /// surveyed — it is bounded to the set of CURRENTLY-open, still-recurring
    /// subjects. A no-op for an unknown key, and always fail-safe: dropping an
    /// entry can only make the gate MORE willing to surface, never less.
    pub fn forget(&mut self, key: &str) {
        self.state.remove(key);
    }

    /// Number of keys currently tracked. Test-only visibility into map growth so
    /// the evict-on-terminal-state contract (bounded `state`) is provable.
    #[cfg(test)]
    pub fn tracked_keys(&self) -> usize {
        self.state.len()
    }
}

/// Stable, namespaced dedup key for one PR's verify-and-merge escalation
/// (issue #4344). The [`BackoffGate`] on the `VerifyAndMergePr` Act path keys on
/// this so a CLEAN + MERGEABLE + all-SUCCESS PR that keeps (correctly or
/// spuriously) escalating pages the operator ONCE per bounded window instead of
/// every ~15-minute tick (PRs #4344 / #4145).
///
/// The `verify_and_merge:` prefix is a fixed namespace that is DISJOINT from the
/// coverage / recall backoff namespaces (`overseer-obs:`, recipe task
/// descriptions), so the merge-escalation rail can never suppress — or be
/// suppressed by — any other Overseer dedup gate. Deterministic per `(repo, pr)`
/// and collision-free across distinct PRs and repos.
pub fn verify_and_merge_dedup_key(repo: &str, pr: u32) -> String {
    format!("verify_and_merge:{repo}#{pr}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_guard() -> RecursionGuard {
        RecursionGuard {
            author_login: "simard-overseer[bot]".to_string(),
            branch_prefix: "overseer/".to_string(),
            goal_source_tag: "overseer:".to_string(),
        }
    }

    // ── item 4: RecursionGuard must fail CLOSED ──────────────────────────────

    #[test]
    fn admit_fails_closed_when_identity_unconfigured() {
        // A default guard has empty identity — it cannot recognise its own work,
        // so `admit` must REFUSE, not allow (a guard that silently disables
        // itself when misconfigured is worse than none).
        let guard = RecursionGuard::default();
        assert!(!guard.is_configured());

        let pr = Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 1,
            author: "anyone".to_string(),
        };
        let commit = Subject::Commit {
            sha: "abc123".to_string(),
            author: "anyone".to_string(),
        };
        let branch = Subject::Branch {
            name: "feature/x".to_string(),
        };
        let goal = Subject::Goal {
            id: "g1".to_string(),
            source: "ooda".to_string(),
        };

        assert!(
            guard.admit(&pr).is_err(),
            "unconfigured guard must refuse a PR subject (fail closed)"
        );
        assert!(
            guard.admit(&commit).is_err(),
            "unconfigured guard must refuse a commit subject (fail closed)"
        );
        assert!(
            guard.admit(&branch).is_err(),
            "unconfigured guard must refuse a branch subject (fail closed)"
        );
        assert!(
            guard.admit(&goal).is_err(),
            "unconfigured guard must refuse a goal subject (fail closed)"
        );
    }

    #[test]
    fn admit_refuses_own_work_when_configured() {
        let guard = configured_guard();
        assert!(guard.is_configured());

        let own_pr = Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 1,
            author: "simard-overseer[bot]".to_string(),
        };
        let own_branch = Subject::Branch {
            name: "overseer/fix-distill".to_string(),
        };
        let own_goal = Subject::Goal {
            id: "g1".to_string(),
            source: "overseer:distill".to_string(),
        };
        assert!(guard.admit(&own_pr).is_err());
        assert!(guard.admit(&own_branch).is_err());
        assert!(guard.admit(&own_goal).is_err());
    }

    #[test]
    fn admit_allows_operator_work_under_distinct_identity() {
        // The Overseer runs under a DISTINCT login, so a correctly-configured
        // guard admits the human operator's PRs (they are not the Overseer's own).
        let guard = configured_guard();
        let operator_pr = Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 2,
            author: "rysweet".to_string(),
        };
        let foreign_branch = Subject::Branch {
            name: "feature/human-work".to_string(),
        };
        assert!(guard.admit(&operator_pr).is_ok());
        assert!(guard.admit(&foreign_branch).is_ok());
    }

    // ── item 7: BudgetGate is single-sourced from the env knob ───────────────

    #[test]
    fn budget_gate_holds_at_or_over_ceiling() {
        let gate = BudgetGate::with_budget(500.0);
        assert!(gate.admit(499.99).is_ok());
        assert!(gate.admit(500.0).is_err(), "spend at ceiling must hold");
        assert!(gate.admit(600.0).is_err(), "spend over ceiling must hold");
    }

    #[test]
    fn budget_gate_from_env_has_positive_ceiling() {
        // Correctness of env parsing is covered by `config` unit tests; here we
        // only assert the default/from_env path yields a usable positive ceiling
        // (never the removed hardcoded duplicate).
        assert!(BudgetGate::from_env().daily_budget_usd > 0.0);
        assert!(BudgetGate::default().daily_budget_usd > 0.0);
    }

    // ── crusty risk #1: VerifyAndMergePr is opt-in, not Routine ──────────────

    #[test]
    fn verify_merge_is_opt_in_not_routine() {
        let iv = Intervention::VerifyAndMergePr {
            repo: "rysweet/Simard".to_string(),
            pr: 1,
        };
        assert_eq!(
            classify(&iv),
            RiskClass::MergeAuthority,
            "merge authority is its own opt-in class, never Routine"
        );
        // The default gate REFUSES it — autonomy is earned, not defaulted.
        assert!(AutonomyGate::default().admit(&iv).is_err());
        // Opt-in admits merge WITHOUT enabling high-risk deploy…
        let gate = AutonomyGate {
            allow_verify_merge: true,
            allow_high_risk: false,
        };
        assert!(gate.admit(&iv).is_ok());
        // …and enabling merge must NOT enable deploy/conflict-resolution.
        assert!(
            gate.admit(&Intervention::Deploy {
                commit: "abc".to_string()
            })
            .is_err(),
            "merge opt-in must not leak into HIGH-RISK deploy authority"
        );
    }

    // ── verify-and-merge escalation dedup KEY (issue #4344) ──────────────────
    //
    // TDD (RED): `verify_and_merge_dedup_key(repo, pr)` does not exist yet, so
    // this block will FAIL TO COMPILE against the current tree — that is the red
    // state. The key is the stable signature the `merge_escalation_backoff`
    // `BackoffGate` dedups on so a CLEAN + MERGEABLE + all-SUCCESS PR that keeps
    // (correctly or spuriously) escalating is paged to the operator ONCE per
    // backoff window instead of every ~15-minute tick (PRs #4344 / #4145).

    #[test]
    fn verify_and_merge_dedup_key_has_the_stable_namespaced_shape() {
        // Deterministic, human-greppable, and NAMESPACED under a fixed prefix so
        // it can never collide with any other Overseer dedup signature.
        let key = verify_and_merge_dedup_key("rysweet/Simard", 4344);
        assert_eq!(
            key, "verify_and_merge:rysweet/Simard#4344",
            "the dedup key must be exactly 'verify_and_merge:{{repo}}#{{pr}}'"
        );
        assert!(
            key.starts_with("verify_and_merge:"),
            "the key must carry the fixed 'verify_and_merge:' namespace prefix"
        );
        assert!(
            key.contains("#4344"),
            "the key must encode the PR number after a '#' separator"
        );
    }

    #[test]
    fn verify_and_merge_dedup_key_is_deterministic() {
        // The same (repo, pr) ALWAYS yields the same key — otherwise the backoff
        // gate could never recognise a repeat escalation of the same PR.
        assert_eq!(
            verify_and_merge_dedup_key("rysweet/Simard", 4145),
            verify_and_merge_dedup_key("rysweet/Simard", 4145),
        );
    }

    #[test]
    fn verify_and_merge_dedup_key_is_collision_free_across_prs_and_repos() {
        let a = verify_and_merge_dedup_key("rysweet/Simard", 4344);
        let b = verify_and_merge_dedup_key("rysweet/Simard", 4145);
        let c = verify_and_merge_dedup_key("rysweet/Other", 4344);
        assert_ne!(a, b, "distinct PR numbers must produce distinct keys");
        assert_ne!(
            a, c,
            "the same PR number in a DIFFERENT repo must not collide"
        );
        assert_ne!(b, c, "distinct (repo, pr) pairs must never collide");
    }

    #[test]
    fn verify_and_merge_dedup_key_namespace_is_disjoint_from_the_coverage_gate() {
        // The coverage backoff keys off recipe task descriptions / the
        // `overseer-obs:` recall prefix; the merge-escalation gate MUST live in
        // its own namespace so the two rails can never suppress each other.
        let key = verify_and_merge_dedup_key("rysweet/Simard", 4344);
        assert!(
            !key.starts_with("overseer-obs:"),
            "the merge-escalation namespace must be disjoint from the recall/coverage namespace"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TDD (issue #4255): per-signature EXPONENTIAL BACKOFF for the blocked-goal
// escalation path.
//
// These tests are written FIRST and MUST FAIL until `WhisperGate` grows an
// additive `with_backoff` constructor. They pin the contract:
//
//   WhisperGate::with_backoff(base_secs: i64, cap_secs: i64, cap_per_hour: usize)
//
// Semantics (additive; the existing fixed-window `new(..)` is unchanged):
//   * The suppression window for a signature GROWS each time that *same*
//     signature is DELIVERED (committed). With `strikes` = number of prior
//     deliveries of the signature, the active window is
//         window = min(base_secs * 2^(strikes - 1), cap_secs)
//     i.e. first window == `base_secs`, then doubles per re-delivery, capped.
//   * Only DELIVERED (committed) whispers advance the strike count — a
//     suppressed peek never grows the window (mirrors the act path, which
//     commits only after a successful dispatch).
//   * Backoff is PER SIGNATURE: distinct signatures have independent strike
//     counts and windows. Because the blocked-goal signature is per goal id
//     (`escalate:{goal_id}` / `unblock:{goal_id}`), a change in the blocked-goal
//     SET surfaces as a *new* signature that fires immediately (strike 0) — this
//     is the "re-fires on state change" requirement.
//   * The rolling-hour delivery cap (`cap_per_hour`) still applies exactly as in
//     `new(..)`.
#[cfg(test)]
mod whisper_backoff_tests {
    use super::*;

    const BASE: i64 = 100;
    const CAP: i64 = 1000;

    /// A single blocked-goal cluster that stays blocked escalates ONCE, is then
    /// suppressed within the (growing) window, and re-fires only after the
    /// cooldown elapses — with the window DOUBLING on each re-delivery.
    #[test]
    fn same_signature_backoff_grows_exponentially_until_capped() {
        let mut gate = WhisperGate::with_backoff(BASE, CAP, 1000);
        let sig = "escalate:goal-alpha";

        // Strike 1: first escalation delivers; window becomes `BASE` (=100).
        assert_eq!(gate.admit(sig, 0), WhisperDecision::Deliver);
        assert_eq!(gate.peek(sig, 50), WhisperDecision::SuppressDuplicate);
        assert_eq!(gate.peek(sig, 99), WhisperDecision::SuppressDuplicate);

        // Strike 2: window (100) elapsed -> re-fire; next window doubles to 200.
        assert_eq!(gate.admit(sig, 100), WhisperDecision::Deliver);
        assert_eq!(
            gate.peek(sig, 299),
            WhisperDecision::SuppressDuplicate,
            "199s after the 2nd delivery is still inside the 200s window"
        );

        // Strike 3: window (200) elapsed -> re-fire; next window doubles to 400.
        assert_eq!(gate.admit(sig, 300), WhisperDecision::Deliver);
        assert_eq!(gate.peek(sig, 699), WhisperDecision::SuppressDuplicate);

        // Strike 4: window (400) elapsed -> re-fire; next window doubles to 800.
        assert_eq!(gate.admit(sig, 700), WhisperDecision::Deliver);

        // Strike 5: window (800) elapsed -> re-fire; next window would be 1600
        // but is CAPPED at CAP (=1000).
        assert_eq!(gate.admit(sig, 1500), WhisperDecision::Deliver);
        assert_eq!(
            gate.peek(sig, 2499),
            WhisperDecision::SuppressDuplicate,
            "999s after the capped delivery is still inside the 1000s cap window"
        );
        assert_eq!(
            gate.peek(sig, 2500),
            WhisperDecision::Deliver,
            "the window must never exceed the cap"
        );
    }

    /// Backoff state is isolated per signature: one goal's escalating cooldown
    /// must not suppress a DIFFERENT goal's first escalation.
    #[test]
    fn backoff_is_per_signature() {
        let mut gate = WhisperGate::with_backoff(BASE, CAP, 1000);

        assert_eq!(
            gate.admit("escalate:goal-alpha", 0),
            WhisperDecision::Deliver
        );
        // A distinct goal escalates immediately (its own strike 0), even though
        // goal-alpha is mid-cooldown.
        assert_eq!(
            gate.admit("escalate:goal-beta", 10),
            WhisperDecision::Deliver
        );

        assert_eq!(
            gate.peek("escalate:goal-alpha", 50),
            WhisperDecision::SuppressDuplicate
        );
        assert_eq!(
            gate.peek("escalate:goal-beta", 50),
            WhisperDecision::SuppressDuplicate
        );
    }

    /// A change to the blocked-goal SET presents as a NEW signature (per-goal
    /// key), which must fire immediately regardless of another goal's active
    /// cooldown — this is the "re-fires on cluster/state change" contract.
    #[test]
    fn cluster_change_new_signature_fires_immediately() {
        let mut gate = WhisperGate::with_backoff(BASE, CAP, 1000);

        assert_eq!(
            gate.admit("escalate:goal-alpha", 0),
            WhisperDecision::Deliver
        );
        assert_eq!(
            gate.peek("escalate:goal-alpha", 30),
            WhisperDecision::SuppressDuplicate
        );

        // The cluster changes: goal-gamma is newly blocked. Its signature has
        // never been delivered, so it escalates at once (strike 0).
        assert_eq!(
            gate.admit("escalate:goal-gamma", 30),
            WhisperDecision::Deliver,
            "a newly-blocked goal must escalate immediately, not inherit backoff"
        );
    }

    /// The rolling-hour delivery cap is preserved by the backoff constructor.
    #[test]
    fn hour_cap_still_enforced_under_backoff() {
        let mut gate = WhisperGate::with_backoff(BASE, CAP, 2);

        assert_eq!(gate.admit("escalate:g-a", 0), WhisperDecision::Deliver);
        assert_eq!(gate.admit("escalate:g-b", 1), WhisperDecision::Deliver);
        // Third distinct signature within the hour trips the cap, not the window.
        assert_eq!(
            gate.admit("escalate:g-c", 2),
            WhisperDecision::SuppressCapReached
        );
    }

    /// `peek` must not advance the strike count — only a committed delivery
    /// grows the window (the act path peeks first, commits only on success).
    #[test]
    fn peek_does_not_grow_backoff_window() {
        let mut gate = WhisperGate::with_backoff(BASE, CAP, 1000);
        let sig = "escalate:goal-delta";

        assert_eq!(gate.admit(sig, 0), WhisperDecision::Deliver);
        // Many peeks while suppressed must not inflate the window beyond BASE.
        for t in 1..100 {
            assert_eq!(gate.peek(sig, t), WhisperDecision::SuppressDuplicate);
        }
        // Exactly one BASE window later it re-fires (peeks did not double it).
        assert_eq!(gate.peek(sig, 100), WhisperDecision::Deliver);
    }

    /// `BackoffGate::forget` evicts a key's armed state so the map stays bounded
    /// to currently-open subjects (issue #4344 review F2). A forgotten key
    /// readmits immediately (as if never seen), and forgetting an unknown key is
    /// a harmless no-op.
    #[test]
    fn backoff_forget_evicts_a_key_and_readmits_immediately() {
        let mut gate = BackoffGate::new(100, 2, 1000);
        let key = "verify_and_merge:rysweet/Simard#4344";

        // Arm the window, then confirm a same-instant repeat is suppressed.
        assert_eq!(gate.admit(key, 0), BackoffDecision::Admit);
        assert_eq!(gate.tracked_keys(), 1, "an admitted key is tracked");
        assert_eq!(gate.peek(key, 1), BackoffDecision::Suppress);

        // Eviction clears the armed state: the very next peek admits and the map
        // no longer retains the key.
        gate.forget(key);
        assert_eq!(gate.tracked_keys(), 0, "forget evicts the entry");
        assert_eq!(
            gate.peek(key, 1),
            BackoffDecision::Admit,
            "a forgotten key must admit as if never seen"
        );

        // Forgetting an unknown key must not panic or add state.
        gate.forget("never-seen");
        assert_eq!(
            gate.tracked_keys(),
            0,
            "forgetting an unknown key is a no-op"
        );
    }
}
