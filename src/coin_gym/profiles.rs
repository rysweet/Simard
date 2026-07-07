//! Profiles + run persistence (research doc Part 3.3, component 7).
//!
//! A **profile** is isolated per-model run state so baseline-vs-team and
//! model-vs-model comparisons are reproducible and never cross-contaminate. On
//! disk:
//!
//! ```text
//! <home>/profiles/<name>/profile.json      # metadata
//! <home>/profiles/<name>/runs/<run_id>.json  # persisted runs (report + targets)
//! ```
//!
//! `<home>` defaults to `target/coin-gym` (relative, like the existing gym) and
//! can be overridden with the `COIN_GYM_HOME` env var. A run is stored together
//! with the target set it was evaluated against so `score`/`compare`/`improve`
//! can reload full context offline.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::target_loader::TargetSet;
use super::types::{CoinGymError, CoinGymResult, RunReport};

/// Default (relative) home when `COIN_GYM_HOME` is unset.
pub const DEFAULT_HOME: &str = "target/coin-gym";

/// Resolve the COIN Gym home directory (`COIN_GYM_HOME` or the default).
#[must_use]
pub fn default_home() -> PathBuf {
    std::env::var_os("COIN_GYM_HOME").map_or_else(|| PathBuf::from(DEFAULT_HOME), PathBuf::from)
}

/// On-disk profile metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Profile name (directory-safe).
    pub name: String,
    /// Model this profile isolates.
    pub model: String,
    /// Creation time (unix epoch milliseconds).
    pub created_at_unix_ms: u128,
}

/// A run persisted with the target set it was evaluated against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersistedRun {
    /// The run report.
    pub report: RunReport,
    /// The target set the run was evaluated against.
    pub targets: TargetSet,
}

/// Directory for a named profile under `home`.
#[must_use]
pub fn profile_dir(home: &Path, name: &str) -> PathBuf {
    home.join("profiles").join(name)
}

/// Directory holding a profile's persisted runs.
#[must_use]
pub fn runs_dir(home: &Path, name: &str) -> PathBuf {
    profile_dir(home, name).join("runs")
}

/// Sanitise an arbitrary string into a directory-safe profile name.
#[must_use]
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "default".to_string()
    } else {
        cleaned
    }
}

/// Create (or load) a profile for `model` under `home`, persisting its metadata.
///
/// Enforces per-model isolation: reusing an existing profile name with a
/// *different* model is rejected so baseline-vs-team and model-vs-model runs
/// never cross-contaminate a profile.
///
/// # Errors
/// Returns [`CoinGymError::Usage`] if the profile already exists for a different
/// model, or [`CoinGymError::Io`] on directory-creation or write failure.
pub fn ensure_profile(home: &Path, name: &str, model: &str) -> CoinGymResult<Profile> {
    let dir = profile_dir(home, name);
    std::fs::create_dir_all(runs_dir(home, name))
        .map_err(|e| CoinGymError::Io(format!("create {}: {e}", dir.display())))?;
    let meta_path = dir.join("profile.json");
    if let Ok(existing) = load_profile_meta(&meta_path) {
        if existing.model != model {
            return Err(CoinGymError::Usage(format!(
                "profile '{name}' is bound to model '{}'; refusing to reuse it for '{model}' \
                 (use a distinct --profile per model)",
                existing.model
            )));
        }
        return Ok(existing);
    }
    let profile = Profile {
        name: name.to_string(),
        model: model.to_string(),
        created_at_unix_ms: now_unix_ms(),
    };
    write_json(&meta_path, &profile)?;
    Ok(profile)
}

fn load_profile_meta(path: &Path) -> CoinGymResult<Profile> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| CoinGymError::Io(format!("read {}: {e}", path.display())))?;
    serde_json::from_str(&raw).map_err(|e| CoinGymError::Parse(format!("profile.json: {e}")))
}

/// List all profiles under `home` (sorted by name). Missing home ⇒ empty list.
///
/// # Errors
/// Returns [`CoinGymError::Io`] if the profiles directory exists but cannot be
/// read.
pub fn list_profiles(home: &Path) -> CoinGymResult<Vec<Profile>> {
    let profiles_root = home.join("profiles");
    if !profiles_root.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&profiles_root)
        .map_err(|e| CoinGymError::Io(format!("read {}: {e}", profiles_root.display())))?;
    let mut profiles = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| CoinGymError::Io(format!("dir entry: {e}")))?;
        let meta = entry.path().join("profile.json");
        if let Ok(profile) = load_profile_meta(&meta) {
            profiles.push(profile);
        }
    }
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// Persist a run (report + targets) under a profile. Returns the file path.
///
/// Refuses to overwrite an existing run file so a run-id collision surfaces
/// loudly instead of silently destroying a prior run.
///
/// # Errors
/// Returns [`CoinGymError::Io`] on write failure or if the run file already
/// exists.
pub fn save_run(home: &Path, profile: &str, run: &PersistedRun) -> CoinGymResult<PathBuf> {
    let dir = runs_dir(home, profile);
    std::fs::create_dir_all(&dir)
        .map_err(|e| CoinGymError::Io(format!("create {}: {e}", dir.display())))?;
    let path = dir.join(format!("{}.json", run.report.run_id));
    if path.exists() {
        return Err(CoinGymError::Io(format!(
            "run '{}' already exists at {}",
            run.report.run_id,
            path.display()
        )));
    }
    write_json(&path, run)?;
    Ok(path)
}

/// Load a persisted run by id. If `profile` is `Some`, look only there; if
/// `None`, search every profile's runs directory.
///
/// # Errors
/// Returns [`CoinGymError::NotFound`] if no matching run exists, or
/// [`CoinGymError`] on read/parse failure.
pub fn load_run(home: &Path, profile: Option<&str>, run_id: &str) -> CoinGymResult<PersistedRun> {
    if let Some(name) = profile {
        let path = runs_dir(home, name).join(format!("{run_id}.json"));
        return read_persisted_run(&path);
    }
    // Search all profiles.
    for p in list_profiles(home)? {
        let path = runs_dir(home, &p.name).join(format!("{run_id}.json"));
        if path.is_file() {
            return read_persisted_run(&path);
        }
    }
    Err(CoinGymError::NotFound(format!(
        "run '{run_id}' not found under {}",
        home.display()
    )))
}

fn read_persisted_run(path: &Path) -> CoinGymResult<PersistedRun> {
    let raw = std::fs::read_to_string(path)
        .map_err(|_| CoinGymError::NotFound(format!("run file {}", path.display())))?;
    serde_json::from_str(&raw).map_err(|e| CoinGymError::Parse(format!("persisted run: {e}")))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> CoinGymResult<()> {
    let body = serde_json::to_string_pretty(value)
        .map_err(|e| CoinGymError::Parse(format!("serialize {}: {e}", path.display())))?;
    std::fs::write(path, body)
        .map_err(|e| CoinGymError::Io(format!("write {}: {e}", path.display())))
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
