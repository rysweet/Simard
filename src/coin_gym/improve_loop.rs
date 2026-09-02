//! Live self-improvement loop (Phase 5, issue #2825).
//!
//! This is the **live** half of the skwaq-style loop whose offline half lives in
//! [`super::improve`] (failure-analyst + overfitting-reviewer gate). It composes
//! that analyst + gate with a *verify-on-held-out* step and durable memory,
//! mirroring skwaq's `failure-analyst → overfitting-reviewer → verify` cycle
//! (`~/src/skwaq/crates/gym/src/improve.rs`):
//!
//! 1. **Analyse + gate** the run's failures into *general* reachability tactics
//!    (reused from [`super::improve`]); memorising / target-specific tactics are
//!    rejected before they are ever verified.
//! 2. **Apply** an accepted tactic and **re-run on held-out *fresh* targets**
//!    (targets the tactic's motivating failure never saw). Keep it **iff** reach
//!    improves and precision does not drop; otherwise **roll back**.
//! 3. **Overfitting-warning**: if a tactic lifts *training* reach but not
//!    *held-out* reach, that train/held-out gap is flagged — the empirical
//!    complement to the static gate.
//! 4. **Durable memory**: kept tactics are persisted per *general family* (never
//!    per project/target — that would be overfitting) under the profile and
//!    **reused** on subsequent runs.
//!
//! ## Offline scaffold — an *idealized* effect model, honestly
//! Like the rest of the Gym (Phase 4), this runs **offline against a mock
//! oracle** so it is exercised in CI without a VM. It models a tactic's effect
//! **idealistically**: an accepted tactic of family `F` is *assumed* to produce
//! the objective grader's reaching input for **every** in-scope target of `F`,
//! including held-out ones. This exercises the loop's **control flow** —
//! analyse → gate → apply → measure held-out → keep/rollback + memory — but it
//! does **not** prove the tactic *text* would solve fresh targets: the held-out
//! grade is synthesised from the same oracle, so it is illustrative, not
//! empirical. The static gate still rejects tactics that *name* a specific
//! target/project/input, and the held-out **coverage** check distinguishes a
//! genuine train/held-out gap from a mere "no held-out target of this family to
//! confirm against". **Real** empirical held-out verification — running the
//! tactic through a live model and grading with `coin verify` — is **Phase 3**
//! (issue #2823). **LOCAL-ONLY**: nothing here is submitted externally, and the
//! stored oracle is a test double, never a real verdict source.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::agent_runner::Candidate;
use super::execute_run;
use super::improve::{
    ReviewVerdict, ReviewedProposal, TacticCategory, analyze_failures, categorize, review_proposal,
};
use super::profiles::{PersistedRun, profile_dir};
use super::scorer::ReachPrecision;
use super::target_loader::{DemoScenario, TargetSet};
use super::types::{CoinGymError, CoinGymResult, RunReport, Strategy, Target};

/// Filename of a profile's durable tactic memory.
pub const TACTIC_MEMORY_FILE: &str = "tactics.json";

/// Float slack when comparing precision percentages (avoid FP jitter).
const PRECISION_EPS: f64 = 1e-9;

// ── Reach/precision snapshot ─────────────────────────────────────────────────

/// A reach/precision measurement over one slice (training or held-out).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SliceMeasurement {
    /// Targets reached.
    pub reached: usize,
    /// Inputs submitted (precision denominator).
    pub submitted: usize,
    /// Total targets in the slice.
    pub total: usize,
    /// reached / total, as a percentage.
    pub reach_pct: f64,
    /// reached / submitted, as a percentage (0 when nothing submitted).
    pub precision_pct: f64,
}

impl SliceMeasurement {
    fn from_rp(rp: &ReachPrecision) -> Self {
        Self {
            reached: rp.reached,
            submitted: rp.submitted,
            total: rp.total,
            reach_pct: rp.reach_pct(),
            precision_pct: rp.precision_pct(),
        }
    }
}

/// Keep-or-rollback verdict for a verified tactic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TacticDecision {
    /// Held-out reach improved without a precision drop — keep the tactic.
    Keep,
    /// No held-out gain (or a precision regression / already banked) — roll back.
    Rollback,
}

impl TacticDecision {
    /// Uppercase label (`KEEP` / `ROLLBACK`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Keep => "KEEP",
            Self::Rollback => "ROLLBACK",
        }
    }
}

/// One accepted tactic after held-out verification.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VerifiedTactic {
    /// The tactic that was verified.
    pub tactic: String,
    /// The general family the tactic was keyed to.
    pub category: String,
    /// The failing target whose analysis motivated the tactic.
    pub source_target_id: String,
    /// Training-slice reach/precision before applying the tactic.
    pub train_before: SliceMeasurement,
    /// Training-slice reach/precision after applying the tactic.
    pub train_after: SliceMeasurement,
    /// Held-out-slice reach/precision before applying the tactic.
    pub holdout_before: SliceMeasurement,
    /// Held-out-slice reach/precision after applying the tactic.
    pub holdout_after: SliceMeasurement,
    /// Keep vs roll back.
    pub decision: TacticDecision,
    /// Set when a train/held-out reach GAP was detected (the issue's
    /// "overfitting-warning"): training reach lifted but held-out reach did not.
    /// In the offline idealized model this provably means "no unreached held-out
    /// target of the family to confirm against" (a coverage gap); the definitive
    /// overfit-vs-coverage verdict is the Phase-3 verifier's.
    pub overfitting_warning: Option<String>,
    /// Whether this tactic was newly banked to durable memory this cycle.
    pub newly_persisted: bool,
    /// Human-readable justification.
    pub reason: String,
}

/// The result of one live self-improvement cycle.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SelfImproveReport {
    /// The run analysed.
    pub run_id: String,
    /// Model under test.
    pub model: String,
    /// Snapshot the targets came from.
    pub snapshot: String,
    /// Tactics accepted by the overfitting-reviewer gate (candidates to verify).
    pub gate_accepted: usize,
    /// Tactics rejected by the gate (never verified).
    pub gate_rejected: usize,
    /// Full gate output, for transparency.
    pub reviewed: Vec<ReviewedProposal>,
    /// Held-out verification results, one per gate-accepted tactic.
    pub verified: Vec<VerifiedTactic>,
    /// Tactics kept (held-out reach improved) and newly banked.
    pub kept: usize,
    /// Tactics rolled back (no held-out gain, precision drop, or already banked).
    pub rolled_back: usize,
    /// Number of overfitting warnings fired.
    pub overfitting_warnings: usize,
    /// Held-out reach at cycle start (with remembered tactics already applied).
    pub holdout_reach_before_pct: f64,
    /// Held-out reach after banking this cycle's kept tactics.
    pub holdout_reach_after_pct: f64,
    /// Durable-memory size before the cycle.
    pub memory_before: usize,
    /// Durable-memory size after the cycle.
    pub memory_after: usize,
    /// Provenance / guardrail note.
    pub note: String,
}

// ── Durable tactic memory ────────────────────────────────────────────────────

/// A durable, accepted tactic keyed by *general family* (never a project or
/// target id). Persisted so a win carries across runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcceptedTactic {
    /// The general family label (`format-gated-decoder`, …).
    pub category: String,
    /// The general tactic text.
    pub tactic: String,
    /// The failing target whose analysis first motivated the tactic.
    pub source_target_id: String,
    /// When the tactic was accepted (unix epoch milliseconds).
    pub accepted_at_unix_ms: u128,
    /// Held-out reach before the tactic was applied (percentage).
    pub holdout_reach_before_pct: f64,
    /// Held-out reach after the tactic was applied (percentage).
    pub holdout_reach_after_pct: f64,
}

/// A profile's durable tactic memory.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TacticMemory {
    /// Accepted tactics, one per general family.
    pub tactics: Vec<AcceptedTactic>,
}

impl TacticMemory {
    /// Whether a tactic for `category` is already banked.
    #[must_use]
    pub(crate) fn has_category(&self, category: TacticCategory) -> bool {
        let label = category.label();
        self.tactics.iter().any(|t| t.category == label)
    }
}

/// Path to a profile's tactic memory file.
#[must_use]
pub fn tactic_memory_path(home: &Path, profile: &str) -> PathBuf {
    profile_dir(home, profile).join(TACTIC_MEMORY_FILE)
}

/// Load a profile's durable tactic memory. A missing file is an empty memory.
///
/// # Errors
/// Returns [`CoinGymError::Parse`] if the file exists but is malformed.
pub fn load_tactic_memory(home: &Path, profile: &str) -> CoinGymResult<TacticMemory> {
    let path = tactic_memory_path(home, profile);
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| CoinGymError::Parse(format!("tactic memory {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TacticMemory::default()),
        Err(e) => Err(CoinGymError::Io(format!("read {}: {e}", path.display()))),
    }
}

/// Persist a profile's durable tactic memory.
///
/// # Errors
/// Returns [`CoinGymError::Io`] on directory-creation or write failure.
pub fn save_tactic_memory(home: &Path, profile: &str, memory: &TacticMemory) -> CoinGymResult<()> {
    let dir = profile_dir(home, profile);
    std::fs::create_dir_all(&dir)
        .map_err(|e| CoinGymError::Io(format!("create {}: {e}", dir.display())))?;
    let path = tactic_memory_path(home, profile);
    let body = serde_json::to_string_pretty(memory)
        .map_err(|e| CoinGymError::Parse(format!("serialize tactic memory: {e}")))?;
    std::fs::write(&path, body)
        .map_err(|e| CoinGymError::Io(format!("write {}: {e}", path.display())))
}

// ── Tactic application (offline scaffold model) ──────────────────────────────

/// Apply a tactic of `category` to a slice under the **idealized effect model**:
/// for every in-scope target of that family, *assume* the tactic drives the
/// agent's candidate to the objective oracle's reaching input. Targets outside
/// the family keep their base candidate. Returns the resulting
/// `target_id -> Candidate` script. This is a **simulation of an assumed
/// family-wide effect**, not proof the tactic text would solve fresh targets;
/// see the module docs.
fn apply_tactic_to_slice(
    category: TacticCategory,
    targets: &[Target],
    oracle: &HashMap<String, String>,
    base_script: &HashMap<String, Candidate>,
) -> HashMap<String, Candidate> {
    let mut script = base_script.clone();
    for target in targets {
        if categorize(target) != category {
            continue;
        }
        if let Some(reaching) = oracle.get(&target.id) {
            script.insert(
                target.id.clone(),
                Candidate {
                    input: reaching.clone(),
                    confidence: 0.9,
                    rationale: format!("applied general '{}' tactic", category.label()),
                },
            );
        }
    }
    script
}

/// Apply every remembered tactic to a slice (cumulative), reconstructing the
/// current agent's script so prior wins carry into this run's baseline.
fn apply_remembered_tactics(
    memory: &TacticMemory,
    targets: &[Target],
    oracle: &HashMap<String, String>,
    base_script: &HashMap<String, Candidate>,
) -> HashMap<String, Candidate> {
    let mut script = base_script.clone();
    for tactic in &memory.tactics {
        if let Some(category) = TacticCategory::from_label(&tactic.category) {
            script = apply_tactic_to_slice(category, targets, oracle, &script);
        }
    }
    script
}

/// Measure baseline reach/precision over `targets` given an oracle + script, by
/// replaying the offline agent through the mock grader.
fn measure_slice(
    model: &str,
    snapshot: &str,
    targets: &[Target],
    oracle: &HashMap<String, String>,
    script: &HashMap<String, Candidate>,
) -> CoinGymResult<SliceMeasurement> {
    if targets.is_empty() {
        let empty: Vec<super::types::Outcome> = Vec::new();
        return Ok(SliceMeasurement::from_rp(&ReachPrecision::compute(&empty)));
    }
    let scenario = DemoScenario {
        targets: TargetSet {
            snapshot: snapshot.to_string(),
            pinned: targets.to_vec(),
            held_out_fresh: Vec::new(),
        },
        oracle: oracle.clone(),
        script: script.clone(),
    };
    let report: RunReport = execute_run(model, Strategy::Baseline, &scenario)?;
    Ok(SliceMeasurement::from_rp(&ReachPrecision::compute(
        &report.outcomes,
    )))
}

/// Keep decision: held-out reach must strictly improve **and** precision must
/// not drop. Exposed for direct testing of the regression (rollback) path.
#[must_use]
pub(crate) fn improves_holdout(before: &SliceMeasurement, after: &SliceMeasurement) -> bool {
    after.reached > before.reached && after.precision_pct + PRECISION_EPS >= before.precision_pct
}

// ── The live loop ────────────────────────────────────────────────────────────

/// Run one live self-improvement cycle against a persisted **offline scaffold**
/// run, verifying each gate-accepted tactic on the held-out fresh slice and
/// persisting kept tactics to `profile`'s durable memory.
///
/// # Errors
/// Returns [`CoinGymError::Usage`] if the run is not an offline scaffold run or
/// has no held-out fresh slice to verify against, or any underlying I/O error.
pub fn run_self_improvement(
    home: &Path,
    profile: &str,
    persisted: &PersistedRun,
) -> CoinGymResult<SelfImproveReport> {
    if persisted.offline.is_empty() || !persisted.report.offline_scaffold {
        return Err(CoinGymError::Usage(
            "improve --holdout fresh needs an OFFLINE SCAFFOLD run (with its mock oracle + \
             script persisted); this run has none. A real held-out grade comes from `coin \
             verify` on the Phase-3 VM (issue #2823)"
                .to_string(),
        ));
    }
    let holdout = persisted.targets.held_out_fresh.clone();
    if holdout.is_empty() {
        return Err(CoinGymError::Usage(
            "improve --holdout fresh needs a held-out fresh slice to verify against; this run's \
             target set reserved none"
                .to_string(),
        ));
    }
    let pinned = persisted.targets.pinned.clone();
    let model = persisted.report.model.clone();
    let snapshot = persisted.targets.snapshot.clone();
    let oracle = persisted.offline.oracle.clone();
    let base_script = persisted.offline.base_candidates();

    let mut memory = load_tactic_memory(home, profile)?;
    let memory_before = memory.tactics.len();

    // Baseline slices with remembered tactics already applied (prior wins reused).
    let mut running_holdout_script =
        apply_remembered_tactics(&memory, &holdout, &oracle, &base_script);
    let mut running_train_script =
        apply_remembered_tactics(&memory, &pinned, &oracle, &base_script);
    let mut running_holdout = measure_slice(
        &model,
        &snapshot,
        &holdout,
        &oracle,
        &running_holdout_script,
    )?;
    let mut running_train =
        measure_slice(&model, &snapshot, &pinned, &oracle, &running_train_script)?;
    let holdout_reach_before_pct = running_holdout.reach_pct;

    // Failure-analyst → overfitting-reviewer gate (reuse the offline slice).
    let proposals = analyze_failures(&persisted.report, &persisted.targets);
    let reviewed: Vec<ReviewedProposal> = proposals
        .iter()
        .map(|p| review_proposal(p, &persisted.targets))
        .collect();
    let gate_accepted = reviewed
        .iter()
        .filter(|r| r.verdict == ReviewVerdict::Accept)
        .count();
    let gate_rejected = reviewed.len() - gate_accepted;

    let all_targets: Vec<Target> = pinned.iter().chain(holdout.iter()).cloned().collect();
    let mut verified = Vec::new();
    let mut kept = 0usize;
    let mut rolled_back = 0usize;
    let mut overfitting_warnings = 0usize;

    for r in reviewed
        .iter()
        .filter(|r| r.verdict == ReviewVerdict::Accept)
    {
        let Some(source) = all_targets.iter().find(|t| t.id == r.proposal.target_id) else {
            continue;
        };
        let category = categorize(source);

        let holdout_before = running_holdout;
        let train_before = running_train;

        let candidate_holdout_script =
            apply_tactic_to_slice(category, &holdout, &oracle, &running_holdout_script);
        let holdout_after = measure_slice(
            &model,
            &snapshot,
            &holdout,
            &oracle,
            &candidate_holdout_script,
        )?;
        let candidate_train_script =
            apply_tactic_to_slice(category, &pinned, &oracle, &running_train_script);
        let train_after =
            measure_slice(&model, &snapshot, &pinned, &oracle, &candidate_train_script)?;

        let train_lifted = train_after.reached > train_before.reached;
        let holdout_lifted = holdout_after.reached > holdout_before.reached;
        // A train-but-not-held-out lift is the issue's "overfitting-warning".
        // In the offline idealized model, `!holdout_lifted` provably means there
        // was no *unreached* held-out target of this family to confirm against —
        // i.e. a coverage gap — so we roll back as UNPROVEN rather than
        // asserting overfit; the definitive verdict is the Phase-3 verifier's.
        let overfitting_warning = if train_lifted && !holdout_lifted {
            overfitting_warnings += 1;
            Some(format!(
                "train/held-out reach GAP: lifts TRAINING reach (+{} target(s)) but produced no \
                 held-out gain — no unreached held-out '{}' target to confirm generalisation; \
                 rolled back as UNPROVEN (a real overfit-vs-coverage verdict needs the Phase-3 \
                 verifier, #2823)",
                train_after.reached - train_before.reached,
                category.label()
            ))
        } else {
            None
        };

        let already_banked = memory.has_category(category);
        let keep = !already_banked && improves_holdout(&holdout_before, &holdout_after);

        let (decision, reason, newly_persisted) = if keep {
            memory.tactics.push(AcceptedTactic {
                category: category.label().to_string(),
                tactic: r.proposal.tactic.clone(),
                source_target_id: r.proposal.target_id.clone(),
                accepted_at_unix_ms: now_unix_ms(),
                holdout_reach_before_pct: holdout_before.reach_pct,
                holdout_reach_after_pct: holdout_after.reach_pct,
            });
            running_holdout_script = candidate_holdout_script;
            running_train_script = candidate_train_script;
            running_holdout = holdout_after;
            running_train = train_after;
            kept += 1;
            (
                TacticDecision::Keep,
                format!(
                    "held-out reach {:.1}% → {:.1}% (precision {:.1}% → {:.1}%); kept and banked \
                     for family '{}'",
                    holdout_before.reach_pct,
                    holdout_after.reach_pct,
                    holdout_before.precision_pct,
                    holdout_after.precision_pct,
                    category.label()
                ),
                true,
            )
        } else {
            rolled_back += 1;
            let reason = if already_banked {
                format!(
                    "family '{}' already in durable memory (reused); no new held-out reach to bank",
                    category.label()
                )
            } else if overfitting_warning.is_some() {
                format!(
                    "train/held-out reach gap for family '{}'; rolled back as UNPROVEN (see warning)",
                    category.label()
                )
            } else {
                format!(
                    "no held-out reach improvement ({:.1}% → {:.1}%) or precision dropped \
                     ({:.1}% → {:.1}%); rolled back",
                    holdout_before.reach_pct,
                    holdout_after.reach_pct,
                    holdout_before.precision_pct,
                    holdout_after.precision_pct
                )
            };
            (TacticDecision::Rollback, reason, false)
        };

        verified.push(VerifiedTactic {
            tactic: r.proposal.tactic.clone(),
            category: category.label().to_string(),
            source_target_id: r.proposal.target_id.clone(),
            train_before,
            train_after,
            holdout_before,
            holdout_after,
            decision,
            overfitting_warning,
            newly_persisted,
            reason,
        });
    }

    save_tactic_memory(home, profile, &memory)?;
    let memory_after = memory.tactics.len();

    Ok(SelfImproveReport {
        run_id: persisted.report.run_id.clone(),
        model,
        snapshot,
        gate_accepted,
        gate_rejected,
        reviewed,
        verified,
        kept,
        rolled_back,
        overfitting_warnings,
        holdout_reach_before_pct,
        holdout_reach_after_pct: running_holdout.reach_pct,
        memory_before,
        memory_after,
        note: "offline scaffold (mock oracle), IDEALIZED effect model — the held-out grade is \
               synthesised from the same oracle, so keep/rollback demonstrates the loop's \
               control flow, not empirical generalisation. Real held-out verification (a live \
               model + `coin verify`) needs the Phase-3 VM (issue #2823). LOCAL-ONLY: nothing \
               submitted externally."
            .to_string(),
    })
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
