//! Contract tests for the retired orphaned JSON-scraper surface of
//! `src/recipe_output/extract.rs` (issue #4991, PR #4992).
//!
//! An earlier phase already deleted the two dead entry points
//! (`extract_json_payload`, `extract_and_parse_json`). The completed retirement
//! also removed the JSON coercion/verdict layer they were the sole callers of:
//! 10 `pub fn`s plus the `VerdictMatch` struct. Only `strip_ansi` and
//! `strip_recipe_noise` (plus their private helpers and the `record_parse_outcome`
//! observability hook in `mod.rs`) survive as the shared public surface.
//!
//! Removed public symbols (must vanish from the crate's public API):
//!   extract_verdict, recover_json_view, balanced_objects, last_balanced_object,
//!   normalize_json_number_specials, normalize_python_json_literals,
//!   strip_json_comments, strip_json_trailing_commas,
//!   escape_json_string_control_chars, escape_json_string_invalid_escapes,
//!   and the `VerdictMatch` struct.
//!
//! This file specifies the contract in five groups:
//!   1. `scrapers_stay_absent`  — regression guard: the two previously-removed
//!      entry points stay gone.
//!   2. `dead_symbol_absence`   — the 10 fns + `VerdictMatch` MUST vanish from
//!      their definition site (`extract.rs`), the `mod.rs` re-export, and the
//!      rest of the source tree.
//!   3. `retained_surface`      — the retained public surface includes
//!      `{strip_ansi, strip_recipe_noise}` (+ the `record_parse_outcome` def in
//!      mod.rs). Those retained symbols and their re-exports MUST survive.
//!   4. `retained_behavior`     — selected ANSI and recipe-noise sanitization
//!      behavior and clean-input borrowing remain intact.
//!   5. `consumers_unchanged`   — selected production consumers keep using the
//!      retained `strip_recipe_noise` and reference no removed name.
//!
//! Absence assertions use WHOLE-WORD matching so that unrelated identifiers that
//! merely embed a removed name as a substring are not counted. In particular the
//! private `extract_balanced_objects` helpers in `stewardship/merge_judge.rs` and
//! `goal_curation/progress_reviewer.rs` (which embed `balanced_objects`) are
//! false positives and are explicitly out of scope (design R3 / ambiguity A2).

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use simard::recipe_output::{strip_ansi, strip_recipe_noise};

/// The two entry points removed by the earlier phase — must stay gone.
const REMOVED_ENTRY_POINTS: &[&str] = &["extract_json_payload", "extract_and_parse_json"];

/// The 10 orphaned public functions removed by this phase.
const REMOVED_FNS: &[&str] = &[
    "extract_verdict",
    "recover_json_view",
    "balanced_objects",
    "last_balanced_object",
    "normalize_json_number_specials",
    "normalize_python_json_literals",
    "strip_json_comments",
    "strip_json_trailing_commas",
    "escape_json_string_control_chars",
    "escape_json_string_invalid_escapes",
];

/// The struct removed alongside the verdict extractor.
const REMOVED_STRUCT: &str = "VerdictMatch";

/// The only two public helpers that survive this phase.
const RETAINED_FNS: &[&str] = &["strip_ansi", "strip_recipe_noise"];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_src(rel: &str) -> String {
    let path = manifest_dir().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// True iff `sym` appears in `haystack` as a whole identifier token — i.e. the
/// characters immediately before and after are not Rust identifier characters.
/// This avoids counting `balanced_objects` inside `extract_balanced_objects`.
fn mentions_symbol(haystack: &str, sym: &str) -> bool {
    let bytes = haystack.as_bytes();
    let is_ident = |b: u8| b == b'_' || b.is_ascii_alphanumeric();
    haystack.match_indices(sym).any(|(start, m)| {
        let end = start + m.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident(bytes[end]);
        before_ok && after_ok
    })
}

// ---------------------------------------------------------------------------
// 1. SCRAPERS STAY ABSENT — regression guard for the earlier phase's deletion.
// ---------------------------------------------------------------------------
mod scrapers_stay_absent {
    use super::*;

    #[test]
    fn removed_entry_points_have_no_definitions() {
        let extract = read_src("src/recipe_output/extract.rs");
        for name in REMOVED_ENTRY_POINTS {
            assert!(
                !mentions_symbol(&extract, name),
                "previously-removed entry point `{name}` must stay gone from extract.rs"
            );
        }
    }

    #[test]
    fn removed_entry_points_not_reexported() {
        let module = read_src("src/recipe_output/mod.rs");
        for name in REMOVED_ENTRY_POINTS {
            assert!(
                !mentions_symbol(&module, name),
                "previously-removed entry point `{name}` must not reappear in mod.rs"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. DEAD SYMBOL ABSENCE — the 10 orphaned fns and the `VerdictMatch` struct
//    remain absent from their definition site, public re-export, and source tree.
// ---------------------------------------------------------------------------
mod dead_symbol_absence {
    use super::*;

    #[test]
    fn orphaned_fn_definitions_are_deleted() {
        let extract = read_src("src/recipe_output/extract.rs");
        for name in REMOVED_FNS {
            let def = format!("pub fn {name}");
            assert!(
                !extract.contains(&def),
                "orphaned `{def}` must be deleted from src/recipe_output/extract.rs"
            );
        }
    }

    #[test]
    fn verdict_match_struct_is_deleted() {
        let extract = read_src("src/recipe_output/extract.rs");
        assert!(
            !extract.contains("pub struct VerdictMatch"),
            "orphaned `pub struct VerdictMatch` must be deleted from extract.rs"
        );
    }

    #[test]
    fn extract_rs_has_zero_residual_mentions_of_removed_symbols() {
        // Definitions, their #[cfg(test)] unit tests, and any doc cross-refs to
        // the removed symbols must all be gone from the primary deletion file.
        let extract = read_src("src/recipe_output/extract.rs");
        for name in REMOVED_FNS {
            assert!(
                !mentions_symbol(&extract, name),
                "no residual whole-word `{name}` mention (def, unit test, or doc) \
                 may remain in extract.rs"
            );
        }
        assert!(
            !mentions_symbol(&extract, REMOVED_STRUCT),
            "no residual whole-word `{REMOVED_STRUCT}` mention may remain in extract.rs"
        );
    }

    #[test]
    fn mod_rs_no_longer_reexports_removed_symbols() {
        let module = read_src("src/recipe_output/mod.rs");
        for name in REMOVED_FNS {
            assert!(
                !mentions_symbol(&module, name),
                "`{name}` must be removed from the `pub use extract::{{…}}` list in mod.rs"
            );
        }
        assert!(
            !mentions_symbol(&module, REMOVED_STRUCT),
            "`{REMOVED_STRUCT}` must be removed from the `pub use extract::{{…}}` list in mod.rs"
        );
    }

    #[test]
    fn recipe_output_module_no_longer_exposes_removed_symbols_tree_wide() {
        // Whole-word grep-zero across src/, excluding the false-positive owners
        // of the private `extract_balanced_objects` helper (design R3 / A2) and
        // this test file's sibling contract tests, if any live under src/.
        const EXCLUDE: &[&str] = &[
            "src/stewardship/merge_judge.rs",
            "src/goal_curation/progress_reviewer.rs",
        ];
        let root = manifest_dir().join("src");
        let mut offenders: Vec<String> = Vec::new();
        visit_rs_files(&root, &mut |path| {
            let rel = path
                .strip_prefix(manifest_dir())
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            if EXCLUDE.iter().any(|e| rel == *e) {
                return;
            }
            let body = fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            for name in REMOVED_FNS {
                if mentions_symbol(&body, name) {
                    offenders.push(format!("{rel}: {name}"));
                }
            }
            if mentions_symbol(&body, REMOVED_STRUCT) {
                offenders.push(format!("{rel}: {REMOVED_STRUCT}"));
            }
        });
        assert!(
            offenders.is_empty(),
            "removed symbols must not be defined, re-exported, or called anywhere in src/ \
             (excluding private-helper owners); offenders: {offenders:?}"
        );
    }

    fn visit_rs_files(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path)) {
        let entries = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("failed to read directory {}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| {
                panic!(
                    "failed to read an entry in directory {}: {e}",
                    dir.display()
                )
            });
            let path = entry.path();
            let metadata = fs::metadata(&path)
                .unwrap_or_else(|e| panic!("failed to read metadata for {}: {e}", path.display()));
            if metadata.is_dir() {
                visit_rs_files(&path, f);
            } else if path.extension().is_some_and(|e| e == "rs") {
                f(&path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. RETAINED SURFACE — {strip_ansi, strip_recipe_noise} remain public, and the
//    `record_parse_outcome` definition in mod.rs remains present.
// ---------------------------------------------------------------------------
mod retained_surface {
    use super::*;

    #[test]
    fn retained_public_helpers_survive_in_extract_rs() {
        let extract = read_src("src/recipe_output/extract.rs");
        for name in RETAINED_FNS {
            let def = format!("pub fn {name}");
            assert!(
                extract.contains(&def),
                "retained helper `{def}` must NOT be deleted from extract.rs"
            );
        }
    }

    #[test]
    fn retained_reexports_survive_in_mod_rs() {
        let module = read_src("src/recipe_output/mod.rs");
        for name in RETAINED_FNS {
            assert!(
                mentions_symbol(&module, name),
                "retained re-export `{name}` must remain in the mod.rs `pub use` list"
            );
        }
    }

    #[test]
    fn record_parse_outcome_definition_is_untouched() {
        let module = read_src("src/recipe_output/mod.rs");
        assert!(
            module.contains("pub fn record_parse_outcome"),
            "`record_parse_outcome` must stay defined in mod.rs (out of scope for removal)"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. RETAINED BEHAVIOR — selected sanitization and clean-path borrowing
//    contracts remain intact through the retained public API.
// ---------------------------------------------------------------------------
mod retained_behavior {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_sequences_and_preserves_text() {
        let colored = "\u{1b}[31mADMIT\u{1b}[0m";
        assert_eq!(strip_ansi(colored), "ADMIT");
    }

    #[test]
    fn strip_ansi_borrows_clean_input() {
        assert!(
            matches!(strip_ansi("plain text with no escapes"), Cow::Borrowed(_)),
            "ANSI-free input must pass through borrowed (no allocation)"
        );
    }

    #[test]
    fn strip_recipe_noise_drops_launcher_preamble_and_keeps_agent_output() {
        let raw = concat!(
            "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference).\n",
            "INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot\n",
            "agent answer"
        );
        let cleaned = strip_recipe_noise(raw);
        assert!(
            cleaned.contains("agent answer"),
            "the agent-output line must survive denoising: {cleaned}"
        );
        assert!(
            !cleaned.contains("NODE_OPTIONS") && !cleaned.contains("launching copilot"),
            "launcher-preamble noise must be dropped: {cleaned}"
        );
    }

    #[test]
    fn strip_recipe_noise_borrows_fully_clean_input() {
        assert!(
            matches!(strip_recipe_noise("plain agent output"), Cow::Borrowed(_)),
            "noise-free input must pass through borrowed (no allocation)"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. CONSUMERS UNCHANGED — selected production consumers keep using the retained
//    `strip_recipe_noise` and reference no removed name.
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
    fn consumers_never_reference_removed_names() {
        for rel in CONSUMERS {
            let src = read_src(rel);
            for name in REMOVED_FNS.iter().chain(REMOVED_ENTRY_POINTS.iter()) {
                assert!(
                    !mentions_symbol(&src, name),
                    "{rel} must not reference the removed `{name}`"
                );
            }
            assert!(
                !mentions_symbol(&src, REMOVED_STRUCT),
                "{rel} must not reference the removed `{REMOVED_STRUCT}`"
            );
        }
    }
}
