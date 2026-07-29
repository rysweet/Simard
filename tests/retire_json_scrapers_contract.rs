//! Contract tests for the recipe-output sanitizer surface.
//!
//! The retained public sanitizer API consists of `strip_ansi` and
//! `strip_recipe_noise`. These helpers remove ANSI escapes and known
//! recipe-runner, logging, and launcher noise while preserving
//! `Cow::Borrowed` for input that needs no sanitization.
//!
//! These tests enforce the finished API boundary, sanitizer behavior, borrowing
//! contract, and continued use by production consumers. Whole-word matching
//! keeps absence guards from matching names embedded in unrelated identifiers.

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;

use simard::recipe_output::{strip_ansi, strip_recipe_noise};

/// Unsupported entry-point names checked by the API absence guards.
const REMOVED_ENTRY_POINTS: &[&str] = &["extract_json_payload", "extract_and_parse_json"];

/// Unsupported function names checked by the API absence guards.
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

/// Unsupported type name checked by the API absence guards.
const REMOVED_STRUCT: &str = "VerdictMatch";

/// The two retained public helpers.
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
// 1. SANITIZER API BOUNDARY — unsupported entry points remain unavailable.
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
// 2. UNSUPPORTED SYMBOL ABSENCE — excluded symbols remain unavailable from the
//    implementation, public re-export, and source tree.
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
        // Definitions, unit tests, and documentation references must all respect
        // the sanitizer API boundary.
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
        // Exclude private helpers whose compound names contain a checked symbol.
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
// 3. PUBLIC SURFACE — {strip_ansi, strip_recipe_noise} remain public, and the
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
// 4. SANITIZER BEHAVIOR — sanitization and clean-path borrowing remain intact
//    through the public API.
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
// 5. CONSUMER CONTRACT — selected production consumers use `strip_recipe_noise`
//    and reference no unsupported name.
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
