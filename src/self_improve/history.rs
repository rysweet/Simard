//! Persistent history of improvement cycles (JSON-lines format).
//!
//! Each completed cycle is appended as a single JSON line to a history file.
//! This enables cross-cycle analysis: deduplicating previously-failed proposals,
//! tracking score trends, and surfacing chronically-weak dimensions.
//!
//! ## Schema versioning (sidecar envelope)
//!
//! A companion `improvement_history.meta.json` file carries
//! `{"schema_version": N}` alongside the append-only JSONL log.  This
//! preserves byte-for-byte JSONL compatibility and O(1) appends while giving
//! the recovery ladder a typed `SchemaTooNew` signal (see issue #2126).
//!
//! * **No sidecar on disk** → legacy v0 data; the sidecar is auto-created.
//! * **Sidecar version ≤ `CURRENT_SCHEMA_VERSION`** → normal operation.
//! * **Sidecar version > `CURRENT_SCHEMA_VERSION`** → `SchemaTooNew` error.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

use super::types::{ImprovementCycle, ImprovementDecision, ProposedChange};

const HISTORY_FILENAME: &str = "improvement_history.jsonl";
const META_FILENAME: &str = "improvement_history.meta.json";

/// The schema version this binary writes and can read.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// On-disk content of the sidecar metadata file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMeta {
    pub schema_version: u32,
}

/// Handle to the on-disk improvement history.
#[derive(Debug, Clone)]
pub struct ImprovementHistory {
    path: PathBuf,
    meta_path: PathBuf,
}

fn io_err(path: &Path, action: &str, reason: impl std::fmt::Display) -> SimardError {
    SimardError::PersistentStoreIo {
        store: "improvement_history".into(),
        action: action.into(),
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

fn schema_too_new(path: &Path, found: u32) -> SimardError {
    SimardError::SchemaTooNew {
        store: "improvement_history".into(),
        found_version: found,
        max_supported: CURRENT_SCHEMA_VERSION,
        path: path.to_path_buf(),
    }
}

impl ImprovementHistory {
    /// Open (or create) a history file in the given directory.
    pub fn open(dir: &Path) -> SimardResult<Self> {
        fs::create_dir_all(dir).map_err(|e| io_err(dir, "create_dir", e))?;
        let path = dir.join(HISTORY_FILENAME);
        let meta_path = dir.join(META_FILENAME);
        Ok(Self { path, meta_path })
    }

    /// Open a history file at an exact path (useful for tests).
    pub fn open_file(path: PathBuf) -> Self {
        let meta_path = sidecar_path_for(&path);
        Self { path, meta_path }
    }

    /// Append a completed cycle to the history file.
    ///
    /// Also creates or updates the sidecar metadata file.
    pub fn append(&self, cycle: &ImprovementCycle) -> SimardResult<()> {
        let json = serde_json::to_string(cycle).map_err(|e| io_err(&self.path, "serialize", e))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| io_err(&self.path, "open_append", e))?;
        writeln!(file, "{json}").map_err(|e| io_err(&self.path, "write", e))?;
        self.write_meta()?;
        Ok(())
    }

    /// Load all cycles from the history file.
    ///
    /// Corrupt lines are silently skipped — partial writes from crashes
    /// should not block future cycles.
    ///
    /// Returns `SchemaTooNew` if the sidecar indicates a version this
    /// binary cannot read. If no sidecar exists the file is treated as
    /// legacy v0 and the sidecar is auto-created.
    pub fn load(&self) -> SimardResult<Vec<ImprovementCycle>> {
        self.check_or_create_meta()?;
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&self.path).map_err(|e| io_err(&self.path, "open_read", e))?;
        let reader = BufReader::new(file);
        let mut cycles = Vec::new();
        for line_result in reader.lines() {
            let line: String = match line_result {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(cycle) = serde_json::from_str::<ImprovementCycle>(&line) {
                cycles.push(cycle);
            }
            // silently skip corrupt lines
        }
        Ok(cycles)
    }

    /// Check whether a proposal (identified by file_path + description) was
    /// previously attempted and reverted.
    pub fn was_previously_reverted(&self, proposal: &ProposedChange) -> SimardResult<bool> {
        let cycles = self.load()?;
        Ok(cycles.iter().any(|c| {
            matches!(&c.decision, Some(ImprovementDecision::Revert { .. }))
                && c.proposed_changes.iter().any(|p| {
                    p.file_path == proposal.file_path && p.description == proposal.description
                })
        }))
    }

    /// Filter out proposals that were previously reverted.
    pub fn dedup_proposals(
        &self,
        proposals: &[ProposedChange],
    ) -> SimardResult<Vec<ProposedChange>> {
        let cycles = self.load()?;
        let reverted_keys: Vec<(&str, &str)> = cycles
            .iter()
            .filter(|c| matches!(&c.decision, Some(ImprovementDecision::Revert { .. })))
            .flat_map(|c| {
                c.proposed_changes
                    .iter()
                    .map(|p| (p.file_path.as_str(), p.description.as_str()))
            })
            .collect();

        Ok(proposals
            .iter()
            .filter(|p| {
                !reverted_keys
                    .iter()
                    .any(|(fp, desc)| *fp == p.file_path && *desc == p.description)
            })
            .cloned()
            .collect())
    }

    /// Return the number of cycles in the history.
    ///
    /// Counts non-blank lines without deserializing, which is cheaper than
    /// `load().len()` for large history files.
    pub fn cycle_count(&self) -> SimardResult<usize> {
        if !self.path.exists() {
            return Ok(0);
        }
        let file = fs::File::open(&self.path).map_err(|e| io_err(&self.path, "open_read", e))?;
        let reader = BufReader::new(file);
        let count = reader
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .count();
        Ok(count)
    }

    /// Truncate the history to the most recent `max_cycles` entries.
    ///
    /// If the history already has fewer entries, this is a no-op.
    /// Corrupt lines are dropped during the rewrite.
    pub fn prune(&self, max_cycles: usize) -> SimardResult<()> {
        let cycles = self.load()?;
        if cycles.len() <= max_cycles {
            return Ok(());
        }
        let keep = &cycles[cycles.len() - max_cycles..];
        let mut file =
            fs::File::create(&self.path).map_err(|e| io_err(&self.path, "prune_rewrite", e))?;
        for cycle in keep {
            let json =
                serde_json::to_string(cycle).map_err(|e| io_err(&self.path, "serialize", e))?;
            writeln!(file, "{json}").map_err(|e| io_err(&self.path, "write", e))?;
        }
        Ok(())
    }

    /// Count how many times a proposal was previously reverted.
    ///
    /// This enables callers to implement exponential backoff on proposals
    /// that keep failing.
    pub fn reverted_count(&self, proposal: &ProposedChange) -> SimardResult<usize> {
        let cycles = self.load()?;
        let count = cycles
            .iter()
            .filter(|c| {
                matches!(&c.decision, Some(ImprovementDecision::Revert { .. }))
                    && c.proposed_changes.iter().any(|p| {
                        p.file_path == proposal.file_path && p.description == proposal.description
                    })
            })
            .count();
        Ok(count)
    }

    /// Return the most recent `n` cycles from the history.
    ///
    /// If fewer than `n` cycles exist, returns all of them.
    pub fn last_n_cycles(&self, n: usize) -> SimardResult<Vec<ImprovementCycle>> {
        let cycles = self.load()?;
        let start = cycles.len().saturating_sub(n);
        Ok(cycles[start..].to_vec())
    }

    /// Return the path to the history file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the path to the sidecar metadata file.
    pub fn meta_path(&self) -> &Path {
        &self.meta_path
    }

    // -- private sidecar helpers ------------------------------------------

    /// Write (or overwrite) the sidecar metadata file with the current schema version.
    fn write_meta(&self) -> SimardResult<()> {
        let meta = HistoryMeta {
            schema_version: CURRENT_SCHEMA_VERSION,
        };
        let json = serde_json::to_string_pretty(&meta)
            .map_err(|e| io_err(&self.meta_path, "serialize_meta", e))?;
        fs::write(&self.meta_path, json).map_err(|e| io_err(&self.meta_path, "write_meta", e))?;
        Ok(())
    }

    /// Check the sidecar. If absent, auto-create (legacy upgrade).
    /// If present and version > supported, return `SchemaTooNew`.
    fn check_or_create_meta(&self) -> SimardResult<()> {
        if !self.meta_path.exists() {
            // Legacy v0: no sidecar. Auto-create it.
            self.write_meta()?;
            return Ok(());
        }
        let raw = fs::read_to_string(&self.meta_path)
            .map_err(|e| io_err(&self.meta_path, "read_meta", e))?;
        let meta: HistoryMeta =
            serde_json::from_str(&raw).map_err(|e| io_err(&self.meta_path, "parse_meta", e))?;
        if meta.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(schema_too_new(&self.meta_path, meta.schema_version));
        }
        Ok(())
    }
}

/// Derive the sidecar path from an arbitrary JSONL path.
fn sidecar_path_for(jsonl_path: &Path) -> PathBuf {
    let stem = jsonl_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("improvement_history");
    jsonl_path.with_file_name(format!("{stem}.meta.json"))
}
