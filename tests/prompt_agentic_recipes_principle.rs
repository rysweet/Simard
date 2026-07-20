//! Drift-guard for the "agentic-recipes-first" reasoning principle.
//!
//! Motivating incident: Simard's Overseer failed to self-heal a bug that
//! crash-looped seven standing goals 286+ times. The reflexive fix first
//! proposed was imperative Rust plumbing (wiring `record_step_failure` into
//! every failure-origin site, consecutive-failure counters, an
//! N-identical-failure threshold heuristic) — exactly the antipattern to
//! eliminate. The operator diagnosed the whole crash-loop *agentically* in a
//! handful of journal reads. The lesson: intelligence-requiring problems
//! (health assessment, root-cause, remediation, scheduling, verification,
//! admission, cleanup) must be solved as agentic recipes on thin deterministic
//! rails, never as imperative code or one-off heuristics.
//!
//! This suite is the thin deterministic RAIL that pins that principle into
//! Simard's own REASONING prompt assets. It asserts that one canonical,
//! byte-identical block is embedded in every asset where Simard decides *how*
//! to solve a problem, that the block carries the pinned keyword invariants and
//! references (does not duplicate) engineer `G3`, and that it sits *before* each
//! prompt's output-contract section so it can never silently drift below the
//! output contract.
//!
//! Design note: this is a drift-guard rail. It checks keyword invariants and
//! ordering, not full snapshots, so additive prose in any host prompt stays
//! safe, while any missing, altered, partial, or mis-ordered copy fails the
//! suite closed.

use std::fs;
use std::path::PathBuf;

/// The pinned canonical sentence. It MUST appear byte-identical in every target
/// asset. Tune-wording flexibility is allowed *around* it (per-host voice), but
/// this exact substring is the drift anchor.
const CANONICAL_SENTENCE: &str = "When a problem requires intelligence or judgment, solve it by composing, reusing, or inventing deterministic recipes of agentic steps run via the recipe runner";

/// Keyword invariant: the recipe runner is the execution vehicle for judgment.
const RECIPE_RUNNER: &str = "recipe runner";

/// Keyword invariant: imperative code is confined to the thin deterministic
/// rail(s). The singular form is a substring of the plural, so this matches
/// both "thin deterministic rail" and "thin deterministic rails".
const RAIL_PHRASE: &str = "thin deterministic rail";

/// The block must REFERENCE engineer `G3` (single source of truth in
/// `engineer_system.md`), not restate it.
const G3_REF: &str = "G3";
const ENGINEER_SYSTEM_REF: &str = "engineer_system.md";

/// The nine REASONING assets that must carry the canonical block, each paired
/// with the output-contract anchor(s) the block must precede. `engineer_system.md`
/// is intentionally absent — it already owns G3, which the block references.
///
/// For each file the first anchor found (searched in listed order) is used as
/// the positional boundary; anchors use the `## ` header prefix (or a unique
/// contract phrase) so prose mentions of "OPTIONS"/"DECISION" do not match.
const TARGETS: &[(&str, &[&str])] = &[
    ("ooda_brain.md", &["## OPTIONS"]),
    ("ooda_orient.md", &["## DECISION"]),
    ("ooda_decide.md", &["## OPTIONS"]),
    ("overseer/observe.md", &["## OUTPUT"]),
    ("overseer/escalation_triage.md", &["## OUTPUT"]),
    ("overseer/deploy_gate.md", &["## OUTPUT"]),
    ("goal_decomposition.md", &["## Output"]),
    ("improvement_curator_system.md", &["## Expected Outcomes"]),
    ("engineer_planning.md", &["Return ONLY"]),
];

fn prompt(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard")
        .join(rel);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read prompt asset {}: {e}", path.display()))
}

#[test]
fn canonical_sentence_embedded_in_all_nine_reasoners() {
    assert_eq!(
        TARGETS.len(),
        9,
        "exactly nine reasoning assets must carry the block"
    );
    for (rel, _) in TARGETS {
        let contents = prompt(rel);
        assert!(
            contents.contains(CANONICAL_SENTENCE),
            "{rel} must embed the pinned canonical agentic-recipes-first sentence \
             byte-identical:\n  {CANONICAL_SENTENCE:?}"
        );
    }
}

#[test]
fn keyword_invariants_present_alongside_the_sentence() {
    for (rel, _) in TARGETS {
        let contents = prompt(rel);
        assert!(
            contents.contains(RECIPE_RUNNER),
            "{rel} must name the {RECIPE_RUNNER:?} as the vehicle for judgment"
        );
        assert!(
            contents.contains(RAIL_PHRASE),
            "{rel} must confine imperative code to the {RAIL_PHRASE:?}(s) \
             (dispatch, I/O, storage, scheduling ticks)"
        );
    }
}

#[test]
fn block_references_engineer_g3_without_duplicating_it() {
    for (rel, _) in TARGETS {
        let contents = prompt(rel);
        assert!(
            contents.contains(G3_REF),
            "{rel} block must reference engineer guideline {G3_REF:?} (single source of truth)"
        );
        assert!(
            contents.contains(ENGINEER_SYSTEM_REF),
            "{rel} block must point at {ENGINEER_SYSTEM_REF:?} as the G3 source of truth, \
             not restate the guideline"
        );
    }
}

#[test]
fn canonical_sentence_precedes_the_output_contract() {
    for (rel, anchors) in TARGETS {
        let contents = prompt(rel);

        let sentence_at = contents.find(CANONICAL_SENTENCE).unwrap_or_else(|| {
            panic!("{rel} must embed the pinned canonical sentence (see other test)")
        });

        let anchor_at = anchors
            .iter()
            .filter_map(|a| contents.find(a).map(|idx| (idx, *a)))
            .min_by_key(|(idx, _)| *idx)
            .unwrap_or_else(|| {
                panic!(
                    "{rel} must contain one of its expected output-contract anchors {anchors:?} \
                     so the block's placement can be pinned"
                )
            });
        let (anchor_idx, anchor_str) = anchor_at;

        assert!(
            sentence_at < anchor_idx,
            "{rel}: the agentic-recipes-first block (byte {sentence_at}) must appear BEFORE \
             the output-contract anchor {anchor_str:?} (byte {anchor_idx}) so it frames how to \
             reason without touching what to emit; a later edit must not move it below the \
             output contract"
        );
    }
}
