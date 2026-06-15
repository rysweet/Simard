//! Regression gate for ws2 #2295: distilled procedures must surface in
//! the OODA preparation phase and in base-type adapter turn preparation
//! through the same unified, tokenized recall path used by bootstrap
//! procedures.
//!
//! **Why this test exists.**
//! Prior to ws2 #2295, two recall paths coexisted:
//!
//! 1. `memory_consolidation::preparation_memory_operations_with_active_slugs`
//!    tokenized the objective text (PR-C, #2281) and fanned out per-token
//!    `recall_procedure` calls, deduped by `node_id`, and so surfaced both
//!    bootstrap procedures and distilled procedures whose trigger lists
//!    overlapped any token >= 3 chars.
//! 2. `base_type_turn::prepare_turn_context` (used by the engineer /
//!    base-type adapters) passed the **entire raw objective sentence** to
//!    a single `recall_procedure(objective, 5)` call. The Cypher
//!    `name CONTAINS '<full sentence>'` predicate never matched any
//!    stored procedure because no procedure name embeds a natural
//!    sentence. Effect: a steady-state of 3 bootstrap procedures
//!    surfacing while the 3 distilled procedures written every cycle
//!    were invisible to the prompt — exactly the
//!    "only the PR-C bootstrap procedures ever appear" symptom in the
//!    cycle 238 report.
//!
//! The unified helper
//! [`simard::recall_procedures_for_objective`] is the single entry point
//! both adapters now share. This gate locks the contract: every required
//! invariant fails the build if it regresses.
//!
//! **Live schema.** The test runs against [`NativeCognitiveMemory::in_memory`]
//! which constructs a real `lbug::Database` and executes the real
//! `SCHEMA_DDL` for the `cognitive_memory.ladybug` schema. There is no
//! storage-layer mock anywhere in the call path: every Cypher statement
//! exercised below hits LadybugDB and returns rows from the real graph.
//!
//! **Hermetic env.** The in-memory backend already isolates itself in a
//! `tempfile::TempDir`, so no `SIMARD_STATE_ROOT` / `$HOME` mutation is
//! needed — the harness simply asserts on returned values.

#![cfg(unix)]

use serial_test::serial;

use simard::cognitive_memory::bootstrap_procedures::{
    BOOTSTRAP_PROCEDURES, seed_bootstrap_procedures,
};
use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use simard::ooda_loop::{ActionKind, compose_procedure_name, derive_triggers_from_objective};
use simard::recall_procedures_for_objective;

// ────────────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────────────

/// Wrap a literal trigger string in the standard PR-C name shape
/// `{pattern}:{scope} | triggers: {csv}` so the regression-gate
/// procedures look structurally identical to what
/// `compose_procedure_name` emits in production.
fn make_name(pattern: &str, scope: &str, triggers: &str) -> String {
    format!("{pattern}:{scope} | triggers: {triggers}")
}

// ────────────────────────────────────────────────────────────────────────────
// Gate 1: explicit user spec — `foo bar` trigger / `foo` objective
// ────────────────────────────────────────────────────────────────────────────

/// Mirrors the explicit regression test from the task spec:
///
/// 1. Store a procedure whose trigger string contains `foo bar`.
/// 2. Run recall with an objective that contains `foo`.
/// 3. The procedure is returned.
/// 4. Its name (and so its trigger list) is intact — byte-for-byte
///    identical to what was stored, no mid-word truncation.
///
/// Uses the live `cognitive_memory.ladybug` schema via
/// `NativeCognitiveMemory::in_memory`.
#[test]
#[serial(cognitive_memory)]
fn distilled_procedure_with_foo_bar_trigger_surfaces_for_foo_objective() {
    let mem = NativeCognitiveMemory::in_memory()
        .expect("construct in-memory NativeCognitiveMemory with real SCHEMA_DDL");

    // 1. Store a distilled-shape procedure with the user-spec trigger.
    let stored_name = make_name("foo-recall", "ad-hoc", "foo,bar");
    let steps = [
        "step-1: probe foo".to_string(),
        "step-2: verify bar".to_string(),
    ];
    let prereqs: [String; 0] = [];
    mem.store_procedure(&stored_name, &steps, &prereqs)
        .expect("store procedure with foo,bar trigger");

    // 2. Recall with an objective containing `foo`.
    let hits = recall_procedures_for_objective(&mem, "fix the foo issue", 5)
        .expect("unified recall pipeline must not error");

    // 3. The procedure is returned (no other procedure stored, so the
    //    expected match count is exactly 1).
    let foo_hits: Vec<_> = hits.iter().filter(|p| p.name == stored_name).collect();
    assert_eq!(
        foo_hits.len(),
        1,
        "expected exactly one hit for the 'foo,bar' procedure under objective \
         'fix the foo issue'; got: {:?}",
        hits.iter().map(|p| &p.name).collect::<Vec<_>>(),
    );

    // 4. The recalled name MUST be byte-for-byte identical to what was
    //    stored. Any mid-word truncation (the cycle-238 symptom — a
    //    name ending in `,distill,g` instead of `,distill,general`)
    //    would fail this assertion. Use exact comparison rather than
    //    `contains` to refuse silent shortening.
    let recalled = foo_hits[0];
    assert_eq!(
        recalled.name, stored_name,
        "trigger list must round-trip intact; storage or read path corrupted the name",
    );

    // Double-belt: the explicit `bar` trigger must appear in the
    // recalled name. A regression that drops trailing triggers would
    // leave `foo` in place (it's earlier) but lose `bar`.
    assert!(
        recalled.name.contains("bar"),
        "recalled name must still contain the 'bar' trigger; got: {}",
        recalled.name,
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Gate 2: bootstrap AND distilled procedures both surface
// ────────────────────────────────────────────────────────────────────────────

/// Once the bootstrap set is seeded and a distilled procedure has been
/// written for goal `g1`, an objective like "ready to merge PR #1234"
/// must surface BOTH a bootstrap procedure (`pr-merge:bootstrap`) AND
/// the distilled procedure (`pr-merge:g1`) — proving the unified path
/// no longer treats the two sources differently.
///
/// This is the structural fix for the cycle-238 symptom: distilled
/// procedures were invisible because adapters bypassed the tokenizer.
#[test]
#[serial(cognitive_memory)]
fn bootstrap_and_distilled_pr_merge_procedures_both_surface() {
    let mem = NativeCognitiveMemory::in_memory()
        .expect("construct in-memory NativeCognitiveMemory with real SCHEMA_DDL");

    // Seed the canonical bootstrap set.
    let seeded = seed_bootstrap_procedures(&mem).expect("seed bootstrap procedures");
    assert_eq!(
        seeded,
        BOOTSTRAP_PROCEDURES.len(),
        "expected all bootstrap procedures to be newly seeded into an empty store",
    );

    // Write a distilled procedure scoped to goal `g1` — exactly what
    // `ooda_loop::cycle` does on a successful AdvanceGoal outcome.
    let distilled_name = compose_procedure_name(
        ActionKind::AdvanceGoal,
        Some("g1"),
        "merge PR #1234 review change",
        "engineer review change",
    );
    mem.store_procedure(
        &distilled_name,
        &["engineer review change".to_string()],
        &[],
    )
    .expect("store distilled procedure for g1");

    // Recall under an objective that should fire BOTH the bootstrap
    // `merge` trigger and the distilled `g1` / `merge` triggers.
    let hits = recall_procedures_for_objective(&mem, "ready to merge PR review", 10)
        .expect("unified recall pipeline must not error");

    let names: Vec<&str> = hits.iter().map(|p| p.name.as_str()).collect();

    assert!(
        names.contains(&"pr-merge:bootstrap | triggers: merge,pr,merge-pr,landing,ready-to-merge"),
        "the bootstrap pr-merge procedure must surface; got: {names:?}"
    );
    assert!(
        names.contains(&distilled_name.as_str()),
        "the distilled pr-merge procedure ('{distilled_name}') must surface alongside the bootstrap; got: {names:?}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Gate 3: case-folding consistency
// ────────────────────────────────────────────────────────────────────────────

/// The write path lowercases base/derived triggers (via
/// `derive_triggers_from_objective` and the lowercase string literals in
/// `BOOTSTRAP_PROCEDURES`). The read path lowercases tokens via
/// `tokenize_objective`. Cypher `CONTAINS` is case-sensitive in
/// Kuzu/lbug, so the round-trip only works if both sides agree on
/// lowercase. This gate proves it: store with lowercase, recall with
/// SHOUTED uppercase, and assert a hit.
#[test]
#[serial(cognitive_memory)]
fn recall_is_case_insensitive_via_consistent_lowercase_folding() {
    let mem = NativeCognitiveMemory::in_memory()
        .expect("construct in-memory NativeCognitiveMemory with real SCHEMA_DDL");

    // Stored name uses lowercase, which is the production invariant.
    let stored_name = make_name("foo-recall", "ad-hoc", "foo,bar");
    mem.store_procedure(&stored_name, &["step".to_string()], &[])
        .expect("store lowercase-triggered procedure");

    // Objective text uses SHOUTED uppercase. The tokenizer must
    // lowercase before issuing the Cypher CONTAINS, otherwise the
    // case-sensitive backend returns zero rows.
    let hits = recall_procedures_for_objective(&mem, "FIX THE FOO ISSUE", 5)
        .expect("unified recall pipeline must not error");

    assert!(
        hits.iter().any(|p| p.name == stored_name),
        "uppercase objective must still hit the lowercase-stored procedure; \
         got: {:?}",
        hits.iter().map(|p| &p.name).collect::<Vec<_>>(),
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Gate 4: short-token derived triggers no longer pollute names
// ────────────────────────────────────────────────────────────────────────────

/// The cycle-238 "trigger list looks truncated" symptom was driven by
/// `derive_triggers_from_objective` emitting 1- or 2-char file
/// extensions (`.g` → `g`, `.rs` → `rs`) that the read-side tokenizer
/// could never match (it floors at 3 chars). When such a token landed
/// as the trailing entry it visually mimicked mid-word truncation.
///
/// This gate proves the floor was raised and that 1/2-char extensions
/// no longer appear in derived triggers, while 3+ char extensions
/// still do.
#[test]
fn derived_triggers_no_longer_emit_sub_three_char_extensions() {
    // Sub-3-char extensions: must be dropped.
    let derived = derive_triggers_from_objective(
        "touch .g read cycle.rs build main.go ship script.py",
        "short-ext probe",
    );
    for shorty in ["g", "rs", "go", "py"] {
        assert!(
            !derived.iter().any(|t| t == shorty),
            "1/2-char file extension '{shorty}' must NOT appear in derived triggers; \
             got: {derived:?}"
        );
    }

    // 3+ char extensions: still extracted.
    let kept =
        derive_triggers_from_objective("update config.toml and manifest.json plus index.html", "");
    for keep in ["toml", "json", "html"] {
        assert!(
            kept.iter().any(|t| t == keep),
            "3+ char file extension '{keep}' must still appear in derived triggers; \
             got: {kept:?}"
        );
    }
}
