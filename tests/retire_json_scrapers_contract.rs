//! TDD Step 7 — failing contract tests for retiring the two dead JSON-scrape
//! functions (`extract_json_payload`, `extract_and_parse_json`).
//!
//! Issue #4991 removes ONLY those two `pub fn`s from
//! `src/recipe_output/extract.rs` and their re-export in
//! `src/recipe_output/mod.rs`, deletes their exclusive test battery, and
//! deletes their sole non-production caller
//! (`merge_judge_envelope_survives_banner_and_ansi` in `recipe_brain.rs`).
//!
//! This file specifies that contract three ways:
//!   1. `absence_guards`   — the two names MUST vanish from the primary sites
//!      (these FAIL today because the code still exists; they pass once the
//!      deletion lands). This is the driving TDD signal.
//!   2. `retention_guards` — every retained `pub` helper and its re-export MUST
//!      survive; the public API only shrinks by exactly the two names.
//!   3. `retained_behavior` — the JSON-hardening the deleted `extract_and_parse_json`
//!      used to compose is preserved by rewriting the two boundary cases onto the
//!      retained primitives (`strip_recipe_noise` + `last_balanced_object`,
//!      `recover_json_view`). These pass today and MUST keep passing.
//!   4. `consumers_unchanged` — the out-of-scope production consumers keep
//!      compiling against the retained `strip_recipe_noise` and never referenced
//!      the deleted names.
//!
//! Absence assertions target the DEFINITION / RE-EXPORT forms and the primary
//! deletion sites only. They are deliberately scoped to `extract.rs`,
//! `mod.rs`, and `recipe_brain.rs` so that string-literal references inside the
//! inverted absence-guards elsewhere in the tree are NOT counted as callers.

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use simard::recipe_output::{last_balanced_object, recover_json_view, strip_recipe_noise};

const FN_PAYLOAD: &str = "extract_json_payload";
const FN_PARSE: &str = "extract_and_parse_json";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_src(rel: &str) -> String {
    let path = manifest_dir().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// 1. ABSENCE GUARDS — the driving TDD failures. The two dead functions, their
//    re-export, and their sole non-production caller MUST be gone.
// ---------------------------------------------------------------------------
mod absence_guards {
    use super::*;

    #[test]
    fn extract_json_payload_definition_is_deleted() {
        let extract = read_src("src/recipe_output/extract.rs");
        assert!(
            !extract.contains("pub fn extract_json_payload"),
            "`pub fn {FN_PAYLOAD}` must be deleted from src/recipe_output/extract.rs"
        );
    }

    #[test]
    fn extract_and_parse_json_definition_is_deleted() {
        let extract = read_src("src/recipe_output/extract.rs");
        assert!(
            !extract.contains("pub fn extract_and_parse_json"),
            "`pub fn {FN_PARSE}` must be deleted from src/recipe_output/extract.rs"
        );
    }

    #[test]
    fn extract_rs_has_zero_residual_mentions() {
        // Section-header comments, doc cross-references, and the whole exclusive
        // test battery for the two functions must all be gone — a grep-zero gate
        // scoped to the primary deletion file (design R2).
        let extract = read_src("src/recipe_output/extract.rs");
        assert!(
            !extract.contains(FN_PAYLOAD),
            "no residual `{FN_PAYLOAD}` mention (defs, section comments, docs, or exclusive tests) \
             may remain in extract.rs"
        );
        assert!(
            !extract.contains(FN_PARSE),
            "no residual `{FN_PARSE}` mention (defs, section comments, docs, or exclusive tests) \
             may remain in extract.rs"
        );
    }

    #[test]
    fn mod_rs_no_longer_reexports_the_two_names() {
        let module = read_src("src/recipe_output/mod.rs");
        assert!(
            !module.contains(FN_PAYLOAD),
            "`{FN_PAYLOAD}` must be removed from the `pub use` re-export list in mod.rs"
        );
        assert!(
            !module.contains(FN_PARSE),
            "`{FN_PARSE}` must be removed from the `pub use` re-export list in mod.rs"
        );
    }

    #[test]
    fn recipe_brain_drops_the_sole_non_production_caller() {
        let brain = read_src("src/ooda_brain/recipe_brain.rs");
        assert!(
            !brain.contains("merge_judge_envelope_survives_banner_and_ansi"),
            "the sole non-production caller test must be deleted from recipe_brain.rs"
        );
        assert!(
            !brain.contains(FN_PAYLOAD),
            "recipe_brain.rs must have zero references to the deleted `{FN_PAYLOAD}`"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. RETENTION GUARDS — the public API shrinks by EXACTLY the two names.
//    Every other retained helper and its re-export survives untouched.
// ---------------------------------------------------------------------------
mod retention_guards {
    use super::*;

    const RETAINED_PUB_FNS: &[&str] = &[
        "pub fn strip_ansi",
        "pub fn strip_recipe_noise",
        "pub fn last_balanced_object",
        "pub fn balanced_objects",
        "pub fn recover_json_view",
        "pub fn strip_json_comments",
        "pub fn strip_json_trailing_commas",
        "pub fn escape_json_string_control_chars",
        "pub fn escape_json_string_invalid_escapes",
        "pub fn normalize_json_number_specials",
        "pub fn normalize_python_json_literals",
        "pub fn extract_verdict",
    ];

    const RETAINED_REEXPORTS: &[&str] = &[
        "VerdictMatch",
        "balanced_objects",
        "escape_json_string_control_chars",
        "escape_json_string_invalid_escapes",
        "extract_verdict",
        "last_balanced_object",
        "normalize_json_number_specials",
        "normalize_python_json_literals",
        "recover_json_view",
        "strip_ansi",
        "strip_json_comments",
        "strip_json_trailing_commas",
        "strip_recipe_noise",
    ];

    #[test]
    fn all_retained_public_helpers_survive_in_extract_rs() {
        let extract = read_src("src/recipe_output/extract.rs");
        for def in RETAINED_PUB_FNS {
            assert!(
                extract.contains(def),
                "retained helper `{def}` must NOT be deleted from extract.rs"
            );
        }
        assert!(
            extract.contains("pub struct VerdictMatch"),
            "retained `pub struct VerdictMatch` must survive in extract.rs"
        );
    }

    #[test]
    fn all_retained_reexports_survive_in_mod_rs() {
        let module = read_src("src/recipe_output/mod.rs");
        for ident in RETAINED_REEXPORTS {
            assert!(
                module.contains(ident),
                "retained re-export `{ident}` must remain in the mod.rs `pub use` list"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. RETAINED BEHAVIOR — the JSON hardening the deleted `extract_and_parse_json`
//    composed is preserved by the retained primitives. These are the two
//    boundary tests rewritten onto retained public helpers (design step 4).
// ---------------------------------------------------------------------------
mod retained_behavior {
    use super::*;

    #[test]
    fn recovers_json_payload_behind_launcher_preamble() {
        // Formerly `extract_json_payload(&raw)`; rewritten onto the retained
        // `strip_recipe_noise` + `last_balanced_object` pipeline it used to call.
        let raw = format!(
            "{marker}\n{launching}\n{{\"facts\":[]}}",
            marker = "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference).",
            launching = "INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot",
        );
        let cleaned = strip_recipe_noise(&raw);
        let payload = last_balanced_object(cleaned.as_ref())
            .expect("balanced object must be recovered from behind launcher preamble");
        assert_eq!(payload, "{\"facts\":[]}");
        assert!(
            !payload.contains("launching copilot") && !payload.contains("NODE_OPTIONS"),
            "recovered payload must not carry launcher-preamble noise: {payload}"
        );
    }

    #[test]
    fn recover_json_view_does_not_forge_literal_across_block_comment() {
        // Whole-token guarantee: a block comment must not splice `T` + `rue` into
        // a `True` the literal view then "recovers". The recovered view is owned
        // (the comment was stripped) but still malformed, so a strict parse fails
        // — the split token is never masked. Rewritten to exercise the retained
        // `recover_json_view` directly, without the deleted `extract_and_parse_json`.
        let fixed = recover_json_view(r#"{"ready": T/*x*/rue}"#);
        assert!(
            matches!(fixed, Cow::Owned(_)),
            "stripping the block comment must yield an owned, rewritten view"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(fixed.as_ref()).is_err(),
            "the split `T/*x*/rue` token must remain malformed — never forged into `True`"
        );
    }

    #[test]
    fn recover_json_view_is_a_noop_on_valid_json() {
        // The retained recovery view must borrow valid JSON byte-for-byte — the
        // provable no-op property the deleted parser relied on for fail-closed
        // behavior on non-recoverable input.
        let view = recover_json_view(r#"{"decision":"admit","score":0.9}"#);
        assert!(
            matches!(view, Cow::Borrowed(_)),
            "valid JSON must pass through recover_json_view unchanged (borrowed)"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. CONSUMERS UNCHANGED — out-of-scope production consumers keep compiling
//    against the retained `strip_recipe_noise` and never used the deleted names.
// ---------------------------------------------------------------------------
mod consumers_unchanged {
    use super::*;

    const CONSUMERS: &[&str] = &[
        "src/journal/pr_source.rs",
        "src/goal_curation/recipe_progress_checker.rs",
    ];

    #[test]
    fn consumers_still_use_the_retained_shared_denoiser() {
        for rel in CONSUMERS {
            let src = read_src(rel);
            assert!(
                src.contains("strip_recipe_noise"),
                "{rel} must keep using the retained `strip_recipe_noise`"
            );
        }
    }

    #[test]
    fn consumers_never_reference_the_deleted_names() {
        for rel in CONSUMERS {
            let src = read_src(rel);
            assert!(
                !src.contains(FN_PAYLOAD) && !src.contains(FN_PARSE),
                "{rel} must not reference the deleted `{FN_PAYLOAD}` / `{FN_PARSE}`"
            );
        }
    }
}
