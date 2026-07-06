//! Outside-in (external-consumer) validation for the issue #2672 distill
//! trailing-comma recovery (Step 13 QA-team outside-in testing).
//!
//! This is a *separate test crate*: it can only see `simard`'s public surface,
//! so it exercises the fix exactly as a downstream consumer of the recovery
//! primitive would. The recurring cognitive-memory signature
//! `overseer-obs:anomaly:distill parse-fail rate 100%` traced to the distiller
//! agent emitting a single trailing comma before a `}`/`]`, which strict
//! `serde_json` rejects — deferring every batch forever.
//!
//! These tests use *novel* payloads (not the in-crate fixtures) and assert the
//! real user-visible contract:
//!   1. the pre-fix failure mode is real (strict serde rejects the raw text),
//!   2. after `strip_json_trailing_commas` the same bytes parse cleanly, and
//!   3. clean input is passed through byte-for-byte with zero allocation.

use std::borrow::Cow;

use simard::recipe_output::strip_json_trailing_commas;

/// Scenario 1 (simple): the most basic user-facing behaviour — a bare
/// trailing-comma object that strict serde rejects must become valid,
/// parseable JSON after recovery, while genuinely clean input is returned
/// zero-copy (`Cow::Borrowed`) and byte-identical.
#[test]
fn scenario1_basic_trailing_comma_recovers_and_clean_input_is_zero_copy() {
    // --- the pre-fix failure mode is real -------------------------------
    let malformed = r#"{"facts":[{"concept":"bug-pattern","content":"fence-post error"},]}"#;
    assert!(
        serde_json::from_str::<serde_json::Value>(malformed).is_err(),
        "precondition: strict serde must reject the raw trailing-comma text \
         (this is the parse-fail-rate-100% signature)"
    );

    // --- the fix: recovery makes it parseable ---------------------------
    let recovered = strip_json_trailing_commas(malformed);
    assert!(
        matches!(recovered, Cow::Owned(_)),
        "a structural trailing comma was removed, so the strip owns the buffer"
    );
    let value: serde_json::Value =
        serde_json::from_str(&recovered).expect("recovered text must now parse under strict serde");
    assert_eq!(
        value["facts"][0]["concept"], "bug-pattern",
        "the grounded fact survives recovery intact"
    );

    // --- clean-path guarantee: no behaviour change on clean output ------
    let clean = r#"{"facts":[{"concept":"lesson","content":"prefer idempotent retries"}]}"#;
    let passthrough = strip_json_trailing_commas(clean);
    assert!(
        matches!(passthrough, Cow::Borrowed(_)),
        "clean input must be returned zero-copy (Cow::Borrowed), allocating nothing"
    );
    assert_eq!(
        &*passthrough, clean,
        "clean input must be byte-for-byte identical"
    );
}

/// Scenario 2 (complex): an enveloped, multi-array distiller payload that ALSO
/// carries a literal `,}` sequence *inside* a fact's string content. The
/// structural trailing commas (before `]` and `}`, across intervening
/// whitespace/newlines) must be stripped so the whole document parses, while
/// the in-string `,}` bytes are preserved verbatim — the string-awareness that
/// keeps recovery from corrupting a fact's content.
#[test]
fn scenario2_enveloped_nested_payload_recovers_and_preserves_in_string_bytes() {
    // Two structural trailing commas (one inside `facts[]`, one before the top
    // object close), an empty `procedures` array, cross-line whitespace, and a
    // fact whose `content` literally contains a `,}` sequence.
    let malformed = concat!(
        "{\n",
        "  \"facts\": [\n",
        "    {\"concept\":\"bug-pattern\",",
        "\"content\":\"guard drops on trailing ,} inside a map\",",
        "\"source_episode_id\":\"epi_42\"},\n",
        "  ],\n",
        "  \"procedures\": [],\n",
        "}",
    );

    // Pre-fix: strict serde rejects it outright.
    assert!(
        serde_json::from_str::<serde_json::Value>(malformed).is_err(),
        "precondition: the enveloped nested payload must be rejected by strict serde"
    );

    // Fix: recover, then parse.
    let recovered = strip_json_trailing_commas(malformed);
    assert!(
        matches!(recovered, Cow::Owned(_)),
        "structural trailing commas were present, so the strip owns the buffer"
    );
    // Recovery is delete-only: never longer than the input.
    assert!(
        recovered.len() <= malformed.len(),
        "delete-only recovery can only shrink the input"
    );

    let value: serde_json::Value = serde_json::from_str(&recovered)
        .expect("the enveloped, nested trailing-comma payload must parse after recovery");

    // The single grounded fact is intact...
    let facts = value["facts"].as_array().expect("facts is an array");
    assert_eq!(facts.len(), 1, "exactly one grounded fact survives");
    assert_eq!(facts[0]["concept"], "bug-pattern");
    assert_eq!(facts[0]["source_episode_id"], "epi_42");
    // ...and the in-string comma-brace sequence is preserved byte-for-byte.
    assert_eq!(
        facts[0]["content"], "guard drops on trailing ,} inside a map",
        "string-awareness: only STRUCTURAL trailing commas are removed; the \
         literal comma-brace sequence inside the content must survive verbatim"
    );
    // The empty companion array is untouched.
    assert!(
        value["procedures"]
            .as_array()
            .expect("procedures array")
            .is_empty(),
        "the empty procedures array is preserved"
    );
}
