//! TDD (RED) tests for the #2679 semantic agent→agent handoff in the
//! distillation RESULT path.
//!
//! ## What #2679 changes
//!
//! Before: the distiller agent printed a `{ "facts": [...] }` envelope; Simard
//! scraped it back out of noisy recipe stdout (launcher banner + ANSI + tracing
//! lines) via `extract_json_payload` → `balanced_objects` →
//! `serde_json::from_str`, and a single malformed token (a trailing comma)
//! failed the strict parse and discarded the ENTIRE batch — the
//! `parse_fail` / 91%→100% failure mode.
//!
//! After: the distiller agentic step writes each fact DIRECTLY through the
//! gated cognitive-memory write boundary during its run. There is **no return
//! payload for Simard to parse**. The two seams that reach the boundary — the
//! IPC server (real subprocess path) and the in-process `DistillFactSink` (test
//! stubs) — share the `crate::fact_reliability` gate, so a fact scores and
//! stores identically no matter which commits it.
//!
//! These tests pin:
//!   * **SEC-T6 (headline):** a finished recipe run is interpreted by exit
//!     status ALONE — `interpret_recipe_exit` never reads stdout, so noisy /
//!     trailing-comma / banner-polluted stdout can no longer fail the pipeline
//!     (there is no parse to fail).
//!   * the agentic pipeline commits grounded facts to memory and marks every
//!     episode, using a run-only stub UNCHANGED (additive-trait contract), and
//!   * the in-process sink applies the SAME reliability gate (ungrounded /
//!     empty content quarantined), and
//!   * retry-safety: an erroring agentic step leaves the batch fully unmarked.
//!
//! They reference `interpret_recipe_exit` (which replaces the removed
//! `harvest_facts_file` + `parse_facts*` machinery) and the additive
//! `run_agentic` handoff; those do not exist yet — the intended TDD red signal.
//!
//! NOTE: the legacy strict-parse / trailing-comma / field-tolerance unit tests
//! in `distillation.rs` and the file-channel stub tests are RETIRED by #2679
//! (they assert a parse that no longer exists). This module is their
//! replacement: it proves the failure mode is now *structurally impossible*
//! rather than merely tolerated.

use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::CognitiveEpisode;
use crate::memory_consolidation::distillation::{
    DISTILL_MIN_EPISODES, DistillRecipeRunner, DistilledFact, distill_recent_episodes_with_runner,
    interpret_recipe_exit,
};

// ───────────────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────────────

/// A finished subprocess `Output` with the given exit code and stdout. The
/// stderr is left empty; the point of #2679 is that NEITHER stream is parsed
/// for facts.
fn output_with(code: i32, stdout: &str) -> Output {
    Output {
        // On Unix, a normal exit with code `c` is wait-status `c << 8`.
        status: ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

/// Seed `n` undistilled episodes into a fresh in-memory library store and
/// return the store. The distiller's facts cite these episodes' ids, so they
/// are grounded by batch membership.
fn store_with_episodes(n: usize) -> LibraryCognitiveMemory {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    for i in 0..n {
        mem.store_episode(
            &format!("episode payload number {i}"),
            "engineer-cycle",
            None,
        )
        .expect("store_episode");
    }
    mem
}

// ───────────────────────────────────────────────────────────────────────────
// SEC-T6 (headline): the result path is an exit-status check — stdout is NEVER
// parsed, so noisy / trailing-comma stdout can no longer fail the pipeline.
// ───────────────────────────────────────────────────────────────────────────

/// The single most important regression for #2679/#2658: a recipe that exits 0
/// while emitting the copilot launcher banner, ANSI colour codes, tracing log
/// lines, AND a *trailing-comma-malformed* `{ "facts": [...] , }` object on
/// stdout must be interpreted as SUCCESS. The old code scraped this stdout and
/// the trailing comma failed `serde_json::from_str`, discarding the batch. The
/// new code never reads stdout for facts — so there is no parse to fail.
#[test]
fn sec_t6_noisy_trailing_comma_stdout_is_not_parsed_and_does_not_fail() {
    let noisy = concat!(
        // launcher banner
        "2026-07-06T09:00:00.000000Z  INFO launching copilot binary=copilot ",
        "version=\"GitHub Copilot CLI 1.0.69-2\"\n",
        // saved-preference notice with a unicode glyph
        "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference)\n",
        // an ANSI-coloured tracing line
        "\u{1b}[32m INFO\u{1b}[0m recipe-runner: step 'distill' completed\n",
        // a MALFORMED facts object with trailing commas after the fact and array
        "{\"facts\":[{\"concept\":\"pr-pattern\",\"content\":\"warm cache before pin bumps\",",
        "\"source_episode_id\":\"epi_1\"},],}\n",
        "Run 'copilot update' to update\n",
    );
    // Precondition: this stdout is NOT valid JSON — the old strict parse died here.
    assert!(
        serde_json::from_str::<serde_json::Value>(noisy).is_err(),
        "precondition: the noisy trailing-comma stdout must be strict-invalid JSON"
    );

    let result = interpret_recipe_exit(&output_with(0, noisy));
    assert!(
        result.is_ok(),
        "an exit-0 recipe run must succeed regardless of stdout content — there is no parse to \
         fail post-#2679; got {result:?}"
    );
}

/// A clean exit-0 run with EMPTY stdout is also success: the agent committed its
/// facts through the write boundary, so an empty terminal is expected, not an
/// error (the old "facts document was empty" parse-failure is gone).
#[test]
fn sec_t6_empty_stdout_on_exit_zero_is_success_not_parse_failure() {
    assert!(
        interpret_recipe_exit(&output_with(0, "")).is_ok(),
        "empty stdout on a clean exit must not be a failure — the result is in memory, not stdout"
    );
}

/// A non-zero exit is still a real, surfaced failure (the recipe process itself
/// broke) — this is the only failure the result path can report now. It must
/// carry context and must NOT be a silent success.
#[test]
fn nonzero_exit_is_a_surfaced_terminal_failure() {
    let err = interpret_recipe_exit(&output_with(1, "some diagnostic on stdout"))
        .expect_err("a non-zero recipe exit must surface an error");
    match err {
        SimardError::RpcError(msg) => assert!(
            msg.contains("distill") && msg.contains("exited"),
            "terminal-failure message must name the distill recipe and the non-zero exit: {msg}"
        ),
        other => panic!("expected RpcError terminal failure, got {other:?}"),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Agentic pipeline: a run-only stub commits grounded facts via the sink and
// every episode is marked — additive-trait contract (stub UNCHANGED).
// ───────────────────────────────────────────────────────────────────────────

/// A deterministic stub that implements ONLY the legacy `run` (returning facts).
/// It must keep working unchanged: the new default `run_agentic` bridges its
/// returned facts to the in-process gated `DistillFactSink`. Each fact cites a
/// real batch episode id so it is grounded.
struct GroundedFactsRunner {
    calls: AtomicU32,
}

impl DistillRecipeRunner for GroundedFactsRunner {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![
            DistilledFact {
                concept: "bug-pattern".into(),
                content: "empty outcome list panics cycle".into(),
                source_episode_id: episodes[0].node_id.clone(),
            },
            DistilledFact {
                concept: "lesson-learned".into(),
                content: "prefer keyword overlap for episodic recall".into(),
                source_episode_id: episodes[1].node_id.clone(),
            },
        ])
    }
}

#[test]
fn agentic_pass_commits_grounded_facts_and_marks_every_episode() {
    let n = DISTILL_MIN_EPISODES as usize + 2;
    let mem = store_with_episodes(n);
    let runner = GroundedFactsRunner {
        calls: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mem as &dyn CognitiveMemoryOps, &runner)
        .expect("agentic pass must succeed");

    assert!(!report.was_skipped(), "above-threshold pass must run");
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "the runner must be invoked exactly once per pass"
    );
    assert_eq!(
        report.fact_count, 2,
        "both grounded, well-formed facts must be committed"
    );
    assert_eq!(
        report.marked_count as usize, n,
        "every input episode must be marked distilled (replay guard)"
    );

    // The facts really reached semantic memory — via the write boundary, with no
    // envelope parsed anywhere.
    let bugs = mem
        .search_facts("bug-pattern", 10, 0.0)
        .expect("search bug");
    assert!(
        bugs.iter()
            .any(|f| f.content == "empty outcome list panics cycle")
    );
    let lessons = mem
        .search_facts("lesson-learned", 10, 0.0)
        .expect("search lesson");
    assert!(
        lessons
            .iter()
            .any(|f| f.content == "prefer keyword overlap for episodic recall")
    );

    // The batch is fully consumed.
    let remaining = mem
        .list_undistilled_episodes(DISTILL_MIN_EPISODES + 10)
        .expect("list undistilled");
    assert!(
        remaining.is_empty(),
        "no episode may remain undistilled after a successful pass"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// In-process sink applies the SAME gate: ungrounded / empty facts quarantined.
// ───────────────────────────────────────────────────────────────────────────

/// A stub that emits one grounded good fact, one *ungrounded* fact (citing an
/// episode not in the batch), and one grounded-but-empty fact. The in-process
/// `DistillFactSink` must apply the shared reliability gate per fact: only the
/// good one is stored; the other two are quarantined. This proves gate parity
/// with the server seam (SEC-T1/T2) on the stub path.
struct MixedReliabilityRunner;

impl DistillRecipeRunner for MixedReliabilityRunner {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        Ok(vec![
            DistilledFact {
                concept: "bug-pattern".into(),
                content: "grounded well formed fact".into(),
                source_episode_id: episodes[0].node_id.clone(),
            },
            DistilledFact {
                concept: "bug-pattern".into(),
                content: "hallucinated provenance fact".into(),
                source_episode_id: "epi_not_in_this_batch".into(),
            },
            DistilledFact {
                concept: "lesson-learned".into(),
                content: "   ".into(), // empty/whitespace → hard quarantine
                source_episode_id: episodes[1].node_id.clone(),
            },
        ])
    }
}

#[test]
fn in_process_sink_quarantines_ungrounded_and_empty_facts() {
    let n = DISTILL_MIN_EPISODES as usize + 2;
    let mem = store_with_episodes(n);

    let report = distill_recent_episodes_with_runner(
        &mem as &dyn CognitiveMemoryOps,
        &MixedReliabilityRunner,
    )
    .expect("pass must succeed even when some facts are quarantined");

    assert_eq!(
        report.fact_count, 1,
        "only the grounded, well-formed fact is stored"
    );
    assert_eq!(
        report.quarantined_count, 2,
        "the ungrounded fact and the empty-content fact must both be quarantined"
    );
    // Quarantine must not break the replay guard: every episode is still marked.
    assert_eq!(report.marked_count as usize, n);

    let facts = mem.search_facts("bug-pattern", 10, 0.0).expect("search");
    assert!(
        facts
            .iter()
            .all(|f| f.content != "hallucinated provenance fact"),
        "an ungrounded fact must never leak into semantic memory"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Retry-safety: an erroring agentic step leaves the batch fully unmarked.
// ───────────────────────────────────────────────────────────────────────────

struct ErroringAgenticRunner;

impl DistillRecipeRunner for ErroringAgenticRunner {
    fn run(&self, _episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        Err(SimardError::RpcError(
            "distill: recipe exited with exit status: 1: stderr= stdout=".into(),
        ))
    }
}

#[test]
fn erroring_agentic_step_leaves_batch_unmarked_and_stores_nothing() {
    let n = DISTILL_MIN_EPISODES as usize + 2;
    let mem = store_with_episodes(n);

    let result = distill_recent_episodes_with_runner(
        &mem as &dyn CognitiveMemoryOps,
        &ErroringAgenticRunner,
    );

    // Either shape is acceptable (fatal Err, or Ok with zero work), but the
    // store MUST be untouched so the batch is fully retry-eligible next pass.
    match result {
        Err(_) => {}
        Ok(r) => {
            assert_eq!(r.fact_count, 0, "no facts on agentic-step error");
            assert_eq!(r.marked_count, 0, "no marks on agentic-step error");
        }
    }

    let remaining = mem
        .list_undistilled_episodes(DISTILL_MIN_EPISODES + 10)
        .expect("list undistilled");
    assert_eq!(
        remaining.len(),
        n,
        "retry-safety: no episode may be marked when the agentic step fails"
    );
    assert!(
        mem.search_facts("bug-pattern", 10, 0.0)
            .expect("search")
            .is_empty(),
        "no facts may be stored when the agentic step fails"
    );
}
