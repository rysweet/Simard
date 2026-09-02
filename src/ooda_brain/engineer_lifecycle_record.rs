//! Typed, file-backed **engineer-lifecycle** OODA Act decision records and their
//! fail-CLOSED reader (Group E of epic #4719; issue #4967).
//!
//! This is the engineer-lifecycle analogue of the OODA orient/decide records in
//! [`super::orient_decide_record`] and the outcome record in [`super`]. It
//! retires the last **reasoner-decision** stdout-scrape seam: instead of the
//! `decide_engineer_lifecycle` rail scraping the recipe's prose for a decision
//! variant (`extract_decision_envelope` + first-word `strip_recipe_noise`
//! fallback + a silent `continue_skipping` default), the lifecycle recipe ACTS
//! by calling the gated `simard ooda record-lifecycle-decision` tool, which
//! validates the closed variant enum and atomically writes a typed, owner-only
//! (`0o600`), identity-bound [`EngineerLifecycleRecord`]. The thin Rust rail
//! reads that record **fail-closed** ([`read_verified_engineer_lifecycle_decision`],
//! R1–R7) and NEVER inspects stdout. EVERY failure mode is an `Err` — escalated
//! through the existing ladder — never a silent synthesized `continue_skipping`
//! (operator zero-fallback contract, #2580 / #1711).
//!
//! One shared closed-variant + free-text chokepoint ([`sanitize_lifecycle_fields`])
//! is invoked by BOTH the CLI writer and the reader, so they can never drift on
//! "what is a valid decision". The closed variant set and the
//! `lifecycle_decision_from_variant` mapping remain the SINGLE source of truth
//! in [`super::recipe_brain`] (imported, never forked).
//!
//! See `docs/reference/ooda-record-orient-decide-cli.md` (Group A) for the
//! sibling contract this mirrors.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cognitive_threads::recipe_rail::secret_scrub;

use super::EngineerLifecycleDecision;
use super::recipe_brain::lifecycle_decision_from_variant;
use super::sanitize::sanitize_context_var;

/// The pinned on-disk schema string for an [`EngineerLifecycleRecord`]. The
/// reader rejects any other value (R3), so a future `…/v2` writer can never be
/// honored by a `…/v1` reader (bumping this is a hard, coordinated change).
pub const ENGINEER_LIFECYCLE_SCHEMA: &str = "simard.ooda.engineer_lifecycle.v1";

/// Freshness window (seconds) for the reader's R7 anti-replay gate. The
/// lifecycle rail allocates a fresh, unique temp dir per call (so a stale file
/// cannot live at the path), and additionally rejects any record whose mtime or
/// embedded epoch skews further than this from now — mirrors the
/// thread-reasoning window.
pub const MAX_AGE_SECS: u64 = 300;

/// Hard character ceiling for a model-controlled `rationale`. Mirrors the
/// `recipe_brain` bound so a runaway model response cannot bloat an operator log
/// line or a persisted audit record. An over-long rationale is REJECTED
/// (fail-closed), never silently truncated.
pub const MAX_RATIONALE_CHARS: usize = 500;

/// One typed, on-disk engineer-lifecycle Act decision. Written by the
/// `simard ooda record-lifecycle-decision` tool and read by
/// [`read_verified_engineer_lifecycle_decision`]. This is a **flat** wire DTO
/// (`decision: String`), distinct from the rich [`EngineerLifecycleDecision`]
/// enum; `deny_unknown_fields` closes off any crafted extra top-level key.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineerLifecycleRecord {
    /// Schema pin. Must equal [`ENGINEER_LIFECYCLE_SCHEMA`] (R3).
    pub schema: String,
    /// The goal this decision is for. Re-verified against the live ctx (R6).
    pub goal_id: String,
    /// The cycle this decision is for. Re-verified against the live ctx (R7).
    pub cycle_number: u32,
    /// The closed lifecycle variant token (validated against the shared
    /// `LIFECYCLE_VARIANT_LIST` via `lifecycle_decision_from_variant`).
    pub decision: String,
    /// Optional model rationale; reused as body/reason/redispatch text by the
    /// extra-field variants exactly as the retired scrape path did. Bounded and
    /// sanitized through the shared chokepoint (R5). Empty is valid.
    #[serde(default)]
    pub rationale: String,
    /// Unix seconds the tool stamped at write time. Freshness defense-in-depth
    /// against mtime spoofing (R7).
    pub written_at_epoch: u64,
}

/// The single shared closed-variant + free-text chokepoint, invoked IDENTICALLY
/// by the CLI writer (reject ⇒ no file) and the reader (reject ⇒ R5).
///
/// * `decision` is matched (trim + case-fold) against the closed variant set via
///   the shared [`lifecycle_decision_from_variant`] mapping — an out-of-set word
///   ⇒ `None` (fail CLOSED — a compromised prompt cannot smuggle a novel action).
/// * `rationale` is stripped of ANSI/C0 control + whitespace-folded
///   ([`sanitize_context_var`]), credential-scrubbed ([`secret_scrub`]), then
///   REJECTED (never truncated) if it still exceeds [`MAX_RATIONALE_CHARS`]. An
///   empty rationale is valid.
///
/// Returns the fully decoded [`EngineerLifecycleDecision`] (carrying the
/// sanitized rationale in its extra-field variants) plus the sanitized
/// rationale string, so the writer stores a normalized record and the reader
/// obtains the validated decision from ONE call — writer and reader can never
/// drift. Callers that need the canonical snake_case token (e.g. the record's
/// `decision` field) project it with [`lifecycle_decision_choice`].
///
/// [`lifecycle_decision_choice`]: super::recipe_brain::lifecycle_decision_choice
pub fn sanitize_lifecycle_fields(
    decision: &str,
    rationale: &str,
) -> Option<(EngineerLifecycleDecision, String)> {
    // Sanitize with a large intermediate bound so control/whitespace is folded
    // WITHOUT truncating — we reject oversize below rather than silently
    // shortening a real rationale.
    let folded = sanitize_context_var(rationale.trim(), MAX_RATIONALE_CHARS * 8);
    let scrubbed = secret_scrub(&folded);
    let clean = scrubbed.trim().to_string();
    if clean.chars().count() > MAX_RATIONALE_CHARS {
        return None;
    }
    // The mapping is the SINGLE authority on the closed variant set; a match
    // yields the rich enum carrying the sanitized rationale.
    let mapped = lifecycle_decision_from_variant(decision.trim(), clean.clone())?;
    Some((mapped, clean))
}

/// A fail-closed read error, carrying the R-code of the check that tripped so
/// the rail can log the canonical `R{n} <reason>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleReadError {
    /// Which check in the R1–R7 matrix failed.
    pub code: u8,
    /// Human-readable detail (never persisted; diagnostics/log only).
    pub detail: String,
}

impl LifecycleReadError {
    fn new(code: u8, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for LifecycleReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "R{} {}", self.code, self.detail)
    }
}

impl std::error::Error for LifecycleReadError {}

/// Read and FULLY verify an engineer-lifecycle decision record, returning the
/// validated [`EngineerLifecycleDecision`] or a fail-closed
/// [`LifecycleReadError`]. Every failure mode is an `Err`, which the rail maps
/// to a low-confidence escalation (never a silent success, never scraped
/// stdout). The fail-CLOSED matrix:
///
/// | # | Condition | Result |
/// |---|---|---|
/// | R1 | file absent / unreadable / no metadata / no mtime | `Err(1)` |
/// | R2 | present but not valid JSON | `Err(2)` |
/// | R3 | `schema != ENGINEER_LIFECYCLE_SCHEMA` | `Err(3)` |
/// | R4 | unknown top-level key (`deny_unknown_fields`) OR file mode `& 0o077 != 0` | `Err(4)` |
/// | R5 | `decision` ∉ variant set (post trim/case-fold) OR `rationale` > cap | `Err(5)` |
/// | R6 | `record.goal_id != expected_goal_id` | `Err(6)` |
/// | R7 | `cycle_number != expected_cycle`, `now - mtime > MAX_AGE_SECS`, or epoch skew | `Err(7)` |
/// | R8 | all checks pass | `Ok(EngineerLifecycleDecision)` |
///
/// Unlike the thread-reasoning reader, the lifecycle rail does not thread an
/// `invoke_start` (it allocates a fresh unique temp dir per call, so a leftover
/// file cannot exist at the path); freshness is enforced by `MAX_AGE_SECS` +
/// epoch skew alone. The perms gate (`mode & 0o077`) is retained because it is a
/// cheap, high-value tamper check demanded by the live canary.
pub fn read_verified_engineer_lifecycle_decision(
    path: &Path,
    expected_goal_id: &str,
    expected_cycle: u32,
) -> Result<EngineerLifecycleDecision, LifecycleReadError> {
    // R1 — absence / unreadable is fail-CLOSED. The tool writes nothing when it
    // cannot resolve its path or fails validation.
    let bytes = std::fs::read(path)
        .map_err(|e| LifecycleReadError::new(1, format!("no record at expected path: {e}")))?;
    let metadata = std::fs::metadata(path)
        .map_err(|e| LifecycleReadError::new(1, format!("no record metadata: {e}")))?;
    let mtime = metadata
        .modified()
        .map_err(|e| LifecycleReadError::new(1, format!("no record mtime: {e}")))?;

    // R4(perms) — owner-only. A group/other-accessible record on the shared host
    // is a tamper signal (another uid could have written/overwritten it).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(LifecycleReadError::new(
                4,
                format!(
                    "record is not owner-only (mode {:#o} has group/other bits)",
                    mode & 0o777
                ),
            ));
        }
    }

    // R2/R4(deny-unknown) — malformed JSON or any unknown top-level key
    // (`deny_unknown_fields`) fails deserialization.
    let record: EngineerLifecycleRecord = serde_json::from_slice(&bytes)
        .map_err(|e| LifecycleReadError::new(2, format!("malformed JSON / unknown field: {e}")))?;

    // R3 — schema version pin.
    if record.schema != ENGINEER_LIFECYCLE_SCHEMA {
        return Err(LifecycleReadError::new(
            3,
            format!(
                "schema mismatch {:?} != {ENGINEER_LIFECYCLE_SCHEMA:?}",
                record.schema
            ),
        ));
    }

    // R6 — goal identity. A record written for another goal must never be
    // honored (cross-goal replay).
    if record.goal_id != expected_goal_id {
        return Err(LifecycleReadError::new(
            6,
            format!(
                "goal_id mismatch (record {:?} != live {:?})",
                record.goal_id, expected_goal_id
            ),
        ));
    }

    // R5 + defense-in-depth — re-validate the closed variant AND re-sanitize the
    // free text through the SAME chokepoint the writer used, obtaining the
    // decoded decision in ONE call. An out-of-set decision or an
    // oversized/hostile rationale fails closed here, never honored verbatim.
    // Success yields the canonical enum (with the extra-field variants' derived
    // body/reason/redispatch text preserved exactly).
    let (decision, _clean_rationale) =
        sanitize_lifecycle_fields(&record.decision, &record.rationale).ok_or_else(|| {
            LifecycleReadError::new(
                5,
                "decision not in the closed variant set OR rationale invalid after sanitize"
                    .to_string(),
            )
        })?;

    // R7 — cycle identity (no replay of a prior cycle's verdict).
    if record.cycle_number != expected_cycle {
        return Err(LifecycleReadError::new(
            7,
            format!(
                "cycle_number mismatch (record {} != live {expected_cycle})",
                record.cycle_number
            ),
        ));
    }

    // R7 — freshness / anti-replay. A small slack absorbs coarse filesystem
    // mtime granularity; a genuinely stale (>MAX_AGE_SECS) record is rejected.
    const MTIME_SLACK: Duration = Duration::from_secs(2);
    let now = SystemTime::now();
    if let Ok(age) = now.duration_since(mtime)
        && age.as_secs() > MAX_AGE_SECS
    {
        return Err(LifecycleReadError::new(
            7,
            format!(
                "freshness/anti-replay (record age {}s > {MAX_AGE_SECS}s)",
                age.as_secs()
            ),
        ));
    }
    // A record whose mtime is in the future beyond slack is equally suspect.
    if let Ok(skew) = mtime.duration_since(now)
        && skew > MTIME_SLACK
    {
        return Err(LifecycleReadError::new(
            7,
            format!(
                "freshness/anti-replay (record mtime {}s in the future)",
                skew.as_secs()
            ),
        ));
    }
    // Embedded-epoch defense-in-depth against mtime spoofing.
    let now_epoch = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now_epoch.abs_diff(record.written_at_epoch) > MAX_AGE_SECS {
        return Err(LifecycleReadError::new(
            7,
            format!(
                "freshness/anti-replay (written_at_epoch {} skews > {MAX_AGE_SECS}s from now {})",
                record.written_at_epoch, now_epoch
            ),
        ));
    }

    Ok(decision)
}

#[cfg(test)]
mod tests {
    use super::super::recipe_brain::lifecycle_decision_choice;
    use super::*;
    use std::io::Write;

    fn now_epoch() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Write a record to `path` at `0o600` (mirrors the tool's persist mode).
    fn write_record(path: &Path, record: &EngineerLifecycleRecord) {
        let bytes = serde_json::to_vec(record).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(&bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn valid_record() -> EngineerLifecycleRecord {
        EngineerLifecycleRecord {
            schema: ENGINEER_LIFECYCLE_SCHEMA.to_string(),
            goal_id: "goal-abc".to_string(),
            cycle_number: 0,
            decision: "reclaim_and_redispatch".to_string(),
            rationale: "engineer idle 7h; reclaim and redispatch".to_string(),
            written_at_epoch: now_epoch(),
        }
    }

    #[test]
    fn r8_accepts_a_well_formed_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        write_record(&path, &valid_record());
        let d = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap();
        assert!(matches!(
            d,
            EngineerLifecycleDecision::ReclaimAndRedispatch { .. }
        ));
    }

    #[test]
    fn r1_absent_file_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap_err();
        assert_eq!(e.code, 1);
    }

    #[test]
    fn r2_malformed_json_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"{not json").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap_err();
        assert_eq!(e.code, 2);
    }

    #[test]
    fn r3_wrong_schema_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        let mut r = valid_record();
        r.schema = "simard.ooda.engineer_lifecycle.v2".to_string();
        write_record(&path, &r);
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap_err();
        assert_eq!(e.code, 3);
    }

    #[test]
    fn r4_unknown_field_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        // Hand-craft JSON with an extra key so deny_unknown_fields trips.
        let json = format!(
            r#"{{"schema":"{ENGINEER_LIFECYCLE_SCHEMA}","goal_id":"goal-abc","cycle_number":0,"decision":"deprioritize","rationale":"x","written_at_epoch":{},"smuggled":"evil"}}"#,
            now_epoch()
        );
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(json.as_bytes()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap_err();
        // deny_unknown_fields surfaces as a deserialize error (R2 code path).
        assert!(
            e.code == 2 || e.code == 4,
            "unknown field must fail closed, got R{}",
            e.code
        );
    }

    #[cfg(unix)]
    #[test]
    fn r4_group_readable_perms_is_err() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        write_record(&path, &valid_record());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap_err();
        assert_eq!(e.code, 4);
    }

    #[test]
    fn r5_out_of_set_decision_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        let mut r = valid_record();
        r.decision = "delete_everything".to_string();
        write_record(&path, &r);
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap_err();
        assert_eq!(e.code, 5);
    }

    #[test]
    fn r5_oversize_rationale_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        let mut r = valid_record();
        r.rationale = "a".repeat(MAX_RATIONALE_CHARS + 50);
        write_record(&path, &r);
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap_err();
        assert_eq!(e.code, 5);
    }

    #[test]
    fn r6_wrong_goal_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        write_record(&path, &valid_record());
        let e = read_verified_engineer_lifecycle_decision(&path, "other-goal", 0).unwrap_err();
        assert_eq!(e.code, 6);
    }

    #[test]
    fn r7_wrong_cycle_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        write_record(&path, &valid_record());
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 1).unwrap_err();
        assert_eq!(e.code, 7);
    }

    #[test]
    fn r7_stale_epoch_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        let mut r = valid_record();
        r.written_at_epoch = now_epoch().saturating_sub(MAX_AGE_SECS + 60);
        write_record(&path, &r);
        let e = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap_err();
        assert_eq!(e.code, 7);
    }

    #[test]
    fn empty_rationale_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lifecycle.json");
        let mut r = valid_record();
        r.decision = "continue_skipping".to_string();
        r.rationale = String::new();
        write_record(&path, &r);
        let d = read_verified_engineer_lifecycle_decision(&path, "goal-abc", 0).unwrap();
        assert!(matches!(
            d,
            EngineerLifecycleDecision::ContinueSkipping { .. }
        ));
    }

    #[test]
    fn shared_chokepoint_folds_case_and_bounds() {
        // Case-insensitive variant, decoded to the canonical decision.
        let (dec, r) = sanitize_lifecycle_fields("Continue_Skipping", "  healthy  ").unwrap();
        assert_eq!(lifecycle_decision_choice(&dec), "continue_skipping");
        assert_eq!(r, "healthy");
        // Out-of-set decision.
        assert!(sanitize_lifecycle_fields("frobnicate", "x").is_none());
        // Oversize rationale.
        assert!(
            sanitize_lifecycle_fields("deprioritize", &"a".repeat(MAX_RATIONALE_CHARS + 1))
                .is_none()
        );
    }
}
