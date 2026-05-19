//! Subprocess prompt delivery — amplihack-rs / Simard parity (issue #1897).
//!
//! Picks one of three transports for handing a prompt to a child process
//! (inline argv, stdin pipe, or postmortem temp file + stdin) based on size,
//! NUL-byte presence, caller override, or the `AMPLIHACK_PROMPT_DELIVERY`
//! environment variable.
//!
//! The full contract — heuristic resolution order, alias grammar, RAII
//! lifecycle, and operational guidance — lives in
//! [`docs/prompt-delivery.md`](../../docs/prompt-delivery.md). The inline
//! `#[cfg(test)] mod tests` block below and `tests/prompt_delivery.rs` pin
//! every behaviour as executable specification.
//!
//! ## Public surface
//!
//! * [`PromptDelivery`] — variant enum with `Auto / Inline / Stdin / TempFile`.
//!   `FromStr` accepts the case-insensitive alias grammar from the doc.
//! * [`PromptDeliveryError`] — `TooLarge`, `TempFile`, `Write`, `Permissions`,
//!   `NulInInlineMode`. All variants are `#[non_exhaustive]`.
//! * [`apply_std`] — mutates a [`std::process::Command`] and returns an
//!   [`AppliedPromptStd`] RAII guard owning the temp file (if any) and the
//!   in-memory prompt bytes (for stdin modes).
//! * [`apply_tokio`] — async sibling for [`tokio::process::Command`].
//! * [`select_mode`] — pure heuristic function (no I/O) exposed for unit tests
//!   and for callers who want to inspect the choice without spawning.
//! * [`AppliedPromptStd::feed`] / [`AppliedPromptTokio::feed`] — consumes the
//!   guard and writes the owned prompt buffer to the child's stdin.
//! * [`AppliedPromptStd::retain_temp_file`] — opts out of RAII cleanup so
//!   operators can capture postmortem artifacts.
//!
//! ## Constants (pinned by tests)
//!
//! * [`INLINE_MAX_BYTES`] — `8 * 1024`. Below this, `Auto` picks `Inline`.
//! * [`STDIN_PREFERRED_MAX_BYTES`] — `100 * 1024`. Below this (and above
//!   `INLINE_MAX_BYTES`) `Auto` picks `Stdin`.
//! * [`HARD_CAP_BYTES`] — `16 * 1024 * 1024`. Above this `apply_*` returns
//!   `PromptDeliveryError::TooLarge` before *any* override is consulted.
//! * [`ENV_OVERRIDE`] — `"AMPLIHACK_PROMPT_DELIVERY"`.

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command};
use std::str::FromStr;
use std::sync::OnceLock;

use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt as _;

/// Internal: log a one-time warning when an invalid env override value is
/// encountered. Subsequent invalid values are silently coerced to the
/// heuristic fallback.
fn warn_invalid_env_value_once(value: &str) {
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.set(()).is_ok() {
        tracing::warn!(
            target: "amplihack::prompt_delivery",
            env_var = ENV_OVERRIDE,
            value = value,
            "invalid AMPLIHACK_PROMPT_DELIVERY value; falling back to auto heuristic. \
             Accepted values: auto, inline|cli|argv|arg, stdin|pipe, tempfile|temp-file|file"
        );
    }
}

/// Internal helper: convert prompt bytes into an `&OsStr` for argv use.
/// Unix-only because Rust's `OsStr::from_bytes` is gated to Unix; the
/// integration tests are likewise `#[cfg(unix)]`.
#[cfg(unix)]
fn prompt_as_osstr(prompt: &[u8]) -> &std::ffi::OsStr {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(prompt)
}

#[cfg(not(unix))]
fn prompt_as_osstr(prompt: &[u8]) -> std::ffi::OsString {
    // On non-Unix targets fall back to UTF-8 (best-effort). amplihack
    // primarily runs on Unix; Inline mode on Windows assumes valid UTF-8.
    std::ffi::OsString::from(String::from_utf8_lossy(prompt).into_owned())
}

/// Internal helper: scan an iterator of OS-strings for an exact `--`
/// terminator. Used to avoid duplicating a caller-supplied flag terminator.
fn args_contain_flag_terminator<'a, I>(args: I) -> bool
where
    I: IntoIterator<Item = &'a std::ffi::OsStr>,
{
    args.into_iter().any(|a| a == "--")
}

/// Internal: write a `0o600` permissions bit on a freshly-created
/// `NamedTempFile`. No-op on non-Unix.
#[cfg(unix)]
fn set_owner_only_permissions(file: &NamedTempFile) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(file.path(), perms)
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_file: &NamedTempFile) -> std::io::Result<()> {
    Ok(())
}

/// Internal: create + populate + lock-down a postmortem temp file holding
/// `prompt`. Wraps the three failure modes in distinct error variants so
/// operators can diagnose temp-file vs. write vs. permission failures.
fn build_postmortem_tempfile(prompt: &[u8]) -> Result<NamedTempFile, PromptDeliveryError> {
    let mut file = tempfile::Builder::new()
        .prefix("amplihack-prompt-")
        .suffix(".txt")
        .tempfile()
        .map_err(PromptDeliveryError::TempFile)?;
    file.write_all(prompt).map_err(PromptDeliveryError::Write)?;
    file.flush().map_err(PromptDeliveryError::Write)?;
    set_owner_only_permissions(&file).map_err(PromptDeliveryError::Permissions)?;
    Ok(file)
}

// ---------------------------------------------------------------------------
// Constants (test-asserted)
// ---------------------------------------------------------------------------

/// Below this size, `Auto` picks `Inline` (assuming no NUL bytes).
pub const INLINE_MAX_BYTES: usize = 8 * 1024;

/// Above `INLINE_MAX_BYTES` and below this, `Auto` picks `Stdin`.
pub const STDIN_PREFERRED_MAX_BYTES: usize = 100 * 1024;

/// Hard upper bound. Prompts above this are rejected before any I/O.
pub const HARD_CAP_BYTES: usize = 16 * 1024 * 1024;

/// Environment variable name consulted by `Auto` mode only.
pub const ENV_OVERRIDE: &str = "AMPLIHACK_PROMPT_DELIVERY";

// ---------------------------------------------------------------------------
// Public enums
// ---------------------------------------------------------------------------

/// Delivery mode for a prompt handed to a child process.
///
/// `Auto` is the only variant that consults [`ENV_OVERRIDE`]. Every other
/// variant is taken at face value (including pathological combinations like
/// `Inline` with a NUL-containing prompt, which yields
/// [`PromptDeliveryError::NulInInlineMode`]).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptDelivery {
    /// Use [`select_mode`] heuristic (consults env var).
    Auto,
    /// Force inline argv delivery. **Argv is world-readable via `ps`.**
    Inline,
    /// Force stdin delivery.
    Stdin,
    /// Force `NamedTempFile` (`0o600`) + stdin delivery. The temp file is a
    /// postmortem artifact; the bytes still travel via stdin.
    TempFile,
}

/// Errors returned by [`apply_std`] / [`apply_tokio`] before the child is
/// spawned. Errors from `spawn(2)` itself (such as `E2BIG`) are surfaced by
/// the caller as `std::io::Error` and are *not* wrapped here.
#[non_exhaustive]
#[derive(Debug)]
pub enum PromptDeliveryError {
    /// Prompt exceeds [`HARD_CAP_BYTES`]. Fires before any override is read.
    TooLarge(usize),
    /// `tempfile::NamedTempFile::new()` failed.
    TempFile(std::io::Error),
    /// Writing the prompt bytes to the temp file failed.
    Write(std::io::Error),
    /// Setting `0o600` permissions on the temp file failed (Unix only).
    Permissions(std::io::Error),
    /// Caller explicitly forced `Inline` but the prompt contains a NUL byte.
    NulInInlineMode,
}

impl fmt::Display for PromptDeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PromptDeliveryError::TooLarge(n) => {
                write!(
                    f,
                    "prompt exceeds {HARD_CAP_BYTES}-byte hard cap (was {n} bytes)"
                )
            }
            PromptDeliveryError::TempFile(e) => {
                write!(f, "failed to create temp file for prompt: {e}")
            }
            PromptDeliveryError::Write(e) => {
                write!(f, "failed to write prompt to temp file: {e}")
            }
            PromptDeliveryError::Permissions(e) => {
                write!(
                    f,
                    "failed to set 0o600 permissions on prompt temp file: {e}"
                )
            }
            PromptDeliveryError::NulInInlineMode => {
                f.write_str("prompt contains NUL byte but Inline mode was explicitly forced")
            }
        }
    }
}

impl std::error::Error for PromptDeliveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PromptDeliveryError::TempFile(e)
            | PromptDeliveryError::Write(e)
            | PromptDeliveryError::Permissions(e) => Some(e),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// FromStr parsing (env var + caller-supplied strings)
// ---------------------------------------------------------------------------

/// Marker error type for failed [`PromptDelivery::from_str`] parses. The
/// stored string is the offending input (after `trim` but before
/// case-folding) for diagnostic logging.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsePromptDeliveryError(pub String);

impl fmt::Display for ParsePromptDeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid PromptDelivery value: {:?}", self.0)
    }
}

impl std::error::Error for ParsePromptDeliveryError {}

impl FromStr for PromptDelivery {
    type Err = ParsePromptDeliveryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(ParsePromptDeliveryError(trimmed.to_string()));
        }
        // Match aliases via `eq_ignore_ascii_case` so we don't allocate a
        // lowercased copy on every env-var lookup (hot path on Auto mode).
        let m = |alias: &str| trimmed.eq_ignore_ascii_case(alias);
        if m("auto") {
            Ok(PromptDelivery::Auto)
        } else if m("inline") || m("cli") || m("argv") || m("arg") {
            Ok(PromptDelivery::Inline)
        } else if m("stdin") || m("pipe") {
            Ok(PromptDelivery::Stdin)
        } else if m("tempfile") || m("temp-file") || m("temp_file") || m("file") {
            Ok(PromptDelivery::TempFile)
        } else {
            Err(ParsePromptDeliveryError(trimmed.to_string()))
        }
    }
}

// ---------------------------------------------------------------------------
// select_mode (pure heuristic)
// ---------------------------------------------------------------------------

/// Pure heuristic. Resolves an effective delivery mode from prompt length,
/// caller-supplied mode, and (only for `Auto`) the value of [`ENV_OVERRIDE`].
///
/// Resolution order (each step is a hard invariant evaluated *before* the
/// next — see `docs/prompt-delivery.md` § Auto-selection heuristic):
///
/// 1. Size cap: `prompt.len() > HARD_CAP_BYTES` → `Err(TooLarge)`.
/// 2. Caller override: any non-`Auto` variant wins. Inline + NUL → `Err(NulInInlineMode)`.
/// 3. Env override: only when caller is `Auto`. Invalid values warn-once and
///    fall back to step 4.
/// 4. NUL fork: prompt contains a `0x00` byte → `Stdin`.
/// 5. `len < INLINE_MAX_BYTES` → `Inline`.
/// 6. `len < STDIN_PREFERRED_MAX_BYTES` → `Stdin`.
/// 7. Otherwise → `TempFile`.
pub fn select_mode(
    prompt: &[u8],
    caller: PromptDelivery,
) -> Result<PromptDelivery, PromptDeliveryError> {
    // Step 1 — hard cap fires before any override is considered.
    if prompt.len() > HARD_CAP_BYTES {
        return Err(PromptDeliveryError::TooLarge(prompt.len()));
    }

    // Step 2 — caller-supplied non-Auto mode wins.
    match caller {
        PromptDelivery::Inline => {
            if prompt.contains(&0u8) {
                return Err(PromptDeliveryError::NulInInlineMode);
            }
            return Ok(PromptDelivery::Inline);
        }
        PromptDelivery::Stdin => return Ok(PromptDelivery::Stdin),
        PromptDelivery::TempFile => return Ok(PromptDelivery::TempFile),
        PromptDelivery::Auto => {}
    }

    // Step 3 — env override (only consulted for Auto).
    if let Ok(raw) = std::env::var(ENV_OVERRIDE) {
        match PromptDelivery::from_str(&raw) {
            Ok(PromptDelivery::Auto) => {
                // explicit "auto" → fall through to heuristic
            }
            Ok(PromptDelivery::Inline) => {
                if prompt.contains(&0u8) {
                    return Err(PromptDeliveryError::NulInInlineMode);
                }
                return Ok(PromptDelivery::Inline);
            }
            Ok(mode @ (PromptDelivery::Stdin | PromptDelivery::TempFile)) => return Ok(mode),
            Err(_) => warn_invalid_env_value_once(&raw),
        }
    }

    // Step 4 — NUL fork (Auto only): silently route to stdin.
    if prompt.contains(&0u8) {
        return Ok(PromptDelivery::Stdin);
    }

    // Steps 5–7 — size-based heuristic. Boundaries are exclusive upper
    // bounds so the threshold values themselves promote to the next mode.
    if prompt.len() < INLINE_MAX_BYTES {
        Ok(PromptDelivery::Inline)
    } else if prompt.len() < STDIN_PREFERRED_MAX_BYTES {
        Ok(PromptDelivery::Stdin)
    } else {
        Ok(PromptDelivery::TempFile)
    }
}

// ---------------------------------------------------------------------------
// AppliedPromptStd (sync RAII guard)
// ---------------------------------------------------------------------------

/// RAII guard returned by [`apply_std`]. Owns the temp file (if any) and the
/// in-memory prompt bytes (for non-`Inline` modes). MUST outlive the spawned
/// child process or the temp file may be unlinked prematurely.
#[must_use = "AppliedPromptStd must outlive the child process or the temp file may unlink prematurely"]
pub struct AppliedPromptStd {
    mode: PromptDelivery,
    /// Owned prompt bytes for stdin delivery. `None` for `Inline` (the bytes
    /// already live in argv) or after the buffer has been consumed by
    /// `feed`.
    prompt: Option<Vec<u8>>,
    /// Postmortem temp file. `Some` for `TempFile` mode (until consumed by
    /// `retain_temp_file`). Drop unlinks the file on disk.
    temp_file: Option<NamedTempFile>,
}

impl AppliedPromptStd {
    /// The mode actually selected (after size cap + caller / env override).
    pub fn mode(&self) -> PromptDelivery {
        self.mode
    }

    /// Path to the temp file, if mode == `TempFile`. `None` otherwise.
    pub fn temp_path(&self) -> Option<&Path> {
        self.temp_file.as_ref().map(|f| f.path())
    }

    /// Write the owned prompt buffer to the child's stdin and consume the
    /// guard. No-op for `Inline` mode (returns `Ok(())`). Closes stdin on
    /// completion so the child reads EOF.
    pub fn feed(mut self, stdin: Option<ChildStdin>) -> std::io::Result<()> {
        let bytes = match self.prompt.take() {
            Some(b) => b,
            // Inline mode (no buffer captured) → nothing to write.
            None => return Ok(()),
        };
        let mut sink = stdin.ok_or_else(|| {
            std::io::Error::other(
                "AppliedPromptStd::feed requires the child's stdin pipe \
                 (set Command::stdin(Stdio::piped()) before spawning)",
            )
        })?;
        sink.write_all(&bytes)?;
        sink.flush()?;
        // `sink` drops at end of scope, closing the pipe so the child reads EOF.
        Ok(())
    }

    /// Detach the temp file from RAII cleanup. Returns the retained path so
    /// the caller can attach it to a bug report.
    pub fn retain_temp_file(mut self) -> Option<PathBuf> {
        let file = self.temp_file.take()?;
        // `keep()` disables the auto-unlink Drop and returns the path.
        file.keep().ok().map(|(_file, path)| path)
    }
}

// ---------------------------------------------------------------------------
// AppliedPromptTokio (async RAII guard)
// ---------------------------------------------------------------------------

/// Async sibling of [`AppliedPromptStd`].
#[must_use = "AppliedPromptTokio must outlive the child process or the temp file may unlink prematurely"]
pub struct AppliedPromptTokio {
    mode: PromptDelivery,
    prompt: Option<Vec<u8>>,
    temp_file: Option<NamedTempFile>,
}

impl AppliedPromptTokio {
    pub fn mode(&self) -> PromptDelivery {
        self.mode
    }

    pub fn temp_path(&self) -> Option<&Path> {
        self.temp_file.as_ref().map(|f| f.path())
    }

    pub async fn feed(mut self, stdin: Option<tokio::process::ChildStdin>) -> std::io::Result<()> {
        let bytes = match self.prompt.take() {
            Some(b) => b,
            None => return Ok(()),
        };
        let mut sink = stdin.ok_or_else(|| {
            std::io::Error::other(
                "AppliedPromptTokio::feed requires the child's stdin pipe \
                 (set Command::stdin(Stdio::piped()) before spawning)",
            )
        })?;
        sink.write_all(&bytes).await?;
        sink.flush().await?;
        sink.shutdown().await?;
        // `sink` drops at end of scope, closing the pipe so the child reads EOF.
        Ok(())
    }

    pub fn retain_temp_file(mut self) -> Option<PathBuf> {
        let file = self.temp_file.take()?;
        file.keep().ok().map(|(_file, path)| path)
    }
}

// ---------------------------------------------------------------------------
// apply_std / apply_tokio
// ---------------------------------------------------------------------------

/// Apply a [`PromptDelivery`] mode to a [`std::process::Command`] and return
/// an [`AppliedPromptStd`] guard. The command is mutated in place:
///
/// * `Inline`: `--` terminator (if not already present) + prompt as final argv.
/// * `Stdin`: `cmd.stdin(Stdio::piped())`; bytes written by `feed`.
/// * `TempFile`: `cmd.stdin(Stdio::piped())` + `NamedTempFile` (0o600) holding
///   the prompt bytes for postmortem inspection; bytes also written to stdin
///   by `feed`.
pub fn apply_std(
    cmd: &mut Command,
    prompt: &[u8],
    mode: PromptDelivery,
) -> Result<AppliedPromptStd, PromptDeliveryError> {
    let resolved = select_mode(prompt, mode)?;
    match resolved {
        PromptDelivery::Inline => {
            // Inject `--` flag terminator (unless caller already supplied one)
            // and append the prompt as the final positional argument.
            if !args_contain_flag_terminator(cmd.get_args()) {
                cmd.arg("--");
            }
            cmd.arg(prompt_as_osstr(prompt));
            Ok(AppliedPromptStd {
                mode: PromptDelivery::Inline,
                prompt: None,
                temp_file: None,
            })
        }
        PromptDelivery::Stdin => {
            cmd.stdin(std::process::Stdio::piped());
            Ok(AppliedPromptStd {
                mode: PromptDelivery::Stdin,
                prompt: Some(prompt.to_vec()),
                temp_file: None,
            })
        }
        PromptDelivery::TempFile => {
            let file = build_postmortem_tempfile(prompt)?;
            cmd.stdin(std::process::Stdio::piped());
            Ok(AppliedPromptStd {
                mode: PromptDelivery::TempFile,
                prompt: Some(prompt.to_vec()),
                temp_file: Some(file),
            })
        }
        // `Auto` is fully resolved by `select_mode`; this branch is
        // unreachable but keeps the match exhaustive against the
        // `non_exhaustive` enum.
        PromptDelivery::Auto => unreachable!("select_mode never returns Auto"),
    }
}

/// Async sibling of [`apply_std`].
pub async fn apply_tokio(
    cmd: &mut tokio::process::Command,
    prompt: &[u8],
    mode: PromptDelivery,
) -> Result<AppliedPromptTokio, PromptDeliveryError> {
    let resolved = select_mode(prompt, mode)?;
    match resolved {
        PromptDelivery::Inline => {
            // Note: tokio::process::Command does not expose `get_args()`, so
            // we always inject `--` here. Callers wanting to skip the
            // injection should pass a non-Inline mode or strip the
            // duplicate `--` themselves.
            cmd.arg("--");
            cmd.arg(prompt_as_osstr(prompt));
            Ok(AppliedPromptTokio {
                mode: PromptDelivery::Inline,
                prompt: None,
                temp_file: None,
            })
        }
        PromptDelivery::Stdin => {
            cmd.stdin(std::process::Stdio::piped());
            Ok(AppliedPromptTokio {
                mode: PromptDelivery::Stdin,
                prompt: Some(prompt.to_vec()),
                temp_file: None,
            })
        }
        PromptDelivery::TempFile => {
            let file = build_postmortem_tempfile(prompt)?;
            cmd.stdin(std::process::Stdio::piped());
            Ok(AppliedPromptTokio {
                mode: PromptDelivery::TempFile,
                prompt: Some(prompt.to_vec()),
                temp_file: Some(file),
            })
        }
        PromptDelivery::Auto => unreachable!("select_mode never returns Auto"),
    }
}

// ===========================================================================
// Inline unit tests
// ===========================================================================
//
// These tests pin the *pure* parts of the contract:
//   * `PromptDelivery::from_str` alias grammar
//   * `select_mode` resolution order
//   * Constant values (so a refactor renaming `INLINE_MAX_BYTES` doesn't
//     silently change `Auto` behaviour)
//
// I/O-bearing behaviour (spawn round-trips, temp-file perms, drop cleanup)
// lives in `tests/prompt_delivery.rs` so it can use `serial_test` and
// `#[cfg(unix)]` gating cleanly.

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // -----------------------------------------------------------------------
    // Constants
    // -----------------------------------------------------------------------

    #[test]
    fn constants_match_design_spec() {
        // Pinned by docs/prompt-delivery.md § Auto-selection heuristic.
        assert_eq!(INLINE_MAX_BYTES, 8 * 1024);
        assert_eq!(STDIN_PREFERRED_MAX_BYTES, 100 * 1024);
        assert_eq!(HARD_CAP_BYTES, 16 * 1024 * 1024);
        assert_eq!(ENV_OVERRIDE, "AMPLIHACK_PROMPT_DELIVERY");
    }

    // -----------------------------------------------------------------------
    // PromptDelivery::from_str — case-insensitive alias grammar (plan D3)
    // -----------------------------------------------------------------------

    #[test]
    fn from_str_accepts_auto() {
        assert_eq!(
            "auto".parse::<PromptDelivery>().unwrap(),
            PromptDelivery::Auto
        );
        assert_eq!(
            "AUTO".parse::<PromptDelivery>().unwrap(),
            PromptDelivery::Auto
        );
        assert_eq!(
            "  Auto  ".parse::<PromptDelivery>().unwrap(),
            PromptDelivery::Auto
        );
    }

    #[test]
    fn from_str_accepts_inline_aliases() {
        for v in ["inline", "Inline", "INLINE", "cli", "CLI", "argv", "arg"] {
            assert_eq!(
                v.parse::<PromptDelivery>().unwrap(),
                PromptDelivery::Inline,
                "alias {v:?} should parse to Inline"
            );
        }
    }

    #[test]
    fn from_str_accepts_stdin_aliases() {
        for v in ["stdin", "STDIN", "Stdin", "pipe", "PIPE"] {
            assert_eq!(
                v.parse::<PromptDelivery>().unwrap(),
                PromptDelivery::Stdin,
                "alias {v:?} should parse to Stdin"
            );
        }
    }

    #[test]
    fn from_str_accepts_tempfile_aliases() {
        for v in [
            "tempfile",
            "TempFile",
            "temp-file",
            "TEMP-FILE",
            "file",
            "FILE",
        ] {
            assert_eq!(
                v.parse::<PromptDelivery>().unwrap(),
                PromptDelivery::TempFile,
                "alias {v:?} should parse to TempFile"
            );
        }
    }

    #[test]
    fn from_str_rejects_unknown_values() {
        let err = "not-a-mode".parse::<PromptDelivery>().unwrap_err();
        assert_eq!(err.0, "not-a-mode");
    }

    #[test]
    fn from_str_rejects_empty_string() {
        assert!("".parse::<PromptDelivery>().is_err());
        assert!("   ".parse::<PromptDelivery>().is_err());
    }

    // -----------------------------------------------------------------------
    // select_mode — resolution order
    // -----------------------------------------------------------------------

    #[test]
    fn select_mode_size_cap_fires_before_caller_override() {
        // Step 1 of the heuristic: size cap is a hard invariant evaluated
        // before any override. Even an explicit Inline must yield TooLarge.
        let too_big = vec![b'a'; HARD_CAP_BYTES + 1];
        assert!(matches!(
            select_mode(&too_big, PromptDelivery::Inline),
            Err(PromptDeliveryError::TooLarge(_))
        ));
        assert!(matches!(
            select_mode(&too_big, PromptDelivery::Stdin),
            Err(PromptDeliveryError::TooLarge(_))
        ));
        assert!(matches!(
            select_mode(&too_big, PromptDelivery::TempFile),
            Err(PromptDeliveryError::TooLarge(_))
        ));
        assert!(matches!(
            select_mode(&too_big, PromptDelivery::Auto),
            Err(PromptDeliveryError::TooLarge(_))
        ));
    }

    #[test]
    fn select_mode_at_hard_cap_is_allowed() {
        // Cap is exclusive — len == HARD_CAP_BYTES is OK.
        let exactly_cap = vec![b'a'; HARD_CAP_BYTES];
        let mode = select_mode(&exactly_cap, PromptDelivery::Auto).unwrap();
        // Past STDIN_PREFERRED_MAX_BYTES → TempFile.
        assert_eq!(mode, PromptDelivery::TempFile);
    }

    #[test]
    fn select_mode_caller_override_short_prompts() {
        // Step 2: caller-supplied non-Auto mode wins (when not size-capped).
        let prompt = b"hello";
        assert_eq!(
            select_mode(prompt, PromptDelivery::Inline).unwrap(),
            PromptDelivery::Inline
        );
        assert_eq!(
            select_mode(prompt, PromptDelivery::Stdin).unwrap(),
            PromptDelivery::Stdin
        );
        assert_eq!(
            select_mode(prompt, PromptDelivery::TempFile).unwrap(),
            PromptDelivery::TempFile
        );
    }

    #[test]
    fn select_mode_inline_with_nul_byte_errors() {
        // Step 2 corner: explicit Inline + embedded NUL → NulInInlineMode.
        let prompt = b"hello\0world";
        let err = select_mode(prompt, PromptDelivery::Inline).unwrap_err();
        assert!(matches!(err, PromptDeliveryError::NulInInlineMode));
    }

    #[test]
    fn select_mode_auto_nul_forks_to_stdin() {
        // Step 4: NUL fork in Auto mode → Stdin, never Inline.
        let prompt = b"hello\0world";
        assert_eq!(
            select_mode(prompt, PromptDelivery::Auto).unwrap(),
            PromptDelivery::Stdin
        );
    }

    #[test]
    fn select_mode_auto_inline_threshold() {
        // Boundary at INLINE_MAX_BYTES (exclusive upper bound for Inline).
        let just_under = vec![b'a'; INLINE_MAX_BYTES - 1];
        assert_eq!(
            select_mode(&just_under, PromptDelivery::Auto).unwrap(),
            PromptDelivery::Inline
        );

        let exactly = vec![b'a'; INLINE_MAX_BYTES];
        assert_eq!(
            select_mode(&exactly, PromptDelivery::Auto).unwrap(),
            PromptDelivery::Stdin,
            "len == INLINE_MAX_BYTES must promote to Stdin (boundary is exclusive)"
        );
    }

    #[test]
    fn select_mode_auto_stdin_threshold() {
        // Boundary at STDIN_PREFERRED_MAX_BYTES.
        let just_under = vec![b'a'; STDIN_PREFERRED_MAX_BYTES - 1];
        assert_eq!(
            select_mode(&just_under, PromptDelivery::Auto).unwrap(),
            PromptDelivery::Stdin
        );

        let exactly = vec![b'a'; STDIN_PREFERRED_MAX_BYTES];
        assert_eq!(
            select_mode(&exactly, PromptDelivery::Auto).unwrap(),
            PromptDelivery::TempFile,
            "len == STDIN_PREFERRED_MAX_BYTES must promote to TempFile"
        );
    }

    #[test]
    fn select_mode_auto_empty_prompt_inline() {
        // Empty prompt is permitted; lands in Inline branch (smallest).
        assert_eq!(
            select_mode(b"", PromptDelivery::Auto).unwrap(),
            PromptDelivery::Inline
        );
    }

    // -----------------------------------------------------------------------
    // Env override resolution (requires mutating env; serial_test enforced)
    // -----------------------------------------------------------------------

    #[test]
    #[serial(prompt_delivery_env)]
    #[cfg_attr(miri, ignore)]
    fn select_mode_env_override_only_for_auto() {
        // SAFETY (Rust 2024): set_var / remove_var require `unsafe` because
        // they are not thread-safe; `#[serial]` ensures no other test in
        // this serial group runs concurrently.
        unsafe {
            std::env::set_var(ENV_OVERRIDE, "stdin");
        }

        let short = b"hi";
        assert_eq!(
            select_mode(short, PromptDelivery::Auto).unwrap(),
            PromptDelivery::Stdin,
            "Auto must honor AMPLIHACK_PROMPT_DELIVERY=stdin"
        );
        // Non-Auto caller mode: env ignored.
        assert_eq!(
            select_mode(short, PromptDelivery::Inline).unwrap(),
            PromptDelivery::Inline,
            "explicit Inline must bypass env var"
        );

        unsafe {
            std::env::set_var(ENV_OVERRIDE, "tempfile");
        }
        assert_eq!(
            select_mode(short, PromptDelivery::Auto).unwrap(),
            PromptDelivery::TempFile
        );

        // Invalid value → warn + fall back to Auto heuristic.
        unsafe {
            std::env::set_var(ENV_OVERRIDE, "bogus-value-xyz");
        }
        assert_eq!(
            select_mode(short, PromptDelivery::Auto).unwrap(),
            PromptDelivery::Inline,
            "invalid env value must fall back to Auto heuristic, not panic"
        );

        unsafe {
            std::env::remove_var(ENV_OVERRIDE);
        }
    }

    // -----------------------------------------------------------------------
    // Error enum — all variants present and Display-formattable
    // -----------------------------------------------------------------------

    #[test]
    fn error_variants_have_display_messages() {
        use std::io;
        let cases: Vec<PromptDeliveryError> = vec![
            PromptDeliveryError::TooLarge(123),
            PromptDeliveryError::TempFile(io::Error::other("x")),
            PromptDeliveryError::Write(io::Error::other("y")),
            PromptDeliveryError::Permissions(io::Error::other("z")),
            PromptDeliveryError::NulInInlineMode,
        ];
        for e in cases {
            let msg = format!("{e}");
            assert!(
                !msg.is_empty(),
                "Display impl produced empty string for {e:?}"
            );
        }
    }

    #[test]
    fn error_source_present_for_io_variants() {
        use std::error::Error;
        use std::io;
        let e = PromptDeliveryError::TempFile(io::Error::other("inner"));
        assert!(e.source().is_some());
        let e = PromptDeliveryError::NulInInlineMode;
        assert!(e.source().is_none());
    }
}
