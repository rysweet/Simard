//! Last-known-green default-branch head-SHA cache — the churn-breaker.
//!
//! The governed-fleet sweep re-reads every workflow and its latest run for all
//! [`GOVERNED_REPOS`](super::GOVERNED_REPOS) on every cycle. When the fleet is
//! already green and unchanged, that is a wasteful full re-audit. This cache
//! records, per repo, the default-branch **head commit SHA** at which the repo
//! was last verified green. On the next sweep, a repo whose head SHA is
//! unchanged is short-circuited (see [`super::gh::collect_fleet`]) instead of
//! re-collected.
//!
//! ## Why a SHA is a sound skip key
//!
//! The cache only *records* a repo as green when [`super::classify::repo_cacheable`]
//! holds: no active workflow **demonstrates** it can run without a new
//! default-branch commit (i.e. none has a completed, non-commit-driven latest
//! run, and none is in progress). A commit-driven workflow cannot produce a new
//! run without a new commit, which changes the head SHA and misses the cache. So
//! for a cached repo, an unchanged head SHA means no active workflow has
//! demonstrably run since the green verdict — the verdict still holds. Repos with
//! active scheduled / dispatch / issue-triggered runs are never cached and are
//! always freshly swept.
//!
//! The cache is a pure optimization: a missing or unreadable cache degrades to a
//! full sweep (the correct, complete behavior), never to a wrong verdict.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::state_root::simard_state_root;

/// Cache file schema version, so a future format change can be detected rather
/// than misparsed.
const CACHE_VERSION: u32 = 1;

/// Persisted map of `owner/repo` → last-known-green default-branch head SHA.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GreenShaCache {
    #[serde(default = "default_version")]
    version: u32,
    /// `BTreeMap` keeps the on-disk JSON key-ordered and diff-stable.
    #[serde(default)]
    green: BTreeMap<String, String>,
}

fn default_version() -> u32 {
    CACHE_VERSION
}

impl GreenShaCache {
    /// An empty cache — every repo will be freshly swept.
    pub fn empty() -> Self {
        Self {
            version: CACHE_VERSION,
            green: BTreeMap::new(),
        }
    }

    /// The last-known-green head SHA recorded for `repo`, if any.
    pub fn get(&self, repo: &str) -> Option<&str> {
        self.green.get(repo).map(String::as_str)
    }

    /// True when `head_sha` is non-empty and equals the cached green SHA for
    /// `repo` — i.e. the repo can be skipped this sweep. An empty `head_sha`
    /// never matches, so a repo whose head SHA could not be read is always
    /// swept.
    pub fn is_green(&self, repo: &str, head_sha: &str) -> bool {
        !head_sha.is_empty() && self.get(repo) == Some(head_sha)
    }

    /// Record `repo` as green at `head_sha`. An empty `head_sha` is ignored so a
    /// blank SHA can never be cached (which would then spuriously match another
    /// blank SHA).
    pub fn record_green(&mut self, repo: &str, head_sha: &str) {
        if head_sha.is_empty() {
            self.green.remove(repo);
            return;
        }
        self.green.insert(repo.to_string(), head_sha.to_string());
    }

    /// Drop any cached green SHA for `repo` (it is no longer known-green or no
    /// longer cacheable).
    pub fn invalidate(&mut self, repo: &str) {
        self.green.remove(repo);
    }

    /// Number of repos currently cached as green (for evidence/tests).
    pub fn len(&self) -> usize {
        self.green.len()
    }

    /// True when no repo is cached as green.
    pub fn is_empty(&self) -> bool {
        self.green.is_empty()
    }

    /// Canonical cache path: `<state_root>/state/ci_health_green_sha.json`.
    pub fn default_path() -> PathBuf {
        simard_state_root()
            .join("state")
            .join("ci_health_green_sha.json")
    }

    /// Load the cache from `path`. **Infallible** by design: a missing file
    /// (first run) or an unreadable/corrupt/out-of-version file degrades to an
    /// empty cache (with a WARN), so a bad cache can never block or corrupt a
    /// sweep — the worst case is a full re-audit.
    pub fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::empty(),
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "ci-health green-SHA cache unreadable; proceeding with a full sweep"
                );
                return Self::empty();
            }
        };
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(cache) if cache.version == CACHE_VERSION => cache,
            Ok(cache) => {
                warn!(
                    path = %path.display(),
                    found = cache.version,
                    expected = CACHE_VERSION,
                    "ci-health green-SHA cache version mismatch; ignoring it"
                );
                Self::empty()
            }
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "ci-health green-SHA cache is corrupt; ignoring it"
                );
                Self::empty()
            }
        }
    }

    /// Persist the cache to `path`, creating the parent directory if needed.
    /// Returns an `io::Result`; callers treat a save failure as non-fatal (the
    /// sweep verdict is already computed) and only warn.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // Write to a sibling temp file and atomically rename over `path`, so a
        // concurrent sweep (e.g. the OODA daemon and a manual `simard
        // ci-health`) can never observe or persist a torn, half-written cache.
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(&mut tmp, &json)?;
        tmp.persist(path).map_err(|e| e.error).map(|_| ())
    }
}
