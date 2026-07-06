//! Outside-in regression gate for the recurring
//! `overseer-obs:anomaly:distill parse-fail rate 100%` defect (issue #2678).
//!
//! **Why this exists.** The unit tests in `src/recipe_output/extract.rs` and
//! `src/memory_consolidation/distillation.rs` pin the low-level stripper and
//! the crate-internal `parse_facts` contract. This file tests the *external*
//! consumer surface: it drives `simard::recipe_output::strip_json_trailing_commas`
//! through the **public crate boundary** exactly as any downstream caller
//! (including the distillation pass) does — receive a contaminated LLM JSON
//! envelope, run the last-resort recovery view, then strict-parse the result.
//!
//! The defect: the distiller intermittently emits an otherwise well-formed
//! `{ "facts": [...] }` envelope carrying a JSON **trailing comma** (a `,`
//! immediately before a `}`/`]`). Strict `serde_json` rejects the whole
//! object, every batch defers, `distill_parse_success_rate` pins at 0, and the
//! Overseer raises the recurring 100%-fail anomaly that stalls the blocked
//! kgpacks-rs parity goals. The user-visible transition this file asserts is
//! **100% parse-fail → 0% parse-fail** on the reproducing envelope, while the
//! recovery stays fail-closed on genuinely malformed input.
//!
//! Run observably with:
//! ```bash
//! cargo test --test distill_trailing_comma_recovery_outside_in -- --nocapture
//! ```

use std::borrow::Cow;

use serde::Deserialize;
use simard::recipe_output::strip_json_trailing_commas;

/// A downstream consumer's view of the distiller's recipe envelope. Mirrors the
/// crate-internal shape an external caller would deserialize into.
#[derive(Debug, Deserialize)]
struct Envelope {
    facts: Vec<Fact>,
}

#[derive(Debug, Deserialize)]
struct Fact {
    concept: String,
    content: String,
    #[serde(default)]
    source_episode_id: String,
}

/// The exact recovery a consumer performs: strict parse first, and **only** on
/// failure retry against the trailing-comma-stripped view. Returns `None` when
/// both attempts fail — preserving the fail-closed deferral contract.
fn recover_envelope(raw: &str) -> Option<Envelope> {
    if let Ok(parsed) = serde_json::from_str::<Envelope>(raw) {
        return Some(parsed);
    }
    match strip_json_trailing_commas(raw) {
        Cow::Owned(stripped) => serde_json::from_str::<Envelope>(&stripped).ok(),
        // A borrowed view means the strict parse above already saw these exact
        // bytes and failed: there is nothing new to try, so stay failed-closed.
        Cow::Borrowed(_) => None,
    }
}

/// Scenario 2a — the reproducing shape. A trailing comma after the last
/// `facts[]` element AND after the last top-level member is rejected by a raw
/// strict parse (the 100%-fail state), but the public stripper recovers the
/// full fact through the consumer boundary (the 0%-fail state).
#[test]
fn trailing_comma_envelope_recovers_through_public_boundary() {
    let raw = r#"{"facts":[{"concept":"lesson-learned","content":"warm the cache first","source_episode_id":"epi_1"},],}"#;

    // Baseline: a raw strict parse fails — this is the defect the Overseer saw.
    assert!(
        serde_json::from_str::<Envelope>(raw).is_err(),
        "precondition: a raw strict parse of the trailing-comma envelope must fail (the 100%% state)"
    );

    // The public stripper materialises an owned, comma-free view (an allocation
    // only happens because a real trailing comma was found).
    let stripped = strip_json_trailing_commas(raw);
    assert!(
        matches!(stripped, Cow::Owned(_)),
        "a genuine trailing comma must produce an owned (actually-stripped) view"
    );

    // Recovery through the consumer boundary now yields the fact.
    let env = recover_envelope(raw).expect("the trailing-comma envelope must recover its fact");
    assert_eq!(env.facts.len(), 1);
    assert_eq!(env.facts[0].concept, "lesson-learned");
    assert_eq!(env.facts[0].content, "warm the cache first");
    assert_eq!(env.facts[0].source_episode_id, "epi_1");

    eprintln!(
        "[issue-2678] recovered {} fact(s) from a trailing-comma envelope via strip_json_trailing_commas (100%->0%)",
        env.facts.len()
    );
}

/// Scenario 2b — string-literal safety AND clean-path no-op in one flow. A
/// comma *inside* a fact's content is preserved verbatim (never mistaken for a
/// structural one), and a clean envelope is returned byte-identical and
/// borrowed (zero-copy), proving the recovery view is a provable no-op on valid
/// input — no regression for the overwhelmingly-common clean batch.
#[test]
fn content_commas_preserved_and_clean_envelope_is_zero_copy() {
    // (i) Trailing structural comma removed, but the content comma survives.
    let with_content_comma = r#"{"facts":[{"concept":"bug-pattern","content":"a, b, and c fail","source_episode_id":"epi_3"},]}"#;
    let env = recover_envelope(with_content_comma)
        .expect("trailing comma removed while content commas are preserved");
    assert_eq!(env.facts.len(), 1);
    assert_eq!(
        env.facts[0].content, "a, b, and c fail",
        "commas inside the string literal must be preserved verbatim"
    );

    // (ii) A clean, well-formed envelope must be borrowed byte-for-byte: the
    // recovery path never allocates and never alters clean output.
    let clean = r#"{"facts":[{"concept":"pr-pattern","content":"warm the cache","source_episode_id":"epi_5"}]}"#;
    let out = strip_json_trailing_commas(clean);
    assert!(
        matches!(out, Cow::Borrowed(_)),
        "a clean envelope must not allocate (zero-copy no-op)"
    );
    assert_eq!(out, clean, "a clean envelope must be byte-identical");
    let env = recover_envelope(clean).expect("a clean envelope must parse exactly as before");
    assert_eq!(env.facts[0].concept, "pr-pattern");
}

/// Scenario 2c — fail-closed on genuinely malformed input. Neither an
/// *adjacent* double comma (`,,]`) nor an unterminated object is laundered into
/// a hollow success by the single-pass, structural-only stripper. The consumer
/// recovery returns `None`, so the batch defers rather than persisting corrupt
/// structure — leniency never widens to accept broken JSON.
#[test]
fn genuinely_malformed_input_still_fails_closed() {
    let adjacent_double_comma =
        r#"{"facts":[{"concept":"bug-pattern","content":"x","source_episode_id":"epi_4"},,]}"#;
    assert!(
        recover_envelope(adjacent_double_comma).is_none(),
        "an adjacent double comma is genuinely malformed and must fail closed"
    );

    let unterminated = r#"{"facts":[{"concept":"bug-pattern","content":"x""#;
    assert!(
        recover_envelope(unterminated).is_none(),
        "an unterminated object must fail closed, never a hollow success"
    );

    eprintln!(
        "[issue-2678] genuinely malformed envelopes stayed fail-closed (deferred, not persisted)"
    );
}
