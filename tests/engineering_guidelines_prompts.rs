//! Durable contract for the engineering guidelines (G1/G2/G3 and now **G4**)
//! that Simard's OODA reasoners, engineers, and reviewers — and human
//! contributors — must follow.
//!
//! G1 — HYBRID BENCHMARK + LIVE SELF-MEASUREMENT: cognition / self-improvement
//!      work must prove gains on BOTH a fixed benchmark AND a live production
//!      self-measurement trended over time — not just one.
//! G2 — MEMORY-ARCH WORK BELONGS UPSTREAM: distillation / recall / ranking /
//!      storage / WAL / forgetting must land in `rysweet/amplihack-memory-lib`
//!      and Simard bumps her pinned dep — do NOT fork the memory logic into
//!      Simard's own repo (`src/memory_consolidation`, `src/cognitive_memory`).
//! G3 — PREFER AGENTIC STEPS OVER BRITTLE PARSING; PREFER RECIPES/PROMPTS OVER
//!      CODE: treat line/substring parsing of model/tool output as a brittle
//!      antipattern; prefer a structured-output contract + agentic extraction,
//!      and prefer recipes/prompts over new code.
//! G4 — DURABLE DOCS ONLY; NEVER COMMIT POINT-IN-TIME REPORT DOCS
//!      (`no-point-in-time-docs`): an investigation / testing / diagnosis /
//!      recurrence / benchmark-snapshot FINDING is recorded as a GitHub issue
//!      and/or memory — NOT as a committed repo doc. Durable feature/architecture
//!      docs remain encouraged (the distinction is doc TYPE, not topic). G4 also
//!      has a hard deterministic backstop — the Overseer pr-verify scan
//!      `scan_no_point_in_time_report_docs` (see `src/overseer/pr_verify.rs`).
//!
//! TDD (Step 7 — write tests first): the G1/G2/G3 assertions landed with #2614
//! and are GREEN; the **G4** assertions at the end of this file are the new RED
//! set — they stay RED until the implementation step threads G4 into the
//! engineer / OODA reasoner prompts, the review gates, and the recipe mirrors.
//! The `CONTRIBUTING.md` assertions (G1–G4) are the GREEN anchor — the human
//! source of truth is already in place, so a regression that deletes it is also
//! caught.
//!
//! The assertions pin STABLE keyword invariants (lowercased), not full-sentence
//! snapshots, so ordinary rewording does not break them — deleting a guideline
//! does. (Keyword invariants, not brittle snapshots, is itself G3.)

use std::fs;
use std::path::PathBuf;

// --- asset readers -------------------------------------------------------

fn prompt(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read prompt asset {}: {e}", path.display()))
}

fn recipe(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard/recipes")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read recipe asset {}: {e}", path.display()))
}

fn repo_file(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read repo file {}: {e}", path.display()))
}

fn prompt_lc(name: &str) -> String {
    prompt(name).to_lowercase()
}

// --- assertion helpers ---------------------------------------------------

fn contains_any(haystack_lc: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|n| haystack_lc.contains(&n.to_lowercase()))
}

fn assert_contains(haystack_lc: &str, needle: &str, file: &str, what: &str) {
    assert!(
        haystack_lc.contains(&needle.to_lowercase()),
        "{file} must express {what} (expected keyword {needle:?})"
    );
}

fn assert_contains_any(haystack_lc: &str, needles: &[&str], file: &str, what: &str) {
    assert!(
        contains_any(haystack_lc, needles),
        "{file} must express {what} (expected one of {needles:?})"
    );
}

fn assert_absent(haystack_lc: &str, needle: &str, file: &str, why: &str) {
    assert!(
        !haystack_lc.contains(&needle.to_lowercase()),
        "{file} must not contain {needle:?} ({why})"
    );
}

// --- canonical guideline keyword vocabulary ------------------------------
//
// One shared, stable vocabulary so every reasoner / gate / doc pins the SAME
// anchors. Rewording the surrounding prose is fine; dropping a guideline is
// what these catch.

/// Shared label that threads every guideline-bearing asset back to the durable
/// `CONTRIBUTING.md` section. Absent everywhere before Step 8, so it is the
/// universal RED discriminator.
const MARKER: &str = "g1/g2/g3";

/// G1 — the live-self-measurement half (the benchmark half alone is NOT
/// sufficient; that is the whole point of the guideline).
const G1_LIVE: &[&str] = &[
    "live self-measurement",
    "live self-metric",
    "live production self-measurement",
];
const G1_TREND: &[&str] = &["trended over time", "trend over time"];

/// G2 — the anti-fork half: memory-arch logic must NOT be forked into Simard's
/// own repo under these paths; it belongs in `amplihack-memory-lib`.
const G2_ANTIFORK: &[&str] = &["src/memory_consolidation", "src/cognitive_memory"];
const MEMORY_LIB: &str = "amplihack-memory-lib";

/// G3 — brittle-parsing antipattern vs. agentic extraction, and prefer
/// recipes/prompts over code.
const G3_AGENTIC: &[&str] = &["agentic step", "agentic extraction"];
const G3_OVER_CODE: &[&str] = &[
    "over code",
    "prefer recipes",
    "prefer prompts",
    "recipes and prompts over",
    "recipes/prompts",
];

// Composite guideline assertions ------------------------------------------

fn assert_g1(lc: &str, file: &str) {
    assert_contains_any(
        lc,
        G1_LIVE,
        file,
        "G1: a LIVE production self-measurement (not just a fixed benchmark)",
    );
    assert_contains_any(
        lc,
        G1_TREND,
        file,
        "G1: the self-metric is trended over time",
    );
    assert_contains(
        lc,
        "benchmark",
        file,
        "G1: a fixed benchmark as the other half of the hybrid bar",
    );
}

fn assert_g2_full(lc: &str, file: &str) {
    assert_contains(
        lc,
        MEMORY_LIB,
        file,
        "G2: memory-arch work routes to amplihack-memory-lib",
    );
    assert_contains_any(
        lc,
        G2_ANTIFORK,
        file,
        "G2: do NOT fork memory logic into Simard's own repo \
         (src/memory_consolidation / src/cognitive_memory)",
    );
}

fn assert_g2_framing(lc: &str, file: &str) {
    // Goal-framing prompts express G2 as a routing/success criterion rather
    // than naming Simard-internal src paths.
    assert_contains(
        lc,
        MEMORY_LIB,
        file,
        "G2: standing goals route memory-arch work to amplihack-memory-lib",
    );
    assert_contains_any(
        lc,
        &["upstream", G2_ANTIFORK[0], G2_ANTIFORK[1]],
        file,
        "G2: memory-arch work lands upstream, not forked into Simard's repo",
    );
}

fn assert_g3(lc: &str, file: &str) {
    assert_contains(
        lc,
        "brittle parsing",
        file,
        "G3: name line/substring parsing of model/tool output as a brittle antipattern",
    );
    assert_contains_any(
        lc,
        G3_AGENTIC,
        file,
        "G3: prefer an agentic step (structured output + agent extraction)",
    );
    assert_contains_any(
        lc,
        G3_OVER_CODE,
        file,
        "G3: prefer recipes/prompts over new code",
    );
}

fn assert_marker(lc: &str, file: &str) {
    assert_contains(
        lc,
        MARKER,
        file,
        "a reference to the durable engineering guidelines (G1/G2/G3)",
    );
}

// --- Layer A: engineer + planning reasoner prompts (full G1+G2+G3) --------

#[test]
fn engineer_system_threads_all_three_guidelines() {
    let lc = prompt_lc("engineer_system.md");
    assert_marker(&lc, "engineer_system.md");
    assert_g1(&lc, "engineer_system.md");
    assert_g2_full(&lc, "engineer_system.md");
    assert_g3(&lc, "engineer_system.md");
}

#[test]
fn engineer_planning_threads_all_three_guidelines() {
    let lc = prompt_lc("engineer_planning.md");
    assert_marker(&lc, "engineer_planning.md");
    assert_g1(&lc, "engineer_planning.md");
    assert_g2_full(&lc, "engineer_planning.md");
    assert_g3(&lc, "engineer_planning.md");
}

// --- Layer A: OODA reasoner prompts (lightweight marker + no-bridge) -------
//
// The Orient / Decide / Act(lifecycle) reasoners are compact meta-brains
// (urgency float, action keyword, lifecycle variant). They carry a reference
// to the guidelines so cognition/memory/parsing judgment inherits G1/G2/G3,
// without over-pinning their narrow output contracts.

const OODA_REASONERS: &[&str] = &["ooda_orient.md", "ooda_decide.md", "ooda_brain.md"];

#[test]
fn ooda_reasoners_reference_the_guidelines() {
    for f in OODA_REASONERS {
        let lc = prompt_lc(f);
        assert_marker(&lc, f);
    }
}

// --- Layer B: review gates (full G1+G2+G3 flag criteria) ------------------

#[test]
fn merge_readiness_judge_flags_all_three_guidelines() {
    let lc = prompt_lc("merge_readiness_judge.md");
    assert_marker(&lc, "merge_readiness_judge.md");
    assert_g1(&lc, "merge_readiness_judge.md");
    assert_g2_full(&lc, "merge_readiness_judge.md");
    assert_g3(&lc, "merge_readiness_judge.md");
}

#[test]
fn review_pipeline_flags_all_three_guidelines() {
    let lc = prompt_lc("review_pipeline.md");
    assert_marker(&lc, "review_pipeline.md");
    assert_g1(&lc, "review_pipeline.md");
    assert_g2_full(&lc, "review_pipeline.md");
    assert_g3(&lc, "review_pipeline.md");
}

#[test]
fn progress_assessment_reviewer_flags_live_self_measurement() {
    // The progress-evidence gate's most relevant flag is G1: a cognition PR
    // that reports only a corpus/proxy number with no live self-metric trended
    // over time. It carries the marker so the G2/G3 context is reachable too.
    let lc = prompt_lc("progress_assessment_reviewer.md");
    assert_marker(&lc, "progress_assessment_reviewer.md");
    assert_g1(&lc, "progress_assessment_reviewer.md");
}

// --- Layer C: goal-framing prompts (G1 + G2 success criteria) -------------

const GOAL_FRAMING: &[&str] = &[
    "goal_session_objective.md",
    "goal_decomposition.md",
    "goal_curator_system.md",
];

#[test]
fn goal_framing_seeds_hybrid_measurement_and_upstream_routing() {
    for f in GOAL_FRAMING {
        let lc = prompt_lc(f);
        assert_marker(&lc, f);
        // G1: standing cognition/self-improvement goals require a hybrid
        // benchmark + live self-measurement trended over time.
        assert_contains_any(
            &lc,
            G1_LIVE,
            f,
            "G1: standing goals require a live self-measurement, not just a benchmark",
        );
        assert_contains_any(
            &lc,
            G1_TREND,
            f,
            "G1: the required self-metric is trended over time",
        );
        // G2: memory-arch work is routed upstream to amplihack-memory-lib.
        assert_g2_framing(&lc, f);
    }
}

// --- Mirror parity: edited reasoner/gate .md stays in sync with its .yaml --
//
// Every guideline-bearing prompt that is ALSO inlined into a recipe the daemon
// runs must carry the marker in BOTH files, so a `.md` edit can't silently
// leave the live recipe path unguided (policy drift).

const MIRROR_PAIRS: &[(&str, &str)] = &[
    ("ooda_orient.md", "ooda-orient.yaml"),
    ("ooda_decide.md", "ooda-decide.yaml"),
    ("ooda_brain.md", "ooda-engineer-lifecycle.yaml"),
    ("merge_readiness_judge.md", "merge-readiness-judge.yaml"),
    (
        "progress_assessment_reviewer.md",
        "progress-assessment.yaml",
    ),
    ("goal_decomposition.md", "goal-decomposition.yaml"),
];

#[test]
fn recipe_mirrors_carry_the_guidelines_marker() {
    let mut drifted = Vec::new();
    for (md, yaml) in MIRROR_PAIRS {
        let md_lc = prompt_lc(md);
        let yaml_lc = recipe(yaml).to_lowercase();
        let md_has = md_lc.contains(MARKER);
        let yaml_has = yaml_lc.contains(MARKER);
        if md_has != yaml_has {
            drifted.push(format!(
                "{md} (marker={md_has}) vs {yaml} (marker={yaml_has})"
            ));
        }
        // Both must ultimately carry the marker once threaded.
        assert!(
            yaml_has,
            "recipe mirror {yaml} must carry the {MARKER:?} guideline marker \
             (parity with {md})"
        );
    }
    assert!(
        drifted.is_empty(),
        "prompt/recipe mirror drift — the guideline marker must be present in \
         BOTH the .md and its .yaml mirror: {drifted:?}"
    );
}

// --- Terminology guard: one-Brain OODA phases are never renamed "Bridge" ---
//
// The edited OODA reasoner regions (and their recipe mirrors) keep one-Brain
// terminology. The guard targets the specific antipattern — renaming a phase
// or brain as a "Bridge" — via collocations, NOT a blanket ban on the
// substring: `engineer_system.md` legitimately hosts the standing "never name
// anything Bridge" rule, and `ooda_decide.md` references a pre-existing
// `RpcCallFailed` error type. Neither is a phase rename.

const FORBIDDEN_PHASE_COLLOCATIONS: &[&str] = &[
    "bridge phase",
    "bridge brain",
    "brain bridge",
    "bridge reasoner",
    "orient bridge",
    "decide bridge",
    "act bridge",
    "ooda bridge",
    "bridge pattern",
];

#[test]
fn edited_ooda_reasoners_do_not_rename_phases_as_bridge() {
    let mirrors = [
        "ooda-orient.yaml",
        "ooda-decide.yaml",
        "ooda-engineer-lifecycle.yaml",
    ];
    for f in OODA_REASONERS {
        let lc = prompt_lc(f);
        for collocation in FORBIDDEN_PHASE_COLLOCATIONS {
            assert_absent(
                &lc,
                collocation,
                f,
                "one-Brain OODA phases must not be renamed as a 'Bridge'",
            );
        }
    }
    for yaml in mirrors {
        let lc = recipe(yaml).to_lowercase();
        for collocation in FORBIDDEN_PHASE_COLLOCATIONS {
            assert_absent(
                &lc,
                collocation,
                yaml,
                "one-Brain OODA recipe mirror must not rename a phase as a 'Bridge'",
            );
        }
    }
}

// --- Layer D: durable doc (GREEN anchor) ----------------------------------
//
// CONTRIBUTING.md is the human-facing source of truth. These assertions are
// already GREEN (the doc landed in the documentation step) and guard against a
// future edit deleting the durable guidelines section.

#[test]
fn contributing_documents_all_three_guidelines() {
    let lc = repo_file("CONTRIBUTING.md").to_lowercase();
    assert_marker(&lc, "CONTRIBUTING.md");
    assert_g1(&lc, "CONTRIBUTING.md");
    assert_g2_full(&lc, "CONTRIBUTING.md");
    assert_g3(&lc, "CONTRIBUTING.md");
    // The durable section and its TOC entry must both be present.
    assert_contains(
        &lc,
        "## engineering guidelines",
        "CONTRIBUTING.md",
        "a durable Engineering Guidelines section heading",
    );
    assert_contains(
        &lc,
        "#engineering-guidelines-g1g2g3",
        "CONTRIBUTING.md",
        "a table-of-contents anchor linking to the guidelines section",
    );
}

// =========================================================================
// G4 — DURABLE-DOCUMENTATION POLICY (no-point-in-time-docs)
// =========================================================================
//
// TDD (Step 7 — write tests first). Everything below is RED until Step 8:
//   * the prompt/gate assertions fail because the G4 marker is absent from the
//     reasoner / gate prompts and their recipe mirrors today;
//   * `contributing_documents_g4_durable_docs_policy` is the GREEN anchor (the
//     CONTRIBUTING.md G4 section already landed in the documentation step).
//
// The vocabulary mirrors the CONTRIBUTING.md G4 language so Step 8 satisfies the
// contract by threading the SAME wording into the prompts — keyword invariants,
// not sentence snapshots.

/// G4 guideline NAME — the stable, universal marker. Present in `CONTRIBUTING.md`
/// today; ABSENT from the reasoner/gate prompts until Step 8 threads it, so it is
/// the RED discriminator for every G4 prompt assertion.
const G4_MARKER: &str = "no-point-in-time-docs";

/// G4 — the banned artifact: a point-in-time investigation/testing/diagnosis
/// REPORT doc committed to the repo.
const G4_REPORT: &[&str] = &["point-in-time report", "point-in-time doc", "report doc"];

/// G4 — where a finding goes INSTEAD: a GitHub issue and/or memory, not a repo doc.
const G4_SINK: &[&str] = &[
    "issue and/or memory",
    "github issue",
    "issue or memory",
    "not a repo doc",
];

/// G4 — durable feature/architecture docs remain encouraged (doc TYPE, not topic).
const G4_DURABLE: &[&str] = &["durable doc", "durable documentation"];

fn assert_g4_marker(lc: &str, file: &str) {
    assert_contains(
        lc,
        G4_MARKER,
        file,
        "a reference to the durable-documentation guideline (G4 / no-point-in-time-docs)",
    );
}

/// Full G4 flag criteria: name the guideline, name the banned artifact, name the
/// correct sink, and preserve the "durable docs are encouraged" half.
fn assert_g4(lc: &str, file: &str) {
    assert_g4_marker(lc, file);
    assert_contains_any(
        lc,
        G4_REPORT,
        file,
        "G4: name the banned artifact — a point-in-time investigation/testing/diagnosis report doc",
    );
    assert_contains_any(
        lc,
        G4_SINK,
        file,
        "G4: findings go to a GitHub issue and/or memory, not a committed repo doc",
    );
    assert_contains_any(
        lc,
        G4_DURABLE,
        file,
        "G4: durable feature/architecture docs remain encouraged (doc type, not topic)",
    );
}

// --- Layer A: engineer + planning reasoner prompts (full G4) --------------

#[test]
fn engineer_prompts_thread_g4_durable_docs_policy() {
    for f in ["engineer_system.md", "engineer_planning.md"] {
        let lc = prompt_lc(f);
        assert_g4(&lc, f);
    }
}

// --- Layer A: OODA reasoner prompts (lightweight G4 marker) ----------------
//
// The compact OODA reasoners carry the G4 NAME so their planning/authoring
// judgment inherits the durable-docs policy, without over-pinning their narrow
// output contracts (same treatment they give the G1/G2/G3 marker).

#[test]
fn ooda_reasoners_reference_g4() {
    for f in OODA_REASONERS {
        let lc = prompt_lc(f);
        assert_g4_marker(&lc, f);
    }
}

// --- Layer B: review gates (full G4 flag criteria) ------------------------

#[test]
fn review_gates_flag_g4_report_docs() {
    for f in ["merge_readiness_judge.md", "review_pipeline.md"] {
        let lc = prompt_lc(f);
        assert_g4(&lc, f);
    }
}

// --- Mirror parity: G4-bearing prompts stay in sync with their recipe .yaml -
//
// The G4-bearing subset of MIRROR_PAIRS (the OODA reasoners + the merge-readiness
// judge) must carry the G4 marker in BOTH the `.md` and its recipe mirror, so a
// `.md` edit can't silently leave the live recipe path un-guided by G4.

const G4_MIRROR_PAIRS: &[(&str, &str)] = &[
    ("ooda_orient.md", "ooda-orient.yaml"),
    ("ooda_decide.md", "ooda-decide.yaml"),
    ("ooda_brain.md", "ooda-engineer-lifecycle.yaml"),
    ("merge_readiness_judge.md", "merge-readiness-judge.yaml"),
];

#[test]
fn recipe_mirrors_carry_the_g4_marker() {
    let mut drifted = Vec::new();
    for (md, yaml) in G4_MIRROR_PAIRS {
        let md_has = prompt_lc(md).contains(G4_MARKER);
        let yaml_has = recipe(yaml).to_lowercase().contains(G4_MARKER);
        if md_has != yaml_has {
            drifted.push(format!("{md} (g4={md_has}) vs {yaml} (g4={yaml_has})"));
        }
        assert!(
            yaml_has,
            "recipe mirror {yaml} must carry the G4 marker {G4_MARKER:?} (parity with {md})"
        );
    }
    assert!(
        drifted.is_empty(),
        "prompt/recipe G4 marker drift — G4 must be present in BOTH the .md and \
         its .yaml mirror: {drifted:?}"
    );
}

// --- Layer D: durable doc (GREEN anchor) ----------------------------------
//
// CONTRIBUTING.md already documents G4 (the documentation step). These
// assertions are GREEN and guard against a future edit deleting the durable
// G4 section, its hard-rail scan reference, or the updated TOC anchor.

#[test]
fn contributing_documents_g4_durable_docs_policy() {
    let lc = repo_file("CONTRIBUTING.md").to_lowercase();
    assert_g4(&lc, "CONTRIBUTING.md");
    // The deterministic backstop scan is named, tying the soft rail to the hard.
    assert_contains(
        &lc,
        "scan_no_point_in_time_report_docs",
        "CONTRIBUTING.md",
        "G4: name the deterministic pr-verify backstop scan (the hard rail)",
    );
    // The guidelines header and its TOC anchor are updated to include G4.
    assert_contains(
        &lc,
        "g1/g2/g3/g4",
        "CONTRIBUTING.md",
        "the guidelines header names all four guidelines",
    );
    assert_contains(
        &lc,
        "#engineering-guidelines-g1g2g3g4",
        "CONTRIBUTING.md",
        "the table-of-contents anchor is updated to include G4",
    );
}
