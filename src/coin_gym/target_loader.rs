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
use super::types::{CoinGymError, CoinGymResult, Target};

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
}
