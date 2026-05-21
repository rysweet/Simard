//! Outside-in integration tests for `simard::prompt_delivery` (issue #1897).
//!
//! These tests are the **TDD red phase** for the new subprocess prompt
//! delivery layer. They spawn real `/bin/cat` and `/usr/bin/env` subprocesses
//! (Unix only) to exercise the full Command-mutation + stdin-write +
//! temp-file lifecycle end-to-end, asserting that:
//!
//! 1. **Round-trip integrity** — prompts containing apostrophes, double
//!    quotes, newlines, tabs, and 64 KiB+ payloads survive delivery
//!    byte-for-byte (the bugs from rysweet/Simard#1871 / #1879).
//! 2. **Mode selection** — `Auto` correctly picks `Inline` / `Stdin` /
//!    `TempFile` based on prompt size; explicit overrides win.
//! 3. **Env override** — `AMPLIHACK_PROMPT_DELIVERY` flips `Auto` choice;
//!    invalid values fall back gracefully.
//! 4. **Temp-file semantics** — file exists on disk during child lifetime,
//!    has `0o600` perms, is unlinked on Drop, persists when
//!    `retain_temp_file()` is called.
//! 5. **Argv hygiene** — `Inline` mode appends `--` flag terminator before
//!    the prompt, so prompts beginning with `-` cannot be reinterpreted as
//!    flags by the child binary.
//! 6. **Size limits** — `> 16 MiB` returns `TooLarge` *before* spawning.
//! 7. **NUL guard** — explicit `Inline` + NUL byte returns `NulInInlineMode`.
//!
//! All tests touching the env var are serialized with `serial_test` under a
//! shared lock key `prompt_delivery_env` (also used by inline unit tests in
//! `src/prompt_delivery/mod.rs`).
//!
//! The tests are gated on `#[cfg(unix)]` because they spawn `/bin/cat` and
//! rely on Unix permission bits. CI is Linux per the repo's `.github/`
//! workflow set; macOS local runs are also expected to pass.

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

use serial_test::serial;
use simard::prompt_delivery::{
    AppliedPromptStd, ENV_OVERRIDE, HARD_CAP_BYTES, PromptDelivery, PromptDeliveryError,
    STDIN_PREFERRED_MAX_BYTES, apply_std, select_mode,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn `/bin/cat` (no args), drive `apply_std` to feed the prompt, capture
/// stdout, and return it. Used to verify byte-for-byte round-trip across
/// `Stdin` and `TempFile` modes — both wire the prompt to the child's stdin.
fn run_cat_with_prompt(prompt: &[u8], mode: PromptDelivery) -> Vec<u8> {
    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let applied =
        apply_std(&mut cmd, prompt, mode).expect("apply_std failed for run_cat_with_prompt");

    let mut child = cmd.spawn().expect("/bin/cat spawn failed");
    let stdin = child.stdin.take();
    applied.feed(stdin).expect("AppliedPromptStd::feed failed");

    let mut stdout_buf = Vec::with_capacity(prompt.len());
    child
        .stdout
        .take()
        .expect("child stdout pipe missing")
        .read_to_end(&mut stdout_buf)
        .expect("reading cat stdout");

    let status = child.wait().expect("cat wait failed");
    assert!(
        status.success(),
        "/bin/cat exited unsuccessfully: {status:?}"
    );
    stdout_buf
}

/// Lock a sane test timeout so a hung subprocess fails the CI run within a
/// minute rather than hanging the suite.
fn assert_within<F: FnOnce() + Send + 'static>(d: Duration, label: &str, f: F) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    match rx.recv_timeout(d) {
        Ok(()) => {}
        Err(_) => panic!("{label} did not complete within {d:?}"),
    }
}

// ===========================================================================
// Round-trip integrity tests (Stdin / TempFile)
// ===========================================================================

#[test]
fn stdin_mode_roundtrips_apostrophes_unchanged() {
    // The shell-quoting bug from #1871: "Don't" was getting mangled.
    let prompt = b"Don't break the build -- the engineer's job depends on it.";
    let got = run_cat_with_prompt(prompt, PromptDelivery::Stdin);
    assert_eq!(got, prompt);
}

#[test]
fn tempfile_mode_roundtrips_apostrophes_unchanged() {
    let prompt = b"Don't break the build -- the engineer's job depends on it.";
    let got = run_cat_with_prompt(prompt, PromptDelivery::TempFile);
    assert_eq!(got, prompt);
}

#[test]
fn stdin_mode_roundtrips_double_quotes_unchanged() {
    let prompt = b"\"quoted\" 'mixed' `backticks` and $variables ${expansions}";
    let got = run_cat_with_prompt(prompt, PromptDelivery::Stdin);
    assert_eq!(got, prompt);
}

#[test]
fn stdin_mode_roundtrips_newlines_and_tabs_unchanged() {
    let prompt = b"line one\nline\ttwo\r\nline three\n\nfinal";
    let got = run_cat_with_prompt(prompt, PromptDelivery::Stdin);
    assert_eq!(got, prompt);
}

#[test]
fn stdin_mode_roundtrips_64kib_payload_unchanged() {
    // The size at which `argv` delivery starts brushing against per-arg
    // ARG_MAX on macOS (256 KiB total) and pushes claude into truncating.
    let prompt: Vec<u8> = (0..(64 * 1024)).map(|i| (i % 256) as u8).collect();
    assert_within(
        Duration::from_secs(15),
        "64 KiB stdin roundtrip",
        move || {
            let got = run_cat_with_prompt(&prompt, PromptDelivery::Stdin);
            assert_eq!(got.len(), prompt.len(), "64 KiB roundtrip changed length");
            assert_eq!(got, prompt, "64 KiB roundtrip changed content");
        },
    );
}

#[test]
fn tempfile_mode_roundtrips_64kib_payload_unchanged() {
    let prompt: Vec<u8> = (0..(64 * 1024)).map(|i| (i % 256) as u8).collect();
    assert_within(
        Duration::from_secs(15),
        "64 KiB tempfile roundtrip",
        move || {
            let got = run_cat_with_prompt(&prompt, PromptDelivery::TempFile);
            assert_eq!(got, prompt);
        },
    );
}

#[test]
fn stdin_mode_roundtrips_nul_bytes_unchanged() {
    // NUL bytes cannot ride argv on POSIX (truncates at NUL), but stdin can
    // carry them fine. apply_std must not lose any bytes.
    let prompt = b"before\0middle\0after";
    let got = run_cat_with_prompt(prompt, PromptDelivery::Stdin);
    assert_eq!(got, prompt);
}

// ===========================================================================
// Mode auto-selection (sizes / explicit overrides)
// ===========================================================================

#[test]
fn applied_mode_reports_inline_for_small_prompt() {
    let mut cmd = Command::new("/bin/cat");
    let applied = apply_std(&mut cmd, b"tiny", PromptDelivery::Auto).unwrap();
    assert_eq!(applied.mode(), PromptDelivery::Inline);
    assert!(applied.temp_path().is_none());
}

#[test]
fn applied_mode_reports_tempfile_for_large_prompt() {
    let mut cmd = Command::new("/bin/cat");
    let big = vec![b'x'; STDIN_PREFERRED_MAX_BYTES + 16];
    let applied = apply_std(&mut cmd, &big, PromptDelivery::Auto).unwrap();
    assert_eq!(applied.mode(), PromptDelivery::TempFile);
    assert!(
        applied.temp_path().is_some(),
        "TempFile mode must expose a temp_path()"
    );
}

#[test]
fn inline_mode_argv_ends_with_flag_terminator_then_prompt() {
    // Argv hygiene: the prompt must NOT be reinterpretable as a flag.
    // apply_std must inject `--` before the prompt argv element if the
    // caller has not already done so. This is the S11 protection.
    let mut cmd = Command::new("/bin/cat");
    cmd.arg("--show-tabs"); // pretend this is a real flag we want to keep
    let prompt = b"--rm -rf /";
    let _applied = apply_std(&mut cmd, prompt, PromptDelivery::Inline).unwrap();

    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    // Last arg is the prompt.
    assert_eq!(
        args.last().map(String::as_str),
        Some("--rm -rf /"),
        "Inline mode must append prompt as last argv element. Got: {args:?}"
    );
    // The element before the prompt is `--`.
    let n = args.len();
    assert!(
        n >= 2,
        "expected at least [..., '--', <prompt>]; got {args:?}"
    );
    assert_eq!(
        args[n - 2],
        "--",
        "Inline mode must inject `--` flag terminator before the prompt"
    );
}

#[test]
fn inline_mode_does_not_duplicate_flag_terminator() {
    // If the caller already terminated with `--`, apply_std must NOT add a
    // second `--` (that would make the second `--` itself a positional).
    let mut cmd = Command::new("/bin/cat");
    cmd.arg("--").arg("first-positional");
    let prompt = b"the-prompt";
    let _applied = apply_std(&mut cmd, prompt, PromptDelivery::Inline).unwrap();

    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    let dashdash_count = args.iter().filter(|a| *a == "--").count();
    assert_eq!(
        dashdash_count, 1,
        "apply_std must not duplicate an existing `--` terminator. Got: {args:?}"
    );
    assert_eq!(args.last().map(String::as_str), Some("the-prompt"));
}

// ===========================================================================
// Env override
// ===========================================================================

#[test]
#[serial(prompt_delivery_env)]
fn env_override_forces_inline_for_short_prompt() {
    // SAFETY (Rust 2024): set_var/remove_var require unsafe — tests only.
    unsafe {
        std::env::set_var(ENV_OVERRIDE, "inline");
    }
    let mode = select_mode(b"short", PromptDelivery::Auto).expect("select_mode");
    assert_eq!(mode, PromptDelivery::Inline);
    unsafe {
        std::env::remove_var(ENV_OVERRIDE);
    }
}

#[test]
#[serial(prompt_delivery_env)]
fn env_override_forces_tempfile_for_short_prompt() {
    unsafe {
        std::env::set_var(ENV_OVERRIDE, "tempfile");
    }
    // Apply for a short prompt: would otherwise be Inline, env wins.
    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    let applied = apply_std(&mut cmd, b"short", PromptDelivery::Auto).unwrap();
    assert_eq!(applied.mode(), PromptDelivery::TempFile);
    assert!(applied.temp_path().is_some());

    // Cleanup driver: drive the child to completion so the guard releases.
    let mut child = cmd.spawn().expect("/bin/cat spawn");
    applied.feed(child.stdin.take()).unwrap();
    let _ = child.wait();

    unsafe {
        std::env::remove_var(ENV_OVERRIDE);
    }
}

#[test]
#[serial(prompt_delivery_env)]
fn invalid_env_value_falls_back_to_auto_without_panic() {
    unsafe {
        std::env::set_var(ENV_OVERRIDE, "totally-bogus");
    }
    // Auto must still resolve cleanly — invalid value warned-once and
    // falls back to the heuristic.
    let mode = select_mode(b"short", PromptDelivery::Auto).expect("must not panic");
    assert_eq!(mode, PromptDelivery::Inline);
    unsafe {
        std::env::remove_var(ENV_OVERRIDE);
    }
}

#[test]
#[serial(prompt_delivery_env)]
fn caller_override_ignores_env_var() {
    unsafe {
        std::env::set_var(ENV_OVERRIDE, "tempfile");
    }
    // Explicit Inline beats env override.
    let mode = select_mode(b"short", PromptDelivery::Inline).unwrap();
    assert_eq!(mode, PromptDelivery::Inline);
    unsafe {
        std::env::remove_var(ENV_OVERRIDE);
    }
}

// ===========================================================================
// TempFile lifecycle / permissions / retention
// ===========================================================================

#[test]
fn tempfile_exists_during_guard_lifetime() {
    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    let applied = apply_std(&mut cmd, b"prompt-for-disk", PromptDelivery::TempFile).unwrap();

    let path = applied
        .temp_path()
        .expect("TempFile mode must expose temp_path")
        .to_path_buf();

    assert!(
        path.exists(),
        "temp file must exist while guard is alive: {path:?}"
    );

    // Drive the child to completion (feed consumes the guard).
    let mut child = cmd.spawn().expect("/bin/cat spawn");
    applied.feed(child.stdin.take()).unwrap();
    let _ = child.wait();

    // Guard dropped on feed → file must be unlinked.
    assert!(
        !path.exists(),
        "temp file must be unlinked after guard is consumed/dropped: {path:?}"
    );
}

#[test]
fn tempfile_dropped_without_feed_still_unlinks() {
    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    let path = {
        let applied = apply_std(&mut cmd, b"prompt-dropped", PromptDelivery::TempFile).unwrap();
        let p = applied.temp_path().unwrap().to_path_buf();
        assert!(p.exists());
        p
        // applied dropped here without feed()
    };
    assert!(
        !path.exists(),
        "Drop of AppliedPromptStd must unlink the temp file: {path:?}"
    );
}

#[test]
fn retain_temp_file_persists_after_drop_then_caller_cleans_up() {
    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    let applied = apply_std(&mut cmd, b"keep-me", PromptDelivery::TempFile).unwrap();
    let retained = applied
        .retain_temp_file()
        .expect("TempFile mode must return Some(path) from retain_temp_file()");
    assert!(
        retained.exists(),
        "retain_temp_file must leave the file on disk: {retained:?}"
    );

    // Caller-side cleanup (this is contract — operator must remove
    // explicitly because retain disables RAII).
    std::fs::remove_file(&retained).expect("manual cleanup must succeed");
    assert!(!retained.exists());
}

#[test]
#[cfg(unix)]
fn tempfile_has_0o600_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    let applied = apply_std(&mut cmd, b"perm-test", PromptDelivery::TempFile).unwrap();
    let path = applied.temp_path().unwrap();
    let meta = std::fs::metadata(path).expect("temp file metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "temp file must be owner-rw-only (0o600); got 0o{mode:o}"
    );

    // Drain and drop.
    let mut child = cmd.spawn().expect("/bin/cat spawn");
    applied.feed(child.stdin.take()).unwrap();
    let _ = child.wait();
}

// ===========================================================================
// Hard cap and NUL guard
// ===========================================================================

#[test]
fn oversize_prompt_returns_too_large_before_spawn() {
    let mut cmd = Command::new("/bin/cat");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    let huge = vec![b'a'; HARD_CAP_BYTES + 1];
    let err = apply_std(&mut cmd, &huge, PromptDelivery::Auto)
        .err()
        .expect("oversize prompt must error before spawn");
    match err {
        PromptDeliveryError::TooLarge(n) => assert_eq!(n, HARD_CAP_BYTES + 1),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn explicit_inline_with_nul_returns_nul_in_inline_mode() {
    let mut cmd = Command::new("/bin/cat");
    let prompt = b"safe\0unsafe";
    let err = apply_std(&mut cmd, prompt, PromptDelivery::Inline)
        .err()
        .expect("NUL in explicit Inline mode must error");
    assert!(
        matches!(err, PromptDeliveryError::NulInInlineMode),
        "expected NulInInlineMode; got {err:?}"
    );
}

// ===========================================================================
// AppliedPromptStd must_use sanity (compile-only assertion via type system)
// ===========================================================================

#[test]
fn applied_prompt_std_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<AppliedPromptStd>();
}

// ===========================================================================
// Quoting-bug regression (#1871 / #1879): a representative engineer brief
// ===========================================================================

#[test]
fn engineer_brief_with_mixed_quoting_roundtrips() {
    // Shape mirrors a real engineer brief — multi-line, apostrophes,
    // double quotes, backticks, command substitution shapes — the exact
    // class of input that motivated this module per issues #1871 / #1879.
    let prompt = b"## Task\n\
        Drive issue #1897 -- `amplihack-rs` parity: subprocess prompt delivery.\n\
        \n\
        You'll need to:\n\
        1. Grep for `Command::new(...)` callsites that pass prompts via `-p \"$prompt\"`.\n\
        2. Add `select_mode` + `apply_std` plumbing.\n\
        3. Don't forget the `--` flag terminator!\n\
        \n\
        Done == `cargo test -p simard --test prompt_delivery` is green.\n";
    let got = run_cat_with_prompt(prompt, PromptDelivery::Stdin);
    assert_eq!(got, prompt, "engineer brief must survive stdin roundtrip");
}
