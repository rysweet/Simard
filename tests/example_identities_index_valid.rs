//! Data-driven staleness gate for the SELF-MAINTAINING example-identity index
//! (issue #4274 — "eliminate the example-identity README name-list treadmill").
//!
//! Adding a new example identity under `examples/identities/<name>/` must be a
//! **pure data change inside that package**: zero edits to `examples/identities/
//! README.md`, zero edits to `docs/concepts/pluggable-identity.md`, and no
//! ordinal renumbering ("ninth"/"tenth"/…). To make that true, the shared
//! README's per-identity list is a **generated marker block** derived from the
//! package directories, each identity's prose blurb lives in its own package
//! `README.md`, and the concept doc keeps only non-enumerated framing.
//!
//! This integration test locks those invariants in the existing
//! `*_assets_valid.rs` data-driven style. It exercises the name-agnostic
//! generator (`list_example_identities` / `render_identity_index`, added to
//! `src/identity/example_loader.rs` and re-exported from `src/identity/mod.rs`)
//! and asserts, against the real repo:
//!
//!   1. `generated_index_is_in_sync` — the committed marker block is byte-for-byte
//!      equal to `render_identity_index(examples/identities)`. Doubles as the
//!      one-command regenerator via `UPDATE_EXPECT=1` (snapshot auto-fix style).
//!   2. `every_indexed_package_has_a_readme` — every derived package ships its
//!      own non-empty `README.md` (self-describing; the blurb is package-owned).
//!   3. `shared_docs_have_no_ordinal_enumeration` — neither the shared README nor
//!      the concept doc contains "an Nth (worked) example" ordinal phrasing.
//!   4. `all_ten_shipped_identities_are_indexed` — content preserved: the ten
//!      shipped identities are all still derived into the index.
//!   5. `phantom_sommelier_name_list_is_gone` — the hand-maintained intro
//!      name-list (which enumerated a phantom `sommelier` with no package) is
//!      removed from the shared README.
//!
//! It also pins the generator's behavior against synthetic tempdir packages
//! (derivation, alphabetical order, exclusion rules, fail-visible errors,
//! name validation, and the "adding a package is a pure data change" property),
//! and unit-tests the pure marker/ordinal helpers so their logic is verified
//! independently of the tree.
//!
//! This target lives under `tests/` (an integration-test target), not under
//! `src/`, so it adds ZERO Rust to Simard's daemon source tree — consistent with
//! the data-only philosophy for example identities.

use std::fs;
use std::path::{Path, PathBuf};

use simard::error::SimardError;
use simard::identity::{
    DEFAULT_EXAMPLE_IDENTITIES_DIR, list_example_identities, render_identity_index,
};
use tempfile::TempDir;

// ── Generated-block markers (literal HTML comment lines) ────────────────────

const BEGIN: &str = "<!-- BEGIN GENERATED IDENTITY INDEX -->";
const END: &str = "<!-- END GENERATED IDENTITY INDEX -->";

/// The ten shipped example identities. Content preservation (R5): each must
/// still be documented (as a derived index entry + a package-owned README),
/// just no longer centrally enumerated.
const SHIPPED_IDENTITIES: &[&str] = &[
    "atelier",
    "bursar",
    "cartographer",
    "concierge",
    "gastronome",
    "kinema",
    "loremaster",
    "maestro",
    "terra",
    "vitruvia",
];

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn example_base() -> PathBuf {
    repo_path(DEFAULT_EXAMPLE_IDENTITIES_DIR)
}

// ── Pure helpers (test-local; verified by their own unit tests) ─────────────

/// Extract the body between the `BEGIN`/`END GENERATED IDENTITY INDEX` markers
/// from a README string — everything after the newline that ends the BEGIN
/// marker line, up to (but not including) the END marker line. Returns `None`
/// if either marker is absent (the block MUST exist — a missing marker is a
/// hard failure, never a silently-empty index).
fn extract_marker_block(readme: &str) -> Option<String> {
    let begin = readme.find(BEGIN)?;
    let after_begin = begin + BEGIN.len();
    let body_start = readme[after_begin..].find('\n')? + after_begin + 1;
    let end = readme[body_start..].find(END)? + body_start;
    Some(readme[body_start..end].to_string())
}

/// Replace the body between the markers with `new_body`, preserving the marker
/// lines and everything outside them. Used by the `UPDATE_EXPECT=1` regenerator
/// so authors never hand-edit the derived list.
fn splice_marker_block(readme: &str, new_body: &str) -> String {
    let begin = readme.find(BEGIN).expect("BEGIN marker present");
    let after_begin = begin + BEGIN.len();
    let body_start = readme[after_begin..]
        .find('\n')
        .map(|n| n + after_begin + 1)
        .expect("newline after BEGIN marker");
    let end = readme[body_start..].find(END).expect("END marker present") + body_start;
    format!("{}{}{}", &readme[..body_start], new_body, &readme[end..])
}

/// Ordinal words used in the fragile "an Nth (worked) example" enumeration that
/// this feature eliminates. Bounded, literal alternation — no unbounded
/// quantifier, so the matcher is ReDoS-safe.
const ORDINALS: &[&str] = &[
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "seventh",
    "eighth",
    "ninth",
    "tenth",
    "eleventh",
    "twelfth",
    "thirteenth",
    "fourteenth",
    "fifteenth",
];

/// True iff `text` contains the identity-ordinal enumeration phrase
/// `a|an <ordinal> [worked ]example` (e.g. "is a tenth example", "an eighth
/// worked example"). Anchored to that exact phrasing so unrelated prose —
/// "first-class", "a second opinion", "the reference example identity",
/// "for example" — never false-positives.
///
/// Implementation is a linear word-window scan (no regex backtracking), so it
/// is bounded and ReDoS-safe.
fn contains_ordinal_example_phrase(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let is_article = |w: &str| w == "a" || w == "an";
    let is_ordinal = |w: &str| ORDINALS.contains(&w);
    for i in 0..words.len() {
        if !is_article(&words[i]) || i + 2 >= words.len() {
            continue;
        }
        if !is_ordinal(&words[i + 1]) {
            continue;
        }
        // <article> <ordinal> example
        if words[i + 2] == "example" {
            return true;
        }
        // <article> <ordinal> worked example
        if words[i + 2] == "worked" && i + 3 < words.len() && words[i + 3] == "example" {
            return true;
        }
    }
    false
}

/// Write a minimal synthetic example-identity package (`identity.toml` +
/// `README.md`) under `<base>/<name>/`. The generator keys off the presence of
/// `identity.toml`, so a minimal stub is sufficient for derivation tests.
fn write_synthetic_package(base: &Path, name: &str) {
    let dir = base.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("identity.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
    )
    .unwrap();
    fs::write(dir.join("README.md"), format!("# {name} example\n")).unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
// Repo invariants — the self-maintaining index against the real tree
// ════════════════════════════════════════════════════════════════════════════

/// INVARIANT 1: the committed marker block in `examples/identities/README.md` is
/// byte-for-byte equal to what `render_identity_index` derives from the package
/// directories — so the index can never go stale.
///
/// Doubles as the regenerator: `UPDATE_EXPECT=1 cargo test --test
/// example_identities_index_valid` splices the freshly-derived block back
/// between the markers and rewrites the README (no manual paste, no
/// stale-commit foot-gun).
#[test]
fn generated_index_is_in_sync() {
    let base = example_base();
    let readme_path = base.join("README.md");
    let readme = fs::read_to_string(&readme_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", readme_path.display()));

    let expected = render_identity_index(&base).expect("render_identity_index must succeed");

    if std::env::var_os("UPDATE_EXPECT").is_some() {
        // Regenerator mode: rewrite the block in place from the package dirs.
        let updated = splice_marker_block(&readme, &expected);
        fs::write(&readme_path, updated).unwrap();
        return;
    }

    let committed = extract_marker_block(&readme).unwrap_or_else(|| {
        panic!(
            "examples/identities/README.md is missing the generated index markers.\n\
             Expected a block delimited by:\n  {BEGIN}\n  {END}\n\
             Insert the markers and run \
             `UPDATE_EXPECT=1 cargo test --test example_identities_index_valid`."
        )
    });

    assert_eq!(
        committed, expected,
        "examples/identities/README.md index is STALE — it is not derived from the \
         package directories.\n\
         Run `UPDATE_EXPECT=1 cargo test --test example_identities_index_valid` to \
         regenerate it.\n\
         Expected block body:\n{expected}"
    );
}

/// INVARIANT 2: every derived package ships its own non-empty `README.md`, so the
/// descriptive blurb is package-owned (self-describing) rather than centrally
/// enumerated in the shared file.
#[test]
fn every_indexed_package_has_a_readme() {
    let base = example_base();
    let names = list_example_identities(&base).expect("list_example_identities must succeed");
    assert!(
        !names.is_empty(),
        "list_example_identities derived no packages from {} — the walk is broken",
        base.display()
    );

    let mut missing: Vec<String> = Vec::new();
    for name in &names {
        let readme = base.join(name).join("README.md");
        let ok = readme
            .is_file()
            .then(|| fs::read_to_string(&readme).unwrap_or_default())
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false);
        if !ok {
            missing.push(format!("examples/identities/{name}/README.md"));
        }
    }

    assert!(
        missing.is_empty(),
        "{} example package(s) are missing a non-empty package README. Each identity's \
         blurb must live in its OWN package README (self-describing), not in the shared \
         index:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

/// INVARIANT 3: the shared README and the concept doc contain NO ordinal
/// enumeration ("an Nth (worked) example"). The fragile "ninth"/"tenth"
/// numbering that forced additive rebase conflicts must be gone.
#[test]
fn shared_docs_have_no_ordinal_enumeration() {
    for rel in [
        "examples/identities/README.md",
        "docs/concepts/pluggable-identity.md",
    ] {
        let text =
            fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
        assert!(
            !contains_ordinal_example_phrase(&text),
            "{rel} still enumerates identities with fragile ordinal phrasing \
             (\"a second example\", \"a tenth example\", …). Remove the ordinal \
             numbering: the shared README's list is a DERIVED marker block and the \
             concept doc must keep only non-enumerated framing."
        );
    }
}

/// Content preservation (R5): all ten shipped identities remain documented via
/// the derived index — relocation must not drop any.
#[test]
fn all_ten_shipped_identities_are_indexed() {
    let names = list_example_identities(&example_base()).expect("list must succeed");
    for shipped in SHIPPED_IDENTITIES {
        assert!(
            names.iter().any(|n| n == shipped),
            "shipped identity {shipped:?} is not derived into the index — content was lost. \
             Present: {names:?}"
        );
    }
}

/// The hand-maintained intro name-list enumerated a phantom `sommelier` that has
/// NO package directory. Deriving the index strictly from directories drops it
/// automatically; assert the phantom (and the hand-list it lived in) is gone
/// from the shared README.
#[test]
fn phantom_sommelier_name_list_is_gone() {
    let readme = fs::read_to_string(example_base().join("README.md")).unwrap();
    assert!(
        !readme.to_ascii_lowercase().contains("sommelier"),
        "examples/identities/README.md still references the phantom `sommelier` from the \
         old hand-maintained intro name-list. The list is now DERIVED from package \
         directories — sommelier has no package and must not appear."
    );
    // And it must not exist as a package either (the derivation would surface it).
    assert!(
        !example_base().join("sommelier").exists(),
        "an unexpected examples/identities/sommelier/ package exists"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Generator behavior — synthetic tempdir packages (no coupling to real names)
// ════════════════════════════════════════════════════════════════════════════

/// `list_example_identities` derives names purely from directories that contain
/// an `identity.toml`, returned in ascending (alphabetical) order.
#[test]
fn list_derives_names_alphabetically_from_dirs() {
    let tmp = TempDir::new().unwrap();
    // Intentionally created out of order.
    for name in ["cartographer", "atelier", "vitruvia", "bursar"] {
        write_synthetic_package(tmp.path(), name);
    }

    let names = list_example_identities(tmp.path()).unwrap();
    assert_eq!(
        names,
        vec![
            "atelier".to_string(),
            "bursar".to_string(),
            "cartographer".to_string(),
            "vitruvia".to_string(),
        ],
        "names must be derived from dirs and sorted ascending"
    );
    assert!(
        names.windows(2).all(|w| w[0] < w[1]),
        "order must be strictly ascending"
    );
}

/// Only directories containing `identity.toml` are included — loose files and
/// package-less directories are excluded, never mistaken for identities.
#[test]
fn list_excludes_non_packages() {
    let tmp = TempDir::new().unwrap();
    write_synthetic_package(tmp.path(), "gastronome");
    // A loose file at the base — not a package.
    fs::write(tmp.path().join("README.md"), "# index\n").unwrap();
    fs::write(tmp.path().join("NOTES.txt"), "scratch\n").unwrap();
    // A directory with NO identity.toml — not a package.
    fs::create_dir_all(tmp.path().join("scratch-dir")).unwrap();

    let names = list_example_identities(tmp.path()).unwrap();
    assert_eq!(
        names,
        vec!["gastronome".to_string()],
        "only dirs containing identity.toml are packages"
    );
}

/// A candidate package directory whose name is not a safe single path segment is
/// a HARD error (fail-visible), never silently skipped — a dropped or malformed
/// package must never vanish.
#[test]
fn list_rejects_unsafe_package_dir_name() {
    let tmp = TempDir::new().unwrap();
    write_synthetic_package(tmp.path(), "cartographer");
    // A package dir whose name contains a space is not a valid identity name.
    write_synthetic_package(tmp.path(), "bad name");

    let err = list_example_identities(tmp.path())
        .expect_err("an unsafe package dir name must fail visibly, not be skipped");
    assert!(
        matches!(err, SimardError::IdentityTomlParseError { .. }),
        "unsafe dir name must be a fail-visible IdentityTomlParseError, got: {err:?}"
    );
}

/// I/O failure (here: a base dir that does not exist) propagates as a
/// `SimardError` — fail-visible, never a silent empty list.
#[test]
fn list_propagates_io_error_on_missing_base() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("does-not-exist");
    let err = list_example_identities(&missing)
        .expect_err("a missing base dir must be a fail-visible error");
    let _ = err; // any SimardError is acceptable; the point is it is NOT Ok(empty).
}

/// `render_identity_index` emits exactly one `- [<name>](./<name>/README.md)`
/// line per package, alphabetical, `\n`-terminated — byte-for-byte.
#[test]
fn render_index_is_exact_and_deterministic() {
    let tmp = TempDir::new().unwrap();
    for name in ["bursar", "atelier", "cartographer"] {
        write_synthetic_package(tmp.path(), name);
    }

    let rendered = render_identity_index(tmp.path()).unwrap();
    let expected = "\
- [atelier](./atelier/README.md)
- [bursar](./bursar/README.md)
- [cartographer](./cartographer/README.md)
";
    assert_eq!(
        rendered, expected,
        "render output must be the exact derived Markdown block body"
    );
    // Deterministic: rendering again yields identical bytes.
    assert_eq!(rendered, render_identity_index(tmp.path()).unwrap());
}

/// The "VERIFY DONE" property: adding a brand-new example identity is a
/// PACKAGE-ONLY change. Dropping a new package directory in changes the derived
/// index with no edit to any shared file — the new entry simply appears in
/// sorted position.
#[test]
fn adding_a_package_is_a_pure_data_change() {
    let tmp = TempDir::new().unwrap();
    for name in SHIPPED_IDENTITIES {
        write_synthetic_package(tmp.path(), name);
    }

    let before = render_identity_index(tmp.path()).unwrap();
    assert!(!before.contains("zzdummy"));

    // Add a throwaway identity — data only, no shared-file edit.
    write_synthetic_package(tmp.path(), "zzdummy");

    let after = render_identity_index(tmp.path()).unwrap();
    assert!(
        after.contains("- [zzdummy](./zzdummy/README.md)\n"),
        "a newly-added package dir must appear in the derived index automatically"
    );
    // Exactly one line was added; everything else is unchanged and still sorted.
    assert_eq!(
        after.lines().count(),
        before.lines().count() + 1,
        "adding one package adds exactly one derived line"
    );
    let names: Vec<&str> = after.lines().collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "derived index remains sorted after the add");
}

// ════════════════════════════════════════════════════════════════════════════
// Pure-helper unit pins — boundary logic verified without the tree
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn ordinal_matcher_flags_enumeration_phrases() {
    for phrase in [
        "bursar is a second example of the pattern",
        "kinema is a sixth example",
        "vitruvia is a ninth example:",
        "maestro is a tenth example: a music identity",
        "terra is an eighth worked example",
        "loremaster is a seventh example — a game master",
    ] {
        assert!(
            contains_ordinal_example_phrase(phrase),
            "matcher must flag ordinal enumeration in: {phrase:?}"
        );
    }
}

#[test]
fn ordinal_matcher_ignores_unrelated_prose() {
    for phrase in [
        "cartographer is the reference example identity",
        "first-class prompt-asset support",
        "seek a second opinion before merging",
        "for example, a dataset and a question",
        "a further example of the same shape",
        "an example identity is defined entirely by its data",
        "the first prompt is the system prompt",
    ] {
        assert!(
            !contains_ordinal_example_phrase(phrase),
            "matcher must NOT flag unrelated prose: {phrase:?}"
        );
    }
}

#[test]
fn marker_extract_and_splice_roundtrip() {
    let readme = format!(
        "# Example identities\n\nIntro framing that is hand-written.\n\n{BEGIN}\n\
         - [atelier](./atelier/README.md)\n- [bursar](./bursar/README.md)\n{END}\n\n\
         ## Trailing stable section\n"
    );

    let body = extract_marker_block(&readme).expect("markers present");
    assert_eq!(
        body, "- [atelier](./atelier/README.md)\n- [bursar](./bursar/README.md)\n",
        "extracted body is exactly the derived block, including trailing newline"
    );

    // Splicing a fresh body preserves the markers and everything outside them.
    let fresh = "- [atelier](./atelier/README.md)\n\
                 - [bursar](./bursar/README.md)\n\
                 - [cartographer](./cartographer/README.md)\n";
    let updated = splice_marker_block(&readme, fresh);
    assert_eq!(extract_marker_block(&updated).unwrap(), fresh);
    assert!(updated.contains("Intro framing that is hand-written."));
    assert!(updated.contains("## Trailing stable section"));
    assert!(updated.contains(BEGIN) && updated.contains(END));

    // Idempotent: splicing the same body back is a no-op.
    assert_eq!(splice_marker_block(&updated, fresh), updated);
}

#[test]
fn marker_extract_returns_none_without_markers() {
    assert!(extract_marker_block("# no markers here\njust prose\n").is_none());
    // A BEGIN with no END is still incomplete → None (the block must be closed).
    let half = format!("{BEGIN}\n- [x](./x/README.md)\n");
    assert!(extract_marker_block(&half).is_none());
}
