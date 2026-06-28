//! TDD red-phase guard for issue #2412 — env-var race in
//! `tests/prompt_delivery.rs` (`AMPLIHACK_PROMPT_DELIVERY`).
//!
//! ## The flake
//! The two Auto-mode *size* tests
//! (`applied_mode_reports_inline_for_small_prompt`,
//! `applied_mode_reports_tempfile_for_large_prompt`) call
//! `apply_std(.., PromptDelivery::Auto)`, which consults
//! `std::env::var(ENV_OVERRIDE)` via `select_mode`. The four env-mutating tests
//! in the same file are already serialized under `#[serial(prompt_delivery_env)]`,
//! but the two *readers* are NOT — so under parallel `cargo test` a reader can
//! observe a leaked `inline`/`tempfile` value mid-flight and assert the wrong
//! mode.
//!
//! ## The fix (#2412)
//! Annotate every Auto-mode test with `#[serial(prompt_delivery_env)]` so the
//! readers cannot run concurrently with the env writers, restoring mutual
//! exclusion on the shared override variable.
//!
//! ## Why a source-parsing guard
//! The fix *is* an attribute. This guard encodes the invariant structurally —
//! it parses the sibling source file and asserts that **every `#[test]` whose
//! body references `PromptDelivery::Auto` carries the `serial(prompt_delivery_env)`
//! key** (and that the env writers stay keyed). That enforces the rule for
//! future tests too, not just today's two. It mirrors the existing
//! `tests/no_legacy_goal_records_references.rs` "rg-shaped" TDD acceptance test.
//!
//! This file FAILS until the two size tests are annotated (TDD red) and PASSES
//! once #2412's fix lands.

use std::fs;
use std::path::PathBuf;

/// The serial key both the env writers and (post-fix) the Auto-mode readers
/// must share so `serial_test` serializes them against each other.
const SERIAL_KEY: &str = "serial(prompt_delivery_env)";

/// Substring marking a test that consults the env override (Auto mode flows
/// through `select_mode` → `std::env::var(ENV_OVERRIDE)`).
const AUTO_MARKER: &str = "PromptDelivery::Auto";

/// Substring marking a test that *mutates* the env override.
const ENV_WRITE_MARKER: &str = "set_var(ENV_OVERRIDE";

/// A prompt exceeding `HARD_CAP_BYTES` makes `select_mode` return `TooLarge`
/// at its first step — *before* it reads `ENV_OVERRIDE` (see
/// `src/prompt_delivery/mod.rs::select_mode`). A test that references this cap
/// is exercising the oversize short-circuit, so it never consults the override
/// and is correctly exempt from the serial requirement.
const HARD_CAP_MARKER: &str = "HARD_CAP_BYTES";

/// The two concrete de-flake targets named in issue #2412.
const SIZE_TESTS: [&str; 2] = [
    "applied_mode_reports_inline_for_small_prompt",
    "applied_mode_reports_tempfile_for_large_prompt",
];

fn source_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/prompt_delivery.rs")
}

/// A parsed `#[test]` function: its attribute block (joined) and its
/// brace-balanced body text.
#[derive(Debug)]
struct TestFn {
    name: String,
    attrs: String,
    body: String,
}

/// Extract the identifier following `fn ` up to the opening `(`.
fn parse_fn_name(sig: &str) -> String {
    sig.split("fn ")
        .nth(1)
        .unwrap_or("")
        .split('(')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Read a brace-balanced function body starting at `start` (the `fn` line).
/// Returns the body text and the index of the line after the closing brace.
/// Naive brace counting is sufficient here: the bodies in
/// `tests/prompt_delivery.rs` keep `{`/`}` balanced inside string literals.
fn read_body(lines: &[&str], start: usize) -> (String, usize) {
    let mut depth: i32 = 0;
    let mut started = false;
    let mut body = String::new();
    let mut i = start;
    while i < lines.len() {
        for ch in lines[i].chars() {
            match ch {
                '{' => {
                    depth += 1;
                    started = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        body.push_str(lines[i]);
        body.push('\n');
        i += 1;
        if started && depth <= 0 {
            break;
        }
    }
    (body, i)
}

/// Parse every `#[test]` / `#[tokio::test]` function out of `src`, capturing the
/// contiguous attribute block immediately preceding each `fn`.
fn parse_test_fns(src: &str) -> Vec<TestFn> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut pending: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();

        if trimmed.starts_with("#[") || trimmed.starts_with("#!") {
            pending.push(trimmed);
            i += 1;
            continue;
        }

        let is_fn = trimmed.starts_with("fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("pub fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("pub(crate) fn ");

        if is_fn {
            let attrs = pending.join("\n");
            if attrs.contains("#[test]") || attrs.contains("tokio::test") {
                let name = parse_fn_name(trimmed);
                let (body, next) = read_body(&lines, i);
                out.push(TestFn { name, attrs, body });
                pending.clear();
                i = next;
                continue;
            }
            pending.clear();
            i += 1;
            continue;
        }

        // Any other non-blank, non-attribute line breaks the attribute run.
        if !trimmed.is_empty() {
            pending.clear();
        }
        i += 1;
    }
    out
}

fn consults_env_override(t: &TestFn) -> bool {
    t.body.contains(AUTO_MARKER) && !t.body.contains(HARD_CAP_MARKER)
}

fn load_tests() -> Vec<TestFn> {
    let src = fs::read_to_string(source_path())
        .unwrap_or_else(|e| panic!("failed to read {:?}: {e}", source_path()));
    let tests = parse_test_fns(&src);
    assert!(
        !tests.is_empty(),
        "parser found no #[test] fns in tests/prompt_delivery.rs — the parser or \
         the source layout changed; fix this guard before trusting it"
    );
    tests
}

/// #2412 — the two named Auto-mode size tests must be serialized on the env key.
#[test]
fn auto_mode_size_tests_are_serialized_on_env_key() {
    let tests = load_tests();

    for name in SIZE_TESTS {
        let t = tests.iter().find(|t| t.name == name).unwrap_or_else(|| {
            panic!(
                "expected test `{name}` in tests/prompt_delivery.rs; file structure \
                 changed — update this guard"
            )
        });

        assert!(
            t.body.contains(AUTO_MARKER),
            "`{name}` is expected to exercise `{AUTO_MARKER}` (it consults the \
             {ENV} override); if that changed, revisit issue #2412",
            ENV = "AMPLIHACK_PROMPT_DELIVERY",
        );

        assert!(
            t.attrs.contains(SERIAL_KEY),
            "#2412: `{name}` calls `apply_std(.., PromptDelivery::Auto)` which reads \
             AMPLIHACK_PROMPT_DELIVERY, so it MUST be annotated \
             `#[serial(prompt_delivery_env)]` to avoid racing the env-mutating \
             tests. Add the attribute to de-flake it.\nAttributes found:\n{}",
            t.attrs
        );
    }
}

/// Forward-looking invariant: ANY test that reads the override (Auto mode) must
/// be serialized on the env key, so new Auto-mode tests cannot reintroduce the
/// #2412 race.
#[test]
fn every_auto_mode_test_is_serialized_on_env_key() {
    let tests = load_tests();

    let offenders: Vec<&str> = tests
        .iter()
        .filter(|t| consults_env_override(t) && !t.attrs.contains(SERIAL_KEY))
        .map(|t| t.name.as_str())
        .collect();

    assert!(
        offenders.is_empty(),
        "#2412 invariant: every test that consults `PromptDelivery::Auto` (and thus \
         the AMPLIHACK_PROMPT_DELIVERY env override) must carry \
         `#[serial(prompt_delivery_env)]`. Un-serialized Auto-mode tests: {offenders:?}"
    );
}

/// Regression guard: the env *writers* must stay serialized (they already are).
/// Keeps both sides of the mutual-exclusion contract enforced.
#[test]
fn env_writer_tests_remain_serialized() {
    let tests = load_tests();

    let offenders: Vec<&str> = tests
        .iter()
        .filter(|t| t.body.contains(ENV_WRITE_MARKER) && !t.attrs.contains(SERIAL_KEY))
        .map(|t| t.name.as_str())
        .collect();

    assert!(
        offenders.is_empty(),
        "every test that mutates AMPLIHACK_PROMPT_DELIVERY must stay \
         `#[serial(prompt_delivery_env)]`. Un-serialized env writers: {offenders:?}"
    );
}
