//! src/journal/tests_clean_result_channel.rs
//!
//! RED regression tests for the journal narrative CLEAN-RESULT-CHANNEL fix
//! (bug #2679 — "the Simard journal's FIRST SECTION is polluted").
//!
//! ## The bug these tests pin
//!
//! The agentic narrative path in [`crate::journal::recipe`] captured
//! `recipe-runner-rs` **raw stdout** and stored it as the narrative. Raw stdout
//! carries the copilot launcher preamble (`WARN nested amplihack session`,
//! `INFO launching copilot binary=…`, `ℹ NODE_OPTIONS=…`) and the agent's own
//! tool-call trace (`● Read draft.ctx`, `│ …`, `└ 153 lines read`). All of that
//! garbage became the LEADING text of the stored journal, ahead of the real
//! prose — the exact raw-stdout-scrape antipattern already fixed for
//! distillation and for goal decomposition (issue #2708).
//!
//! ## The contract these tests specify
//!
//! The fix is the SAME clean-result-file channel proven in
//! [`crate::goal_curation::decompose`] (`harvest_subgoals_file`): the agent is
//! told a dedicated result-file path and writes ONLY its final report there, and
//! Simard reads the narrative from that **file** — never from stdout. These
//! tests drive a split-out, hermetically testable seam:
//!
//! ```ignore
//! pub(crate) fn harvest_narrative_file(
//!     output: &std::process::Output,
//!     path: &std::path::Path,
//! ) -> crate::error::SimardResult<String>
//! ```
//!
//! that reads the narrative from `path` and treats stdout as INERT. Because it
//! takes a fabricated [`std::process::Output`], the "stdout noise never leaks
//! into the narrative" contract is provable WITHOUT spawning a subprocess.
//!
//! These tests are RED until that seam exists in `src/journal/recipe.rs`
//! (exposed at least `pub(crate)` so this sibling test module can call it).
//!
//! No literal secret is committed: fixtures use only the launcher-banner /
//! tool-trace shapes from the live #2679 evidence and obviously-synthetic prose.

use crate::error::SimardError;
use crate::journal::recipe::harvest_narrative_file;

// ── The live #2679 contamination, verbatim in shape ─────────────────────────

/// The clean narrative the agent actually wrote — the ONLY thing that belongs in
/// the journal. Mirrors the real prose that (in the live bug) began only AFTER
/// the launcher/tool-trace garbage.
const CLEAN_NARRATIVE: &str = "On July 7, 2026, Simard operated in a largely \
self-directed decision cycle, advancing several engineering goals and folding \
what it learned into long-term memory.\n\n\
## Engineering work\n\n\
Simard shipped a fix for the journal's contaminated opening paragraph and \
verified it on the live dashboard.\n\n\
## Remembered moments\n\n\
- [2026-07-07 01:03 UTC] began the day's self-directed review";

/// A realistic slice of the LIVE #2679 raw `recipe-runner-rs` stdout: the
/// copilot launcher banner (nested-session WARN, launching-copilot INFO,
/// NODE_OPTIONS notice) followed by the agent's own box-drawing tool-call trace
/// for reading `draft.ctx`. Under the old raw-stdout-scrape code this whole blob
/// — banner + trace + prose — became the stored narrative, so its FIRST
/// paragraph was garbage. It must be completely INERT to the clean channel.
const CONTAMINATED_STDOUT: &str = "2026-07-07T01:03:03.969016Z  WARN nested \
amplihack session detected — launching anyway session_id=\"session-18bfdc2635821690\" depth=2\n\
2026-07-07T01:03:04.604657Z  INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot version=\"GitHub Copilot CLI 1.0.69-2.\"\n\
\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/azureuser/.amplihack/config\n\
\u{25CF} Read draft.ctx  \u{2502} /tmp/simard-journal-ctx-XXXX/draft.ctx  \u{2514} 153 lines read\n\
On July 7, 2026, Simard operated in a largely self-directed decision cycle...";

/// Every substring / glyph that proves launcher-banner or tool-trace
/// contamination. The stored narrative must contain NONE of these (deliverable
/// requirement #2).
const FORBIDDEN_MARKERS: &[&str] = &[
    "nested amplihack session",
    "launching copilot binary",
    "NODE_OPTIONS=",
    "Read draft.ctx",
    "lines read",
    "\u{25CF}", // ● black circle — tool-call bullet
    "\u{2502}", // │ box-drawing vertical — tool-trace gutter
    "\u{2514}", // └ box-drawing up-and-right — "N lines read" leader
];

/// Build a synthetic finished process result with the given stdout and exit
/// code, so `harvest_narrative_file`'s "stdout is inert" contract is provable
/// without a real subprocess. `code << 8` places `code` in the exit-code byte
/// (low byte 0 ⇒ not signalled), so `.success()` is true iff `code == 0`.
fn output_with(stdout: &[u8], code: i32) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}

/// Assert a narrative is clean: starts with the real prose and carries none of
/// the launcher-banner / tool-trace markers.
fn assert_clean_narrative(narrative: &str) {
    assert!(
        narrative.starts_with("On July 7, 2026, Simard operated"),
        "the stored narrative must BEGIN with the real prose, not launcher/tool-trace \
         garbage; got: {narrative:?}"
    );
    for marker in FORBIDDEN_MARKERS {
        assert!(
            !narrative.contains(marker),
            "the stored narrative leaked launcher/tool-trace marker {marker:?}: {narrative:?}"
        );
    }
}

/// Assert a loud journal failure: the clean-channel seam never degrades to a
/// silent success, and every failure is the same `AdapterInvocationFailed`
/// under the `journal` adapter tag.
fn assert_loud_journal_error(err: SimardError) {
    match err {
        SimardError::AdapterInvocationFailed { base_type, .. } => {
            assert_eq!(
                base_type, "journal",
                "a clean-channel failure must be attributed to the journal adapter"
            );
        }
        other => panic!("expected AdapterInvocationFailed{{base_type:\"journal\"}}, got {other:?}"),
    }
}

// ── Group A: `harvest_narrative_file` reads the clean result FILE and treats
//    stdout as inert (the #2679 fix). ──────────────────────────────────────────

/// THE headline #2679 regression. `recipe-runner-rs` stdout is the full live
/// contamination (launcher banner + box-drawing tool trace + a prose prefix),
/// yet the agent wrote ONLY clean prose to its dedicated result file. Harvest
/// MUST return the clean prose from the file, and NONE of the stdout garbage may
/// appear in it. This is exactly deliverable requirement #2.
#[test]
fn contaminated_stdout_never_pollutes_the_narrative() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("narrative_output.md");
    std::fs::write(&path, CLEAN_NARRATIVE).unwrap();

    let output = output_with(CONTAMINATED_STDOUT.as_bytes(), 0);
    let narrative = harvest_narrative_file(&output, &path)
        .expect("clean result file must be read even when stdout is full of launcher/tool noise");

    assert_clean_narrative(&narrative);
    assert_eq!(
        narrative, CLEAN_NARRATIVE,
        "the narrative must be the file's clean prose verbatim, not scraped stdout"
    );
}

/// A clean exit-0 run with empty stdout reads the narrative verbatim from the
/// file (trimmed), proving the file is the sole narrative source.
#[test]
fn clean_file_is_read_verbatim_and_trimmed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("narrative_output.md");
    std::fs::write(&path, format!("\n\n{CLEAN_NARRATIVE}\n\n")).unwrap();

    let output = output_with(b"", 0);
    let narrative =
        harvest_narrative_file(&output, &path).expect("a clean exit-0 run reads the result file");

    assert_eq!(
        narrative, CLEAN_NARRATIVE,
        "surrounding whitespace must be trimmed, leaving the exact prose"
    );
}

/// No silent fallback: a missing result file is a LOUD journal error even when
/// stdout carries a perfectly clean, well-formed narrative — proving stdout is
/// NEVER scraped as a fallback narrative channel (the antipattern this fix kills).
#[test]
fn missing_result_file_is_loud_error_never_stdout_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("narrative_output.md"); // deliberately never written

    // stdout is clean, complete prose — tempting to scrape. It must be ignored.
    let output = output_with(CLEAN_NARRATIVE.as_bytes(), 0);
    let err = harvest_narrative_file(&output, &path)
        .expect_err("a missing result file must be a loud error, never a stdout fallback");
    assert_loud_journal_error(err);
}

/// The agent created the result file but wrote nothing (or only whitespace): a
/// loud journal error, never a hollow empty narrative that would silently pass.
#[test]
fn empty_result_file_is_loud_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("narrative_output.md");
    std::fs::write(&path, "   \n\t  \n").unwrap();

    let output = output_with(b"", 0);
    let err = harvest_narrative_file(&output, &path)
        .expect_err("an empty/whitespace-only result file must surface loudly");
    assert_loud_journal_error(err);
}

/// A runaway agent that writes an oversized result file (> 1 MiB) is rejected by
/// the size guard BEFORE the read — a loud journal error, never an OOM. Mirrors
/// the decomposition seam's `MAX_SUBGOALS_FILE_BYTES` guard.
#[test]
fn oversized_result_file_is_loud_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("narrative_output.md");
    let oversized = vec![b'a'; 1024 * 1024 + 1];
    std::fs::write(&path, &oversized).unwrap();

    let output = output_with(b"", 0);
    let err = harvest_narrative_file(&output, &path)
        .expect_err("an oversized result file must be rejected loudly before the read");
    assert_loud_journal_error(err);
}

/// A non-zero recipe exit is a LOUD terminal journal failure — never a silent
/// success and never a scraped-stdout narrative. Even with clean prose sitting
/// in the file, a failed run must not be treated as success.
#[test]
fn nonzero_exit_is_loud_error_even_with_a_populated_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("narrative_output.md");
    std::fs::write(&path, CLEAN_NARRATIVE).unwrap();

    let output = output_with(b"boom on stdout", 3);
    let err = harvest_narrative_file(&output, &path)
        .expect_err("a non-zero exit must surface an explicit error, not a scraped success");
    assert_loud_journal_error(err);
}

/// Defensive: a UTF-8-lossy result file (invalid bytes) must not panic the
/// reader — it is read losslessly-or-lossily and, if it still contains real
/// prose, returned; the point is NO panic on malformed agent output.
#[test]
fn invalid_utf8_in_result_file_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("narrative_output.md");
    // Valid clean prose followed by a lone invalid continuation byte.
    let mut bytes = CLEAN_NARRATIVE.as_bytes().to_vec();
    bytes.push(0xFF);
    std::fs::write(&path, &bytes).unwrap();

    let output = output_with(b"", 0);
    let narrative = harvest_narrative_file(&output, &path)
        .expect("invalid UTF-8 must be handled lossily, never panic the reader");
    // The clean prose prefix is preserved; the launcher/tool markers are absent.
    assert_clean_narrative(&narrative);
}
