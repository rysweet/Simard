//! Structured root-cause diagnosis of a failed decision-cycle / engineer /
//! terminal-shell step (issue #2640, PART 2).
//!
//! The operator ask: when one of these steps fails, Simard must NOT merely LOG
//! the error and move on — she must INSPECT and DIAGNOSE *why* it happened, then
//! drive a corrective action. This module is the thin, structured trigger for
//! that: it classifies a raw `(ExitStatus, transcript)` failure into a typed
//! [`FailureCause`] + [`FailureDiagnosis`]. The agentic "WHY + remedy" reasoning
//! lives in the `prompt_assets/simard/overseer/self_diagnose.md` prompt asset
//! (guideline G3: agentic over brittle heuristics); this classifier only decides
//! *which* failure mode fired so the Overseer can route a corrective workstream.
//!
//! The classification is deliberately transcript-first (it reads the shell's own
//! diagnostic) with exit-code fallbacks, so the headline live defect — exit 126
//! carrying the kernel's "Argument list too long" (E2BIG) — is diagnosed as
//! [`FailureCause::ArgListTooLong`] rather than a bare "not executable".

use std::process::ExitStatus;

use serde::Serialize;

/// A classified root cause of a failed step — the "WHY" behind the failure.
///
/// `#[non_exhaustive]` so new causes can be added without breaking downstream
/// matches; every variant carries a stable [`FailureCause::as_str`] label used in
/// logs, signals, dedup keys, and JSON.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureCause {
    /// `exec` failed with `E2BIG` — an argv token (typically the prompt) exceeded
    /// the kernel's `ARG_MAX` ("Argument list too long", surfaced as exit 126).
    /// The exact live defect PART 1 fixes.
    ArgListTooLong,
    /// A command was not found on `PATH` (exit 127 / "command not found").
    CommandNotFound,
    /// A command was found but is not executable / permission was denied
    /// (exit 126 / "Permission denied").
    PermissionDenied,
    /// The filesystem is out of space (`ENOSPC` / "No space left on device").
    DiskFull,
    /// The process ran out of memory / was OOM-killed.
    OutOfMemory,
    /// A network / DNS / auth failure (could not resolve host, connection
    /// refused/unreachable/timed out, or authentication rejected).
    NetworkOrAuth,
    /// No known cause matched. Still recorded structurally so the failure is
    /// never a silent drop — the agentic diagnostic step reasons over it.
    Unknown,
    /// A cognitive-thread tick returned failure or panicked inside the [`Mind`]
    /// scheduler (issue #4786). Recorded durably so a caught thread error flows
    /// to the Overseer as a corrective signal instead of being swallowed.
    CognitiveThread,
    /// A fail-CLOSED thread-reasoning rail reader (e.g.
    /// `read_verified_thread_reasoning`) found no record at its expected path —
    /// the "R1" absent-record branch, surfaced as `ENOENT`
    /// ("No such file or directory (os error 2)"). Issue #4986: a reflective
    /// recipe that exited 0 without writing its typed reasoning record trips
    /// this. Classified distinctly (not the catch-all [`Unknown`]) so a
    /// recurrence self-diagnoses to a clear, actionable cause.
    MissingReasoningRecord,
}

impl FailureCause {
    /// Stable kebab-case label used in logs, signals, dedup keys, and JSON. This
    /// is the single source of truth for the serialised form.
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureCause::ArgListTooLong => "arg-list-too-long",
            FailureCause::CommandNotFound => "command-not-found",
            FailureCause::PermissionDenied => "permission-denied",
            FailureCause::DiskFull => "disk-full",
            FailureCause::OutOfMemory => "out-of-memory",
            FailureCause::NetworkOrAuth => "network-or-auth",
            FailureCause::Unknown => "unknown",
            FailureCause::CognitiveThread => "cognitive-thread",
            FailureCause::MissingReasoningRecord => "missing-reasoning-record",
        }
    }
}

impl std::fmt::Display for FailureCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for FailureCause {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Serialise as the stable kebab-case label so the cause travels on the
        // Overseer activity feed / structured log verbatim (never a bare int).
        serializer.serialize_str(self.as_str())
    }
}

/// A structured, serialisable diagnosis of one failed step — the record the
/// Overseer acts on instead of a formatted log line.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FailureDiagnosis {
    /// The classified root cause.
    pub cause: FailureCause,
    /// The process exit code, when the step exited (`None` if signal-terminated).
    pub exit_code: Option<i32>,
    /// A bounded, single-line excerpt of the failure evidence (the shell's own
    /// diagnostic / last terminal output) the classification rested on. Bounded
    /// at [`MAX_EVIDENCE_LEN`] so a pathological transcript can never inflate the
    /// sink, a notification, or a log line.
    pub evidence: String,
}

/// Maximum evidence excerpt retained in a diagnosis. Bounds memory and the size
/// of any downstream notification / issue body / log line built from it.
pub const MAX_EVIDENCE_LEN: usize = 400;

/// Classify a raw terminal-shell / decision-cycle / engineer step failure into a
/// structured [`FailureDiagnosis`]. Never panics and never drops silently — an
/// unrecognised failure classifies as [`FailureCause::Unknown`] with the same
/// bounded evidence so the caller always records *something* structured.
pub fn classify_terminal_failure(status: &ExitStatus, transcript: &str) -> FailureDiagnosis {
    let exit_code = status.code();
    FailureDiagnosis {
        cause: classify_cause(exit_code, transcript),
        exit_code,
        evidence: bounded_evidence(transcript),
    }
}

/// Classify a **pre-exec spawn** failure — an [`std::io::Error`] returned by
/// `Command::output()`/`spawn()` BEFORE any child process exists — into a
/// structured [`FailureDiagnosis`] (issues #2640/#2692).
///
/// The live journal defect surfaces here, not in [`classify_terminal_failure`]:
/// when the inlined `-c day_context=<…>` argv token exceeds `ARG_MAX`, `execve`
/// fails with `E2BIG` (`errno 7`) and the runner never runs, so there is no
/// [`ExitStatus`] and no transcript for the exit-code classifier to read. This
/// sibling keys off the OS errno first (the authoritative signal), with a
/// message-string fallback for platforms/wrappers that surface no numeric errno.
///
/// A spawn failure has no child, so `exit_code` is always `None`. Never panics
/// and never drops silently: an unmapped errno classifies as
/// [`FailureCause::Unknown`] with the same bounded evidence, so the caller always
/// records *something* structured for the Overseer to act on.
pub fn classify_spawn_failure(err: &std::io::Error) -> FailureDiagnosis {
    FailureDiagnosis {
        cause: classify_spawn_cause(err),
        exit_code: None,
        evidence: bounded_spawn_evidence(&err.to_string()),
    }
}

/// Pure classification of a spawn [`std::io::Error`]: errno first (E2BIG=7,
/// ENOSPC=28, ENOMEM=12), then a message-string fallback for the E2BIG marker
/// when no numeric errno is present. Any other errno is a structured
/// [`FailureCause::Unknown`] — never a silent drop.
fn classify_spawn_cause(err: &std::io::Error) -> FailureCause {
    if let Some(errno) = err.raw_os_error() {
        match errno {
            // E2BIG — the exact journal defect: an argv token exceeded ARG_MAX.
            7 => return FailureCause::ArgListTooLong,
            // ENOSPC — the temp-file write for the file-channel could fail here.
            28 => return FailureCause::DiskFull,
            // ENOMEM — the host could not allocate to fork/exec the child.
            12 => return FailureCause::OutOfMemory,
            _ => {}
        }
    }
    // Fallback for errors that carry no numeric errno (e.g. a wrapped message):
    // still catch the E2BIG marker so the headline cause is never missed.
    let lower = err.to_string().to_ascii_lowercase();
    if lower.contains("argument list too long") || lower.contains("e2big") {
        return FailureCause::ArgListTooLong;
    }
    FailureCause::Unknown
}

/// Pure classification: transcript markers first (the shell's own diagnostic),
/// then well-known exit-code fallbacks. First match wins; ordering matters so
/// exit-126 + "Argument list too long" diagnoses as E2BIG, not "not executable".
fn classify_cause(exit_code: Option<i32>, transcript: &str) -> FailureCause {
    let lower = transcript.to_ascii_lowercase();
    let has = |needle: &str| lower.contains(needle);

    // The headline live defect: E2BIG. The arg-list marker wins over the bare
    // exit-126 "not executable" reading.
    if has("argument list too long") || has("e2big") {
        return FailureCause::ArgListTooLong;
    }
    // Disk full (ENOSPC).
    if has("no space left on device") || has("enospc") {
        return FailureCause::DiskFull;
    }
    // Out of memory / OOM-kill.
    if has("out of memory") || has("oom-kill") || has("oom killer") || has("killed process") {
        return FailureCause::OutOfMemory;
    }
    // Network / DNS / auth.
    if has("could not resolve host")
        || has("temporary failure in name resolution")
        || has("connection refused")
        || has("network is unreachable")
        || has("no route to host")
        || has("connection timed out")
        || has("authentication failed")
        || has("could not read username")
    {
        return FailureCause::NetworkOrAuth;
    }
    // Absent thread-reasoning record (issue #4986). A fail-CLOSED rail reader
    // (e.g. `read_verified_thread_reasoning`) emits its own transcript marker
    // when the typed record is missing at the expected path. Keyed on the
    // reader's specific strings — the human "no record at expected path" and the
    // Rust `ENOENT` rendering "(os error 2)" — both specific enough not to
    // collide with a shell's bare "No such file or directory". Transcript-marker
    // first (module doctrine), so it wins over a bare exit-code hint.
    if has("no record at expected path") || has("no such file or directory (os error 2)") {
        return FailureCause::MissingReasoningRecord;
    }
    // Command not found (marker or the canonical exit 127).
    if has("command not found") || exit_code == Some(127) {
        return FailureCause::CommandNotFound;
    }
    // Permission denied / not executable (marker or the canonical exit 126).
    if has("permission denied") || exit_code == Some(126) {
        return FailureCause::PermissionDenied;
    }
    // An OOM-kill often surfaces as exit 137 (128 + SIGKILL) with no marker;
    // treat that as OOM only as a last-resort code hint.
    if exit_code == Some(137) {
        return FailureCause::OutOfMemory;
    }
    FailureCause::Unknown
}

/// Build a bounded, single-line evidence excerpt from the transcript, preferring
/// the tail (where the "last terminal output" / shell diagnostic lives).
fn bounded_evidence(transcript: &str) -> String {
    let one_line = transcript.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX_EVIDENCE_LEN {
        return one_line;
    }
    let tail: String = {
        let chars: Vec<char> = one_line.chars().collect();
        chars[chars.len() - MAX_EVIDENCE_LEN..].iter().collect()
    };
    format!("…{tail}")
}

/// Build a bounded, single-line excerpt of a spawn [`std::io::Error`] message,
/// capped so the WHOLE string (ellipsis included) never exceeds
/// [`MAX_EVIDENCE_LEN`]. Unlike [`bounded_evidence`] (which keeps the transcript
/// tail and may run one char over with its leading ellipsis), an io-error
/// message is short and front-loaded — the cause is at the start — so we keep the
/// head and cap the total length exactly.
fn bounded_spawn_evidence(message: &str) -> String {
    let one_line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX_EVIDENCE_LEN {
        return one_line;
    }
    let head: String = one_line.chars().take(MAX_EVIDENCE_LEN - 1).collect();
    format!("{head}…")
}
