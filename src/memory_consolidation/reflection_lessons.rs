//! Verified-signal gate + failure→lesson distillation for the procedural-learning
//! loop (issues #2441, #2458).
//!
//! This module is the small, pure-where-possible policy layer that closes the
//! episodic→procedural loop:
//!
//! * **#2441 (skill reuse):** on an externally *verified* success the successful
//!   action sequence is distilled into a reusable procedure (a "skill"), and
//!   *applying* a recalled procedure emits [`record_skill_reuse`] — the
//!   measurable half of "reuse, don't just store".
//! * **#2458 (failure→lesson):** on an externally *verified* failure a verbal
//!   Reflexion-style reflection is stored ([`record_failure_reflection`]); when
//!   the same `(goal_type, error_class)` failure has **recurred**
//!   (`>= LESSON_RECURRENCE_THRESHOLD`) it is distilled into a `lesson:` procedure
//!   ([`maybe_distill_lesson`]). A one-off failure must NOT become a lesson.
//!
//! ## Load-bearing invariant (R10)
//!
//! Every learning entry point takes a [`Verdict`] and is a **no-op on
//! [`Verdict::Unverified`]**. The verdict is sourced from a real external signal
//! ([`verified_outcome`] maps an engineer-loop [`VerificationReport`]); the
//! brain's own self-judged `ActionOutcome.success` is never the gate. When no
//! external signal is available the loop learns nothing (fail-safe OFF). This is
//! the caveat both issues call out (cf. *LLMs Cannot Self-Correct Yet*).
//!
//! ## Lessons are name-prefixed procedures, not a new node type
//!
//! A lesson is an ordinary [`CognitiveProcedure`](crate::memory_cognitive::CognitiveProcedure)
//! whose name follows the reserved `lesson:<goal_type>:<error_class>` convention
//! ([`lesson_name`]). This keeps the change additive: the `CognitiveMemoryOps`
//! trait and the procedure schema are unchanged, lessons co-rank with skills by
//! `usage_count`, and `is_lesson(name)` is a pure prefix check used by metrics.
//!
//! ## Metrics
//!
//! Each metric pairs a **pure context builder** (unit-tested, no I/O) with a
//! best-effort emitter that is a no-op under `cfg!(test)` so the broad test
//! suite never writes to `~/.simard/metrics/metrics.jsonl` — mirroring
//! [`super::distillation`]'s reliability-gate metric.

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::engineer_loop::VerificationReport;
use crate::error::SimardResult;

/// External verification verdict for an action sequence (#2441/#2458).
///
/// Sourced from a real outside signal (engineer-loop [`VerificationReport`], a
/// verified subprocess exit, or a gym eval) — never from the model's own
/// `ActionOutcome.success`. [`Verdict::Unverified`] is the fail-safe default:
/// when no external signal is available, the loop learns nothing.
///
/// Intentionally distinct from the unrelated `stewardship::merge_judge::Verdict`;
/// the two never share a scope and are always module-qualified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// An external check confirmed success → distil/reinforce a skill.
    VerifiedSuccess,
    /// An external check confirmed failure → reflect, maybe distil a lesson.
    /// `error_class` is the normalized failure class (e.g. `cargo_test_failed`).
    VerifiedFailure {
        /// Normalized failure category used as part of the recurrence key.
        error_class: String,
    },
    /// No external signal available → learn nothing (fail-safe).
    Unverified,
}

/// Minimum number of `(goal_type, error_class)` reflections before a recurring
/// failure is distilled into a `lesson:` procedure (AMB-6). `1` would turn every
/// one-off failure into a lesson; `2` is the smallest value that excludes
/// singletons.
pub const LESSON_RECURRENCE_THRESHOLD: u32 = 2;

/// Environment override for the lesson recurrence threshold (mirrors the
/// distillation scheduler's `SIMARD_DISTILL_*` knobs).
pub const LESSON_RECURRENCE_THRESHOLD_ENV: &str = "SIMARD_LESSON_RECURRENCE_THRESHOLD";

/// Reserved name prefix marking a procedure as a failure-derived lesson.
pub const LESSON_NAME_PREFIX: &str = "lesson:";

// ── verified-signal gate ────────────────────────────────────────────────────

/// #2441 gate: distil the successful action sequence into a reusable skill only
/// on a verified success. `VerifiedFailure`/`Unverified` => `false` (fail-safe).
pub fn should_distill_skill(verdict: &Verdict) -> bool {
    matches!(verdict, Verdict::VerifiedSuccess)
}

/// #2458 gate: promote a failure reflection to a procedural lesson only when the
/// verdict is a `VerifiedFailure` AND the `(goal_type, error_class)` failure has
/// recurred `recurrence_count >= threshold` times. Success/unverified verdicts —
/// and failures below the threshold (one-offs) — never produce a lesson.
pub fn should_distill_lesson(verdict: &Verdict, recurrence_count: u32, threshold: u32) -> bool {
    matches!(verdict, Verdict::VerifiedFailure { .. }) && recurrence_count >= threshold
}

/// Derive the learning gate from an external verification report (AC-10).
///
/// Conservative — anything that is not an unambiguous external pass/fail becomes
/// [`Verdict::Unverified`]:
///
/// - `status` of `passed` / `verified` / `success` (case-insensitive) **and** at
///   least one recorded check → [`Verdict::VerifiedSuccess`].
/// - `status` of `failed` / `error` → [`Verdict::VerifiedFailure`] with
///   `error_class` derived from the first failing check (or the summary),
///   normalized via [`normalize_error_class`].
/// - anything else (empty/`skipped`/`unverified`/`unknown` status, or no checks)
///   → [`Verdict::Unverified`].
pub fn verified_outcome(report: &VerificationReport) -> Verdict {
    let status = report.status.trim().to_lowercase();
    match status.as_str() {
        "passed" | "verified" | "success" if !report.checks.is_empty() => Verdict::VerifiedSuccess,
        "failed" | "error" => {
            let raw = report
                .checks
                .first()
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(report.summary.as_str());
            Verdict::VerifiedFailure {
                error_class: normalize_error_class(raw),
            }
        }
        _ => Verdict::Unverified,
    }
}

// ── normalization + lesson naming ───────────────────────────────────────────

/// Normalize an objective string into a goal-type key: lowercased with
/// non-alphanumeric runs collapsed to single `-` (AC-11). Deterministic and
/// idempotent (`-` is itself non-alphanumeric, so re-splitting reproduces the
/// same tokens).
pub fn normalize_goal_type(objective: &str) -> String {
    join_alnum_tokens(objective, '-')
}

/// Normalize a raw failure descriptor into an error-class key: lowercased with
/// non-alphanumeric runs collapsed to single `_` (AC-11). Deterministic and
/// idempotent (e.g. `"Cargo Test FAILED"` → `"cargo_test_failed"`).
pub fn normalize_error_class(raw: &str) -> String {
    join_alnum_tokens(raw, '_')
}

/// Lowercase, split on non-alphanumeric runs, drop empties, and rejoin with
/// `sep`. The shared engine behind [`normalize_goal_type`] / [`normalize_error_class`].
fn join_alnum_tokens(raw: &str, sep: char) -> String {
    raw.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(&sep.to_string())
}

/// Compose the canonical `lesson:<goal_type>:<error_class>` procedure name. Both
/// keys are normalized first so the same logical failure always maps to one name.
pub fn lesson_name(goal_type: &str, error_class: &str) -> String {
    format!(
        "{LESSON_NAME_PREFIX}{}:{}",
        normalize_goal_type(goal_type),
        normalize_error_class(error_class),
    )
}

/// `true` if `name` is a lesson (failure-derived) procedure.
pub fn is_lesson(name: &str) -> bool {
    name.starts_with(LESSON_NAME_PREFIX)
}

/// `skill` / `lesson` kind label for a procedure name (used by the metrics).
pub fn procedure_kind(name: &str) -> &'static str {
    if is_lesson(name) { "lesson" } else { "skill" }
}

// ── reflection text + recurrence marker ─────────────────────────────────────

/// A unique, fully-delimited content marker keyed by `(goal_type, error_class)`.
/// Embedded in every reflection episode so [`count_recurring_failures`] can find
/// exactly the reflections for one logical failure via a substring search. The
/// surrounding brackets make the match collision-safe across key prefixes
/// (`fix` vs `fix-ci`).
fn reflection_marker(goal_type: &str, error_class: &str) -> String {
    format!(
        "[reflect-key={}|{}]",
        normalize_goal_type(goal_type),
        normalize_error_class(error_class),
    )
}

/// Build the verbal reflection body (attempted / external verdict / next time).
/// Pure and deterministic — extracted for unit testing (AC-4).
pub fn reflection_text(objective: &str, error_class: &str, hint: &str) -> String {
    format!(
        "Reflection (verified failure). Attempted: {objective}. \
         External verdict: failed ({error_class}). Next time: {hint}.",
    )
}

// ── memory-backed entry points (all no-ops on Unverified, R10) ──────────────

/// Generate and store a verbal failure reflection for a *verified* failure as an
/// episodic note (source label `reflection:failure`), keyed by
/// `(goal_type, error_class)` so [`count_recurring_failures`] can find it
/// (#2458, AC-4).
///
/// No-op (returns `Ok(None)`) unless `verdict` is [`Verdict::VerifiedFailure`].
/// Returns the stored episode id — the provenance anchor for a future lesson.
pub fn record_failure_reflection(
    memory: &dyn CognitiveMemoryOps,
    objective: &str,
    verdict: &Verdict,
    hint: &str,
) -> SimardResult<Option<String>> {
    record_failure_reflection_marked(memory, objective, verdict, hint, None)
}

/// As [`record_failure_reflection`], but optionally embeds a per-occurrence
/// dedup marker so [`occurrence_already_reflected`] can recognise a repeat
/// observation of the *same* blocked completion across cycles.
fn record_failure_reflection_marked(
    memory: &dyn CognitiveMemoryOps,
    objective: &str,
    verdict: &Verdict,
    hint: &str,
    occurrence_key: Option<&str>,
) -> SimardResult<Option<String>> {
    let Verdict::VerifiedFailure { error_class } = verdict else {
        return Ok(None);
    };
    let goal_type = normalize_goal_type(objective);
    let marker = reflection_marker(&goal_type, error_class);
    let mut content = format!("{} {marker}", reflection_text(objective, error_class, hint));
    if let Some(key) = occurrence_key {
        content.push(' ');
        content.push_str(&occurrence_marker(key));
    }
    let metadata = serde_json::json!({
        "kind": "reflection",
        "failure": true,
        "goal_type": goal_type,
        "error_class": error_class,
        "occurrence_key": occurrence_key,
    });
    let id = memory.store_episode(&content, "reflection:failure", Some(&metadata))?;
    Ok(Some(id))
}

/// A fully-delimited content marker keyed by a per-occurrence identity (e.g. the
/// goal id). Embedded alongside the `(goal_type, error_class)` marker so the
/// same blocked completion, re-observed every OODA cycle, can be deduped to a
/// single reflection. The brackets make the match collision-safe across keys
/// that share a prefix.
fn occurrence_marker(occurrence_key: &str) -> String {
    format!("[reflect-occ={}]", normalize_error_class(occurrence_key))
}

/// `true` if a `reflection:failure` episode has already been recorded for this
/// exact occurrence key. Used to bound the failure-reflection trail for a goal
/// that stays blocked across many cycles (so one unresolved in-flight PR cannot
/// accrue an unbounded reflection stream or a per-cycle `brain_repeat_failure`).
fn occurrence_already_reflected(memory: &dyn CognitiveMemoryOps, occurrence_key: &str) -> bool {
    let marker = occurrence_marker(occurrence_key);
    memory
        .search_episodes_by_keywords(std::slice::from_ref(&marker), 1)
        .map(|hits| hits.iter().any(|e| e.content.contains(&marker)))
        .unwrap_or(false)
}

/// Count stored `reflection:failure` episodes recorded for this
/// `(goal_type, error_class)` key (#2458). Implemented as an exact-marker
/// substring search so it is collision-safe across key prefixes.
pub fn count_recurring_failures(
    memory: &dyn CognitiveMemoryOps,
    goal_type: &str,
    error_class: &str,
) -> SimardResult<u32> {
    let marker = reflection_marker(goal_type, error_class);
    let hits = memory.search_episodes_by_keywords(std::slice::from_ref(&marker), u32::MAX)?;
    let count = hits.iter().filter(|e| e.content.contains(&marker)).count();
    Ok(count as u32)
}

/// `true` if a `lesson:<goal_type>:<error_class>` procedure already exists
/// (#2458). Uses exact-name equality on recall hits (mirrors `procedure_exists`,
/// #2298), not a bare `CONTAINS` emptiness check.
pub fn has_lesson_for(
    memory: &dyn CognitiveMemoryOps,
    goal_type: &str,
    error_class: &str,
) -> SimardResult<bool> {
    memory.procedure_exists(&lesson_name(goal_type, error_class))
}

/// Distil a lesson from recurring reflections, gated on recurrence (#2458).
///
/// Returns `Ok(None)` when the verdict is not a [`Verdict::VerifiedFailure`] or
/// the recurrence count is `< threshold` (a one-off failure, AC-6). When the
/// count is `>= threshold` (AC-5) it stores a `lesson:<goal_type>:<error_class>`
/// procedure via `store_procedure_with_provenance` (linking the source
/// reflection episodes with `PROCEDURE_DERIVES_FROM` edges, #2325) and returns
/// the lesson node id. Storing is idempotent by name (#2298): a recurring
/// failure reinforces the existing lesson's `usage_count` rather than
/// duplicating it. A *newly* created lesson emits `brain_new_procedure`.
pub fn maybe_distill_lesson(
    memory: &dyn CognitiveMemoryOps,
    verdict: &Verdict,
    objective: &str,
    threshold: u32,
    source_episode_ids: &[String],
) -> SimardResult<Option<String>> {
    let Verdict::VerifiedFailure { error_class } = verdict else {
        return Ok(None);
    };
    let goal_type = normalize_goal_type(objective);
    let count = count_recurring_failures(memory, &goal_type, error_class)?;
    if !should_distill_lesson(verdict, count, threshold) {
        return Ok(None);
    }
    let name = lesson_name(&goal_type, error_class);
    let newly = !memory.procedure_exists(&name)?;
    let steps = vec![
        format!(
            "Recurring failure ({count}x) on goal-type '{goal_type}' with error \
             class '{error_class}'. Recall this lesson before re-attempting."
        ),
        "Inspect the prior reflections (PROCEDURE_DERIVES_FROM) before acting.".to_string(),
    ];
    let id = memory.store_procedure_with_provenance(&name, &steps, &[], source_episode_ids)?;
    if newly {
        record_new_procedure(&id, &name);
    }
    Ok(Some(id))
}

// ── live wiring: verified failures → reflections → lessons (#2458) ───────────

/// One externally-verified failure observation sourced from the FU1 (#2456)
/// completion gate — a [`VerificationOutcome::Refuted`](crate::goal_curation::VerificationOutcome)
/// false completion where a derivable external postcondition contradicted the
/// done-claim.
///
/// `objective` is the failed goal's description (the goal-type source);
/// `error_class` is the normalized refuting signal (e.g. `pr_not_merged`). Both
/// are re-normalized downstream, so callers may pass raw text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedFailureObservation {
    /// The failed goal's objective/description — normalized into the goal-type.
    pub objective: String,
    /// The normalized refuting signal (the `error_class` half of the key).
    pub error_class: String,
    /// Stable per-occurrence identity (e.g. the goal id). When `Some`, the same
    /// blocked completion re-observed across cycles is deduped to **one**
    /// reflection — bounding the failure-reflection trail and keeping recurrence
    /// honest (distinct occurrences, not cycle count). `None` disables dedup, so
    /// every call is treated as a fresh occurrence.
    pub occurrence_key: Option<String>,
}

impl VerifiedFailureObservation {
    /// Construct an observation with **no** per-occurrence dedup — every call is
    /// a distinct occurrence. Use [`deduped`](Self::deduped) for the live cycle
    /// path where the same blocked goal recurs across cycles.
    pub fn new(objective: impl Into<String>, error_class: impl Into<String>) -> Self {
        Self {
            objective: objective.into(),
            error_class: error_class.into(),
            occurrence_key: None,
        }
    }

    /// Construct an observation deduped across cycles by `occurrence_key` (the
    /// goal id at the live OODA seam). A goal that stays blocked over many cycles
    /// then contributes exactly one reflection, and `brain_repeat_failure` fires
    /// only on a genuinely *new* occurrence — never once per cycle.
    pub fn deduped(
        objective: impl Into<String>,
        error_class: impl Into<String>,
        occurrence_key: impl Into<String>,
    ) -> Self {
        Self {
            objective: objective.into(),
            error_class: error_class.into(),
            occurrence_key: Some(occurrence_key.into()),
        }
    }
}

/// Aggregate result of one [`learn_from_verified_failures`] pass. All counters
/// are observable so the OODA cycle can log what the failure-reflection pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LessonLearningReport {
    /// Reflections written this pass (one per verified failure observation).
    pub reflections_recorded: u32,
    /// **New** `lesson:` procedures distilled this pass (recurrence first
    /// reached the threshold — not a reinforcing re-store).
    pub lessons_distilled: u32,
    /// Verified failures that recurred on a goal-type that *already* carried a
    /// lesson — the loop's self-regression signal (`brain_repeat_failure`).
    pub repeat_failures: u32,
}

impl LessonLearningReport {
    /// `true` when the pass did nothing (empty/all-skipped batch).
    pub fn is_empty(&self) -> bool {
        self.reflections_recorded == 0 && self.lessons_distilled == 0 && self.repeat_failures == 0
    }
}

/// Deterministic default reflection hint, grounded in the **external** signal
/// (the refuting `error_class`) rather than any self-judgement — the same
/// fail-safe invariant the rest of this module enforces (R10).
fn default_failure_hint(error_class: &str) -> String {
    format!(
        "Recall the lesson for this goal-type and resolve the refuting signal \
         ({error_class}) before re-claiming completion.",
    )
}

/// Live production trigger (#2458): turn a batch of **externally-verified**
/// failures into reflections and, on recurrence, procedural lessons.
///
/// This is the wiring the issue gated on FU1 (#2456) for: every observation
/// here is sourced from a real external signal
/// ([`VerificationOutcome::Refuted`](crate::goal_curation::VerificationOutcome)),
/// never the brain's self-reported `ActionOutcome.success` (R10). The OODA
/// curate phase calls it with the goals the completion gate refuted.
///
/// For each observation:
/// 0. **dedup** — if the observation carries an `occurrence_key` already
///    reflected (the same blocked goal seen on a prior cycle), skip it entirely.
///    This bounds the reflection trail and keeps recurrence honest (distinct
///    occurrences, not cycle count);
/// 1. record a `reflection:failure` episode ([`record_failure_reflection`]);
/// 2. capture whether a lesson *already* existed for this
///    `(goal_type, error_class)` **before** distilling — a failure that recurs
///    despite an existing lesson is the self-regression signal, emitted as
///    `brain_repeat_failure` ([`record_repeat_failure`]);
/// 3. distil a `lesson:` procedure once recurrence reaches `threshold`
///    ([`maybe_distill_lesson`]); idempotent by name (#2298).
///
/// Best-effort per observation: an error on one failure is logged and skipped so
/// one bad record never drops the rest of the batch. Never returns `Err` and
/// never blocks the cycle. A `threshold` of `0` is treated as
/// [`LESSON_RECURRENCE_THRESHOLD`] so a misconfiguration cannot turn every
/// one-off failure into a lesson.
pub fn learn_from_verified_failures(
    memory: &dyn CognitiveMemoryOps,
    failures: &[VerifiedFailureObservation],
    threshold: u32,
) -> LessonLearningReport {
    let threshold = if threshold == 0 {
        LESSON_RECURRENCE_THRESHOLD
    } else {
        threshold
    };
    let mut report = LessonLearningReport::default();

    for obs in failures {
        let error_class = normalize_error_class(&obs.error_class);

        // Per-occurrence dedup (review #2510): a goal that stays blocked across
        // cycles (e.g. a normal in-flight PR not yet merged) must not re-reflect
        // every cycle — that would grow episodic memory without bound, distil a
        // lesson from a single premature claim, and emit `brain_repeat_failure`
        // every cycle. Skip an observation whose occurrence was already
        // reflected; recurrence is measured across *distinct* occurrences.
        if let Some(key) = &obs.occurrence_key
            && occurrence_already_reflected(memory, key)
        {
            continue;
        }

        let verdict = Verdict::VerifiedFailure {
            error_class: error_class.clone(),
        };

        // The self-regression check must read state BEFORE this failure's
        // reflection/lesson is (re)stored — otherwise a freshly-distilled lesson
        // would mask the very recurrence we want to flag.
        let had_lesson = has_lesson_for(memory, &obs.objective, &error_class).unwrap_or(false);

        let reflection_id = match record_failure_reflection_marked(
            memory,
            &obs.objective,
            &verdict,
            &default_failure_hint(&error_class),
            obs.occurrence_key.as_deref(),
        ) {
            Ok(Some(id)) => {
                report.reflections_recorded += 1;
                Some(id)
            }
            // Unreachable for a `VerifiedFailure`, but harmless: distillation
            // still gates on the persisted recurrence count.
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                    target: "simard::reflection_lessons",
                    objective = %obs.objective,
                    error = %e,
                    "record_failure_reflection failed; skipping observation",
                );
                continue;
            }
        };
        let sources: Vec<String> = reflection_id.into_iter().collect();

        match maybe_distill_lesson(memory, &verdict, &obs.objective, threshold, &sources) {
            Ok(Some(lesson_id)) => {
                if had_lesson {
                    // Recurred despite an existing lesson — self-regression.
                    report.repeat_failures += 1;
                    record_repeat_failure(
                        &normalize_goal_type(&obs.objective),
                        &error_class,
                        &lesson_id,
                    );
                } else {
                    // First crossing of the recurrence threshold — a new lesson.
                    report.lessons_distilled += 1;
                }
            }
            // Below threshold — a one-off failure, no lesson yet (AC-6).
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    target: "simard::reflection_lessons",
                    objective = %obs.objective,
                    error = %e,
                    "maybe_distill_lesson failed; reflection retained for next pass",
                );
            }
        }
    }

    report
}

// ── metrics ─────────────────────────────────────────────────────────────────

/// `brain_skill_reuse` — a recalled procedure was applied and reinforced (#2441).
pub const SKILL_REUSE_METRIC: &str = "brain_skill_reuse";

/// `brain_new_procedure` — a new skill or lesson procedure was stored (#2441/#2458).
pub const NEW_PROCEDURE_METRIC: &str = "brain_new_procedure";

/// `brain_repeat_failure` — a verified failure recurred on a goal-type that
/// already carries a lesson (#2458): the loop's self-regression signal.
pub const REPEAT_FAILURE_METRIC: &str = "brain_repeat_failure";

/// Build the `{procedure_id, kind}` context for a `brain_skill_reuse` data point.
/// Pure — no I/O.
pub fn skill_reuse_context(procedure_id: &str, procedure_name: &str) -> String {
    serde_json::json!({
        "procedure_id": procedure_id,
        "kind": procedure_kind(procedure_name),
    })
    .to_string()
}

/// Build the `{procedure_id, name, kind}` context for a `brain_new_procedure`
/// data point. Pure — no I/O.
pub fn new_procedure_context(procedure_id: &str, procedure_name: &str) -> String {
    serde_json::json!({
        "procedure_id": procedure_id,
        "name": procedure_name,
        "kind": procedure_kind(procedure_name),
    })
    .to_string()
}

/// Build the `{goal_type, error_class, lesson_id}` context for a
/// `brain_repeat_failure` data point. Pure — no I/O. Keys are normalized first.
pub fn repeat_failure_context(goal_type: &str, error_class: &str, lesson_id: &str) -> String {
    serde_json::json!({
        "goal_type": normalize_goal_type(goal_type),
        "error_class": normalize_error_class(error_class),
        "lesson_id": lesson_id,
    })
    .to_string()
}

/// Emit `brain_skill_reuse` (= `1.0`) for one applied recalled procedure (#2441).
///
/// Best-effort: a metrics-write failure is logged, never propagated — the reuse
/// already happened. A `cfg!(test)` no-op so the broad suite never writes
/// metrics (the metric *shape* is covered by [`skill_reuse_context`]); mirrors
/// [`super::distillation`]'s reliability-gate metric.
pub fn record_skill_reuse(procedure_id: &str, procedure_name: &str) {
    emit(
        SKILL_REUSE_METRIC,
        &skill_reuse_context(procedure_id, procedure_name),
    );
}

/// Emit `brain_new_procedure` (= `1.0`) when a new skill/lesson procedure is
/// first stored (#2441/#2458). Best-effort + `cfg!(test)` no-op.
pub fn record_new_procedure(procedure_id: &str, procedure_name: &str) {
    emit(
        NEW_PROCEDURE_METRIC,
        &new_procedure_context(procedure_id, procedure_name),
    );
}

/// Emit `brain_repeat_failure` (= `1.0`) when a verified failure recurs on a
/// goal-type that already carries a lesson (#2458). Best-effort + `cfg!(test)`
/// no-op. The caller establishes the precondition via [`has_lesson_for`].
pub fn record_repeat_failure(goal_type: &str, error_class: &str, lesson_id: &str) {
    emit(
        REPEAT_FAILURE_METRIC,
        &repeat_failure_context(goal_type, error_class, lesson_id),
    );
}

/// Shared best-effort, `cfg!(test)`-guarded metric write for this module.
fn emit(metric: &str, context: &str) {
    if cfg!(test) {
        return;
    }
    if let Err(e) = crate::self_metrics::record_metric(metric, 1.0, context) {
        tracing::warn!(
            target: "simard::reflection_lessons",
            metric,
            error = %e,
            "record_metric failed (loop event still occurred)",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cargo_failure() -> Verdict {
        Verdict::VerifiedFailure {
            error_class: "cargo_test_failed".to_string(),
        }
    }

    #[test]
    fn skill_distilled_only_on_verified_success() {
        assert!(should_distill_skill(&Verdict::VerifiedSuccess));
        assert!(!should_distill_skill(&cargo_failure()));
        assert!(!should_distill_skill(&Verdict::Unverified));
    }

    #[test]
    fn lesson_distilled_only_on_recurring_verified_failure() {
        assert!(!should_distill_lesson(
            &cargo_failure(),
            1,
            LESSON_RECURRENCE_THRESHOLD
        ));
        assert!(should_distill_lesson(
            &cargo_failure(),
            LESSON_RECURRENCE_THRESHOLD,
            LESSON_RECURRENCE_THRESHOLD
        ));
        assert!(should_distill_lesson(
            &cargo_failure(),
            5,
            LESSON_RECURRENCE_THRESHOLD
        ));
        assert!(!should_distill_lesson(
            &Verdict::VerifiedSuccess,
            5,
            LESSON_RECURRENCE_THRESHOLD
        ));
        assert!(!should_distill_lesson(
            &Verdict::Unverified,
            5,
            LESSON_RECURRENCE_THRESHOLD
        ));
    }

    #[test]
    fn recurrence_threshold_excludes_singletons() {
        assert_eq!(LESSON_RECURRENCE_THRESHOLD, 2);
    }

    /// AC-10: `verified_outcome` maps pass/fail/unknown reports to the right
    /// verdict, conservatively defaulting to `Unverified`.
    #[test]
    fn verified_outcome_mapping() {
        let pass = VerificationReport {
            status: "verified".to_string(),
            summary: "ok".to_string(),
            checks: vec!["git: 1 new commit".to_string()],
        };
        assert_eq!(verified_outcome(&pass), Verdict::VerifiedSuccess);

        // pass status but no checks → not trustworthy → Unverified.
        let pass_no_checks = VerificationReport {
            status: "passed".to_string(),
            summary: String::new(),
            checks: vec![],
        };
        assert_eq!(verified_outcome(&pass_no_checks), Verdict::Unverified);

        let fail = VerificationReport {
            status: "FAILED".to_string(),
            summary: "cargo test failed".to_string(),
            checks: vec!["cargo test: 3 failed".to_string()],
        };
        assert_eq!(
            verified_outcome(&fail),
            Verdict::VerifiedFailure {
                error_class: "cargo_test_3_failed".to_string()
            }
        );

        for s in ["unverified", "skipped", "", "unknown"] {
            let r = VerificationReport {
                status: s.to_string(),
                summary: "x".to_string(),
                checks: vec!["c".to_string()],
            };
            assert_eq!(verified_outcome(&r), Verdict::Unverified, "status {s:?}");
        }
    }

    /// AC-11: normalization is deterministic and idempotent.
    #[test]
    fn normalization_is_idempotent() {
        let gt = normalize_goal_type("Fix CI linker OOM!");
        assert_eq!(gt, "fix-ci-linker-oom");
        assert_eq!(normalize_goal_type(&gt), gt);

        let ec = normalize_error_class("Cargo Test FAILED");
        assert_eq!(ec, "cargo_test_failed");
        assert_eq!(normalize_error_class(&ec), ec);
    }

    #[test]
    fn lesson_name_and_is_lesson() {
        let name = lesson_name("Fix CI", "Cargo Test FAILED");
        assert_eq!(name, "lesson:fix-ci:cargo_test_failed");
        assert!(is_lesson(&name));
        assert!(!is_lesson("merge branch updater"));
        assert_eq!(procedure_kind(&name), "lesson");
        assert_eq!(procedure_kind("merge branch updater"), "skill");
    }

    #[test]
    fn reflection_text_is_structured() {
        let t = reflection_text("rebuild index", "build_failed", "pin the toolchain");
        assert!(t.contains("Attempted: rebuild index"));
        assert!(t.contains("failed (build_failed)"));
        assert!(t.contains("Next time: pin the toolchain"));
    }

    #[test]
    fn metric_contexts_have_expected_shapes() {
        let reuse: serde_json::Value =
            serde_json::from_str(&skill_reuse_context("p1", "lesson:g:e")).unwrap();
        assert_eq!(reuse["procedure_id"], "p1");
        assert_eq!(reuse["kind"], "lesson");

        let newp: serde_json::Value =
            serde_json::from_str(&new_procedure_context("p2", "deploy-service")).unwrap();
        assert_eq!(newp["procedure_id"], "p2");
        assert_eq!(newp["name"], "deploy-service");
        assert_eq!(newp["kind"], "skill");

        let rep: serde_json::Value =
            serde_json::from_str(&repeat_failure_context("Fix CI", "Cargo FAILED", "p3")).unwrap();
        assert_eq!(rep["goal_type"], "fix-ci");
        assert_eq!(rep["error_class"], "cargo_failed");
        assert_eq!(rep["lesson_id"], "p3");

        assert_eq!(SKILL_REUSE_METRIC, "brain_skill_reuse");
        assert_eq!(NEW_PROCEDURE_METRIC, "brain_new_procedure");
        assert_eq!(REPEAT_FAILURE_METRIC, "brain_repeat_failure");
    }
}
