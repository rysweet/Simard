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
