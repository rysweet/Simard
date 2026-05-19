//! Subprocess prompt delivery — TDD stub (issue #1897).
//!
//! Status: **scaffold only**. Every public function and method here is a
//! stub returning [`unimplemented!()`]. The full contract is documented in
//! [`docs/prompt-delivery.md`](../../docs/prompt-delivery.md); the tests in
//! [`tests/prompt_delivery.rs`](../../tests/prompt_delivery.rs) plus the
//! inline `#[cfg(test)] mod tests` block below pin every behaviour the
//! implementation must satisfy.
//!
//! Per the TDD ordering decision (plan D10), this file is committed in its
//! stub state alongside the failing tests. The implementation commit comes
//! next and removes every `unimplemented!()` below.
//!
//! ## Public surface (pinned by tests)
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
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command};
use std::str::FromStr;

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

    fn from_str(_s: &str) -> Result<Self, Self::Err> {
        unimplemented!("PromptDelivery::from_str stub — see docs/prompt-delivery.md")
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
    _prompt: &[u8],
    _caller: PromptDelivery,
) -> Result<PromptDelivery, PromptDeliveryError> {
    unimplemented!("select_mode stub — see docs/prompt-delivery.md")
}

// ---------------------------------------------------------------------------
// AppliedPromptStd (sync RAII guard)
// ---------------------------------------------------------------------------

/// RAII guard returned by [`apply_std`]. Owns the temp file (if any) and the
/// in-memory prompt bytes (for non-`Inline` modes). MUST outlive the spawned
/// child process or the temp file may be unlinked prematurely.
#[must_use = "AppliedPromptStd must outlive the child process or the temp file may unlink prematurely"]
pub struct AppliedPromptStd {
    // Private fields will be filled in by the implementation commit.
    _private: (),
}

impl AppliedPromptStd {
    /// The mode actually selected (after size cap + caller / env override).
    pub fn mode(&self) -> PromptDelivery {
        unimplemented!("AppliedPromptStd::mode stub")
    }

    /// Path to the temp file, if mode == `TempFile`. `None` otherwise.
    pub fn temp_path(&self) -> Option<&Path> {
        unimplemented!("AppliedPromptStd::temp_path stub")
    }

    /// Write the owned prompt buffer to the child's stdin and consume the
    /// guard. No-op for `Inline` mode (returns `Ok(())`). Closes stdin on
    /// completion so the child reads EOF.
    pub fn feed(self, _stdin: Option<ChildStdin>) -> std::io::Result<()> {
        unimplemented!("AppliedPromptStd::feed stub")
    }

    /// Detach the temp file from RAII cleanup. Returns the retained path so
    /// the caller can attach it to a bug report. **Caller must
    /// `std::fs::remove_file` the path once the postmortem capture is
    /// finished** — opt-in retention disables Drop unlink.
    pub fn retain_temp_file(self) -> Option<PathBuf> {
        unimplemented!("AppliedPromptStd::retain_temp_file stub")
    }
}

// ---------------------------------------------------------------------------
// AppliedPromptTokio (async RAII guard)
// ---------------------------------------------------------------------------

/// Async sibling of [`AppliedPromptStd`].
#[must_use = "AppliedPromptTokio must outlive the child process or the temp file may unlink prematurely"]
pub struct AppliedPromptTokio {
    _private: (),
}

impl AppliedPromptTokio {
    pub fn mode(&self) -> PromptDelivery {
        unimplemented!("AppliedPromptTokio::mode stub")
    }

    pub fn temp_path(&self) -> Option<&Path> {
        unimplemented!("AppliedPromptTokio::temp_path stub")
    }

    pub async fn feed(self, _stdin: Option<tokio::process::ChildStdin>) -> std::io::Result<()> {
        unimplemented!("AppliedPromptTokio::feed stub")
    }

    pub fn retain_temp_file(self) -> Option<PathBuf> {
        unimplemented!("AppliedPromptTokio::retain_temp_file stub")
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
    _cmd: &mut Command,
    _prompt: &[u8],
    _mode: PromptDelivery,
) -> Result<AppliedPromptStd, PromptDeliveryError> {
    unimplemented!("apply_std stub — see docs/prompt-delivery.md")
}

/// Async sibling of [`apply_std`].
pub async fn apply_tokio(
    _cmd: &mut tokio::process::Command,
    _prompt: &[u8],
    _mode: PromptDelivery,
) -> Result<AppliedPromptTokio, PromptDeliveryError> {
    unimplemented!("apply_tokio stub — see docs/prompt-delivery.md")
}

// ===========================================================================
// Inline unit tests (TDD red phase)
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
