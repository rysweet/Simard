//! Typed, file-backed **brain-introspection** record and its fail-CLOSED reader
//! (issue #4968).
//!
//! This retires the last-but-one brittle-parse antipattern survivor: the
//! `brain-introspection` adapter used to scrape concatenated recipe step-output
//! text for `BRAIN_HEALTH:`/`PRUNE_REQUESTED=`/`ISSUE_URL=` markers. It now
//! follows the same typed-record contract Groups A–C use: the recipe ACTS by
//! calling the gated `simard cognition record-brain-introspection` tool, which
//! writes a typed, owner-only (`0o600`), freshness-checked
//! [`BrainIntrospectionRecord`]; the thin Rust rail reads it **fail-closed**
//! ([`read_verified_brain_introspection`], R1–R7) and NEVER a silent default.
//!
//! One shared bounds chokepoint ([`check_bounds`]) is invoked by BOTH the CLI
//! writer and the reader, so they can never drift on "what is a valid record".

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

/// Stable adapter tag used in the fail-closed error envelopes.
const ADAPTER_TAG: &str = "brain-introspection";

/// The pinned on-disk schema string. The reader rejects any other value (R3),
/// so a future `…/v2` writer can never be honored by a `…/v1` reader.
pub const BRAIN_INTROSPECTION_SCHEMA: &str = "brain-introspection/v1";

/// Freshness window (seconds) for the reader's R7 anti-replay gate. The record
/// is written and read within a single recipe run, so five minutes tolerates
/// recipe-runner spin-up while still rejecting any stale artifact.
pub const MAX_AGE_SECS: u64 = 300;

/// Per-list element cap (defense in depth) — an over-cap list fails closed (R4),
/// never silently truncated.
const MAX_LIST_LEN: usize = 32;
/// Hard byte ceiling for a single list element or the issue URL (R4).
const MAX_ELEMENT_BYTES: usize = 256;
/// Coarse-mtime slack absorbing filesystem granularity without admitting a
/// genuinely stale record.
const MTIME_SLACK: Duration = Duration::from_secs(2);

/// One typed, on-disk brain-introspection record. Written by the
/// `simard cognition record-brain-introspection` tool and read by
/// [`read_verified_brain_introspection`]. `deny_unknown_fields` closes off any
/// crafted extra top-level key (R4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrainIntrospectionRecord {
    /// Schema pin. Must equal [`BRAIN_INTROSPECTION_SCHEMA`] (R3).
    pub schema: String,
    /// Unix seconds the recipe stamped at write time. Freshness defense (R7).
    pub written_at_epoch: u64,
    /// Brain-health findings (≥1 REQUIRED — an empty list is R5).
    pub brain_health: Vec<String>,
    /// Recurring patterns mined from recent episodes / cycle reports.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Regressions detected against the rolling baseline.
    #[serde(default)]
    pub regressions: Vec<String>,
    /// Human-readable value-bearing prune candidates (recommendation-only).
    #[serde(default)]
    pub prune_candidates: Vec<String>,
    /// Value-bearing prune candidates recommended (clamped by the caller).
    #[serde(default)]
    pub prune_requested: usize,
    /// URL of the created/updated brain-introspection issue, if emitted.
    #[serde(default)]
    pub issue_url: Option<String>,
}

/// Re-validate the closed structural invariants shared by writer and reader:
/// per-list element/byte caps (R4) and the required non-empty `brain_health`
/// (R5). Returns `(r_code, detail)` on the first breach so the reader can render
/// the canonical `R{n} <reason>` line and the writer can fail closed BEFORE it
/// writes any file. Invoked IDENTICALLY on write and read, so they can never
/// drift.
pub(crate) fn check_bounds(rec: &BrainIntrospectionRecord) -> Result<(), (u8, String)> {
    for (name, list) in [
        ("brain_health", &rec.brain_health),
        ("patterns", &rec.patterns),
        ("regressions", &rec.regressions),
        ("prune_candidates", &rec.prune_candidates),
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
    if let Some(url) = &rec.issue_url
        && url.len() > MAX_ELEMENT_BYTES
    {
        return Err((
            4,
            format!(
                "issue_url over {MAX_ELEMENT_BYTES}-byte cap ({} bytes)",
                url.len()
            ),
        ));
    }
    if rec.brain_health.iter().all(|s| s.trim().is_empty()) {
        return Err((
            5,
            "brain_health is empty (at least one non-empty finding is REQUIRED)".to_string(),
        ));
    }
    Ok(())
}

/// A fail-closed `AdapterInvocationFailed` whose reason carries the R-code of the
/// check that tripped (so a caller can grep `R{n}`), tagged to this adapter.
fn fail(code: u8, detail: impl AsRef<str>) -> SimardError {
    SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason: format!("R{code} {}", detail.as_ref()),
    }
}

/// Read and FULLY verify a brain-introspection record, returning the validated
/// record or a fail-closed [`SimardError::AdapterInvocationFailed`] whose reason
/// names the R-check that tripped. Every failure mode is an `Err`, NEVER a silent
/// default. The fail-CLOSED matrix:
///
/// | # | Condition | Result |
/// |---|---|---|
/// | R1 | file absent / unreadable / no mtime | `Err` |
/// | R2 | present but not valid JSON | `Err` |
/// | R3 | `schema != BRAIN_INTROSPECTION_SCHEMA` | `Err` |
/// | R4 | unknown/typed field, extra key, or a broken list bound | `Err` |
/// | R5 | `brain_health` empty | `Err` |
/// | R6 | file is not owner-only (`0o600`) | `Err` (unix) |
/// | R7 | `mtime < invoke_start`, `now - mtime > MAX_AGE_SECS`, or epoch skew | `Err` |
/// | R8 | all checks pass | `Ok(record)` |
pub fn read_verified_brain_introspection(
    path: &Path,
    invoke_start: SystemTime,
) -> SimardResult<BrainIntrospectionRecord> {
    // R1 — absence / unreadable is fail-CLOSED.
    let bytes =
        std::fs::read(path).map_err(|e| fail(1, format!("no record at expected path: {e}")))?;
    let metadata =
        std::fs::metadata(path).map_err(|e| fail(1, format!("no record metadata: {e}")))?;
    let mtime = metadata
        .modified()
        .map_err(|e| fail(1, format!("no record mtime: {e}")))?;

    // R2 — malformed JSON (parsed to a generic Value first, so an unknown key is
    // distinguished from a truly malformed document).
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| fail(2, format!("malformed JSON: {e}")))?;

    // R3 — schema version pin (checked on the raw Value, before typed decode).
    let schema = value.get("schema").and_then(|v| v.as_str());
    if schema != Some(BRAIN_INTROSPECTION_SCHEMA) {
        return Err(fail(
            3,
            format!("schema mismatch {schema:?} != {BRAIN_INTROSPECTION_SCHEMA:?}"),
        ));
    }

    // R4(parse) — unknown top-level key (deny_unknown_fields) or a wrong field
    // type fails the typed decode.
    let record: BrainIntrospectionRecord = serde_json::from_value(value).map_err(|e| {
        fail(
            4,
            format!("typed decode failed (unknown key / wrong type): {e}"),
        )
    })?;

    // R4(bounds) + R5(required) — the SAME chokepoint the writer applied.
    check_bounds(&record).map_err(|(code, detail)| fail(code, detail))?;

    // R6 — owner-only permissions (the trusted 0o600 writer never emits a
    // group/other-readable record).
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

    // R7 — freshness / anti-replay. The rail pre-truncates + captures
    // `invoke_start` before spawn, so a leftover file whose mtime predates it is
    // a prior run's artifact.
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
