//! TOML-based developer watch list loading.
//!
//! Loads developer watches from a `watches.toml` file.
//! - File not found → returns compile-time default watches (soft fallback).
//! - Parse error → returns hard error (malformed TOML is not silently ignored).

use std::path::Path;

use serde::Deserialize;

use super::types::DeveloperWatch;
use super::watches::default_developer_watches;
use crate::error::{SimardError, SimardResult};

/// TOML wrapper for the watches file.
#[derive(Debug, Deserialize)]
struct TomlWatchesFile {
    #[serde(default)]
    watches: Vec<TomlWatch>,
}

/// A single watch entry in watches.toml.
#[derive(Debug, Deserialize)]
struct TomlWatch {
    github_id: String,
    focus_areas: Vec<String>,
}

/// Load developer watches from a TOML file.
///
/// Returns the default watch list if the file does not exist.
/// Returns a hard error if the file exists but cannot be parsed.
pub fn load_watches_from_file(path: &Path) -> SimardResult<Vec<DeveloperWatch>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(default_developer_watches());
        }
        Err(e) => {
            return Err(SimardError::IdentityTomlParseError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            });
        }
    };

    let file: TomlWatchesFile =
        toml::from_str(&content).map_err(|e| SimardError::IdentityTomlParseError {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

    Ok(file
        .watches
        .into_iter()
        .map(|w| DeveloperWatch {
            github_id: w.github_id,
            focus_areas: w.focus_areas,
            last_checked: None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const VALID_WATCHES_TOML: &str = r#"
[[watches]]
github_id = "octocat"
focus_areas = ["rust", "wasm", "llm"]

[[watches]]
github_id = "ferris"
focus_areas = ["systems-programming"]
"#;

    // ── Happy path ──────────────────────────────────────────────────

    #[test]
    fn load_watches_from_valid_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("watches.toml");
        fs::write(&path, VALID_WATCHES_TOML).unwrap();

        let watches = load_watches_from_file(&path).unwrap();
        assert_eq!(watches.len(), 2);
        assert_eq!(watches[0].github_id, "octocat");
        assert_eq!(watches[0].focus_areas, vec!["rust", "wasm", "llm"]);
        assert_eq!(watches[1].github_id, "ferris");
        assert!(watches[0].last_checked.is_none());
    }

    #[test]
    fn load_watches_parses_focus_areas() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("watches.toml");
        let toml_content = r#"
[[watches]]
github_id = "dev1"
focus_areas = ["area-a", "area-b", "area-c"]
"#;
        fs::write(&path, toml_content).unwrap();

        let watches = load_watches_from_file(&path).unwrap();
        assert_eq!(watches[0].focus_areas.len(), 3);
    }

    #[test]
    fn load_watches_all_have_none_last_checked() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("watches.toml");
        fs::write(&path, VALID_WATCHES_TOML).unwrap();

        let watches = load_watches_from_file(&path).unwrap();
        for watch in &watches {
            assert!(
                watch.last_checked.is_none(),
                "watches loaded from TOML should have last_checked = None"
            );
        }
    }

    // ── Soft fallback on missing file ───────────────────────────────

    #[test]
    fn load_watches_returns_defaults_on_missing_file() {
        let path = Path::new("/nonexistent/path/watches.toml");
        let watches = load_watches_from_file(path).unwrap();
        assert_eq!(
            watches.len(),
            crate::research_tracker::DEFAULT_DEVELOPER_WATCHES.len()
        );
    }

    // ── Hard error on malformed TOML ────────────────────────────────

    #[test]
    fn load_watches_hard_error_on_malformed_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("watches.toml");
        fs::write(&path, "this is {{ not valid TOML").unwrap();

        let err = load_watches_from_file(&path).unwrap_err();
        assert!(
            matches!(err, SimardError::IdentityTomlParseError { .. }),
            "malformed watches.toml should produce a parse error, got: {err:?}"
        );
    }

    // ── Edge cases ──────────────────────────────────────────────────

    #[test]
    fn load_watches_empty_array() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("watches.toml");
        fs::write(&path, "watches = []").unwrap();

        let watches = load_watches_from_file(&path).unwrap();
        assert!(watches.is_empty());
    }

    #[test]
    fn load_watches_single_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("watches.toml");
        let toml_content = r#"
[[watches]]
github_id = "solo-dev"
focus_areas = ["ai-coding"]
"#;
        fs::write(&path, toml_content).unwrap();

        let watches = load_watches_from_file(&path).unwrap();
        assert_eq!(watches.len(), 1);
        assert_eq!(watches[0].github_id, "solo-dev");
    }
}
