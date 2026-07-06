//! Public contract for the shared recipe context-file transport brick
//! (`simard::recipe_context_file::ContextFile`), the fix for the journal E2BIG
//! recipe-spawn failure (issues #2640/#2692).
//!
//! The invariant (see `docs/reference/recipe-context-file-transport.md`): a
//! recipe context value whose size is unbounded NEVER appears in `argv`/`envp`.
//! It is written to a private temp file, and only the file's short PATH is
//! passed as `-c <key>_path=<abs>`. These tests pin that public contract
//! hermetically — no `recipe-runner-rs`, no agent binary, no network.
//!
//! TDD status: RED until the fix adds `src/recipe_context_file.rs` and
//! registers `pub mod recipe_context_file;` in `src/lib.rs`. This is an
//! isolated integration crate, so the red compile does not affect the rest of
//! the test suite (same convention as `tests/ooda_argv_free_invocation.rs`).

use std::path::Path;

use simard::recipe_context_file::ContextFile;

/// A payload comfortably larger than Linux `MAX_ARG_STRLEN` (128 KiB) and the
/// \>256 KiB realistic-24h-volume threshold called out in the issue — the exact
/// size that overflowed `argv` in the old inline `-c day_context=<...>` form.
const OVERSIZED_BYTES: usize = 1024 * 1024; // 1 MiB

fn oversized_payload() -> String {
    // A distinctive, repeating marker so a leak into argv would be detectable,
    // and so the round-trip assertion is meaningful.
    "CONTEXT-PAYLOAD-MARKER-2692:"
        .chars()
        .cycle()
        .take(OVERSIZED_BYTES)
        .collect()
}

/// `write` must round-trip the value verbatim into a file on disk, and `path()`
/// must point at that file while the guard is alive.
#[test]
fn write_round_trips_value_into_a_file() {
    let value = "the whole day's structured journal context\nwith newlines\n";
    let cf = ContextFile::write("journal", "day_context", value)
        .expect("writing a small context file must succeed");

    let path = cf.path();
    assert!(
        Path::new(path).is_absolute(),
        "the context-file path must be absolute so the recipe can read it \
         regardless of the runner's CWD: got {path:?}"
    );
    assert!(
        Path::new(path).is_file(),
        "the context file must exist on disk while the guard is alive: {path:?}"
    );
    let read_back = std::fs::read_to_string(path).expect("read context file");
    assert_eq!(
        read_back, value,
        "the payload must be recoverable byte-for-byte from the file (no \
         truncation, full fidelity — guideline G3)"
    );
}

/// `arg_value()` must be exactly the `-c` value grammar `"<key>_path=<abs>"`,
/// and the absolute path in it must match `path()`.
#[test]
fn arg_value_uses_the_key_path_grammar() {
    let cf = ContextFile::write("journal", "day_context", "small").expect("write must succeed");

    let arg = cf.arg_value();
    assert!(
        arg.starts_with("day_context_path="),
        "arg_value must be `<key>_path=<abs>` so the recipe reads {{{{day_context_path}}}}: \
         got {arg:?}"
    );
    let path_in_arg = arg
        .strip_prefix("day_context_path=")
        .expect("arg_value must carry the key_path= prefix");
    assert_eq!(
        path_in_arg,
        cf.path(),
        "the path embedded in arg_value must equal path(): {arg:?}"
    );
    assert!(
        Path::new(path_in_arg).is_absolute(),
        "the embedded path must be absolute: {arg:?}"
    );
}

/// The whole point of the fix: even a >256 KiB / >ARG_MAX payload yields a tiny,
/// constant-size `arg_value()` that CANNOT contribute to ARG_MAX, and never
/// contains the payload itself.
#[test]
fn arg_value_stays_tiny_and_payload_free_for_an_oversized_value() {
    let payload = oversized_payload();
    assert!(
        payload.len() > 256 * 1024,
        "payload must exceed the >256KB verification threshold"
    );

    let cf = ContextFile::write("journal", "day_context", &payload)
        .expect("writing an oversized context file must succeed (it goes to disk)");

    let arg = cf.arg_value();
    assert!(
        arg.len() < 4096,
        "arg_value must be a short path even for a 1 MiB payload — it must never \
         inline the content into argv: arg_value was {} bytes",
        arg.len()
    );
    assert!(
        !arg.contains("CONTEXT-PAYLOAD-MARKER-2692"),
        "arg_value must NOT contain any of the payload (that is the E2BIG bug): {arg:?}"
    );
    // ...but the payload is fully recoverable from the file the path points at.
    let read_back = std::fs::read_to_string(cf.path()).expect("read oversized context file");
    assert_eq!(
        read_back.len(),
        payload.len(),
        "the full oversized payload must be on disk, byte-for-byte"
    );
    assert_eq!(
        read_back, payload,
        "oversized payload must round-trip exactly"
    );
}

/// Ownership/lifetime: dropping the `ContextFile` guard removes the private temp
/// file (and its directory), so a per-invocation payload cannot linger on disk.
#[test]
fn dropping_the_guard_removes_the_file() {
    let cf =
        ContextFile::write("journal", "draft", "ephemeral draft body").expect("write must succeed");
    let path = cf.path().to_owned();
    assert!(Path::new(&path).exists(), "file must exist before drop");

    drop(cf);

    assert!(
        !Path::new(&path).exists(),
        "the context file must be unlinked when the guard is dropped, so the \
         per-invocation payload does not persist: {path:?}"
    );
}

/// Concurrent journal ticks / distillation passes must never collide: two
/// writes for the same key produce two DISTINCT files.
#[test]
fn separate_writes_get_distinct_paths() {
    let a = ContextFile::write("journal", "day_context", "A").expect("write a");
    let b = ContextFile::write("journal", "day_context", "B").expect("write b");
    assert_ne!(
        a.path(),
        b.path(),
        "each invocation must get a unique temp file so concurrent runs cannot \
         overwrite each other's context"
    );
    assert_eq!(std::fs::read_to_string(a.path()).unwrap(), "A");
    assert_eq!(std::fs::read_to_string(b.path()).unwrap(), "B");
}
