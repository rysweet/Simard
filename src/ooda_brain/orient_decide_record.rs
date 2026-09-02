//! Typed, file-backed OODA **Orient** + **Decide** decision records and their
//! fail-CLOSED readers (Group A of epic #4719; issue #4785).
//!
//! This module is the orient/decide analogue of the per-goal-cycle seam added
//! in #4734 (`PerGoalDecisionRecord` + `read_verified` in [`super`]). It
//! replaces the forbidden "agentic recipe emits JSON/decimal → Rust scrapes
//! stdout → Rust acts" pattern on the two core OODA reasoning phases with the
//! correct one: the recipe ACTS by calling a gated `simard ooda
//! record-orient|record-decide` tool that writes a typed, owner-only (`0o600`),
//! identity-bound record; the thin Rust rail reads that record **fail-closed**
//! (any bad/absent/mismatched record ⇒ `Err` ⇒ a safe no-op — the Decide caller
//! SKIPS the priority, the Orient caller KEEPS the base urgency — NEVER a
//! synthesized default action or demotion, #1711).
//!
//! Two shared validation chokepoints ([`DecideChoice::from_choice_fields`] and
//! [`OrientFields::from_fields`]) are each invoked by BOTH the CLI writer and
//! the reader, so the writer and reader can never drift on "what is a valid
//! judgment".
//!
//! See `docs/reference/ooda-record-orient-decide-cli.md` for the full contract.

use crate::error::{SimardError, SimardResult};
use crate::ooda_loop::ActionKind;

use super::sanitize;

/// Max characters retained for a model-controlled `reason` after sanitization.
/// Mirrors the per-goal-cycle bound so a runaway model response cannot bloat an
/// operator log line or a persisted audit record.
const REASON_MAX_CHARS: usize = 500;

/// Tiny floating-point slack so a brain echoing `base_urgency` EXACTLY (zero
/// demotion) is not rejected on rounding. Mirrors [`super::OrientJudgment::validate`].
const URGENCY_FP_SLACK: f64 = 1e-9;

// ===========================================================================
// DECIDE side
// ===========================================================================

/// The pinned on-disk schema string for a [`DecideDecisionRecord`]. The reader
/// rejects any other value, so a future `…v2` writer can never be honored by a
/// `…v1` reader (bumping this is a hard, coordinated change).
pub const DECIDE_SCHEMA: &str = "simard.ooda.decide.v1";

/// The closed set of 10 action kinds the OODA **Decide** phase may route a
/// priority to — the SINGLE authority on "what is a valid decide choice".
///
/// The variant set is exactly [`super::DecideJudgment`]'s (`advance_goal`,
/// `run_improvement`, `consolidate_memory`, `research_query`, `run_gym_eval`,
/// `build_skill`, `launch_session`, `poll_developer_activity`, `extract_ideas`,
/// `safe_update`); each carries the mandatory `reason`. Tagged on `choice` and
/// `rename_all = "snake_case"` so it serializes into the record as
/// `{"choice":"advance_goal","reason":"…"}` — flattened by
/// [`DecideDecisionRecord`] — exactly the shape the CLI tool writes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum DecideChoice {
    PollDeveloperActivity { reason: String },
    ConsolidateMemory { reason: String },
    RunImprovement { reason: String },
    ExtractIdeas { reason: String },
    SafeUpdate { reason: String },
    ResearchQuery { reason: String },
    RunGymEval { reason: String },
    BuildSkill { reason: String },
    LaunchSession { reason: String },
    AdvanceGoal { reason: String },
}

impl DecideChoice {
    /// The SINGLE shared closed-enum validation chokepoint, invoked by BOTH the
    /// `simard ooda record-decide` CLI writer and [`read_verified_decide`].
    ///
    /// * `choice` is matched case-insensitively against the closed 10-variant
    ///   set; an unknown tag ⇒ `None` (fail CLOSED — a compromised prompt cannot
    ///   smuggle a novel action).
    /// * `reason` is sanitized (ANSI/C0 control stripped, whitespace folded) and
    ///   bounded to [`REASON_MAX_CHARS`] BEFORE the emptiness gate, so a reason
    ///   made up ENTIRELY of control/ANSI bytes (which `str::trim` does not
    ///   remove) collapses to empty and is rejected ⇒ `None`.
    pub fn from_choice_fields(choice: &str, reason: &str) -> Option<Self> {
        // SECURITY (mirror of #2751 / #4734): `reason` is MODEL-CONTROLLED and
        // flows verbatim to operator stderr logs and the *persisted* record.
        // Sanitize at this one canonical chokepoint, before the emptiness gate,
        // so a control-only reason fails CLOSED (empty after sanitize ⇒ None).
        let reason = sanitize::sanitize_context_var(reason.trim(), REASON_MAX_CHARS);
        if reason.is_empty() {
            return None;
        }
        match choice.trim() {
            c if c.eq_ignore_ascii_case("poll_developer_activity") => {
                Some(Self::PollDeveloperActivity { reason })
            }
            c if c.eq_ignore_ascii_case("consolidate_memory") => {
                Some(Self::ConsolidateMemory { reason })
            }
            c if c.eq_ignore_ascii_case("run_improvement") => Some(Self::RunImprovement { reason }),
            c if c.eq_ignore_ascii_case("extract_ideas") => Some(Self::ExtractIdeas { reason }),
            c if c.eq_ignore_ascii_case("safe_update") => Some(Self::SafeUpdate { reason }),
            c if c.eq_ignore_ascii_case("research_query") => Some(Self::ResearchQuery { reason }),
            c if c.eq_ignore_ascii_case("run_gym_eval") => Some(Self::RunGymEval { reason }),
            c if c.eq_ignore_ascii_case("build_skill") => Some(Self::BuildSkill { reason }),
            c if c.eq_ignore_ascii_case("launch_session") => Some(Self::LaunchSession { reason }),
            c if c.eq_ignore_ascii_case("advance_goal") => Some(Self::AdvanceGoal { reason }),
            _ => None,
        }
    }

    /// Stable snake_case label — identical to the serde `choice` tag.
    pub fn variant_label(&self) -> &'static str {
        match self {
            Self::PollDeveloperActivity { .. } => "poll_developer_activity",
            Self::ConsolidateMemory { .. } => "consolidate_memory",
            Self::RunImprovement { .. } => "run_improvement",
            Self::ExtractIdeas { .. } => "extract_ideas",
            Self::SafeUpdate { .. } => "safe_update",
            Self::ResearchQuery { .. } => "research_query",
            Self::RunGymEval { .. } => "run_gym_eval",
            Self::BuildSkill { .. } => "build_skill",
            Self::LaunchSession { .. } => "launch_session",
            Self::AdvanceGoal { .. } => "advance_goal",
        }
    }

    /// The mandatory reasoning the reasoner carried on the chosen action.
    pub fn reason(&self) -> &str {
        match self {
            Self::PollDeveloperActivity { reason }
            | Self::ConsolidateMemory { reason }
            | Self::RunImprovement { reason }
            | Self::ExtractIdeas { reason }
            | Self::SafeUpdate { reason }
            | Self::ResearchQuery { reason }
            | Self::RunGymEval { reason }
            | Self::BuildSkill { reason }
            | Self::LaunchSession { reason }
            | Self::AdvanceGoal { reason } => reason,
        }
    }

    /// Project the closed choice back onto the existing [`ActionKind`] so the
    /// Decide phase keeps emitting `PlannedAction` values unchanged.
    pub fn action_kind(&self) -> ActionKind {
        match self {
            Self::PollDeveloperActivity { .. } => ActionKind::PollDeveloperActivity,
            Self::ConsolidateMemory { .. } => ActionKind::ConsolidateMemory,
            Self::RunImprovement { .. } => ActionKind::RunImprovement,
            Self::ExtractIdeas { .. } => ActionKind::ExtractIdeas,
            Self::SafeUpdate { .. } => ActionKind::SafeUpdate,
            Self::ResearchQuery { .. } => ActionKind::ResearchQuery,
            Self::RunGymEval { .. } => ActionKind::RunGymEval,
            Self::BuildSkill { .. } => ActionKind::BuildSkill,
            Self::LaunchSession { .. } => ActionKind::LaunchSession,
            Self::AdvanceGoal { .. } => ActionKind::AdvanceGoal,
        }
    }

    /// Project the closed choice onto a [`super::DecideJudgment`] so the
    /// `OodaDecideBrain` trait can keep returning its established type. The
    /// validated `reason` becomes the judgment's `rationale`.
    pub fn to_judgment(&self) -> super::DecideJudgment {
        use super::DecideJudgment as J;
        let rationale = self.reason().to_string();
        match self {
            Self::PollDeveloperActivity { .. } => J::PollDeveloperActivity { rationale },
            Self::ConsolidateMemory { .. } => J::ConsolidateMemory { rationale },
            Self::RunImprovement { .. } => J::RunImprovement { rationale },
            Self::ExtractIdeas { .. } => J::ExtractIdeas { rationale },
            Self::SafeUpdate { .. } => J::SafeUpdate { rationale },
            Self::ResearchQuery { .. } => J::ResearchQuery { rationale },
            Self::RunGymEval { .. } => J::RunGymEval { rationale },
            Self::BuildSkill { .. } => J::BuildSkill { rationale },
            Self::LaunchSession { .. } => J::LaunchSession { rationale },
            Self::AdvanceGoal { .. } => J::AdvanceGoal { rationale },
        }
    }
}

/// One typed, on-disk OODA **Decide** verdict, written by the
/// `simard ooda record-decide` tool and read by [`super::RecipeBrain`] via
/// [`read_verified_decide`]. Never scraped from agent prose.
///
/// The `choice` discriminator + its `reason` field are flattened from
/// [`DecideChoice`]'s tagged representation, so the tool and the enum can never
/// disagree on the wire shape:
///
/// ```json
/// {"schema":"simard.ooda.decide.v1","goal_id":"…","cycle_number":42,
///  "choice":"advance_goal","reason":"…"}
/// ```
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecideDecisionRecord {
    /// Schema pin. Must equal [`DECIDE_SCHEMA`].
    pub schema: String,
    /// The goal this decision is for. Re-verified against the live ctx (R6).
    pub goal_id: String,
    /// The cycle this decision is for. Re-verified against the live ctx (R7).
    pub cycle_number: u32,
    /// The validated, closed-enum choice (flattened `choice` + `reason`).
    #[serde(flatten)]
    pub choice: DecideChoice,
}

/// Read and FULLY verify a decide record, returning the validated closed choice.
///
/// Every failure mode is an `Err` (a safe no-op: the Decide caller SKIPS the
/// priority), never a default action (#1711). The fail-CLOSED matrix:
///
/// | # | Condition | Result |
/// |---|---|---|
/// | R1 | file absent / unreadable | `Err` |
/// | R2 | present but not valid JSON / truncated | `Err` |
/// | R3 | `schema != DECIDE_SCHEMA` | `Err` |
/// | R4 | `choice` not one of the 10 closed variants | `Err` |
/// | R5 | `reason` missing / empty / control-only after sanitize | `Err` |
/// | R6 | `goal_id` ≠ live ctx | `Err` |
/// | R7 | `cycle_number` ≠ live ctx | `Err` |
/// | R8 | all checks pass | `Ok(choice)` |
pub fn read_verified_decide(
    path: &std::path::Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<DecideChoice> {
    let fail = |reason: String| SimardError::AdapterInvocationFailed {
        base_type: "recipe-decide-brain".to_string(),
        reason,
    };

    // R1 — absence (or any read error) is fail-CLOSED. The tool writes nothing
    // when it cannot resolve its binary or fails validation.
    let bytes = std::fs::read(path).map_err(|e| {
        fail(format!(
            "decide record absent/unreadable at {}: {e} (fail-CLOSED)",
            path.display()
        ))
    })?;

    // R2/R4/R5(missing) — malformed JSON, an unknown `choice` tag, or a missing
    // `reason` field all fail deserialization into the closed record type.
    let record: DecideDecisionRecord = serde_json::from_slice(&bytes).map_err(|e| {
        fail(format!(
            "decide record did not deserialize (malformed/unknown-choice/missing-reason): {e} (fail-CLOSED)"
        ))
    })?;

    // R3 — schema version pin.
    if record.schema != DECIDE_SCHEMA {
        return Err(fail(format!(
            "decide record schema {:?} != expected {DECIDE_SCHEMA:?} (fail-CLOSED)",
            record.schema
        )));
    }

    // R6 — goal identity (no other-goal record honored).
    if record.goal_id != goal_id {
        return Err(fail(format!(
            "decide record goal_id {:?} != live ctx {goal_id:?} (stale/other-goal; fail-CLOSED)",
            record.goal_id
        )));
    }

    // R7 — cycle identity (no replay of a prior cycle's verdict).
    if record.cycle_number != cycle_number {
        return Err(fail(format!(
            "decide record cycle_number {} != live ctx {cycle_number} (prior-cycle; fail-CLOSED)",
            record.cycle_number
        )));
    }

    // R5 + defense-in-depth — re-validate AND re-sanitize the free text through
    // the SAME chokepoint the tool used on write. A hostile record the tool
    // would never produce (a control-byte-only reason, or raw ANSI/C0 bound for
    // operator logs) is rejected (empty after sanitize ⇒ None ⇒ fail-CLOSED) or
    // cleaned here, never honored verbatim.
    DecideChoice::from_choice_fields(record.choice.variant_label(), record.choice.reason())
        .ok_or_else(|| {
            fail("decide record reason is empty after sanitization (fail-CLOSED)".to_string())
        })
}

// ===========================================================================
// ORIENT side
// ===========================================================================

/// The pinned on-disk schema string for an [`OrientDecisionRecord`].
pub const ORIENT_SCHEMA: &str = "simard.ooda.orient.v1";

/// The validated numeric + free-text fields of an OODA **Orient** judgment.
///
/// Flattened into [`OrientDecisionRecord`] so the record serializes as
/// `{…,"adjusted_urgency":…,"confidence":…,"demotion_applied":…,"reason":…}`.
/// Constructed ONLY through [`OrientFields::from_fields`] (the shared writer /
/// reader chokepoint), which enforces finiteness, `[0,1]` bounds, the
/// no-escalation invariant, and a non-empty sanitized reason.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OrientFields {
    /// Validated final urgency, in `[0,1]`, `≤ base_urgency` (+ FP slack).
    pub adjusted_urgency: f64,
    /// Brain's self-reported confidence, finite and in `[0,1]`.
    pub confidence: f64,
    /// The demotion the brain applied (`base_urgency − adjusted_urgency`),
    /// carried for the audit trail.
    pub demotion_applied: f64,
    /// Sanitized, bounded (`≤ 500` chars), non-empty rationale.
    pub reason: String,
}

impl OrientFields {
    /// The SINGLE shared range/escalation validation chokepoint, invoked by BOTH
    /// the `simard ooda record-orient` CLI writer and [`read_verified_orient`].
    ///
    /// This is a strict superset of [`super::OrientJudgment::validate`]: it
    /// additionally bounds `confidence` and `base_urgency` to `[0,1]` and
    /// requires a non-empty sanitized `reason`. Returns `Err(reason)` on any
    /// violation so both writer and reader fail CLOSED identically.
    pub fn from_fields(
        adjusted_urgency: f64,
        confidence: f64,
        demotion_applied: f64,
        reason: &str,
        base_urgency: f64,
    ) -> Result<Self, String> {
        if !adjusted_urgency.is_finite() {
            return Err(format!(
                "adjusted_urgency must be finite, got {adjusted_urgency}"
            ));
        }
        if !(0.0..=1.0).contains(&adjusted_urgency) {
            return Err(format!("adjusted_urgency {adjusted_urgency} out of [0, 1]"));
        }
        if !confidence.is_finite() {
            return Err(format!("confidence must be finite, got {confidence}"));
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err(format!("confidence {confidence} out of [0, 1]"));
        }
        if !demotion_applied.is_finite() {
            return Err(format!(
                "demotion_applied must be finite, got {demotion_applied}"
            ));
        }
        if !base_urgency.is_finite() {
            return Err(format!("base_urgency must be finite, got {base_urgency}"));
        }
        if !(0.0..=1.0).contains(&base_urgency) {
            return Err(format!("base_urgency {base_urgency} out of [0, 1]"));
        }
        // No-escalation invariant (tiny FP slack so an exact echo is accepted).
        if adjusted_urgency > base_urgency + URGENCY_FP_SLACK {
            return Err(format!(
                "adjusted_urgency {adjusted_urgency} > base_urgency {base_urgency} (escalation forbidden)"
            ));
        }
        // `reason` is MODEL-CONTROLLED — sanitize (strip ANSI/C0, fold
        // whitespace, bound) BEFORE the emptiness gate so a control-only reason
        // collapses to empty and is rejected (fail CLOSED).
        let reason = sanitize::sanitize_context_var(reason.trim(), REASON_MAX_CHARS);
        if reason.is_empty() {
            return Err("reason is empty after sanitization".to_string());
        }
        Ok(Self {
            adjusted_urgency,
            confidence,
            demotion_applied,
            reason,
        })
    }
}

/// One typed, on-disk OODA **Orient** judgment, written by the
/// `simard ooda record-orient` tool and read by [`super::RecipeBrain`] via
/// [`read_verified_orient`]. Never scraped from agent prose.
///
/// The record ALSO persists `base_urgency` so the reader can re-run the exact
/// same [`OrientFields::from_fields`] no-escalation check the writer ran —
/// closing the writer/reader drift gap self-consistently without trusting the
/// on-disk `adjusted_urgency`/`demotion_applied` alone.
///
/// ```json
/// {"schema":"simard.ooda.orient.v1","goal_id":"…","cycle_number":42,
///  "base_urgency":0.80,"adjusted_urgency":0.60,"confidence":0.90,
///  "demotion_applied":0.20,"reason":"…"}
/// ```
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OrientDecisionRecord {
    /// Schema pin. Must equal [`ORIENT_SCHEMA`].
    pub schema: String,
    /// The goal this judgment is for. Re-verified against the live ctx (R6).
    pub goal_id: String,
    /// The cycle this judgment is for. Re-verified against the live ctx (R7).
    pub cycle_number: u32,
    /// The pre-penalty urgency, persisted so the reader can re-check the
    /// no-escalation invariant self-consistently.
    pub base_urgency: f64,
    /// The validated judgment fields (flattened).
    #[serde(flatten)]
    pub fields: OrientFields,
}

/// Read and FULLY verify an orient record, returning the validated fields.
///
/// Every failure mode is an `Err` (a safe no-op: the Orient caller KEEPS the
/// base urgency), never a synthesized demotion (#1711). The fail-CLOSED matrix:
///
/// | # | Condition | Result |
/// |---|---|---|
/// | R1 | file absent / unreadable | `Err` |
/// | R2 | present but not valid JSON / truncated / missing field | `Err` |
/// | R3 | `schema != ORIENT_SCHEMA` | `Err` |
/// | R4 | numerics non-finite / out of `[0,1]` / escalating (`adjusted > base`) | `Err` |
/// | R5 | `reason` empty / control-only after sanitize | `Err` |
/// | R6 | `goal_id` ≠ live ctx | `Err` |
/// | R7 | `cycle_number` ≠ live ctx | `Err` |
/// | R8 | all checks pass | `Ok(fields)` |
pub fn read_verified_orient(
    path: &std::path::Path,
    goal_id: &str,
    cycle_number: u32,
) -> SimardResult<OrientFields> {
    let fail = |reason: String| SimardError::AdapterInvocationFailed {
        base_type: "recipe-orient-brain".to_string(),
        reason,
    };

    // R1 — absence (or any read error) is fail-CLOSED.
    let bytes = std::fs::read(path).map_err(|e| {
        fail(format!(
            "orient record absent/unreadable at {}: {e} (fail-CLOSED)",
            path.display()
        ))
    })?;

    // R2 — malformed JSON or any missing field (confidence/demotion are
    // REQUIRED — the typed CLI deliberately tightens them vs the legacy wire).
    let record: OrientDecisionRecord = serde_json::from_slice(&bytes).map_err(|e| {
        fail(format!(
            "orient record did not deserialize (malformed/missing-field): {e} (fail-CLOSED)"
        ))
    })?;

    // R3 — schema version pin.
    if record.schema != ORIENT_SCHEMA {
        return Err(fail(format!(
            "orient record schema {:?} != expected {ORIENT_SCHEMA:?} (fail-CLOSED)",
            record.schema
        )));
    }

    // R6 — goal identity.
    if record.goal_id != goal_id {
        return Err(fail(format!(
            "orient record goal_id {:?} != live ctx {goal_id:?} (stale/other-goal; fail-CLOSED)",
            record.goal_id
        )));
    }

    // R7 — cycle identity (no replay of a prior cycle's judgment).
    if record.cycle_number != cycle_number {
        return Err(fail(format!(
            "orient record cycle_number {} != live ctx {cycle_number} (prior-cycle; fail-CLOSED)",
            record.cycle_number
        )));
    }

    // R4/R5 + anti-drift — re-run the SAME chokepoint against the PERSISTED
    // base_urgency, so an escalating or out-of-range record (adjusted > the
    // recorded base) is rejected even when goal_id + cycle_number match, and the
    // free text is re-sanitized.
    OrientFields::from_fields(
        record.fields.adjusted_urgency,
        record.fields.confidence,
        record.fields.demotion_applied,
        &record.fields.reason,
        record.base_urgency,
    )
    .map_err(|e| {
        fail(format!(
            "orient record failed re-validation: {e} (fail-CLOSED)"
        ))
    })
}
