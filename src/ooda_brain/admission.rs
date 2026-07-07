//! Types, trait, and the deterministic hard-rail for RESOURCE-AWARE engineer
//! **admission** — the resource-aware augmentation of the AIMD count cap.
//!
//! # Problem
//! The AIMD controller bounds concurrent engineer *count* but nothing accounts
//! for DISK / build-cache / system load. Parallel `cargo` builds across 40+
//! worktrees drove disk to 91% and ENOSPC killed recipes. Count-control is not
//! resource-admission.
//!
//! # Design (mirrors [`super::decide`])
//! The *intelligence* is agentic: a structured-reasoning brain reasons over the
//! current resource picture and emits ADMIT / DEFER / RECLAIM-FIRST at every
//! fresh-spawn admission cycle (repeated structured thought). The LLM-backed
//! impl drives the `ooda-resource-admission.yaml` recipe. The only deterministic
//! code in this module is:
//!
//!   * [`resolve_admission`] — a THIN safety rail that BLOCKS admission when
//!     disk% is *known* to be at/above a configurable ceiling, **regardless of
//!     what the reasoner says**. Irreversible ENOSPC must never be reachable.
//!   * [`judge_and_resolve`] — the seam core (reason → apply). It surfaces a
//!     brain error loudly (NO FALLBACK, per the standing operator constraint).
//!   * [`parse_ceiling`] / [`configured_ceiling_pct`] — read + clamp the single
//!     configurable threshold.
//!
//! No hardcoded admission heuristics live in Rust beyond that single ceiling —
//! the ADMIT/DEFER/RECLAIM judgement itself lives in the prompt.

use crate::error::SimardResult;

/// Recipe asset that drives the agentic admission reasoner. Read fresh per call
/// by the recipe-backed brain (hot-reload), mirroring the other OODA-phase
/// recipes (`ooda-decide.yaml`, `ooda-engineer-lifecycle.yaml`).
pub const RECIPE_NAME: &str = "ooda-resource-admission.yaml";

/// Env var naming the deterministic disk-admission ceiling (percent full). Read
/// at the seam, parsed best-effort, and clamped to
/// [`CEILING_MIN`]..=[`CEILING_MAX`].
pub const CEILING_ENV: &str = "SIMARD_DISK_ADMISSION_CEILING_PCT";

/// Default disk ceiling (percent full). The incident tripped at 91%; 90 leaves
/// headroom below ENOSPC without over-throttling.
pub const DEFAULT_CEILING_PCT: u8 = 90;

/// Lower clamp bound for the configured ceiling (a `0` ceiling would block all
/// admission forever and deadlock progress).
pub const CEILING_MIN: u8 = 1;

/// Upper clamp bound for the configured ceiling (a `100` ceiling would let the
/// rail fire only at true-full, i.e. never usefully — keep headroom).
pub const CEILING_MAX: u8 = 99;

/// Clamp a raw ceiling into the admissible [`CEILING_MIN`]..=[`CEILING_MAX`]
/// range.
pub fn clamp_ceiling(pct: u8) -> u8 {
    pct.clamp(CEILING_MIN, CEILING_MAX)
}

/// Parse + clamp a raw env string into a usable ceiling. Best-effort: any
/// missing / non-numeric / out-of-`u8` value degrades to [`DEFAULT_CEILING_PCT`]
/// rather than failing (never panic; safe default).
pub fn parse_ceiling(raw: Option<&str>) -> u8 {
    raw.and_then(|v| v.trim().parse::<u8>().ok())
        .map(clamp_ceiling)
        .unwrap_or(DEFAULT_CEILING_PCT)
}

/// Read the configured disk-admission ceiling from the environment (see
/// [`CEILING_ENV`]), applying [`parse_ceiling`].
pub fn configured_ceiling_pct() -> u8 {
    parse_ceiling(std::env::var(CEILING_ENV).ok().as_deref())
}

// ---------------------------------------------------------------------------
// Context fed to the admission brain (A8)
// ---------------------------------------------------------------------------

/// Best-effort snapshot of the resource picture the admission brain reasons
/// over. Every probe degrades to `None` on error (never panic); the `None` is
/// passed through to the reasoner so it can weigh "unknown" explicitly.
///
/// The rail in [`resolve_admission`] only consults [`Self::disk_usage_pct`] and
/// [`Self::ceiling_pct`]; the other fields exist so the *prompt* can normalize
/// load-by-cpu and weigh build-cache size / in-flight builds.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResourceAdmissionCtx {
    /// Disk usage of the state/worktree filesystem, 0..=100. `None` when the
    /// `df` probe failed (rail fails open — see [`resolve_admission`]).
    pub disk_usage_pct: Option<u8>,
    /// Total bytes under the engineer-worktrees / build-cache root (`du -sb`).
    pub worktree_cache_bytes: Option<u64>,
    /// 1-minute load average (`/proc/loadavg`).
    pub load_avg_1m: Option<f64>,
    /// Logical CPU count, so the prompt can normalize load per core.
    pub cpu_count: Option<usize>,
    /// Number of live engineer claims already in flight this cycle.
    pub in_flight_engineers: u32,
    /// The deterministic hard-rail ceiling in effect for this decision.
    pub ceiling_pct: u8,
}

// ---------------------------------------------------------------------------
// Decision: tagged enum the LLM emits as `{"choice":"...","rationale":"..."}`
// ---------------------------------------------------------------------------

/// What the admission brain decided. Tagged on `choice` for
/// forward-compatibility (unknown tags fail to parse → the caller surfaces the
/// parse error rather than silently admitting).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum AdmissionDecision {
    /// Resources are healthy enough — admit one more engineer (still subject to
    /// the deterministic hard rail in [`resolve_admission`]).
    Admit { rationale: String },
    /// Resource pressure — skip this cycle without allocating anything. Benign:
    /// the goal is retried next cycle and this is NOT a failure.
    Defer { rationale: String },
    /// Reclaim disk first (invoke the disk-health recipe), then defer this cycle
    /// and re-evaluate next cycle.
    ReclaimFirst { rationale: String },
}

impl AdmissionDecision {
    /// The rationale carried by every variant.
    pub fn rationale(&self) -> &str {
        match self {
            Self::Admit { rationale }
            | Self::Defer { rationale }
            | Self::ReclaimFirst { rationale } => rationale,
        }
    }

    /// Stable snake_case label (matches the serde `choice` tag) for
    /// observability records / metrics.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Admit { .. } => "admit",
            Self::Defer { .. } => "defer",
            Self::ReclaimFirst { .. } => "reclaim_first",
        }
    }
}

// ---------------------------------------------------------------------------
// The trait (A1) — single decision site, sync to match the other OODA brains
// ---------------------------------------------------------------------------

/// Single-decision-site trait for resource-aware admission. Sync on purpose to
/// match [`super::OodaBrain`] / [`super::OodaDecideBrain`]; the LLM-backed impl
/// bridges to async internally so callers need no runtime.
pub trait OodaAdmissionBrain: Send + Sync {
    fn judge_admission(&self, ctx: &ResourceAdmissionCtx) -> SimardResult<AdmissionDecision>;
}

// ---------------------------------------------------------------------------
// Resolved gate — what the seam actually applies (post hard-rail)
// ---------------------------------------------------------------------------

/// The resolved admission outcome after the deterministic hard rail has had the
/// final say over the brain's [`AdmissionDecision`]. This is what the spawn seam
/// applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionGate {
    /// Admit: proceed with the fresh spawn (allocate worktree, spawn engineer).
    Proceed,
    /// Skip this cycle without allocating resources. **Benign** — the caller
    /// MUST NOT record failure (no `goal_failure_counts` bump, no 3-strike
    /// safeguard). The goal is retried next cycle.
    Defer { reason: String },
    /// Run disk reclamation now (disk-health recipe), then defer this cycle and
    /// re-evaluate next cycle.
    Reclaim { reason: String },
}

impl AdmissionGate {
    /// `true` only for [`AdmissionGate::Proceed`] — i.e. an actual admission.
    pub fn is_proceed(&self) -> bool {
        matches!(self, Self::Proceed)
    }
}

/// Apply the THIN deterministic hard rail (FR-5) over the brain's decision, then
/// map it to an [`AdmissionGate`] (FR-4).
///
/// **Safety invariant:** when `ctx.disk_usage_pct` is `Some(pct)` and
/// `pct >= ctx.ceiling_pct`, the result is NEVER [`AdmissionGate::Proceed`],
/// regardless of what the brain decided — irreversible ENOSPC must never be
/// reachable.
///
/// **Fail-open:** a `None` `disk_usage_pct` (probe failed) does NOT fire the
/// rail; the "unknown" was already handed to the reasoner, and a spurious block
/// on a transient `df` error would deadlock all progress. Layered ENOSPC
/// protection (emergency cleanup tier) remains the backstop.
///
/// The rail only overrides [`AdmissionDecision::Admit`] (the sole `Proceed`
/// source). `Defer` / `ReclaimFirst` are already non-admitting and pass through
/// unchanged.
pub fn resolve_admission(ctx: &ResourceAdmissionCtx, decision: AdmissionDecision) -> AdmissionGate {
    let rail_blocks = matches!(ctx.disk_usage_pct, Some(pct) if pct >= ctx.ceiling_pct);
    match decision {
        AdmissionDecision::Admit { rationale } => {
            if rail_blocks {
                let pct = ctx.disk_usage_pct.unwrap_or_default();
                AdmissionGate::Defer {
                    reason: format!(
                        "hard-rail: disk {pct}% >= admission ceiling {}%; ADMIT overridden to \
                         block (brain rationale: {rationale})",
                        ctx.ceiling_pct
                    ),
                }
            } else {
                AdmissionGate::Proceed
            }
        }
        AdmissionDecision::Defer { rationale } => AdmissionGate::Defer { reason: rationale },
        AdmissionDecision::ReclaimFirst { rationale } => {
            AdmissionGate::Reclaim { reason: rationale }
        }
    }
}

/// Seam core: reason → apply. Calls the brain for an [`AdmissionDecision`] and
/// resolves it through the hard rail into an [`AdmissionGate`].
///
/// **NO FALLBACK:** a brain error propagates unchanged so the seam can surface
/// it as a visible cycle failure (mirrors `decide_engineer_lifecycle` — a
/// broken brain must never look like a silent "admit").
pub fn judge_and_resolve(
    brain: &dyn OodaAdmissionBrain,
    ctx: &ResourceAdmissionCtx,
) -> SimardResult<AdmissionGate> {
    let decision = brain.judge_admission(ctx)?;
    Ok(resolve_admission(ctx, decision))
}

// ---------------------------------------------------------------------------
// Deterministic brain — no-LLM floor (NOT the intelligence)
// ---------------------------------------------------------------------------

/// Deterministic admission floor used when no LLM brain is configured. It
/// preserves pre-feature behaviour bit-for-bit: always `Admit` (count-cap only).
/// This is NOT the resource intelligence — that lives in the recipe/prompt. The
/// deterministic ENOSPC protection is provided by the hard rail in
/// [`resolve_admission`], which still fires over this brain's `Admit`.
#[derive(Debug, Default)]
pub struct DeterministicAdmissionBrain;

impl OodaAdmissionBrain for DeterministicAdmissionBrain {
    fn judge_admission(&self, _ctx: &ResourceAdmissionCtx) -> SimardResult<AdmissionDecision> {
        Ok(AdmissionDecision::Admit {
            rationale: "deterministic-brain: no LLM configured; admit (count-cap only); ENOSPC \
                        still guarded by the disk hard-rail"
                .to_string(),
        })
    }
}
