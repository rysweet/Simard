//! Operator subcommand `simard cognition salience-signal` — the CLAMPING /
//! VALIDATING write tool for the numeric OODA-Decide salience projection
//! (`state/salience_signal.json`).
//!
//! This is the salience counterpart to `simard memory remember`: the
//! `salience-appraise` recipe performs its numeric side effect by CALLING this
//! tool, not by emitting a JSON envelope that Rust parses. The tool owns all
//! validation so the recipe never has to be trusted with numeric hygiene:
//!
//! - every `valence` is clamped into `[-1, 1]` and every `urgency` into `[0, 1]`
//!   inside [`salience_signal::write_signal`] (defense in depth);
//! - only ids present on the LIVE goal board reach the file — off-board ids are
//!   dropped (S1: no unvalidated id reaches Decide);
//! - the generation epoch is stamped by the tool (`now`), driving the staleness
//!   guard the fail-closed reader enforces (I7);
//! - the write is atomic (temp + rename) so a concurrent reader never tears.
//!
//! Small rankings ride repeatable `--entry goal_id:valence:urgency` flags;
//! large rankings ride a JSON array on stdin (`--stdin`), never argv (E2BIG).
//! A non-numeric or missing score is a hard error — NEVER silently defaulted.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cognitive_threads::salience_signal::{self, SalienceEntry, SalienceSignal};
use crate::error::SimardResult;

pub(super) const COGNITION_HELP: &str = "\
Simard cognition subcommand — cognitive-thread write tools

Usage:
  simard cognition salience-signal [state-root] \\
      [--entry <goal_id>:<valence>:<urgency>]... [--stdin]

salience-signal  Write the numeric OODA-Decide salience ranking to
                 state/salience_signal.json. Each entry's valence is clamped
                 into [-1,1] and urgency into [0,1]; ids not on the live goal
                 board are dropped; the generation epoch is stamped by the tool.
                 Provide the ranking via repeatable --entry flags (small) OR a
                 JSON array of {goal_id,valence,urgency} objects on stdin with
                 --stdin (large). A non-numeric/missing score is a hard error.

There is no JSON to print: the file write IS the effect. With no [state-root]
the tool resolves $SIMARD_STATE_ROOT, then $HOME/.simard.
";

/// One input ranking entry as supplied by the recipe (pre-validation). The
/// clamp + board-validation happen inside [`write_salience_signal`]; this type
/// is a faithful, un-massaged carrier of what the recipe asked for.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SalienceEntryInput {
    /// Candidate goal id — validated against the live board before it is written.
    pub goal_id: String,
    /// Appraised valence (clamped into `[-1, 1]` on write).
    pub valence: f64,
    /// Appraised urgency (clamped into `[0, 1]` on write).
    pub urgency: f64,
}

/// Parse a single `--entry` value of the form `goal_id:valence:urgency`.
///
/// Exactly three colon-separated fields are required. A missing field or a
/// non-numeric score is a hard error — a score is NEVER silently defaulted to 0
/// (that would fabricate an appraisal the recipe never made).
pub fn parse_entry_arg(raw: &str) -> Result<SalienceEntryInput, Box<dyn std::error::Error>> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 3 {
        return Err(format!(
            "malformed --entry `{raw}`: expected exactly `goal_id:valence:urgency`"
        )
        .into());
    }
    let goal_id = parts[0].trim();
    if goal_id.is_empty() {
        return Err(format!("malformed --entry `{raw}`: empty goal_id").into());
    }
    let valence: f64 = parts[1]
        .trim()
        .parse()
        .map_err(|e| format!("malformed --entry `{raw}`: non-numeric valence: {e}"))?;
    let urgency: f64 = parts[2]
        .trim()
        .parse()
        .map_err(|e| format!("malformed --entry `{raw}`: non-numeric urgency: {e}"))?;
    Ok(SalienceEntryInput {
        goal_id: goal_id.to_string(),
        valence,
        urgency,
    })
}

/// Parse a JSON array of `{goal_id, valence, urgency}` objects from a reader
/// (stdin or a file). Large rankings ride this path so they never hit argv.
pub fn parse_entries_json<R: Read>(
    reader: R,
) -> Result<Vec<SalienceEntryInput>, Box<dyn std::error::Error>> {
    let entries: Vec<SalienceEntryInput> = serde_json::from_reader(reader)
        .map_err(|e| format!("reading salience ranking JSON: {e}"))?;
    Ok(entries)
}

/// Write the numeric salience signal, clamping every score and dropping every
/// off-board id inside [`salience_signal::write_signal`]. The caller supplies
/// the generation epoch and the set of valid (live-board) goal ids.
pub fn write_salience_signal(
    state_root: &Path,
    generated_epoch: u64,
    entries: &[SalienceEntryInput],
    valid_goal_ids: &[String],
) -> SimardResult<()> {
    let ranking: Vec<SalienceEntry> = entries
        .iter()
        .map(|e| {
            SalienceEntry {
                goal_id: e.goal_id.clone(),
                valence: e.valence,
                urgency: e.urgency,
            }
            .clamped()
        })
        .collect();
    let signal = SalienceSignal {
        generated_epoch,
        ranking,
    };
    salience_signal::write_signal(state_root, &signal, valid_goal_ids)
}

/// Dispatch `simard cognition <subcommand>`.
pub(crate) fn dispatch_cognition_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = match args.next() {
        Some(s) => s,
        None => {
            print!("{COGNITION_HELP}");
            return Ok(());
        }
    };

    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{COGNITION_HELP}");
            Ok(())
        }
        "salience-signal" => run_salience_signal(args),
        "record-thread-reasoning" => dispatch_record_thread_reasoning(args),
        other => Err(format!("unknown cognition subcommand: {other}").into()),
    }
}

/// `simard cognition salience-signal [state-root] [--entry ...]... [--stdin]`.
fn run_salience_signal(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut state_root: Option<PathBuf> = None;
    let mut entries: Vec<SalienceEntryInput> = Vec::new();
    let mut from_stdin = false;

    let mut it = args.peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{COGNITION_HELP}");
                return Ok(());
            }
            "--entry" => {
                let raw = it
                    .next()
                    .ok_or("--entry requires a `goal_id:valence:urgency` value")?;
                entries.push(parse_entry_arg(&raw)?);
            }
            "--stdin" => {
                from_stdin = true;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag: {other}").into());
            }
            _ if state_root.is_none() => {
                state_root = Some(PathBuf::from(arg));
            }
            _ => {
                return Err(format!("unexpected trailing argument: {arg}").into());
            }
        }
    }

    if from_stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        entries.extend(parse_entries_json(std::io::Cursor::new(buf))?);
    }

    let state_root = state_root.unwrap_or_else(crate::state_root::simard_state_root);

    // Validate ids against the LIVE goal board — off-board ids are dropped by
    // `write_signal` (S1). An absent/empty board yields an empty valid set, so
    // the signal is written with an empty ranking (present, fail-closed).
    let board = crate::goal_board_store::load(&state_root).board;
    let valid_goal_ids: Vec<String> = board.active.iter().map(|g| g.id.clone()).collect();

    let generated_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    write_salience_signal(&state_root, generated_epoch, &entries, &valid_goal_ids)?;
    Ok(())
}

// ===========================================================================
// `simard cognition record-thread-reasoning` — the gated ACT-step writer verb
// every cognitive-thread recipe calls exactly once (issue #4970, WS-A.2).
// ===========================================================================

use crate::ooda_brain::{
    THREAD_REASONING_SCHEMA, ThreadDomain, ThreadName, ThreadReasoningRecord,
    sanitize_reasoning_summary,
};

/// Every flag the writer accepts. An unknown flag is rejected (nothing is ever
/// silently ignored); a repeatable flag may appear more than once.
const KNOWN_FLAGS: &[&str] = &[
    "thread",
    "domain",
    "reasoning-summary",
    "reasoning-summary-path",
    "record-path",
    "written-at-epoch",
    // salience
    "top-signal",
    "priority",
    // interoception
    "probe",
    "breach",
    // maintenance
    "candidate",
    "freed-bytes",
    // creative_ideas
    "ideas-considered",
    "kept-after-dedup",
    // engineer_log_analysis
    "signature",
    "novel",
    // notes (shared reflective bucket)
    "note",
];

/// The flags that may legitimately repeat (list fields). Everything else is a
/// scalar and a second occurrence is a hard error.
const REPEATABLE_FLAGS: &[&str] = &["top-signal", "probe", "candidate", "signature", "note"];

/// A parsed `--flag value` bag: scalars keep the last-wins value (a duplicate
/// scalar is rejected), repeatables accumulate.
struct ParsedArgs {
    scalars: std::collections::BTreeMap<String, String>,
    lists: std::collections::BTreeMap<String, Vec<String>>,
}

impl ParsedArgs {
    fn scalar(&self, key: &str) -> Option<&str> {
        self.scalars.get(key).map(String::as_str)
    }

    fn list(&self, key: &str) -> Vec<String> {
        self.lists.get(key).cloned().unwrap_or_default()
    }
}

/// Parse the writer's `--flag value` argv, rejecting unknown flags and duplicate
/// scalars. Repeatable flags accumulate in order.
fn parse_record_args(
    args: impl Iterator<Item = String>,
) -> Result<ParsedArgs, Box<dyn std::error::Error>> {
    let values: Vec<String> = args.collect();
    let mut scalars: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut lists: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let mut index = 0;
    while index < values.len() {
        let flag = values[index]
            .strip_prefix("--")
            .ok_or_else(|| format!("expected named option, got {:?}", values[index]))?;
        if !KNOWN_FLAGS.contains(&flag) {
            return Err(format!("unknown option --{flag}").into());
        }
        let value = values
            .get(index + 1)
            .ok_or_else(|| format!("--{flag} requires a value"))?
            .clone();
        if REPEATABLE_FLAGS.contains(&flag) {
            lists.entry(flag.to_string()).or_default().push(value);
        } else if scalars.insert(flag.to_string(), value).is_some() {
            return Err(format!("duplicate option --{flag}").into());
        }
        index += 2;
    }
    Ok(ParsedArgs { scalars, lists })
}

/// Harden a `--record-path`: absolute and free of any `..` component. Mirrors
/// the OODA record tools' `harden_path`.
fn harden_record_path(path: &Path, flag: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !path.is_absolute() {
        return Err(format!("--{flag} must be absolute, got {}", path.display()).into());
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("--{flag} must not contain '..', got {}", path.display()).into());
    }
    Ok(())
}

/// Resolve the `reasoning_summary` from EXACTLY ONE of `--reasoning-summary`
/// (inline) or `--reasoning-summary-path` (an absolute, `..`-free file read under
/// a 64 KiB cap). Neither ⇒ error; both ⇒ error.
fn resolve_summary_source(parsed: &ParsedArgs) -> Result<String, Box<dyn std::error::Error>> {
    const MAX_SUMMARY_FILE_BYTES: u64 = 64 * 1024;
    match (
        parsed.scalar("reasoning-summary"),
        parsed.scalar("reasoning-summary-path"),
    ) {
        (Some(_), Some(_)) => {
            Err("--reasoning-summary and --reasoning-summary-path are mutually exclusive".into())
        }
        (Some(inline), None) => Ok(inline.to_string()),
        (None, Some(file)) => {
            let file_path = Path::new(file);
            harden_record_path(file_path, "reasoning-summary-path")?;
            use std::io::Read;
            let mut reader = std::fs::File::open(file_path)?.take(MAX_SUMMARY_FILE_BYTES + 1);
            let mut buf = String::new();
            reader.read_to_string(&mut buf)?;
            if buf.len() as u64 > MAX_SUMMARY_FILE_BYTES {
                return Err("--reasoning-summary-path file exceeds the 64 KiB cap".into());
            }
            Ok(buf)
        }
        (None, None) => Err(
            "a reasoning record requires --reasoning-summary or --reasoning-summary-path".into(),
        ),
    }
}

/// Parse an optional boolean flag (`true`/`false`, case-insensitive). Absent ⇒
/// `default`; malformed ⇒ error (never silently coerced).
fn parse_bool_flag(
    parsed: &ParsedArgs,
    flag: &str,
    default: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    match parsed.scalar(flag) {
        None => Ok(default),
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!("--{flag} expects true|false, got {other:?}").into()),
        },
    }
}

/// Build the closed [`ThreadDomain`] selected by `--domain`, pulling each
/// per-domain field from `parsed`. An unknown `--domain` tag is rejected.
fn build_domain(
    parsed: &ParsedArgs,
    domain_tag: &str,
) -> Result<ThreadDomain, Box<dyn std::error::Error>> {
    let domain = match domain_tag {
        "salience" => ThreadDomain::Salience {
            top_signals: parsed.list("top-signal"),
            priority: match parsed.scalar("priority") {
                None => 0.0,
                Some(v) => v
                    .trim()
                    .parse::<f64>()
                    .map_err(|e| format!("invalid --priority (expected an f64): {e}"))?,
            },
        },
        "interoception" => ThreadDomain::Interoception {
            probes: parsed.list("probe"),
            breach: parse_bool_flag(parsed, "breach", false)?,
        },
        "maintenance" => ThreadDomain::Maintenance {
            candidates: parsed.list("candidate"),
            freed_bytes: match parsed.scalar("freed-bytes") {
                None => 0,
                Some(v) => v
                    .trim()
                    .parse::<u64>()
                    .map_err(|e| format!("invalid --freed-bytes (expected a u64): {e}"))?,
            },
        },
        "creative_ideas" => ThreadDomain::CreativeIdeas {
            ideas_considered: parse_u32_flag(parsed, "ideas-considered")?,
            kept_after_dedup: parse_u32_flag(parsed, "kept-after-dedup")?,
        },
        "engineer_log_analysis" => ThreadDomain::EngineerLogAnalysis {
            signatures: parsed.list("signature"),
            novel: parse_bool_flag(parsed, "novel", false)?,
        },
        "notes" => ThreadDomain::Notes {
            notes: parsed.list("note"),
        },
        other => return Err(format!("unknown --domain {other:?}").into()),
    };
    Ok(domain)
}

/// Parse an optional `u32` flag, defaulting to `0` when absent.
fn parse_u32_flag(parsed: &ParsedArgs, flag: &str) -> Result<u32, Box<dyn std::error::Error>> {
    match parsed.scalar(flag) {
        None => Ok(0),
        Some(v) => v
            .trim()
            .parse::<u32>()
            .map_err(|e| format!("invalid --{flag} (expected a u32): {e}").into()),
    }
}

/// `simard cognition record-thread-reasoning` — the zero-privilege ACT-step tool
/// every cognitive-thread recipe calls EXACTLY ONCE to record its per-invocation
/// reasoning.
///
/// It validates `--thread` (closed 13-variant enum, case-insensitive), `--domain`
/// (must match the thread's expected domain), the per-domain fields (bounded +
/// clamped), and `--reasoning-summary` through the shared
/// [`sanitize_reasoning_summary`](crate::ooda_brain::sanitize_reasoning_summary)
/// chokepoint, hardens `--record-path` (absolute, no `..`), then writes EXACTLY
/// ONE atomic `0o600` [`ThreadReasoningRecord`](crate::ooda_brain::ThreadReasoningRecord).
/// Any validation failure ⇒ a non-zero exit AND **no file on disk**
/// (validate-all-then-write-once).
///
/// See `docs/reference/simard-cognition-record-thread-reasoning-cli.md`.
pub(crate) fn dispatch_record_thread_reasoning(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = parse_record_args(args)?;

    // --thread — closed roster, case-insensitive.
    let thread_raw = parsed
        .scalar("thread")
        .ok_or("missing required option --thread")?;
    let thread = ThreadName::from_cli_label(thread_raw)
        .ok_or_else(|| format!("unknown --thread {thread_raw:?} (not one of the 13 threads)"))?;

    // --domain — must match the thread's single expected domain.
    let domain_tag = parsed
        .scalar("domain")
        .ok_or("missing required option --domain")?;
    if domain_tag != thread.expected_domain() {
        return Err(format!(
            "--domain {domain_tag:?} does not match --thread {:?} (expected {:?})",
            thread.label(),
            thread.expected_domain()
        )
        .into());
    }
    let domain = build_domain(&parsed, domain_tag)?;
    // Re-validate the closed structural bounds (list caps, kept<=considered,
    // finite/clamped numerics) — the SAME chokepoint the reader applies, so the
    // writer never produces a record the reader would reject.
    let domain = domain
        .normalized()
        .ok_or("domain fields breach a closed bound (list over cap or kept>considered)")?;

    // --written-at-epoch — required freshness stamp.
    let written_at_epoch: u64 = parsed
        .scalar("written-at-epoch")
        .ok_or("missing required option --written-at-epoch")?
        .trim()
        .parse()
        .map_err(|_| "invalid --written-at-epoch (expected unix seconds, u64)")?;

    // --record-path — absolute, no `..`.
    let record_path_raw = parsed
        .scalar("record-path")
        .ok_or("missing required option --record-path")?;
    let record_path = Path::new(record_path_raw);
    harden_record_path(record_path, "record-path")?;

    // reasoning_summary — exactly one source, through the shared chokepoint.
    let raw_summary = resolve_summary_source(&parsed)?;
    let reasoning_summary = sanitize_reasoning_summary(&raw_summary)
        .ok_or("--reasoning-summary is empty/too-short/too-long/control-only after sanitize")?;

    // ALL validation passed — write exactly one atomic 0o600 record.
    let record = ThreadReasoningRecord {
        schema: THREAD_REASONING_SCHEMA.to_string(),
        thread,
        reasoning_summary,
        written_at_epoch,
        domain,
    };
    crate::persistence::persist_json("cognition-thread-reasoning", record_path, &record)?;
    Ok(())
}
