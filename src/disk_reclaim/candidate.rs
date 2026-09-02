//! The proposal contract between the analysis agent and the Rust executor.
//!
//! The agent (via `disk-reclaim.yaml`) *proposes* a list of [`ReclaimCandidate`]s
//! as text markers. This module parses those markers. **Every field the agent
//! supplies is advisory** — `parent_repo`, `reason`, and `est_bytes` are
//! re-derived or re-measured by the executor/guard and are never trusted for a
//! safety decision. `deny_unknown_fields` rejects any field the agent invents.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A single path the agent nominates for reclamation.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReclaimCandidate {
    /// Absolute path the agent nominates for reclamation.
    pub path: PathBuf,
    /// Which reclamation primitive the agent believes applies.
    pub kind: CandidateKind,
    /// Repo the path belongs to (informational; re-derived by the guard).
    #[serde(default)]
    pub parent_repo: Option<PathBuf>,
    /// Agent's free-text rationale (sanitized before any logging).
    #[serde(default)]
    pub reason: Option<String>,
    /// Agent's size estimate in bytes (re-measured by the executor).
    #[serde(default)]
    pub est_bytes: Option<u64>,
}

/// The reclamation primitive class the agent proposes for a candidate. This is
/// **advisory only**: the guard re-derives the real primitive at vet time and
/// the agent's `kind` may only ever *deepen* vetting, never shorten it. A path
/// that is actually a git worktree (a `.git` entry at its root) is always run
/// through the uncommitted/unpushed + merged/closed-PR vetoes even if labelled
/// `orphan_dir`/`stale_build_cache`, so a mislabelled `kind` cannot cause a
/// dirty worktree to be `rm -rf`ed.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// A git-tracked worktree → `git worktree remove --force` (after prune).
    TrackedWorktree,
    /// An orphaned, de-registered (untracked) leftover dir → `rm -rf`.
    OrphanDir,
    /// A stale `target/` or shared cargo cache → `rm -rf`.
    StaleBuildCache,
}

/// Upper bound on how many candidates a single recipe run may propose. A larger
/// array is treated as malformed input and hard-errors (fail-closed).
pub const MAX_CANDIDATES: usize = 4096;

/// Parse the recipe agent's text markers out of the (possibly noisy) step
/// output.
///
/// | Marker | Required | Meaning |
/// | ------ | -------- | ------- |
/// | `DISK_USED_PCT=<0..=100>` | yes | current `%-used` the agent measured |
/// | `CANDIDATES_JSON=<json array>` | yes | the proposed `[ReclaimCandidate]` list |
/// | `CANDIDATES_SCHEMA=<version>` | optional | schema version for forward-compat |
///
/// Fail-closed parsing rules:
/// - A malformed **array** (not valid JSON, not an array) → **hard error**.
/// - A malformed **element** inside a valid array → that element is **skipped**
///   and reported via `tracing`; parsing continues with the valid elements.
/// - `DISK_USED_PCT` must be `0..=100`; the candidate count is bounded by
///   [`MAX_CANDIDATES`].
/// - Unknown / noise lines are ignored (the agent may emit `df` output, prose).
pub fn parse_candidates(step_output: &str) -> Result<(Vec<ReclaimCandidate>, u8), String> {
    let mut used_pct: Option<u8> = None;
    let mut json_line: Option<String> = None;

    for line in step_output.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("DISK_USED_PCT=") {
            let n = val
                .trim()
                .parse::<u8>()
                .map_err(|e| format!("invalid DISK_USED_PCT value '{val}': {e}"))?;
            if n > 100 {
                return Err(format!("DISK_USED_PCT out of range (0..=100): {n}"));
            }
            used_pct = Some(n);
        } else if let Some(val) = trimmed.strip_prefix("CANDIDATES_JSON=") {
            json_line = Some(val.trim().to_string());
        }
        // CANDIDATES_SCHEMA and unknown lines are ignored (forward-compat).
    }

    let used_pct = used_pct.ok_or_else(|| "missing DISK_USED_PCT marker".to_string())?;
    let json = json_line.ok_or_else(|| "missing CANDIDATES_JSON marker".to_string())?;

    // A malformed array is a hard error: garbage in, nothing deleted.
    let value: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| format!("CANDIDATES_JSON is not valid JSON: {e}"))?;
    let arr = value
        .as_array()
        .ok_or_else(|| "CANDIDATES_JSON is not a JSON array".to_string())?;
    if arr.len() > MAX_CANDIDATES {
        return Err(format!(
            "too many candidates: {} > {MAX_CANDIDATES}",
            arr.len()
        ));
    }

    // A malformed *element* is skipped, not fatal.
    let mut out = Vec::with_capacity(arr.len());
    for (index, element) in arr.iter().enumerate() {
        match serde_json::from_value::<ReclaimCandidate>(element.clone()) {
            Ok(candidate) => out.push(candidate),
            Err(e) => tracing::warn!(
                target: "simard::disk_reclaim",
                index,
                error = %e,
                "skipping malformed reclaim candidate",
            ),
        }
    }

    Ok((out, used_pct))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_used_pct_and_candidates() {
        let output = "\
noise line the agent emitted\n\
DISK_USED_PCT=88\n\
CANDIDATES_SCHEMA=1\n\
CANDIDATES_JSON=[{\"path\":\"/a\",\"kind\":\"tracked_worktree\"},{\"path\":\"/b\",\"kind\":\"orphan_dir\",\"est_bytes\":1024}]\n";
        let (cands, pct) = parse_candidates(output).expect("parse ok");
        assert_eq!(pct, 88);
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].path, PathBuf::from("/a"));
        assert_eq!(cands[0].kind, CandidateKind::TrackedWorktree);
        assert_eq!(cands[1].kind, CandidateKind::OrphanDir);
        assert_eq!(cands[1].est_bytes, Some(1024));
    }

    #[test]
    fn unknown_field_causes_that_element_to_be_skipped_not_a_hard_error() {
        // `deny_unknown_fields` rejects the first element; the array itself is
        // valid so parsing continues and keeps the good second element.
        let output = "\
DISK_USED_PCT=90\n\
CANDIDATES_JSON=[{\"path\":\"/bad\",\"kind\":\"orphan_dir\",\"evil\":true},{\"path\":\"/good\",\"kind\":\"stale_build_cache\"}]\n";
        let (cands, _pct) = parse_candidates(output).expect("array itself is valid");
        assert_eq!(cands.len(), 1, "the unknown-field element must be dropped");
        assert_eq!(cands[0].path, PathBuf::from("/good"));
        assert_eq!(cands[0].kind, CandidateKind::StaleBuildCache);
    }

    #[test]
    fn malformed_array_is_a_hard_error() {
        let output = "DISK_USED_PCT=90\nCANDIDATES_JSON=not-json-at-all\n";
        assert!(parse_candidates(output).is_err());
    }

    #[test]
    fn json_object_instead_of_array_is_a_hard_error() {
        let output =
            "DISK_USED_PCT=90\nCANDIDATES_JSON={\"path\":\"/a\",\"kind\":\"orphan_dir\"}\n";
        let err = parse_candidates(output).expect_err("object is not an array");
        assert!(err.contains("not a JSON array"), "got: {err}");
    }

    #[test]
    fn bad_element_is_skipped_but_valid_ones_survive() {
        // First element has an invalid `kind`; it is skipped, not fatal.
        let output = "\
DISK_USED_PCT=91\n\
CANDIDATES_JSON=[{\"path\":\"/x\",\"kind\":\"not_a_kind\"},{\"path\":\"/y\",\"kind\":\"orphan_dir\"}]\n";
        let (cands, _pct) = parse_candidates(output).expect("array valid");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].path, PathBuf::from("/y"));
    }

    #[test]
    fn missing_disk_used_pct_marker_errors() {
        let output = "CANDIDATES_JSON=[]\n";
        assert!(parse_candidates(output).is_err());
    }

    #[test]
    fn missing_candidates_json_marker_errors() {
        let output = "DISK_USED_PCT=80\n";
        assert!(parse_candidates(output).is_err());
    }

    #[test]
    fn disk_used_pct_above_100_errors() {
        let output = "DISK_USED_PCT=150\nCANDIDATES_JSON=[]\n";
        assert!(parse_candidates(output).is_err());
    }

    #[test]
    fn empty_candidate_array_is_ok() {
        let output = "DISK_USED_PCT=42\nCANDIDATES_JSON=[]\n";
        let (cands, pct) = parse_candidates(output).expect("empty array is valid");
        assert!(cands.is_empty());
        assert_eq!(pct, 42);
    }

    #[test]
    fn kind_roundtrips_through_snake_case() {
        for (kind, wire) in [
            (CandidateKind::TrackedWorktree, "\"tracked_worktree\""),
            (CandidateKind::OrphanDir, "\"orphan_dir\""),
            (CandidateKind::StaleBuildCache, "\"stale_build_cache\""),
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, wire);
            let back: CandidateKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }
}
