//! Recipe-asset regression for the file-channel transport (issues #2640/#2692).
//!
//! The fix moves every unbounded recipe context value off `argv` and into a
//! temp file, passing only `-c <key>_path=<abs>`. For that to work the prompt
//! assets must READ the file (`{{<key>_path}}`) instead of interpolating the raw
//! payload (`{{<key>}}`) — otherwise the Rust caller and the recipe disagree and
//! the agent sees an empty/short context.
//!
//! This test reads the shipped YAMLs from the source tree and asserts each
//! affected recipe (journal draft, journal de-jargon, distillation, merge judge)
//! references its `*_path` var and NO LONGER interpolates the raw payload var.
//! It also covers the audited Tier-A sites beyond the journal
//! (`episodes_path`, `pr_body_path`), per the whole-repo spawn-site audit.
//!
//! Fully hermetic (reads files only). TDD status: RED — it fails now because the
//! assets still interpolate the raw `{{day_context}}` / `{{draft}}` /
//! `{{episodes}}` / `{{pr_body}}` placeholders; it goes GREEN once the assets are
//! migrated to the `*_path` reads.

use std::path::PathBuf;

/// Absolute path to a shipped recipe asset in the source tree.
fn recipe_path(filename: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard/recipes")
        .join(filename)
}

fn read_recipe(filename: &str) -> String {
    let path = recipe_path(filename);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("recipe asset {} must be readable: {e}", path.display()))
}

/// Assert `filename` interpolates the file-path placeholder `{{<new_var>}}` and
/// NO LONGER interpolates the raw payload placeholder `{{<raw_var>}}`.
fn assert_reads_path_not_raw(filename: &str, raw_var: &str, path_var: &str) {
    let body = read_recipe(filename);
    let raw = format!("{{{{{raw_var}}}}}"); // {{raw_var}}
    let pathed = format!("{{{{{path_var}}}}}"); // {{path_var}}

    assert!(
        body.contains(&pathed),
        "{filename}: must read the file-channel var {pathed} (issue #2692)"
    );
    assert!(
        !body.contains(&raw),
        "{filename}: must NOT interpolate the raw payload placeholder {raw} — that \
         re-inlines the unbounded value the file channel exists to avoid"
    );
}

#[test]
fn journal_narrative_reads_day_context_path() {
    assert_reads_path_not_raw("journal-narrative.yaml", "day_context", "day_context_path");
}

#[test]
fn journal_plain_language_reads_draft_path() {
    assert_reads_path_not_raw("journal-plain-language.yaml", "draft", "draft_path");
}

#[test]
fn distill_episodes_reads_episodes_path() {
    // Audited Tier-A site: distillation inlined the whole `episodes` batch.
    assert_reads_path_not_raw("distill-episodes.yaml", "episodes", "episodes_path");
}

#[test]
fn merge_readiness_judge_reads_pr_body_path() {
    // Audited Tier-A site: the merge judge inlined an arbitrary-size PR body.
    assert_reads_path_not_raw("merge-readiness-judge.yaml", "pr_body", "pr_body_path");
}

/// The `context:` default block must declare the new `*_path` key so the
/// substitution always resolves even if a caller omits it (mirrors the existing
/// declared defaults).
#[test]
fn journal_recipes_declare_the_path_context_keys() {
    let narrative = read_recipe("journal-narrative.yaml");
    let plain = read_recipe("journal-plain-language.yaml");
    assert!(
        narrative.contains("day_context_path"),
        "journal-narrative.yaml must declare/use day_context_path"
    );
    assert!(
        plain.contains("draft_path"),
        "journal-plain-language.yaml must declare/use draft_path"
    );
}
