//! Shared recipe context-file transport (issues #2640/#2692).
//!
//! `recipe-runner-rs` only accepts context on `argv` as `-c KEY=VALUE`. When a
//! value is *unbounded* — a full day's journal context, a 24 h batch of
//! episodes, an arbitrary-size PR body — inlining it as a single `-c` token
//! overflows the kernel's per-argument limit (`MAX_ARG_STRLEN`, 128 KiB on
//! Linux) and `execve` fails with `E2BIG` (`errno 7`) BEFORE the child runs.
//! That was the live, once-per-hour journal failure: the whole `day_context`
//! JSON was inlined as `-c day_context=<…>` and the spawn died with
//! "Argument list too long".
//!
//! [`ContextFile`] is the fix's transport, mirroring the already-proven
//! distillation `facts_output_path` pattern (issues #2622/#2619): the payload is
//! written to a private, per-invocation temp file and ONLY the file's short
//! absolute PATH rides on `argv` as `-c <key>_path=<abs>`. The recipe asset then
//! reads the file via the agent's file-reading tool (`{{<key>_path}}`), so the
//! model still sees the full payload byte-for-byte (guideline G3 — no
//! truncation) while `argv` stays tiny and `ARG_MAX` becomes irrelevant.
//!
//! The temp file lives in a fresh `0700` temp directory (via the `tempfile`
//! crate) so concurrent journal ticks / distillation passes never collide, and
//! the whole directory is unlinked when the guard drops — a per-invocation
//! payload never lingers on disk.

use std::io;
use std::path::Path;

use tempfile::TempDir;

/// A recipe context value delivered out-of-band via a private temp file.
///
/// Construct one per large context var with [`ContextFile::write`]; keep the
/// guard alive until `recipe-runner-rs` has finished reading (its `Drop` removes
/// the file). Pass [`ContextFile::arg_value`] as the `-c` value so only the file
/// PATH — never the payload — touches `argv`.
pub struct ContextFile {
    /// The recipe context key this file backs; `arg_value` emits `<key>_path`.
    key: String,
    /// Absolute path to the payload file, as a UTF-8 string for `-c` embedding.
    path: String,
    /// Owning temp directory; dropping it unlinks the file and the directory.
    _dir: TempDir,
}

impl ContextFile {
    /// Write `value` to a private temp file and return a guard.
    ///
    /// `base_type` namespaces the temp directory (e.g. `"journal"`), and `key`
    /// is the recipe context key whose payload this is (e.g. `"day_context"`);
    /// [`arg_value`](Self::arg_value) will emit `<key>_path=<abs>`.
    ///
    /// The payload is written verbatim (no truncation), so an oversized value
    /// that would have overflowed `argv` lands safely on disk instead.
    pub fn write(base_type: &str, key: &str, value: &str) -> io::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix(&format!("simard-{base_type}-ctx-"))
            .tempdir()?;
        // A stable, per-key filename inside the unique directory keeps the path
        // readable in logs while the directory guarantees uniqueness.
        let file_path = dir.path().join(format!("{key}.ctx"));
        std::fs::write(&file_path, value)?;
        let path = file_path.to_string_lossy().into_owned();
        Ok(Self {
            key: key.to_string(),
            path,
            _dir: dir,
        })
    }

    /// The absolute path to the payload file, safe to pass as a subprocess
    /// argument or open directly.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The `-c` value to pass to `recipe-runner-rs`: `"<key>_path=<abs>"`. Small
    /// and constant-size regardless of the payload, so it can never contribute
    /// to `ARG_MAX`.
    pub fn arg_value(&self) -> String {
        format!("{}_path={}", self.key, self.path)
    }
}

impl std::fmt::Debug for ContextFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the payload — only the key and path (the payload is the
        // whole point of keeping OFF argv/logs).
        f.debug_struct("ContextFile")
            .field("key", &self.key)
            .field("path", &Path::new(&self.path))
            .finish()
    }
}
