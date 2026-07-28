//! Typed, file-backed **cognitive-thread reasoning** records and their
//! fail-CLOSED reader (WS-A of issue #4970).
//!
//! This is the cognitive-thread analogue of the OODA orient/decide records in
//! [`super::orient_decide_record`]. It replaces the boolean `"{recipe}: ok"`
//! collapse in `recipe_rail.rs` with the correct agentic-recipes-first pattern:
//! a thread's recipe ACTS by calling the gated `simard cognition
//! record-thread-reasoning` tool, which writes a typed, owner-only (`0o600`),
//! identity-bound [`ThreadReasoningRecord`] carrying a REQUIRED natural-language
//! `reasoning_summary`; the thin Rust rail reads that record **fail-closed**
//! ([`read_verified_thread_reasoning`], R1–R7) and surfaces `reasoning_summary`
//! into `ThreadOutcome.summary` — so the daemon log becomes the thread's actual
//! reasoning, never `"<recipe>: ok"`.
//!
//! One shared free-text chokepoint ([`sanitize_reasoning_summary`]) is invoked
//! by BOTH the CLI writer and the reader, so they can never drift on "what is a
//! valid summary". The record type and the closed [`ThreadName`] / [`ThreadDomain`]
//! enums are likewise a single source of truth.
//!
//! See `docs/reference/simard-cognition-record-thread-reasoning-cli.md` for the
//! full contract.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cognitive_threads::recipe_rail::secret_scrub;

use super::sanitize::sanitize_context_var;

/// The pinned on-disk schema string for a [`ThreadReasoningRecord`]. The reader
/// rejects any other value (R3), so a future `…/v2` writer can never be honored
/// by a `…/v1` reader.
pub const THREAD_REASONING_SCHEMA: &str = "thread-reasoning/v1";

/// Freshness window (seconds) for the reader's R7 anti-replay gate. Threads are
/// not latency-bound; five minutes tolerates recipe-runner spin-up while still
/// rejecting any stale artifact.
pub const MAX_AGE_SECS: u64 = 300;

/// Minimum grapheme (approximated by `char`) floor for a `reasoning_summary`.
const MIN_SUMMARY_CHARS: usize = 8;
/// Hard byte ceiling for a `reasoning_summary` — rejected, never truncated.
const MAX_SUMMARY_BYTES: usize = 600;

/// Per-domain list-element cap and per-element byte cap (defense in depth).
const MAX_TOP_SIGNALS: usize = 5;
const MAX_NOTES: usize = 5;
const MAX_PROBES: usize = 8;
const MAX_CANDIDATES: usize = 16;
const MAX_SIGNATURES: usize = 16;
/// Max bytes retained for a single list element after sanitize.
const MAX_LIST_ELEMENT_BYTES: usize = 256;

/// The closed roster of thirteen cognitive threads. The `snake_case` wire tag is
/// the thread's stable identity: it is embedded in the record and re-verified by
/// the reader (R6), and it names the per-thread record file. Any other value is
/// rejected by both the writer and the reader (R4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadName {
    Salience,
    Metacognition,
    Reflection,
    Prospection,
    OperatorModel,
    Analogy,
    Narrative,
    ValuesDeliberation,
    Consolidation,
    CreativeIdeas,
    EngineerLogAnalysis,
    Interoception,
    Maintenance,
}

impl ThreadName {
    /// The stable `snake_case` label — identical to the serde wire tag and the
    /// per-thread record filename stem.
    pub fn label(self) -> &'static str {
        match self {
            Self::Salience => "salience",
            Self::Metacognition => "metacognition",
            Self::Reflection => "reflection",
            Self::Prospection => "prospection",
            Self::OperatorModel => "operator_model",
            Self::Analogy => "analogy",
            Self::Narrative => "narrative",
            Self::ValuesDeliberation => "values_deliberation",
            Self::Consolidation => "consolidation",
            Self::CreativeIdeas => "creative_ideas",
            Self::EngineerLogAnalysis => "engineer_log_analysis",
            Self::Interoception => "interoception",
            Self::Maintenance => "maintenance",
        }
    }

    /// Resolve a CLI `--thread` value case-insensitively against the closed
    /// roster. `None` (fail closed) for anything not in the thirteen.
    pub fn from_cli_label(raw: &str) -> Option<Self> {
        let key = raw.trim();
        [
            Self::Salience,
            Self::Metacognition,
            Self::Reflection,
            Self::Prospection,
            Self::OperatorModel,
            Self::Analogy,
            Self::Narrative,
            Self::ValuesDeliberation,
            Self::Consolidation,
            Self::CreativeIdeas,
            Self::EngineerLogAnalysis,
            Self::Interoception,
            Self::Maintenance,
        ]
        .into_iter()
        .find(|t| key.eq_ignore_ascii_case(t.label()))
    }

    /// The single [`ThreadDomain`] tag this thread is allowed to carry. A record
    /// whose `--domain` does not match is rejected by the writer, and a record
    /// whose domain tag does not match `thread` fails the reader's R4/R6.
    pub fn expected_domain(self) -> &'static str {
        match self {
            Self::Salience => "salience",
            Self::Interoception => "interoception",
            Self::Maintenance => "maintenance",
            Self::CreativeIdeas => "creative_ideas",
            Self::EngineerLogAnalysis => "engineer_log_analysis",
            // The eight reflective threads without a specialized domain share the
            // generic `notes` bucket.
            Self::Metacognition
            | Self::Reflection
            | Self::Prospection
            | Self::OperatorModel
            | Self::Analogy
            | Self::Narrative
            | Self::ValuesDeliberation
            | Self::Consolidation => "notes",
        }
    }
}

/// The closed, internally-tagged (`"kind"`) set of per-thread structured domain
/// fields. Only [`ThreadReasoningRecord::reasoning_summary`] reaches the daemon
/// log; these fields are for record consumers, tests, and audit. An unknown tag
/// fails deserialization (R4). Shared verbatim by the CLI writer and the reader.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadDomain {
    Salience {
        top_signals: Vec<String>,
        priority: f64,
    },
    Interoception {
        probes: Vec<String>,
        breach: bool,
    },
    Maintenance {
        candidates: Vec<String>,
        freed_bytes: u64,
    },
    CreativeIdeas {
        ideas_considered: u32,
        kept_after_dedup: u32,
    },
    EngineerLogAnalysis {
        signatures: Vec<String>,
        novel: bool,
    },
    Notes {
        notes: Vec<String>,
    },
}

impl ThreadDomain {
    /// The stable `snake_case` tag — identical to the serde `kind` discriminator.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Salience { .. } => "salience",
            Self::Interoception { .. } => "interoception",
            Self::Maintenance { .. } => "maintenance",
            Self::CreativeIdeas { .. } => "creative_ideas",
            Self::EngineerLogAnalysis { .. } => "engineer_log_analysis",
            Self::Notes { .. } => "notes",
        }
    }

    /// Re-validate the closed structural invariants shared by writer and reader:
    /// per-domain list caps, per-element sanitization/byte cap, finite numerics,
    /// and `kept_after_dedup <= ideas_considered`. Returns a normalized domain on
    /// success or `None` (fail closed) on any breach. Invoked identically on
    /// write (reject ⇒ no file) and on read (reject ⇒ R4 `Err`), so the writer
    /// and reader can never drift.
    pub fn normalized(&self) -> Option<Self> {
        let norm = match self {
            Self::Salience {
                top_signals,
                priority,
            } => {
                if !priority.is_finite() {
                    return None;
                }
                Self::Salience {
                    top_signals: bounded_list(top_signals, MAX_TOP_SIGNALS)?,
                    priority: priority.clamp(0.0, 1.0),
                }
            }
            Self::Interoception { probes, breach } => Self::Interoception {
                probes: bounded_list(probes, MAX_PROBES)?,
                breach: *breach,
            },
            Self::Maintenance {
                candidates,
                freed_bytes,
            } => Self::Maintenance {
                candidates: bounded_list(candidates, MAX_CANDIDATES)?,
                freed_bytes: *freed_bytes,
            },
            Self::CreativeIdeas {
                ideas_considered,
                kept_after_dedup,
            } => {
                if kept_after_dedup > ideas_considered {
                    return None;
                }
                Self::CreativeIdeas {
                    ideas_considered: *ideas_considered,
                    kept_after_dedup: *kept_after_dedup,
                }
            }
            Self::EngineerLogAnalysis { signatures, novel } => Self::EngineerLogAnalysis {
                signatures: bounded_list(signatures, MAX_SIGNATURES)?,
                novel: *novel,
            },
            Self::Notes { notes } => Self::Notes {
                notes: bounded_list(notes, MAX_NOTES)?,
            },
        };
        Some(norm)
    }
}

/// Cap a list at `max` elements and sanitize each element (control/ANSI stripped,
/// whitespace folded, secrets scrubbed, byte-capped). An over-cap list fails
/// closed (`None`) rather than being silently truncated; an element that
/// collapses to empty after sanitize is dropped.
fn bounded_list(list: &[String], max: usize) -> Option<Vec<String>> {
    if list.len() > max {
        return None;
    }
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        let folded = sanitize_context_var(item, MAX_LIST_ELEMENT_BYTES);
        let scrubbed = secret_scrub(&folded);
        let trimmed = scrubbed.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.len() > MAX_LIST_ELEMENT_BYTES {
            return None;
        }
        out.push(trimmed.to_string());
    }
    Some(out)
}

/// One typed, on-disk cognitive-thread reasoning record. Written by the
/// `simard cognition record-thread-reasoning` tool and read by
/// [`read_verified_thread_reasoning`]. `deny_unknown_fields` closes off any
/// crafted extra top-level key (the record uses a nested `domain`, not a
/// `flatten`, so the deny attribute applies).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadReasoningRecord {
    /// Schema pin. Must equal [`THREAD_REASONING_SCHEMA`] (R3).
    pub schema: String,
    /// The thread that wrote it. Re-verified against the invoking thread (R6).
    pub thread: ThreadName,
    /// The REQUIRED natural-language domain reasoning (1–3 sentences). The ONLY
    /// field surfaced to `ThreadOutcome.summary` and the daemon log.
    pub reasoning_summary: String,
    /// Unix seconds the recipe stamped at write time. Freshness defense-in-depth
    /// (R7).
    pub written_at_epoch: u64,
    /// The closed, internally-tagged per-thread structured fields.
    pub domain: ThreadDomain,
}

/// The single shared free-text chokepoint for a `reasoning_summary`, invoked
/// IDENTICALLY by the CLI writer (reject ⇒ no file) and the reader (reject ⇒ R5).
///
/// Steps: strip ANSI/C0 control + fold whitespace ([`sanitize_context_var`]),
/// scrub credential-shaped substrings ([`secret_scrub`]), then enforce
/// non-empty, `>= MIN_SUMMARY_CHARS` graphemes, `<= MAX_SUMMARY_BYTES` bytes.
/// A summary made up entirely of control bytes collapses to empty and fails
/// closed; an oversized summary is REJECTED (never silently truncated).
pub fn sanitize_reasoning_summary(raw: &str) -> Option<String> {
    // A large intermediate bound so `sanitize_context_var` folds control /
    // whitespace WITHOUT truncating — we reject oversize below rather than
    // silently shortening a real summary.
    let folded = sanitize_context_var(raw, MAX_SUMMARY_BYTES * 8);
    let scrubbed = secret_scrub(&folded);
    let summary = scrubbed.trim();
    if summary.is_empty() || summary.chars().count() < MIN_SUMMARY_CHARS {
        return None;
    }
    if summary.len() > MAX_SUMMARY_BYTES {
        return None;
    }
    Some(summary.to_string())
}

/// A fail-closed read error, carrying the R-code of the check that tripped so the
/// rail can log the canonical `FAILED — R{n} <reason>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadReasoningReadError {
    /// Which check in the R1–R7 matrix failed.
    pub code: u8,
    /// Human-readable detail (never persisted; diagnostics/log only).
    pub detail: String,
}

impl ThreadReasoningReadError {
    fn new(code: u8, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for ThreadReasoningReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "R{} {}", self.code, self.detail)
    }
}

impl std::error::Error for ThreadReasoningReadError {}

/// Read and FULLY verify a cognitive-thread reasoning record, returning the
/// validated record or a fail-closed [`ThreadReasoningReadError`]. Every failure
/// mode is an `Err`, which the rail maps to a FAILED tick — never a silent
/// success and never scraped stdout. The fail-CLOSED matrix:
///
/// | # | Condition | Result |
/// |---|---|---|
/// | R1 | file absent / unreadable / no mtime | `Err` |
/// | R2 | present but not valid JSON | `Err` |
/// | R3 | `schema != THREAD_REASONING_SCHEMA` | `Err` |
/// | R4 | unknown ThreadName / ThreadDomain tag, extra key, or a broken domain bound | `Err` |
/// | R5 | `reasoning_summary` empty/short/long/control-only after sanitize | `Err` |
/// | R6 | `record.thread != expected_thread` | `Err` |
/// | R7 | `mtime < invoke_start`, `now - mtime > MAX_AGE_SECS`, or epoch skew > MAX_AGE_SECS | `Err` |
/// | R8 | all checks pass | `Ok(record)` |
pub fn read_verified_thread_reasoning(
    path: &Path,
    expected_thread: ThreadName,
    invoke_start: SystemTime,
) -> Result<ThreadReasoningRecord, ThreadReasoningReadError> {
    // R1 — absence / unreadable is fail-CLOSED. The tool writes nothing when it
    // cannot resolve its path or fails validation.
    let bytes = std::fs::read(path).map_err(|e| {
        ThreadReasoningReadError::new(1, format!("no record at expected path: {e}"))
    })?;
    let metadata = std::fs::metadata(path)
        .map_err(|e| ThreadReasoningReadError::new(1, format!("no record metadata: {e}")))?;
    let mtime = metadata
        .modified()
        .map_err(|e| ThreadReasoningReadError::new(1, format!("no record mtime: {e}")))?;

    // R2/R4(parse) — malformed JSON, an unknown ThreadName/ThreadDomain tag, or
    // any unknown top-level key (deny_unknown_fields) fails deserialization.
    let record: ThreadReasoningRecord = serde_json::from_slice(&bytes).map_err(|e| {
        ThreadReasoningReadError::new(
            2,
            format!("malformed JSON / unknown ThreadName or ThreadDomain / extra key: {e}"),
        )
    })?;

    // R3 — schema version pin.
    if record.schema != THREAD_REASONING_SCHEMA {
        return Err(ThreadReasoningReadError::new(
            3,
            format!(
                "schema mismatch {:?} != {THREAD_REASONING_SCHEMA:?}",
                record.schema
            ),
        ));
    }

    // R6 — thread identity (the only stable identity a thread has). A record
    // written by another thread must never be honored.
    if record.thread != expected_thread {
        return Err(ThreadReasoningReadError::new(
            6,
            format!(
                "identity mismatch (record.thread {:?} != invoked thread {:?})",
                record.thread.label(),
                expected_thread.label()
            ),
        ));
    }

    // R4(domain) — the record's domain tag must match its thread, and its lists /
    // numerics must satisfy the shared bounds (re-validated here, not trusted).
    if record.domain.kind_label() != expected_thread.expected_domain() {
        return Err(ThreadReasoningReadError::new(
            4,
            format!(
                "domain {:?} does not match thread {:?} (expected {:?})",
                record.domain.kind_label(),
                expected_thread.label(),
                expected_thread.expected_domain()
            ),
        ));
    }
    let normalized_domain = record.domain.normalized().ok_or_else(|| {
        ThreadReasoningReadError::new(4, "domain fields breach a closed bound".to_string())
    })?;

    // R5 + defense-in-depth — re-validate AND re-sanitize the free text through
    // the SAME chokepoint the tool used on write. A hostile record (control-only
    // or oversized summary) fails closed here, never honored verbatim.
    let clean_summary = sanitize_reasoning_summary(&record.reasoning_summary).ok_or_else(|| {
        ThreadReasoningReadError::new(
            5,
            "reasoning_summary invalid (empty/too-short/too-long after sanitize)".to_string(),
        )
    })?;

    // R7 — freshness / anti-replay. The rail pre-truncates + captures
    // `invoke_start` before spawn, so a leftover file whose mtime predates it is
    // a prior run's artifact. A small slack absorbs coarse filesystem mtime
    // granularity without admitting a genuinely stale (600 s) record.
    const MTIME_SLACK: Duration = Duration::from_secs(2);
    if mtime + MTIME_SLACK < invoke_start {
        return Err(ThreadReasoningReadError::new(
            7,
            "freshness/anti-replay (mtime predates invoke_start — stale/replayed record)"
                .to_string(),
        ));
    }
    let now = SystemTime::now();
    if let Ok(age) = now.duration_since(mtime)
        && age.as_secs() > MAX_AGE_SECS
    {
        return Err(ThreadReasoningReadError::new(
            7,
            format!(
                "freshness/anti-replay (record age {}s > {MAX_AGE_SECS}s)",
                age.as_secs()
            ),
        ));
    }
    // Embedded-epoch defense-in-depth against mtime spoofing.
    let now_epoch = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now_epoch.abs_diff(record.written_at_epoch) > MAX_AGE_SECS {
        return Err(ThreadReasoningReadError::new(
            7,
            format!(
                "freshness/anti-replay (written_at_epoch {} skews > {MAX_AGE_SECS}s from now {})",
                record.written_at_epoch, now_epoch
            ),
        ));
    }

    Ok(ThreadReasoningRecord {
        schema: record.schema,
        thread: record.thread,
        reasoning_summary: clean_summary,
        written_at_epoch: record.written_at_epoch,
        domain: normalized_domain,
    })
}
