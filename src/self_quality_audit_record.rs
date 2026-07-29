//! Typed, file-backed **self-quality-audit** record and its fail-CLOSED reader
//! (issue #4968).
//!
//! This retires the last brittle-parse antipattern survivor: the monthly
//! self-quality-audit adapter used to scrape concatenated recipe step-output
//! text for `WAVE_COMPLETE=`/`PR_OPENED=`/`AUDIT_COMPLETE=` markers. It now
//! follows the same typed-record contract Groups A–C use: the recipe ACTS by
//! calling the gated `simard cognition record-self-quality-audit` tool, which
//! writes a typed, owner-only (`0o600`), freshness-checked
//! [`SelfQualityAuditRecord`]; the thin Rust rail reads it **fail-closed**
//! ([`read_verified_self_quality_audit`], R1–R7) and NEVER a silent default.
//!
//! One shared bounds chokepoint ([`check_bounds`]) is invoked by BOTH the CLI
//! writer and the reader, so they can never drift on "what is a valid record".

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

/// Stable adapter tag used in the fail-closed error envelopes.
const ADAPTER_TAG: &str = "monthly-self-quality-audit";

/// The pinned on-disk schema string. The reader rejects any other value (R3).
pub const SELF_QUALITY_AUDIT_SCHEMA: &str = "self-quality-audit/v1";

/// Freshness window (seconds) for the reader's R7 anti-replay gate.
pub const MAX_AGE_SECS: u64 = 300;

/// The audit runs exactly five SEEK→VALIDATE→FIX waves, so a completed count
/// above five is impossible and rejected (R4).
const MAX_WAVES: u32 = 5;
/// Per-list element cap (defense in depth) — an over-cap list fails closed (R4).
const MAX_LIST_LEN: usize = 64;
/// Hard byte ceiling for a single PR URL or the summary line (R4).
const MAX_ELEMENT_BYTES: usize = 512;
/// Coarse-mtime slack absorbing filesystem granularity without admitting a
/// genuinely stale record.
const MTIME_SLACK: Duration = Duration::from_secs(2);

/// One typed, on-disk self-quality-audit record. Written by the
/// `simard cognition record-self-quality-audit` tool and read by
/// [`read_verified_self_quality_audit`]. `deny_unknown_fields` closes off any
/// crafted extra top-level key (R4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfQualityAuditRecord {
    /// Schema pin. Must equal [`SELF_QUALITY_AUDIT_SCHEMA`] (R3).
    pub schema: String,
    /// Unix seconds the recipe stamped at write time. Freshness defense (R7).
    pub written_at_epoch: u64,
    /// SEEK→VALIDATE→FIX waves that reached completion (bounded `0..=5`).
    pub waves_completed: u32,
    /// Pull request URLs opened across all waves.
    #[serde(default)]
    pub prs_opened: Vec<String>,
    /// Pull request URLs self-merged.
    #[serde(default)]
    pub prs_merged: Vec<String>,
    /// Pull request URLs crusty-old-engineer approved.
    #[serde(default)]
    pub crusty_approved: Vec<String>,
    /// Pull request URLs left open after the bounded crusty loop gave up.
    #[serde(default)]
    pub crusty_unresolved: Vec<String>,
    /// The agent's own terminal one-line summary (REQUIRED, non-empty → R5).
    pub summary_line: String,
}

/// Re-validate the closed structural invariants shared by writer and reader:
/// the `waves_completed` ceiling + per-list element/byte caps (R4) and the
/// required non-empty `summary_line` (R5). Returns `(r_code, detail)` on the
/// first breach. Invoked IDENTICALLY on write and read, so they can never drift.
pub(crate) fn check_bounds(rec: &SelfQualityAuditRecord) -> Result<(), (u8, String)> {
    if rec.waves_completed > MAX_WAVES {
        return Err((
            4,
            format!(
                "waves_completed {} exceeds the {MAX_WAVES}-wave cap",
                rec.waves_completed
            ),
        ));
    }
    for (name, list) in [
        ("prs_opened", &rec.prs_opened),
        ("prs_merged", &rec.prs_merged),
        ("crusty_approved", &rec.crusty_approved),
        ("crusty_unresolved", &rec.crusty_unresolved),
    ] {
        if list.len() > MAX_LIST_LEN {
            return Err((
                4,
                format!("{name} over cap ({} > {MAX_LIST_LEN})", list.len()),
            ));
        }
        for element in list {
            if element.len() > MAX_ELEMENT_BYTES {
                return Err((
                    4,
                    format!(
                        "{name} element over {MAX_ELEMENT_BYTES}-byte cap ({} bytes)",
                        element.len()
                    ),
                ));
            }
        }
    }
    if rec.summary_line.len() > MAX_ELEMENT_BYTES {
        return Err((
            4,
            format!(
                "summary_line over {MAX_ELEMENT_BYTES}-byte cap ({} bytes)",
                rec.summary_line.len()
            ),
        ));
    }
    if rec.summary_line.trim().is_empty() {
        return Err((
            5,
            "summary_line is empty (a non-empty summary is REQUIRED)".to_string(),
        ));
    }
    Ok(())
}

/// A fail-closed `AdapterInvocationFailed` whose reason carries the R-code of the
/// check that tripped, tagged to this adapter.
fn fail(code: u8, detail: impl AsRef<str>) -> SimardError {
    SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason: format!("R{code} {}", detail.as_ref()),
    }
}

/// Read and FULLY verify a self-quality-audit record, returning the validated
/// record or a fail-closed [`SimardError::AdapterInvocationFailed`] whose reason
/// names the R-check that tripped. Every failure mode is an `Err`, NEVER a silent
/// default. The fail-CLOSED matrix:
///
/// | # | Condition | Result |
/// |---|---|---|
/// | R1 | file absent / unreadable / no mtime | `Err` |
/// | R2 | present but not valid JSON | `Err` |
/// | R3 | `schema != SELF_QUALITY_AUDIT_SCHEMA` | `Err` |
/// | R4 | unknown/typed field, extra key, waves > 5, or a broken list bound | `Err` |
/// | R5 | `summary_line` empty | `Err` |
/// | R6 | file is not owner-only (`0o600`) | `Err` (unix) |
/// | R7 | `mtime < invoke_start`, `now - mtime > MAX_AGE_SECS`, or epoch skew | `Err` |
/// | R8 | all checks pass | `Ok(record)` |
pub fn read_verified_self_quality_audit(
    path: &Path,
    invoke_start: SystemTime,
) -> SimardResult<SelfQualityAuditRecord> {
    // R1 — absence / unreadable is fail-CLOSED.
    let bytes =
        std::fs::read(path).map_err(|e| fail(1, format!("no record at expected path: {e}")))?;
    let metadata =
        std::fs::metadata(path).map_err(|e| fail(1, format!("no record metadata: {e}")))?;
    let mtime = metadata
        .modified()
        .map_err(|e| fail(1, format!("no record mtime: {e}")))?;

    // R2 — malformed JSON (parsed to a generic Value first).
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| fail(2, format!("malformed JSON: {e}")))?;

    // R3 — schema version pin (checked on the raw Value, before typed decode).
    let schema = value.get("schema").and_then(|v| v.as_str());
    if schema != Some(SELF_QUALITY_AUDIT_SCHEMA) {
        return Err(fail(
            3,
            format!("schema mismatch {schema:?} != {SELF_QUALITY_AUDIT_SCHEMA:?}"),
        ));
    }

    // R4(parse) — unknown top-level key (deny_unknown_fields) or a wrong field
    // type fails the typed decode.
    let record: SelfQualityAuditRecord = serde_json::from_value(value).map_err(|e| {
        fail(
            4,
            format!("typed decode failed (unknown key / wrong type): {e}"),
        )
    })?;

    // R4(bounds) + R5(required) — the SAME chokepoint the writer applied.
    check_bounds(&record).map_err(|(code, detail)| fail(code, detail))?;

    // R6 — owner-only permissions.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(fail(
                6,
                format!(
                    "record is not owner-only (mode {:o}, expected 0o600)",
                    mode & 0o777
                ),
            ));
        }
    }

    // R7 — freshness / anti-replay.
    if mtime + MTIME_SLACK < invoke_start {
        return Err(fail(
            7,
            "freshness/anti-replay (mtime predates invoke_start — stale/replayed record)",
        ));
    }
    let now = SystemTime::now();
    if let Ok(age) = now.duration_since(mtime)
        && age.as_secs() > MAX_AGE_SECS
    {
        return Err(fail(
            7,
            format!(
                "freshness/anti-replay (record age {}s > {MAX_AGE_SECS}s)",
                age.as_secs()
            ),
        ));
    }
    let now_epoch = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now_epoch.abs_diff(record.written_at_epoch) > MAX_AGE_SECS {
        return Err(fail(
            7,
            format!(
                "freshness/anti-replay (written_at_epoch {} skews > {MAX_AGE_SECS}s from now {})",
                record.written_at_epoch, now_epoch
            ),
        ));
    }

    Ok(record)
}
