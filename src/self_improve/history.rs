//! Persistent history of improvement cycles (JSON-lines format).
//!
//! Each completed cycle is appended as a single JSON line to a history file.
//! This enables cross-cycle analysis: deduplicating previously-failed proposals,
//! tracking score trends, and surfacing chronically-weak dimensions.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

use super::types::{ImprovementCycle, ImprovementDecision, ProposedChange};

const HISTORY_FILENAME: &str = "improvement_history.jsonl";

/// Handle to the on-disk improvement history.
#[derive(Debug, Clone)]
pub struct ImprovementHistory {
    path: PathBuf,
}

fn io_err(path: &Path, action: &str, reason: impl std::fmt::Display) -> SimardError {
    SimardError::PersistentStoreIo {
        store: "improvement_history".into(),
        action: action.into(),
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

impl ImprovementHistory {
    /// Open (or create) a history file in the given directory.
    pub fn open(dir: &Path) -> SimardResult<Self> {
        fs::create_dir_all(dir).map_err(|e| io_err(dir, "create_dir", e))?;
        Ok(Self {
            path: dir.join(HISTORY_FILENAME),
        })
    }

    /// Open a history file at an exact path (useful for tests).
    pub fn open_file(path: PathBuf) -> Self {
        Self { path }
    }

    /// Append a completed cycle to the history file.
    pub fn append(&self, cycle: &ImprovementCycle) -> SimardResult<()> {
        let json = serde_json::to_string(cycle).map_err(|e| io_err(&self.path, "serialize", e))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| io_err(&self.path, "open_append", e))?;
        writeln!(file, "{json}").map_err(|e| io_err(&self.path, "write", e))?;
        Ok(())
    }

    /// Load all cycles from the history file.
    ///
    /// Corrupt lines are silently skipped — partial writes from crashes
    /// should not block future cycles.
    pub fn load(&self) -> SimardResult<Vec<ImprovementCycle>> {
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
}
