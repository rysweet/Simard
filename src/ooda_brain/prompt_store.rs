//! Hot-reloading prompt asset store for the prompt-driven OODA brains
//! (`ooda_brain.md`, `ooda_decide.md`, `ooda_orient.md`).
//!
//! ## Why
//!
//! The three prompt-driven brains (`RustyClawdBrain`, `RustyClawdDecideBrain`,
//! `RustyClawdOrientBrain` — PRs #1458, #1469, #1471, wired in #1474) embed
//! their prompts via `include_str!`, so editing a prompt requires a full
//! rebuild + daemon restart (`scripts/redeploy-local.sh`, ~2 minutes). The
//! standing project goal is **"iterate on prompts not code"**, which only
//! holds if a prompt edit can take effect on the next OODA cycle.
//!
//! ## What
//!
//! [`PromptStore`] resolves a prompt by name (e.g. `"ooda_brain.md"`) in this
//! priority order:
//!
//! 1. Disk file at `<resolved-dir>/<name>` — if present, this wins so prompt
//!    edits take effect immediately.
//! 2. Embedded fallback baked in via `include_str!` at build time — used when
//!    the file is missing so the daemon NEVER fails to start because a prompt
//!    file was deleted.
//!
//! `<resolved-dir>` is taken from:
//!
//! 1. `$SIMARD_PROMPT_ASSETS_DIR` (if set and non-empty), else
//! 2. `$HOME/.simard/prompt_assets/simard/` (the path
//!    `scripts/redeploy-local.sh` syncs to), else
//! 3. `None` — pure embedded mode (HOME unset / no env var → behaves
//!    identically to the pre-#1266 baked-in prompts).
//!
//! ## Cache
//!
//! Prompts are cached keyed by file path + mtime. Each `load()` call stats
//! the file (cheap — single syscall) and only re-reads when the mtime has
//! changed, so the steady-state cost is one `metadata()` call per OODA cycle
//! per prompt. Touching a prompt file (`touch ooda_brain.md` or any editor
//! save) bumps mtime and invalidates the cache on the next call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Environment variable that overrides the prompt-asset directory.
pub const ENV_VAR: &str = "SIMARD_PROMPT_ASSETS_DIR";

/// Default subpath under `$HOME` that `scripts/redeploy-local.sh` syncs the
/// prompt assets to.
const DEFAULT_HOME_SUBPATH: &str = ".simard/prompt_assets/simard";

// --- Embedded fallbacks ----------------------------------------------------
//
// These constants are the single source of truth for the daemon's
// out-of-the-box behaviour. They are baked at compile time so the daemon
// always has a working prompt even when no on-disk asset directory exists.

const EMBEDDED_BRAIN: &str = include_str!("../../prompt_assets/simard/ooda_brain.md");
const EMBEDDED_DECIDE: &str = include_str!("../../prompt_assets/simard/ooda_decide.md");
const EMBEDDED_ORIENT: &str = include_str!("../../prompt_assets/simard/ooda_orient.md");

/// Look up the embedded fallback for a known prompt name. Returns `None` for
/// unknown names so callers can surface a configuration error rather than
/// silently serving an empty prompt.
pub fn embedded_fallback(name: &str) -> Option<&'static str> {
    match name {
        "ooda_brain.md" => Some(EMBEDDED_BRAIN),
        "ooda_decide.md" => Some(EMBEDDED_DECIDE),
        "ooda_orient.md" => Some(EMBEDDED_ORIENT),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct CachedPrompt {
    mtime: SystemTime,
    contents: String,
}

/// Thread-safe, mtime-aware loader for prompt-asset markdown files.
#[derive(Debug)]
pub struct PromptStore {
    dir: Option<PathBuf>,
    cache: Mutex<HashMap<String, CachedPrompt>>,
}

impl PromptStore {
    /// Construct a store rooted at the given directory. `None` disables
    /// disk lookup so every `load()` returns the embedded fallback.
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Construct a store with the directory resolved from environment
    /// variables — see module docs for the resolution order.
    pub fn from_env() -> Self {
        Self::new(resolve_dir_from_env())
    }

    /// Resolved on-disk directory (if any). `None` means the store is
    /// running in pure embedded mode.
    pub fn resolved_dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Load a prompt by file name (e.g. `"ooda_brain.md"`).
    ///
    /// Behaviour:
    /// - If the resolved directory contains the file, return its contents
    ///   (cached by mtime).
    /// - Otherwise, return the compiled-in embedded fallback.
    /// - If neither is available (unknown prompt name with no disk file),
    ///   return an empty string. Callers treat this as a configuration
    ///   error surfaced via the LLM response parser.
    pub fn load(&self, name: &str) -> String {
        let fallback = embedded_fallback(name).unwrap_or("");

        let Some(dir) = self.dir.as_ref() else {
            return fallback.to_string();
        };
        let path = dir.join(name);

        let Ok(meta) = std::fs::metadata(&path) else {
            return fallback.to_string();
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        // Fast path: cache hit at the same mtime.
        {
            let cache = self.cache.lock().expect("prompt cache poisoned");
            if let Some(c) = cache.get(name)
                && c.mtime == mtime
            {
                return c.contents.clone();
            }
        }

        // Slow path: re-read and refresh the cache entry.
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                let mut cache = self.cache.lock().expect("prompt cache poisoned");
                cache.insert(
                    name.to_string(),
                    CachedPrompt {
                        mtime,
                        contents: s.clone(),
                    },
                );
                s
            }
            Err(_) => fallback.to_string(),
        }
    }
}

/// Resolve the prompt directory from `$SIMARD_PROMPT_ASSETS_DIR` then
/// `$HOME/.simard/prompt_assets/simard/`. Returns `None` if neither yields
/// a path (forces pure embedded mode).
pub fn resolve_dir_from_env() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(ENV_VAR)
        && !v.is_empty()
    {
        return Some(PathBuf::from(v));
    }
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(DEFAULT_HOME_SUBPATH))
}

// --- Singleton -------------------------------------------------------------

static GLOBAL: OnceLock<PromptStore> = OnceLock::new();

/// Process-wide singleton. Initialized lazily on first call from environment.
/// Subsequent calls are guaranteed to return the same instance.
pub fn global() -> &'static PromptStore {
    GLOBAL.get_or_init(PromptStore::from_env)
}
