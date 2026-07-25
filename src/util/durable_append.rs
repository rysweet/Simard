//! Durable, loss-free line-append helper shared by every append-only JSONL
//! writer in Simard (the cost ledger and the self-metrics stream).
//!
//! See `docs/reference/durable-append-api.md`. The contract:
//!
//! [`append_line`] records exactly one parseable line per successful call,
//! never tearing, gluing, or dropping a concurrently-appended record under
//! massively-parallel host load, and propagates every `io::Result` (no
//! silently-swallowed IO error).
//!
//! ## Durability model
//!
//! 1. Build the whole record (`line` + one `\n`) in a single buffer.
//! 2. Take a process-global, poison-tolerant append mutex. This is what
//!    guarantees no two in-process threads interleave or drop a record: every
//!    append is fully serialized, so read-back always sees exactly one intact
//!    line per successful call.
//! 3. Open the file `O_CREAT | O_APPEND` and emit the record with ONE
//!    `write_all`, then `flush`. `O_APPEND` keeps writes positioned at EOF so
//!    sequential appends never overwrite each other; combined with the mutex,
//!    concurrent in-process appenders never tear or clobber a record.
//! 4. Propagate every `io::Result`; no IO error is ever swallowed.
//!
//! ## Scope of the guarantee
//!
//! This helper guarantees **no dropped or torn entries among concurrent
//! in-process appenders** (the failure the cost ledger exhibited under load —
//! the whole test suite is a single process). `flush` here is
//! `std::io::Write::flush`, a no-op for `File` (there is no user-space buffer),
//! so it does **not** provide crash durability against power loss; the bytes
//! reach the page cache, not stable storage. Add an explicit `fsync` only if
//! crash-durability is ever required — it is not for the ledger's no-loss
//! contract.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// Process-global append serializer. Poison-tolerant: a panic in an unrelated
/// writer must never wedge the ledger, so we recover the guard on poison.
static APPEND_LOCK: Mutex<()> = Mutex::new(());

/// Append `line` plus exactly one trailing `\n` to `path` as a single atomic
/// `O_APPEND` write, creating the parent directory if needed.
///
/// `line` must NOT already contain a trailing newline — the helper owns the
/// record terminator. Every IO error propagates to the caller.
pub fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        // Skip the empty parent of a bare relative filename; create_dir_all("")
        // errors on some platforms.
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let mut record = String::with_capacity(line.len() + 1);
    record.push_str(line);
    record.push('\n');

    let _guard = APPEND_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(record.as_bytes())?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;

    /// A single append creates the parent dir and writes exactly one
    /// newline-terminated line (the helper owns the terminator — no double
    /// newline even though the caller passes no trailing `\n`).
    #[test]
    fn append_line_creates_parent_and_writes_single_terminated_line() {
        let dir = tempfile::TempDir::new().unwrap();
        // Nested, not-yet-existing parent proves create_dir_all runs.
        let path = dir.path().join("nested").join("ledger.jsonl");

        append_line(&path, "first").unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "first\n", "exactly one record, one terminator");
    }

    /// Sequential appends accumulate as distinct lines in order.
    #[test]
    fn append_line_appends_without_truncating() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");

        append_line(&path, "a").unwrap();
        append_line(&path, "b").unwrap();
        append_line(&path, "c").unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "a\nb\nc\n");
    }

    /// The core durability guarantee: N threads each appending M JSON records
    /// to ONE file (in a unique, per-test `TempDir`) must yield EXACTLY N*M
    /// parseable, unique lines — zero dropped, zero torn/glued.
    #[test]
    fn concurrent_appends_never_drop_or_tear() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("concurrent.jsonl");

        const THREADS: usize = 16;
        const PER_THREAD: usize = 64;

        std::thread::scope(|scope| {
            for t in 0..THREADS {
                let path = path.clone();
                scope.spawn(move || {
                    for i in 0..PER_THREAD {
                        // Distinct JSON record per (thread, i); body padded so a
                        // torn splice is detectable as a parse failure.
                        let line =
                            format!(r#"{{"thread":{t},"seq":{i},"pad":"{}"}}"#, "x".repeat(64));
                        append_line(&path, &line).unwrap();
                    }
                });
            }
        });

        let contents = fs::read_to_string(&path).unwrap();
        let mut seen: HashSet<(u64, u64)> = HashSet::new();
        for raw in contents.lines() {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(trimmed)
                .unwrap_or_else(|e| panic!("torn/glued line is not valid JSON ({e}): {trimmed:?}"));
            let thread = v["thread"].as_u64().expect("thread field");
            let seq = v["seq"].as_u64().expect("seq field");
            assert!(
                seen.insert((thread, seq)),
                "duplicate record (thread={thread}, seq={seq}) — a concurrent append was corrupted"
            );
        }
        assert_eq!(
            seen.len(),
            THREADS * PER_THREAD,
            "every concurrently-appended record must be present and intact"
        );
    }

    /// IO errors must propagate, never be swallowed: when the parent path is a
    /// regular file (so `create_dir_all` of the record's parent must fail),
    /// `append_line` returns `Err`.
    #[test]
    fn append_line_propagates_io_error() {
        let dir = tempfile::TempDir::new().unwrap();
        // `blocker` is a FILE; using it as a directory component must error.
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, b"i am a file, not a dir").unwrap();
        let path = blocker.join("child.jsonl");

        let result = append_line(&path, "should not be written");
        assert!(
            result.is_err(),
            "append under a non-directory parent must return Err, not swallow it"
        );
    }
}
