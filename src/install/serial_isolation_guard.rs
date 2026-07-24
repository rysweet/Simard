//! Regression-guard meta-test for the `serial(install)` contract
//! (issue [#4536](https://github.com/rysweet/Simard/issues/4536)).
//!
//! The `cargo test --lib` binary runs many tests concurrently in ONE process.
//! The install unit tests in `entrypoint.rs` and `paths.rs` share two
//! process-global resources that intermittently race when the suite runs in
//! parallel:
//!
//! 1. **`fork` + `exec`** — the reconciler classifies an on-disk entrypoint by
//!    spawning it with `--version` (`Command::new(..).arg("--version")`). When
//!    two install tests write-then-exec their own fake `simard` scripts at the
//!    same instant, one exec can hit `ETXTBSY` (text file busy: the file is
//!    still open for writing in a sibling test's `fork`ed child) and the
//!    classification flips — flaking
//!    `reconcile_replaces_ours_marker_at_entrypoint`.
//! 2. **`flock`** — `install_lock_is_exclusive_per_simard_home` asserts a
//!    `LOCK_EX | LOCK_NB` guard is released after drop; a concurrent
//!    `fork`/`exec` in a sibling test can hold an inherited fd across the lock
//!    window and perturb the exclusivity assertion.
//!
//! Both tests pass in isolation but race under `cargo test --lib`. The fix is a
//! single rule: every hand-written `#[test]` in the install module that spawns a
//! child process (routes through `reconcile`/`classify`) or acquires the install
//! `flock` MUST carry the `install` serial key so these tests never run
//! concurrently *with each other*. A dedicated key (not `cognitive_memory`)
//! keeps them serial among themselves while staying parallel with the rest of
//! the suite — none of them mutate process-global env.
//!
//! This module parses the two source files with `syn` (AST-based, robust to
//! multi-line attributes, ordering, and `#[cfg]` gating) and fails the build if
//! any required install test is missing the `install` serial key. It reads
//! source only; it touches no env, no filesystem under `SIMARD_HOME`, and spawns
//! no child, so it intentionally carries NO serial key.
//!
//! See `docs/testing/install-serial-isolation.md` for the full contract.

#![cfg(test)]

use std::collections::BTreeMap;
use std::path::Path;

use proc_macro2::TokenTree;
use syn::visit::Visit;
use syn::{Attribute, ItemFn, Meta};

/// The serial key that serializes the install `fork`/`exec` + `flock` tests.
const REQUIRED_KEY: &str = "install";

/// Every hand-written install unit test that must carry the `install` serial
/// key, addressed as `(source file relative to CARGO_MANIFEST_DIR, fn name)`.
///
/// `entrypoint.rs` — the whole `mod unix::tests` group is serialized. Four
/// tests (`reconcile_removes_ours_marker_orphan`, `reconcile_preserves_foreign_orphan`,
/// `reconcile_replaces_ours_marker_at_entrypoint`,
/// `reconcile_surfaces_foreign_shadow_at_entrypoint_untouched`) place a *regular
/// file* that `classify` execs with `--version`; the other four never exec but
/// share the same `fork`/`exec` window against their siblings, so serializing
/// the entire group is the only way to close the race deterministically.
///
/// `paths.rs` — `install_lock_is_exclusive_per_simard_home` acquires the install
/// `flock` and is serialized against the same group.
const REQUIRED_INSTALL_TESTS: &[(&str, &str)] = &[
    (
        "src/install/entrypoint.rs",
        "reconcile_creates_owned_symlink_on_fresh_home",
    ),
    (
        "src/install/entrypoint.rs",
        "reconcile_removes_ours_symlink_orphan",
    ),
    (
        "src/install/entrypoint.rs",
        "reconcile_removes_ours_marker_orphan",
    ),
    (
        "src/install/entrypoint.rs",
        "reconcile_preserves_foreign_orphan",
    ),
    (
        "src/install/entrypoint.rs",
        "reconcile_replaces_ours_marker_at_entrypoint",
    ),
    (
        "src/install/entrypoint.rs",
        "reconcile_surfaces_foreign_shadow_at_entrypoint_untouched",
    ),
    ("src/install/entrypoint.rs", "reconcile_is_idempotent"),
    (
        "src/install/entrypoint.rs",
        "classify_broken_symlink_is_foreign",
    ),
    (
        "src/install/paths.rs",
        "install_lock_is_exclusive_per_simard_home",
    ),
];

/// A function is a test if any attribute's path ends in `test`
/// (`#[test]`, `#[tokio::test]`, …).
fn is_test_fn(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .map(|s| s.ident == "test")
            .unwrap_or(false)
    })
}

/// Collect the named `serial_test::serial(...)` keys across all attributes.
/// A bare `#[serial]` contributes no named key.
fn serial_keys(attrs: &[Attribute]) -> Vec<String> {
    let mut keys = Vec::new();
    for attr in attrs {
        let is_serial = attr
            .path()
            .segments
            .last()
            .map(|s| s.ident == "serial")
            .unwrap_or(false);
        if !is_serial {
            continue;
        }
        if let Meta::List(list) = &attr.meta {
            for tt in list.tokens.clone() {
                if let TokenTree::Ident(id) = tt {
                    keys.push(id.to_string());
                }
            }
        }
    }
    keys
}

/// Records, per test-fn name, whether it is a `#[test]` and its serial keys.
#[derive(Default)]
struct TestFnCollector {
    /// fn name -> (is_test, serial keys)
    tests: BTreeMap<String, (bool, Vec<String>)>,
}

impl<'ast> Visit<'ast> for TestFnCollector {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let name = node.sig.ident.to_string();
        let is_test = is_test_fn(&node.attrs);
        let keys = serial_keys(&node.attrs);
        self.tests.insert(name, (is_test, keys));
        // Recurse (nested fns/modules) to keep the scan exhaustive.
        syn::visit::visit_item_fn(self, node);
    }
}

/// Parse a source file (relative to `CARGO_MANIFEST_DIR`) and collect its
/// test-fn serial-key map. Panics with an actionable message on read/parse
/// failure so a moved/renamed source file cannot silently disable the guard.
fn collect_tests(rel: &str) -> BTreeMap<String, (bool, Vec<String>)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("install serial-guard: cannot read {}: {e}", path.display()));
    let file = syn::parse_file(&src)
        .unwrap_or_else(|e| panic!("install serial-guard: cannot parse {}: {e}", path.display()));
    let mut collector = TestFnCollector::default();
    collector.visit_file(&file);
    collector.tests
}

/// Fails the build if any required install `fork`/`exec` or `flock` test is
/// missing the `install` serial key.
///
/// This meta-test reads source files only; it touches no env, no `SIMARD_HOME`
/// filesystem, and spawns no child, so it intentionally carries NO serial key.
#[test]
fn every_install_forking_test_is_serialized() {
    // file -> parsed test map, parsed once per distinct file.
    let mut parsed: BTreeMap<&str, BTreeMap<String, (bool, Vec<String>)>> = BTreeMap::new();
    let mut offenders: Vec<String> = Vec::new();

    for (file, fn_name) in REQUIRED_INSTALL_TESTS {
        let tests = parsed.entry(file).or_insert_with(|| collect_tests(file));
        match tests.get(*fn_name) {
            None => offenders.push(format!(
                "  {file}  fn {fn_name}\n      reason: expected install test not found \
(renamed or removed?) — the guard cannot enforce serialization for a test it \
cannot locate",
            )),
            Some((is_test, _)) if !*is_test => offenders.push(format!(
                "  {file}  fn {fn_name}\n      reason: located but is not a `#[test]` — \
the serialization contract only applies to hand-written tests",
            )),
            Some((_, keys)) if !keys.iter().any(|k| k == REQUIRED_KEY) => offenders.push(format!(
                "  {file}  fn {fn_name}\n      reason: missing the `{REQUIRED_KEY}` serial key \
(current keys: {keys:?})\n      fix:    add #[serial_test::serial({REQUIRED_KEY})] below #[test]",
            )),
            Some(_) => {}
        }
    }

    if offenders.is_empty() {
        return;
    }

    let mut report = String::new();
    report.push_str(&format!(
        "install serial-guard: {} install test(s) that `fork`/`exec` a child or acquire the \
install `flock` are missing the `{REQUIRED_KEY}` serial key.\n\
Every such test in the lib binary must share that key so their concurrent \
`fork`+`exec` (ETXTBSY / classification-flip) and `flock` windows never overlap.\n\
See docs/testing/install-serial-isolation.md.\n\n",
        offenders.len()
    ));
    for o in &offenders {
        report.push_str(o);
        report.push('\n');
    }
    panic!("{report}");
}
