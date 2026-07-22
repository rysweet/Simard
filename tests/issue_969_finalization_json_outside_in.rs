//! Outside-in (external-consumer) verification for issue #969: the
//! agentic-workflow finalization path must (a) tolerate noisy-but-recoverable
//! agent JSON, (b) surface a *diagnosable typed error* on a drop instead of a
//! silent `None`, and (c) never embed an unbounded copy of a pathological
//! multi-megabyte agent response into that error / its log record.
//!
//! These tests exercise ONLY the public `simard` crate surface a downstream
//! consumer sees — `simard::recipe_output::{extract_and_parse_json,
//! extract_and_parse_json_result, JsonExtractError, RAW_EXTRACT_TRUNCATE_BYTES}`
//! and `simard::util::string_truncate::head_within_budget` — the same
//! chokepoint the hardened orient brain (`ooda_brain::orient`) now routes
//! through. No internal item is touched, so the test survives refactoring and
//! proves behaviour a real consumer of the finalization API observes.

use serde::Deserialize;
use simard::recipe_output::{
    JsonExtractError, RAW_EXTRACT_TRUNCATE_BYTES, extract_and_parse_json,
    extract_and_parse_json_result,
};
use simard::util::string_truncate::head_within_budget;

/// A realistic finalization envelope a recipe-backed reasoning phase emits.
#[derive(Debug, Deserialize, PartialEq)]
struct Finalization {
    decision: String,
    items: Vec<String>,
}

// ---------------------------------------------------------------------------
// Scenario 1 (simple): the basic user-facing behaviour — a finalization
// response wrapped in the exact banner + ANSI-coloured log noise a live recipe
// run prints, and carrying a well-formed-but-strict-invalid trailing comma,
// still finalizes into the typed value instead of being dropped.
// ---------------------------------------------------------------------------
#[test]
fn scenario1_noisy_recoverable_finalization_parses() {
    // Recipe banner line + an ANSI-dimmed tracing line + the object, with a
    // trailing comma inside the array AND before the closing brace. This is the
    // shape #969 says used to sink a whole judgment.
    let raw = "Recipe: ooda SUCCESS (3.0s)\n\
               \x1b[2m2026-07-22T20:00:00.000000Z\x1b[0m INFO decide\n\
               {\"decision\": \"admit\", \"items\": [\"a\",],}";

    // Ergonomic `Option` entry point (what most call sites use).
    let via_option: Finalization =
        extract_and_parse_json(raw).expect("noisy+trailing-comma agent output must finalize");
    assert_eq!(via_option.decision, "admit");
    assert_eq!(via_option.items, vec!["a".to_string()]);

    // Typed entry point must return the identical value — no behavioural
    // divergence between the two public entry points on the success path.
    let via_result: Finalization =
        extract_and_parse_json_result(raw).expect("typed entry point finalizes identically");
    assert_eq!(via_result, via_option);

    // A comma that is legitimate *string content* must survive unchanged (the
    // recovery must not corrupt real data).
    let content_comma = r#"{"decision": "defer", "items": ["a, b, c"]}"#;
    let parsed: Finalization =
        extract_and_parse_json(content_comma).expect("string-content comma preserved");
    assert_eq!(parsed.items, vec!["a, b, c".to_string()]);
}

// ---------------------------------------------------------------------------
// Scenario 2 (complex): the #969 contract itself — every drop is a *typed,
// diagnosable* error (never a silent `None`), each of the three drop sites maps
// to its own variant with a low-cardinality `kind()` tag, and a pathological
// multi-megabyte response is capped at `RAW_EXTRACT_TRUNCATE_BYTES` in the
// embedded record so it cannot flood the logs.
// ---------------------------------------------------------------------------
#[test]
fn scenario2_drops_are_typed_diagnosable_and_bounded() {
    // (a) No balanced `{…}` object in any cleaned view -> NoBalancedObject,
    //     carrying a truncated copy of the raw for the operator.
    let no_obj = "2026-07-22 INFO no json object on this line";
    let err = extract_and_parse_json_result::<Finalization>(no_obj)
        .expect_err("no object must be a typed error, not a silent None");
    match &err {
        JsonExtractError::NoBalancedObject { raw_truncated } => {
            assert!(raw_truncated.contains("no json object"));
        }
        other => panic!("expected NoBalancedObject, got {other:?}"),
    }
    assert_eq!(err.kind(), "no_balanced_object");
    // The `Option` wrapper still drops the same input to `None` (parity).
    assert_eq!(extract_and_parse_json::<Finalization>(no_obj), None);
    // Diagnosability: Display is human-readable, source() is None here.
    assert!(err.to_string().contains("no balanced JSON object"));
    assert!(std::error::Error::source(&err).is_none());

    // (b) A balanced object outside the four recoverable defects (unquoted key
    //     / missing value) -> Unrecoverable, preserving the serde source error.
    for bad in [r#"{decision: "admit"}"#, r#"{"decision":}"#] {
        let err = extract_and_parse_json_result::<Finalization>(bad)
            .expect_err("non-recoverable malformed must be typed");
        match &err {
            JsonExtractError::Unrecoverable {
                payload_truncated,
                source,
            } => {
                assert!(!payload_truncated.is_empty());
                assert!(!source.to_string().is_empty());
            }
            other => panic!("expected Unrecoverable for {bad:?}, got {other:?}"),
        }
        assert_eq!(err.kind(), "unrecoverable");
        assert!(std::error::Error::source(&err).is_some());
        assert_eq!(extract_and_parse_json::<Finalization>(bad), None);
    }

    // (c) Recovery rewrites the payload (trailing comma stripped) but the retry
    //     still fails on a type mismatch (`items` holds numbers, not strings)
    //     -> RecoveredParseFailed.
    let recovered_but_typed_wrong = r#"{"decision": "admit", "items": [1, 2],}"#;
    let err = extract_and_parse_json_result::<Finalization>(recovered_but_typed_wrong)
        .expect_err("recovered-but-type-invalid must be typed");
    match &err {
        JsonExtractError::RecoveredParseFailed {
            recovered_truncated,
            source,
        } => {
            assert!(!recovered_truncated.contains(",]"));
            assert!(!source.to_string().is_empty());
        }
        other => panic!("expected RecoveredParseFailed, got {other:?}"),
    }
    assert_eq!(err.kind(), "recovered_parse_failed");
    assert_eq!(
        extract_and_parse_json::<Finalization>(recovered_but_typed_wrong),
        None
    );

    // (d) Bounded allocation: a pathological multi-megabyte response embeds at
    //     most RAW_EXTRACT_TRUNCATE_BYTES into the typed error record.
    let huge = "x".repeat(RAW_EXTRACT_TRUNCATE_BYTES * 4);
    let err = extract_and_parse_json_result::<Finalization>(&huge)
        .expect_err("a wall of x's has no object");
    match err {
        JsonExtractError::NoBalancedObject { raw_truncated } => {
            assert!(
                raw_truncated.len() <= RAW_EXTRACT_TRUNCATE_BYTES,
                "embedded raw must respect the {RAW_EXTRACT_TRUNCATE_BYTES}-byte budget, got {}",
                raw_truncated.len()
            );
        }
        other => panic!("expected NoBalancedObject, got {other:?}"),
    }

    // (e) The bounded-prefix primitive the error path relies on must itself cap
    //     a huge *multi-byte* input on a char boundary without panicking.
    let multibyte = "é".repeat(RAW_EXTRACT_TRUNCATE_BYTES); // 2 bytes each
    let capped = head_within_budget(&multibyte, RAW_EXTRACT_TRUNCATE_BYTES);
    assert!(capped.len() <= RAW_EXTRACT_TRUNCATE_BYTES);
    assert!(std::str::from_utf8(capped.as_bytes()).is_ok());
    // A short input under budget is returned whole (no lossy truncation).
    assert_eq!(
        head_within_budget("short", RAW_EXTRACT_TRUNCATE_BYTES),
        "short"
    );
}
