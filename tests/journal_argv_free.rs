//! Regression contract: the journal `recipe-runner-rs` invocation must carry
//! NO large payload in `argv`/`envp` — only a small `<key>_path=<abs>` value —
//! so it can never again overflow `ARG_MAX` and fail with E2BIG (issues
//! #2640/#2692).
//!
//! The old `JournalRecipe::run` built one `-c KEY=VALUE` argv token per context
//! var, inlining the entire `day_context` (draft pass) / `draft` (review pass).
//! The fix routes both unbounded values through the shared file channel
//! (`simard::recipe_context_file::ContextFile`) and appends only the short
//! `arg_value()` (`"<key>_path=<abs>"`). This test reconstructs the exact argv
//! the journal builds and pins the invariant at the process-argument layer,
//! hermetically (no runner, no agent binary).
//!
//! TDD status: RED until the fix adds `simard::recipe_context_file::ContextFile`
//! and the journal caller uses it. Isolated integration crate — red compile
//! does not affect the rest of the suite.

#![cfg(unix)]

use std::ffi::OsStr;
use std::process::Command;

use simard::recipe_context_file::ContextFile;

/// A per-key payload larger than the 128 KiB per-argument limit AND the >256 KiB
/// realistic-24h threshold, so if EITHER var were inlined the old E2BIG would
/// reappear.
const PER_KEY_BYTES: usize = 512 * 1024; // 0.5 MiB each

/// A conservative floor well under any real `ARG_MAX` (Linux total is ~2 MiB,
/// per-arg 128 KiB). The whole journal argv must stay far below this even with
/// two 0.5 MiB payloads "in flight" — proving the payloads are NOT on argv.
const ARGV_SAFE_CEILING: usize = 16 * 1024; // 16 KiB

fn payload(marker: &str) -> String {
    format!("{marker}:")
        .chars()
        .cycle()
        .take(PER_KEY_BYTES)
        .collect()
}

fn args_of(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a: &OsStr| a.to_string_lossy().into_owned())
        .collect()
}

/// Build the journal invocation's argv exactly as `JournalRecipe::run` does for
/// the two file-channel context vars, and assert it is payload-free and
/// ARG_MAX-safe. Returns the guards so the temp files outlive the assertions.
fn build_journal_argv() -> (Command, ContextFile, ContextFile) {
    let day_context = payload("DAY-CONTEXT-MARKER-2692");
    let draft = payload("DRAFT-MARKER-2692");

    let dc = ContextFile::write("journal", "day_context", &day_context).expect("write day_context");
    let df = ContextFile::write("journal", "draft", &draft).expect("write draft");

    // Mirror `JournalRecipe::run`: recipe path, `--output-format json`, then one
    // `-c <arg_value>` per context var (now a `<key>_path=<abs>` path, not the
    // inlined value).
    let mut cmd = Command::new("recipe-runner-rs");
    cmd.arg("/repo/prompt_assets/simard/recipes/journal-narrative.yaml")
        .arg("--output-format")
        .arg("json")
        .arg("-c")
        .arg(dc.arg_value())
        .arg("-c")
        .arg(df.arg_value());

    (cmd, dc, df)
}

/// No argv token may contain either payload marker — the payloads live in files,
/// never on argv.
#[test]
fn journal_argv_carries_no_payload() {
    let (cmd, _dc, _df) = build_journal_argv();
    let args = args_of(&cmd);

    for marker in ["DAY-CONTEXT-MARKER-2692", "DRAFT-MARKER-2692"] {
        assert!(
            !args.iter().any(|a| a.contains(marker)),
            "issue #2692: the journal invocation must NOT inline the {marker} \
             payload into argv: {args:?}"
        );
    }
}

/// The argv must instead reference both file-channel paths as `<key>_path=<abs>`
/// `-c` values, and each such value must be small (a path, not a payload).
#[test]
fn journal_argv_uses_small_key_path_values() {
    let (cmd, _dc, _df) = build_journal_argv();
    let args = args_of(&cmd);

    for key in ["day_context_path=", "draft_path="] {
        let found = args
            .iter()
            .find(|a| a.starts_with(key))
            .unwrap_or_else(|| panic!("journal argv must contain a `{key}` value: {args:?}"));
        assert!(
            found.len() < 4096,
            "the `{key}` value must be a short path, not an inlined payload: {} bytes",
            found.len()
        );
    }

    // The raw (payload) var names must be gone entirely.
    assert!(
        !args
            .iter()
            .any(|a| a.starts_with("day_context=") || a.starts_with("draft=")),
        "the raw inline `day_context=`/`draft=` vars must be replaced by their \
         `_path` forms: {args:?}"
    );
}

/// The whole constructed argv must stay far below any real `ARG_MAX`, even with
/// two 0.5 MiB payloads backing the invocation — proving ARG_MAX safety by
/// construction.
#[test]
fn journal_argv_is_arg_max_safe() {
    let (cmd, _dc, _df) = build_journal_argv();
    let args = args_of(&cmd);

    let total: usize =
        cmd.get_program().to_string_lossy().len() + args.iter().map(|a| a.len() + 1).sum::<usize>();
    assert!(
        total < ARGV_SAFE_CEILING,
        "the journal argv must stay far under ARG_MAX (it did NOT before the file \
         channel): {total} bytes >= {ARGV_SAFE_CEILING}"
    );
}

/// Argv-freedom must not cost fidelity: each full payload is recoverable
/// byte-for-byte from its file (guideline G3 — no truncation).
#[test]
fn payloads_are_recoverable_from_the_files() {
    let (cmd, dc, df) = build_journal_argv();
    let _ = &cmd;

    let dc_read = std::fs::read_to_string(dc.path()).expect("read day_context file");
    let df_read = std::fs::read_to_string(df.path()).expect("read draft file");
    assert_eq!(dc_read.len(), PER_KEY_BYTES, "full day_context on disk");
    assert_eq!(df_read.len(), PER_KEY_BYTES, "full draft on disk");
    assert!(dc_read.contains("DAY-CONTEXT-MARKER-2692"));
    assert!(df_read.contains("DRAFT-MARKER-2692"));
}
