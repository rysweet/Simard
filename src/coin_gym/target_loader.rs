//! Target loader (research doc Part 3.3, component 1).
//!
//! Pulls a COIN snapshot's targets and reserves a **held-out fresh** slice for
//! the anti-overfit verification gate. Real snapshots are published Docker/HF
//! artifacts pulled on a VM (Phase 3); this loader deliberately targets a JSON
//! manifest so the whole pipeline runs offline in tests and CI. A bundled sample
//! manifest ships with the crate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::agent_runner::Candidate;
use super::types::{CoinGymError, CoinGymResult, Target, TargetFamily};

/// The bundled sample snapshot manifest, embedded at compile time so tests and
/// the offline demo `run` never depend on a checked-out path.
pub const SAMPLE_SNAPSHOT_JSON: &str = include_str!("fixtures/sample_snapshot.json");

/// A loaded target set: the pinned evaluation slice plus a reserved held-out
/// fresh slice used only for verification (research doc Part 3.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetSet {
    /// Snapshot identifier the targets came from.
    pub snapshot: String,
    /// The pinned evaluation targets.
    pub pinned: Vec<Target>,
    /// Held-out fresh targets reserved for the overfitting-verification gate.
    pub held_out_fresh: Vec<Target>,
}

impl TargetSet {
    /// Total number of targets across both slices.
    #[must_use]
    pub fn total(&self) -> usize {
        self.pinned.len() + self.held_out_fresh.len()
    }
}

/// A source of COIN targets. Implementors resolve a snapshot into a
/// [`TargetSet`]. The trait keeps the real (VM-backed) snapshot puller and the
/// offline fixture loader interchangeable.
pub trait TargetSource {
    /// Load the target set.
    ///
    /// # Errors
    /// Returns [`CoinGymError`] if the underlying manifest cannot be read or
    /// parsed.
    fn load(&self) -> CoinGymResult<TargetSet>;
}

// ── Fixture-backed source ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SnapshotManifest {
    snapshot: String,
    targets: ManifestTargets,
    #[serde(default)]
    oracle: HashMap<String, String>,
    #[serde(default)]
    script: HashMap<String, ScriptEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestTargets {
    #[serde(default)]
    pinned: Vec<Target>,
    #[serde(default)]
    held_out_fresh: Vec<Target>,
}

#[derive(Debug, Deserialize)]
struct ScriptEntry {
    input: String,
    confidence: f64,
    #[serde(default)]
    rationale: String,
}

fn parse_manifest(raw: &str) -> CoinGymResult<SnapshotManifest> {
    serde_json::from_str(raw).map_err(|e| CoinGymError::Parse(format!("snapshot manifest: {e}")))
}

/// Loads targets from a JSON snapshot manifest on disk.
#[derive(Clone, Debug)]
pub struct FixtureTargetSource {
    path: PathBuf,
}

impl FixtureTargetSource {
    /// Create a source that reads the manifest at `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl TargetSource for FixtureTargetSource {
    fn load(&self) -> CoinGymResult<TargetSet> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| CoinGymError::Io(format!("read {}: {e}", self.path.display())))?;
        let manifest = parse_manifest(&raw)?;
        Ok(TargetSet {
            snapshot: manifest.snapshot,
            pinned: manifest.targets.pinned,
            held_out_fresh: manifest.targets.held_out_fresh,
        })
    }
}

/// An in-memory source over a raw JSON manifest string. Used by the embedded
/// sample and by tests.
#[derive(Clone, Debug)]
pub struct InMemoryTargetSource {
    raw: String,
}

impl InMemoryTargetSource {
    /// Create a source over an owned JSON manifest string.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }

    /// A source over the bundled sample snapshot.
    #[must_use]
    pub fn sample() -> Self {
        Self::new(SAMPLE_SNAPSHOT_JSON)
    }
}

impl TargetSource for InMemoryTargetSource {
    fn load(&self) -> CoinGymResult<TargetSet> {
        let manifest = parse_manifest(&self.raw)?;
        Ok(TargetSet {
            snapshot: manifest.snapshot,
            pinned: manifest.targets.pinned,
            held_out_fresh: manifest.targets.held_out_fresh,
        })
    }
}

/// A fully-parsed demo scenario: the target set plus the offline `oracle`
/// (ground-truth reaching inputs, consumed by the mock executor) and the
/// `script` (the scaffold agent's proposed candidates). These auxiliary maps
/// exist ONLY to exercise the pipeline offline — a real run gets its oracle from
/// `coin evaluate` and its candidates from a live model.
#[derive(Clone, Debug)]
pub struct DemoScenario {
    /// The target set.
    pub targets: TargetSet,
    /// Ground-truth reaching input per target id (mock-oracle truth).
    pub oracle: HashMap<String, String>,
    /// Scripted agent candidate per target id.
    pub script: HashMap<String, Candidate>,
}

impl DemoScenario {
    /// Parse a full demo scenario (targets + oracle + script) from a JSON
    /// manifest string.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Parse`] on malformed JSON.
    pub fn from_manifest(raw: &str) -> CoinGymResult<Self> {
        let manifest = parse_manifest(raw)?;
        let script = manifest
            .script
            .into_iter()
            .map(|(id, entry)| {
                (
                    id,
                    Candidate {
                        input: entry.input,
                        confidence: entry.confidence,
                        rationale: entry.rationale,
                    },
                )
            })
            .collect();
        Ok(Self {
            targets: TargetSet {
                snapshot: manifest.snapshot,
                pinned: manifest.targets.pinned,
                held_out_fresh: manifest.targets.held_out_fresh,
            },
            oracle: manifest.oracle,
            script,
        })
    }

    /// The bundled sample demo scenario.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Parse`] if the embedded fixture is malformed
    /// (which would be a build-time bug caught by tests).
    pub fn sample() -> CoinGymResult<Self> {
        Self::from_manifest(SAMPLE_SNAPSHOT_JSON)
    }

    /// Load a demo scenario from a JSON manifest file.
    ///
    /// # Errors
    /// Returns [`CoinGymError`] on read or parse failure.
    pub fn from_path(path: &Path) -> CoinGymResult<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| CoinGymError::Io(format!("read {}: {e}", path.display())))?;
        Self::from_manifest(&raw)
    }

    /// The serialisable offline mock context (oracle + script) this scenario
    /// grades against, so a persisted run stays self-contained for the offline
    /// self-improvement loop (`improve --holdout fresh`). This is **mock
    /// ground-truth only** (a test double); real runs get their verdicts from
    /// `coin verify`, never from a stored oracle.
    #[must_use]
    pub fn offline_scaffold(&self) -> OfflineScaffold {
        let script = self
            .script
            .iter()
            .map(|(id, c)| {
                (
                    id.clone(),
                    OfflineScriptEntry {
                        input: c.input.clone(),
                        confidence: c.confidence,
                        rationale: c.rationale.clone(),
                    },
                )
            })
            .collect();
        OfflineScaffold {
            oracle: self.oracle.clone(),
            script,
        }
    }
}

/// The offline mock context (oracle + scripted candidates) that graded an
/// offline scaffold run. Persisted alongside a run so the offline
/// self-improvement loop can re-grade held-out fresh targets and reconstruct the
/// agent's base candidates without re-reading the manifest. **Mock ground-truth
/// only**: empty for real (`coin verify`-graded) runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OfflineScaffold {
    /// Ground-truth reaching input per target id (the mock oracle).
    #[serde(default)]
    pub oracle: HashMap<String, String>,
    /// The agent's base scripted candidate per target id.
    #[serde(default)]
    pub script: HashMap<String, OfflineScriptEntry>,
}

impl OfflineScaffold {
    /// Whether this scaffold carries no mock context (a real run, or an empty
    /// scenario). The self-improvement loop refuses to run on such a run.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.oracle.is_empty() && self.script.is_empty()
    }

    /// Reconstruct the base scripted candidates as a `target_id -> Candidate`
    /// map (the reasoner's script), so the loop can replay the agent offline.
    #[must_use]
    pub fn base_candidates(&self) -> HashMap<String, Candidate> {
        self.script
            .iter()
            .map(|(id, e)| {
                (
                    id.clone(),
                    Candidate {
                        input: e.input.clone(),
                        confidence: e.confidence,
                        rationale: e.rationale.clone(),
                    },
                )
            })
            .collect()
    }
}

/// A serialisable scripted candidate (the persistable twin of
/// [`Candidate`], which is intentionally not `serde`-derived).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfflineScriptEntry {
    /// The candidate input (placeholder UTF-8 in the scaffold).
    pub input: String,
    /// The agent's self-reported confidence in `0.0..=1.0`.
    pub confidence: f64,
    /// Free-text rationale.
    #[serde(default)]
    pub rationale: String,
}

// ── Real COIN dataset schema (Hugging Face rows) ─────────────────────────────
//
// The offline demo above uses the compact scaffold fixture shape. This section
// parses COIN's **real** published dataset schema (one row per target, see
// `docs/reference/coin-benchmark.md#dataset-schema`) so the loader is validated
// against the actual contract, pinned by `revision`.

/// The `<project>:<harness>:<file>:<line_start>[-<line_end>]` components decoded
/// from a COIN `target_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTargetId {
    /// OSS-Fuzz project name.
    pub project: String,
    /// Primary reaching harness binary.
    pub harness: String,
    /// Canonical source path.
    pub file: String,
    /// 1-based start line.
    pub line_start: u32,
    /// 1-based end line (`None` for a single-line target).
    pub line_end: Option<u32>,
}

/// Decode a COIN `target_id` of the form
/// `<project>:<harness>:<file>:<line_start>[-<line_end>]`.
///
/// The line spec is the final `:`-separated segment, so a `file` containing no
/// colon (the normal case for `/src/<project>/<rel>` paths) is preserved intact.
///
/// # Errors
/// Returns [`CoinGymError::Parse`] if the id is missing components or the line
/// spec is not a positive integer (optionally a `start-end` range).
pub fn parse_target_id(target_id: &str) -> CoinGymResult<ParsedTargetId> {
    let (rest, line_spec) = target_id.rsplit_once(':').ok_or_else(|| {
        CoinGymError::Parse(format!(
            "target_id '{target_id}' must be '<project>:<harness>:<file>:<line_start>[-<line_end>]'"
        ))
    })?;
    let mut parts = rest.splitn(3, ':');
    let project = parts.next().unwrap_or_default();
    let harness = parts.next();
    let file = parts.next();
    let (Some(harness), Some(file)) = (harness, file) else {
        return Err(CoinGymError::Parse(format!(
            "target_id '{target_id}' must have project, harness, file, and line components"
        )));
    };
    if project.is_empty() || harness.is_empty() || file.is_empty() {
        return Err(CoinGymError::Parse(format!(
            "target_id '{target_id}' has an empty project/harness/file component"
        )));
    }
    let (line_start, line_end) = parse_line_spec(line_spec, target_id)?;
    Ok(ParsedTargetId {
        project: project.to_string(),
        harness: harness.to_string(),
        file: file.to_string(),
        line_start,
        line_end,
    })
}

fn parse_line_spec(spec: &str, target_id: &str) -> CoinGymResult<(u32, Option<u32>)> {
    let parse_u32 = |s: &str| -> CoinGymResult<u32> {
        s.parse::<u32>().map_err(|_| {
            CoinGymError::Parse(format!(
                "target_id '{target_id}' has a non-numeric line component '{s}'"
            ))
        })
    };
    match spec.split_once('-') {
        Some((start, end)) => {
            let start = parse_u32(start)?;
            let end = parse_u32(end)?;
            if end < start {
                return Err(CoinGymError::Parse(format!(
                    "target_id '{target_id}' has line_end {end} < line_start {start}"
                )));
            }
            Ok((start, if end == start { None } else { Some(end) }))
        }
        None => Ok((parse_u32(spec)?, None)),
    }
}

/// Map a dataset `split` name to a target [`TargetFamily`].
#[must_use]
pub fn family_for_split(split: &str) -> Option<TargetFamily> {
    match split {
        "codeql_only" => Some(TargetFamily::Frontier),
        "gcs_reachable" => Some(TargetFamily::NonTrivialReachable),
        _ => None,
    }
}

/// One row of the real COIN dataset (selected columns; the rest are ignored). A
/// row is self-describing via `target_id` but may repeat the components as
/// explicit columns; explicit columns win when present.
#[derive(Clone, Debug, Deserialize)]
pub struct CoinDatasetRow {
    /// `<project>:<harness>:<file>:<line_start>[-<line_end>]`.
    pub target_id: String,
    /// Snapshot tag the row is pinned to (`coin_version`, e.g. `v2026-07`).
    #[serde(default)]
    pub coin_version: Option<String>,
    /// OSS-Fuzz commit pinned for the snapshot (becomes [`Target::commit`]).
    #[serde(default)]
    pub oss_fuzz_commit: Option<String>,
    /// COIN commit pinned for the snapshot (fallback for [`Target::commit`]).
    #[serde(default)]
    pub coin_commit: Option<String>,
    /// Explicit project override.
    #[serde(default)]
    pub project: Option<String>,
    /// Explicit harness override.
    #[serde(default)]
    pub harness: Option<String>,
    /// Explicit source-file override.
    #[serde(default)]
    pub file: Option<String>,
    /// Explicit start-line override.
    #[serde(default)]
    pub line_start: Option<u32>,
    /// Explicit end-line override.
    #[serde(default)]
    pub line_end: Option<u32>,
    /// Split the row belongs to (`codeql_only` / `gcs_reachable`), used to
    /// derive the family when `family` is absent.
    #[serde(default)]
    pub split: Option<String>,
    /// Explicit family override (wins over `split`).
    #[serde(default)]
    pub family: Option<TargetFamily>,
}

impl CoinDatasetRow {
    /// Convert to a [`Target`], verifying the row is pinned to
    /// `expected_revision` and deriving its family from `family`/`split`.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Parse`] if the row is pinned to a different
    /// revision, its `target_id` is malformed, or its family cannot be resolved.
    pub fn to_target(&self, expected_revision: &str) -> CoinGymResult<Target> {
        if let Some(ver) = &self.coin_version
            && ver != expected_revision
        {
            return Err(CoinGymError::Parse(format!(
                "target '{}' is pinned to revision '{ver}', not the requested '{expected_revision}'",
                self.target_id
            )));
        }
        let parsed = parse_target_id(&self.target_id)?;
        let family = self
            .family
            .or_else(|| self.split.as_deref().and_then(family_for_split))
            .ok_or_else(|| {
                CoinGymError::Parse(format!(
                    "target '{}' has no family and no recognised split to derive one from",
                    self.target_id
                ))
            })?;
        let line = self.line_start.unwrap_or(parsed.line_start);
        // An explicit `line_end` column must not contradict the start; a range
        // that ends before it starts is malformed data, not a single-line target.
        if let Some(end) = self.line_end
            && end < line
        {
            return Err(CoinGymError::Parse(format!(
                "target '{}' has line_end {end} < line_start {line}",
                self.target_id
            )));
        }
        let line_end = self.line_end.or(parsed.line_end).filter(|e| *e != line);
        Ok(Target {
            id: self.target_id.clone(),
            project: self.project.clone().unwrap_or(parsed.project),
            commit: self
                .oss_fuzz_commit
                .clone()
                .or_else(|| self.coin_commit.clone())
                .unwrap_or_default(),
            harness: self.harness.clone().unwrap_or(parsed.harness),
            file: self.file.clone().unwrap_or(parsed.file),
            line,
            line_end,
            family,
        })
    }
}

/// A real-schema dataset snapshot: pinned evaluation rows plus a held-out fresh
/// slice (the anti-overfit oracle, which may come from a newer revision).
#[derive(Clone, Debug, Deserialize)]
pub struct DatasetManifest {
    /// Hugging Face dataset repo, e.g. `COIN-Bench/coin`.
    pub dataset: String,
    /// Pinned snapshot revision, e.g. `v2026-07`.
    pub revision: String,
    /// Revision of the held-out fresh slice (defaults to `revision`). A distinct
    /// value models "reserve a *newer* snapshot slice as the overfit oracle".
    #[serde(default)]
    pub held_out_revision: Option<String>,
    /// Pinned evaluation rows.
    #[serde(default)]
    pub pinned: Vec<CoinDatasetRow>,
    /// Held-out fresh rows.
    #[serde(default)]
    pub held_out_fresh: Vec<CoinDatasetRow>,
}

/// Loads targets from the **real** COIN dataset schema, pinned by revision.
#[derive(Clone, Debug)]
pub struct DatasetSource {
    manifest: DatasetManifest,
}

impl DatasetSource {
    /// Create a source from an already-parsed manifest.
    #[must_use]
    pub fn new(manifest: DatasetManifest) -> Self {
        Self { manifest }
    }

    /// Parse a dataset manifest (JSON) into a source.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Parse`] on malformed JSON.
    pub fn from_manifest(raw: &str) -> CoinGymResult<Self> {
        let manifest: DatasetManifest = serde_json::from_str(raw)
            .map_err(|e| CoinGymError::Parse(format!("dataset manifest: {e}")))?;
        Ok(Self::new(manifest))
    }

    /// Parse a dataset manifest from a file on disk.
    ///
    /// # Errors
    /// Returns [`CoinGymError`] on read or parse failure.
    pub fn from_path(path: &Path) -> CoinGymResult<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| CoinGymError::Io(format!("read {}: {e}", path.display())))?;
        Self::from_manifest(&raw)
    }
}

impl TargetSource for DatasetSource {
    fn load(&self) -> CoinGymResult<TargetSet> {
        let revision = &self.manifest.revision;
        let held_out_rev = self
            .manifest
            .held_out_revision
            .as_deref()
            .unwrap_or(revision);
        let pinned = self
            .manifest
            .pinned
            .iter()
            .map(|r| r.to_target(revision))
            .collect::<CoinGymResult<Vec<_>>>()?;
        let held_out_fresh = self
            .manifest
            .held_out_fresh
            .iter()
            .map(|r| r.to_target(held_out_rev))
            .collect::<CoinGymResult<Vec<_>>>()?;
        Ok(TargetSet {
            snapshot: format!("{}@{}", self.manifest.dataset, revision),
            pinned,
            held_out_fresh,
        })
    }
}
