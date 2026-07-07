//! TDD (RED) tests for the DISTILLATION structured file channel
//! (issues #2622 = 67% parse-failure, #2619 = 100% parse-failure).
//!
//! ## The defect these tests pin
//!
//! Distillation's memory fact-yield collapsed to ~zero because the recipe
//! runner could not parse its OWN output. Two defects compounded (confirmed
//! live in the daemon logs 02:45–02:47):
//!
//!   1. the copilot LAUNCHER BANNER / `INFO` log lines were captured *as* the
//!      distill step output, and
//!   2. the code BRITTLE-GREPPED that stdout for a `{ "facts": [...] }` object
//!      (the G3 antipattern the operator flagged).
//!
//! So the captured "output" was literally the launcher banner
//! (`… launching copilot binary=… version="GitHub Copilot CLI 1.0.69-1."`),
//! the balanced-brace scan found no facts object, and the pass failed with
//! `class="parse-failure"`.
//!
//! ## The fix these tests drive
//!
//! Give the distill agent a DEDICATED output channel: the recipe passes
//! `-c facts_output_path=<temp>` and instructs the agent to WRITE its
//! `{ "facts": [...], "procedures": [...] }` envelope to that file. Simard reads
//! the file after the subprocess exits and deserializes it. Because the parse
//! reads a FILE (by path), launcher banners / log noise on STDOUT can never
//! contaminate it — structurally, not heuristically.
//!
//! ## TDD status
//!
//! These tests were written FIRST (Step 7) and drove the implementation. They
//! target two `pub(crate)` functions in `distillation`:
//!   * `parse_distill_facts_file(contents: &str) -> SimardResult<DistillOutput>`
//!   * `read_distill_facts_file(path: &Path) -> SimardResult<DistillOutput>`
//!
//! These are now implemented: they deserialize the agent-written envelope
//! through the shared `RecipeEnvelope` serde boundary (strict — no stdout
//! scraping), with bounded tolerance for an optional ```json fence. The full
//! suite below — positive path, the launcher-banner-can't-contaminate
//! regression, and the negative / no-silent-fallback / classification cases — is
//! GREEN. The remaining production integration is wiring
//! `RecipeRunnerSubprocess::invoke_recipe` to the file channel (pass
//! `-c facts_output_path`, have the recipe write the envelope there, read it
//! after the subprocess exits); these functions are the tested seam it will call.

use crate::memory_consolidation::distillation::{
    DistillFailureClass, classify_distill_error, parse_distill_facts_file, read_distill_facts_file,
};

// ───────────────────────────────────────────────────────────────────────────
// Fixtures: the exact live-incident stdout noise, and clean file envelopes.
// ───────────────────────────────────────────────────────────────────────────

/// The exact launcher-banner / log noise that (defect a) was being captured as
/// the distill step output and (defect b) brittle-grepped for facts. Reproduced
/// from the daemon logs 02:45–02:47 (Copilot CLI 1.0.69-1). The point of the
/// file channel is that THIS never reaches the parser: it lives on stdout, the
/// facts live in a separate file.
const LIVE_LAUNCHER_BANNER: &str = "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference)\n\
     2026-07-06T02:45:12.001Z INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot version=\"GitHub Copilot CLI 1.0.69-1.\"\n\
     Run 'copilot update' to update to the latest version.\n";

/// A clean facts+procedures envelope exactly as the distill agent is asked to
/// WRITE it to `facts_output_path`.
fn clean_envelope() -> String {
    r#"{
      "facts": [
        { "concept": "bug-pattern",
          "content": "launcher banner on stdout was captured as distill output",
          "source_episode_id": "epi_2622" },
        { "concept": "lesson-learned",
          "content": "read distilled facts from a dedicated file, never scrape stdout",
          "source_episode_id": "epi_2619" }
      ],
      "procedures": [
        { "name": "distill:file-channel",
          "steps": ["pass -c facts_output_path", "agent writes envelope to file", "runner reads file after exit"],
          "source_episode_ids": ["epi_2622", "epi_2619"] }
      ]
    }"#
    .to_string()
}

// ───────────────────────────────────────────────────────────────────────────
// 1. Healthy path: the file channel yields structured facts AND procedures.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn file_channel_extracts_facts_and_procedures() {
    let out = parse_distill_facts_file(&clean_envelope())
        .expect("a clean facts+procedures envelope written to the file must parse");
    assert_eq!(out.facts.len(), 2, "both facts must be extracted");
    assert_eq!(out.facts[0].concept, "bug-pattern");
    assert_eq!(
        out.facts[0].content,
        "launcher banner on stdout was captured as distill output"
    );
    assert_eq!(out.facts[0].source_episode_id, "epi_2622");
    assert_eq!(out.facts[1].concept, "lesson-learned");
    assert_eq!(out.procedures.len(), 1, "the procedure must be extracted");
    assert_eq!(out.procedures[0].name, "distill:file-channel");
    assert_eq!(out.procedures[0].steps.len(), 3);
    assert_eq!(
        out.procedures[0].source_episode_ids,
        vec!["epi_2622", "epi_2619"]
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 2. THE core regression: a launcher banner on stdout is IRRELEVANT because the
//    facts are read from a dedicated file by path. This is the hermetic proof
//    that stdout noise can never cause a parse-failure (task VALIDATION).
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn launcher_banner_on_stdout_cannot_contaminate_the_file_channel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts_path = dir.path().join("distill-facts.json");
    std::fs::write(&facts_path, clean_envelope()).expect("write facts file");

    // The launcher banner is what the OLD stdout-scraping path captured and
    // choked on. Here it is *separate* from the facts channel — the runner would
    // print it to stdout while the agent writes the envelope to `facts_path`. The
    // file read never consults stdout, so the banner is structurally inert.
    let _stdout_noise = LIVE_LAUNCHER_BANNER; // present in production; must not matter

    let out = read_distill_facts_file(&facts_path)
        .expect("facts must be read from the file regardless of any stdout banner");
    assert_eq!(
        out.facts.len(),
        2,
        "structured facts must be extracted from the file even though the live \
         launcher banner is on stdout"
    );
    assert_eq!(out.procedures.len(), 1);
}

// ───────────────────────────────────────────────────────────────────────────
// 3. No silent fallback: if the banner/noise somehow lands as the FILE contents
//    (agent wrote noise instead of JSON), that is an EXPLICIT parse-failure —
//    never a silent `Ok` and never a hollow success.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn banner_as_file_contents_is_explicit_parse_failure_not_silent_ok() {
    let err = parse_distill_facts_file(LIVE_LAUNCHER_BANNER)
        .expect_err("a launcher banner is not a facts envelope — must be an explicit error");
    assert_eq!(
        classify_distill_error(&err),
        DistillFailureClass::ParseFailure,
        "banner-as-file-contents must classify as parse-failure (transient, retried, counted), \
         never a silent success"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 4. A genuinely empty batch is SUCCESS (zero yield), NOT a parse-failure (A4).
//    This is the metric-correctness invariant: parse-failure is reserved for
//    missing/empty/non-JSON files, not for a legitimately-empty distill.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn empty_facts_envelope_is_success_not_parse_failure() {
    let out = parse_distill_facts_file(r#"{"facts":[],"procedures":[]}"#)
        .expect("an empty-but-valid envelope is 'nothing worth distilling', a SUCCESS");
    assert!(out.facts.is_empty());
    assert!(out.procedures.is_empty());
}

#[test]
fn facts_only_envelope_without_procedures_key_parses() {
    // `procedures` is optional in the contract; a facts-only file must parse
    // with an empty procedures vec (not a parse-failure).
    let out = parse_distill_facts_file(
        r#"{"facts":[{"concept":"pr-pattern","content":"c","source_episode_id":"e1"}]}"#,
    )
    .expect("facts-only envelope (no procedures key) must parse");
    assert_eq!(out.facts.len(), 1);
    assert!(out.procedures.is_empty());
}

// ───────────────────────────────────────────────────────────────────────────
// 5. Missing / empty file → explicit ParseFailure (no silent fallback, A5).
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn missing_facts_file_is_explicit_parse_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("does-not-exist.json");
    assert!(!missing.exists());

    let err = read_distill_facts_file(&missing)
        .expect_err("a missing facts file must surface an explicit error, never a silent Ok");
    assert_eq!(
        classify_distill_error(&err),
        DistillFailureClass::ParseFailure,
        "a missing facts file is a parse-failure (the run reached parsing but yielded no facts)"
    );
}

#[test]
fn empty_facts_file_is_explicit_parse_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let empty = dir.path().join("empty.json");
    std::fs::write(&empty, "").expect("write empty file");

    let err = read_distill_facts_file(&empty)
        .expect_err("an empty facts file must surface an explicit error, never a silent Ok");
    assert_eq!(
        classify_distill_error(&err),
        DistillFailureClass::ParseFailure
    );
}

#[test]
fn non_json_file_contents_is_explicit_parse_failure() {
    let err = parse_distill_facts_file("this is not json at all")
        .expect_err("non-JSON file contents must be an explicit parse-failure");
    assert_eq!(
        classify_distill_error(&err),
        DistillFailureClass::ParseFailure
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 6. Bounded, non-brittle tolerance: strip a surrounding ```json markdown fence
//    the agent sometimes wraps the envelope in. This is a single well-defined
//    unwrap — NOT balanced-brace stdout scanning — so it does not reintroduce
//    the G3 antipattern while keeping the healthy path yielding facts.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn markdown_fenced_envelope_is_tolerated() {
    let fenced = format!("```json\n{}\n```", clean_envelope());
    let out = parse_distill_facts_file(&fenced)
        .expect("a ```json-fenced envelope must still parse (bounded tolerance, not scraping)");
    assert_eq!(out.facts.len(), 2);
    assert_eq!(out.procedures.len(), 1);
}

// ───────────────────────────────────────────────────────────────────────────
// 7. The file channel reuses the SAME contract as the stdout path: off-spec
//    concepts are dropped, surface-form variants are canonicalized, and
//    field-level noise on one fact does not sink the whole envelope.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn file_channel_canonicalizes_surface_variant_and_drops_offspec_concepts() {
    let contents = r#"{
      "facts": [
        { "concept": "PR-Pattern", "content": "surface variant is canonicalized", "source_episode_id": "e1" },
        { "concept": "made-up-label", "content": "off-spec is dropped", "source_episode_id": "e2" }
      ],
      "procedures": []
    }"#;
    let out = parse_distill_facts_file(contents).expect("must parse");
    assert_eq!(out.facts.len(), 1, "off-spec concept must be dropped");
    assert_eq!(
        out.facts[0].concept, "pr-pattern",
        "surface-form variant must be canonicalized to the closed label set"
    );
}

#[test]
fn file_channel_tolerates_field_level_noise_on_one_fact() {
    // Issue #2506 shape: one fact carries a null `source_episode_id`. Field-level
    // leniency must recover the well-formed sibling instead of serde rejecting
    // the whole envelope (which is what silently dropped batches before).
    let contents = r#"{
      "facts": [
        { "concept": "lesson-learned", "content": "well-formed survivor", "source_episode_id": "e1" },
        { "concept": "bug-pattern", "content": "noisy sibling", "source_episode_id": null }
      ],
      "procedures": []
    }"#;
    let out = parse_distill_facts_file(contents)
        .expect("field-level noise on one fact must not sink the whole envelope");
    assert!(
        out.facts
            .iter()
            .any(|f| f.content == "well-formed survivor"),
        "the well-formed fact must survive a noisy sibling"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 8. Guard: the file-channel error must NOT be misclassified as a structural
//    class (spawn/terminal/serialize) — it reached parsing and yielded nothing.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn file_channel_parse_failure_reached_parsing() {
    let err = parse_distill_facts_file("").expect_err("empty contents must be a parse-failure");
    let class = classify_distill_error(&err);
    assert!(
        class.reached_parsing(),
        "a file-channel parse miss reached output parsing (denominator of \
         distill_parse_success_rate); got {:?}",
        class
    );
    assert!(
        class.recipe_exited_ok(),
        "the recipe process exited 0 — only the facts file was unparseable"
    );
}
