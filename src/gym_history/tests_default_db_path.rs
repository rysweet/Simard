//! Failing TDD tests for the shared gym score-history path (issue #2491 /
//! #2494, G1 hybrid measurement, Step 7 — design fix R5 / DATA-3).
//!
//! The benchmark **writer**, the OODA gym step, and the correlation **reader**
//! must all resolve `gym_history.db` through **one** helper so a benchmark score
//! written on one rail is the exact score the correlation endpoint reads back —
//! no writer/reader drift. Today `ooda_loop::observe` hard-codes
//! `Path::new("gym_history.db")` in two places; this suite pins the single
//! canonical resolver and guards against the literal creeping back.
//!
//! Reference: `docs/reference/recall-precision-hybrid-api.md#shared-score-history-path`
//!
//! ```rust
//! // src/gym_history/mod.rs
//! /// The one canonical gym score-history database path, shared by the benchmark
//! /// writer, the OODA gym step, and the correlation reader. Resolved relative to
//! /// the process working directory: `<cwd>/gym_history.db`.
//! pub fn default_db_path() -> PathBuf;
//! ```
//!
//! These reference `crate::gym_history::default_db_path`, which does not exist
//! yet — the compile failure is the intended TDD red state.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::gym_history::default_db_path;

    /// The canonical path's file name is exactly `gym_history.db` — the same
    /// database name every rail has always used, so an existing operator DB is
    /// picked up unchanged after the de-drift refactor.
    #[test]
    fn default_db_path_file_name_is_gym_history_db() {
        let p: PathBuf = default_db_path();
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("gym_history.db"),
            "the shared score-history db must be named gym_history.db"
        );
        assert!(
            p.ends_with("gym_history.db"),
            "path must end with the gym_history.db component, got {p:?}"
        );
    }

    /// Resolution is deterministic: the writer and the reader, calling the same
    /// helper, must land on byte-identical paths (this is the whole point of
    /// R5 — no drift).
    #[test]
    fn default_db_path_is_deterministic() {
        assert_eq!(
            default_db_path(),
            default_db_path(),
            "default_db_path() must return the same path on every call"
        );
    }

    /// The path is resolved relative to the process working directory
    /// (`<cwd>/gym_history.db`), matching the pre-refactor behaviour so the
    /// daemon (which runs from its repo root) keeps reading/writing the same
    /// file. Either the bare relative literal or a cwd-joined absolute form
    /// satisfies "resolves to `<cwd>/gym_history.db`".
    #[test]
    fn default_db_path_resolves_under_cwd() {
        let p = default_db_path();
        let cwd_joined = std::env::current_dir().unwrap().join("gym_history.db");
        let bare = PathBuf::from("gym_history.db");
        assert!(
            p == cwd_joined || p == bare,
            "default_db_path() must resolve to <cwd>/gym_history.db (got {p:?}; \
             expected {cwd_joined:?} or {bare:?})"
        );
    }

    /// R5 durability guard: `ooda_loop::observe` must resolve the score-history
    /// database via `default_db_path()` and must NOT re-introduce the
    /// hard-coded `Path::new("gym_history.db")` literal that caused the
    /// writer/reader drift this refactor removes.
    #[test]
    fn observe_routes_through_default_db_path_not_a_literal() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ooda_loop/observe.rs");
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));

        assert!(
            src.contains("default_db_path"),
            "observe.rs must resolve the score-history db via default_db_path() (R5)"
        );
        assert!(
            !src.contains("Path::new(\"gym_history.db\")"),
            "observe.rs must not hard-code Path::new(\"gym_history.db\") — route \
             through gym_history::default_db_path() so writer and reader cannot drift (R5)"
        );
    }
}
