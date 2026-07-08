//! Failing TDD acceptance tests for issue #2626 — bump Simard's pinned
//! amplihack dependencies to current upstream `main` (Step 7).
//!
//! # Policy this encodes
//!
//! When Simard consumes `amplihack-rs` (`amplihack-agent-eval`) and
//! `amplihack-memory-lib` (`amplihack-memory`) as git-pinned dependencies, a
//! self-improvement bump means: point *her own* pins at current upstream
//! `main` and run the new code. The pins are advanced as upstream lands work:
//!
//!   * `amplihack-agent-eval`  59548a96… → **2a93441d…** (amplihack-rs main)
//!   * `amplihack-memory`       901f63ad… → **72c5ea1b…** (memory-lib main —
//!     the squash-merge of PR #126, which serves the ranked-recall graph term
//!     from a single bulk graph-adjacency scan per edge type instead of a
//!     per-node `query_neighbors` BFS; cuts OODA prepare-context from ~11 min/
//!     cycle toward seconds at ~7,590 facts with byte-identical ranking, no
//!     store-format change; Simard consumes it to fix the "memory graph never
//!     loads" pathology, issue #40)
//!
//! These are the exact 40-char SHAs verified against `git ls-remote … main`
//! at authoring time.
//!
//! # The lockstep invariant
//!
//! The `persistent` feature of `amplihack-memory` compiles the LadybugDB
//! engine through the published `lbug` crate, and the standalone `simard-tui`
//! binary links `lbug` directly to render the goal board read-only. The final
//! binary must link **exactly one** `lbug` (one engine, one on-disk store
//! format). So Simard's direct `lbug = "=X"` pin must equal whatever single
//! version the memory-lib bump resolves — never a second, conflicting line.
//!
//! # Why these are file-shaped (rg/grep-shaped)
//!
//! They read the raw `Cargo.toml` / `Cargo.lock` with std only — no network,
//! no toolchain, no crate import — so an operator running the equivalent
//! `grep` gets the same answer CI does, and the guard stays decoupled from the
//! heavy `simard` (LadybugDB C++) build. They start **RED** on the un-bumped
//! tree and turn **GREEN** once the two pins + lockfile are updated.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

// ── Target / stale pin constants (verified against upstream `main`) ──────────

/// amplihack-rs `main` HEAD carrying the `amplihack-agent-eval` crate to adopt.
const AGENT_EVAL_TARGET_REV: &str = "14dc30b10e87764120c6f2bae7f3630522c29e5d";
/// amplihack-memory-lib `main` commit carrying the `amplihack-memory` crate:
/// PR #126's squash-merge — the bulk graph-adjacency index for ranked recall,
/// which serves the graph-proximity term from one bulk edge scan per type
/// instead of a per-node `query_neighbors` BFS (N+1 fan-out of ~3N Cypher scans
/// per recall). Cuts OODA prepare-context from ~11 min/cycle toward seconds at
/// ~7,590 facts with byte-identical ranking; no store-format change (v41).
/// Simard consumes it to fix the "memory graph never loads" pathology
/// (issue #40). One commit ahead of the prior #125 pin, no regression.
const MEMORY_TARGET_REV: &str = "72c5ea1bfcca7e6f3e314dfd99fbe4998378ffe8";

/// The stale revs the bump must move *off of* (anti-regression sentinels).
const AGENT_EVAL_STALE_REV: &str = "59548a96049ab8d558110bcaf9c82a4316f1bbf0";
const MEMORY_STALE_REV: &str = "901f63ad79eb0c2d87cd8263d26025877af43cc5";

/// The only git remotes these two crates may resolve from. A bump must never
/// introduce a *new* git source (typosquat / allowlist-bypass guard, R1).
const AGENT_EVAL_REMOTE: &str = "https://github.com/rysweet/amplihack-rs.git";
const MEMORY_REMOTE: &str = "https://github.com/rysweet/amplihack-memory-lib.git";

// ── Path / IO helpers ───────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {} ({e})", path.display()))
}

fn cargo_toml() -> String {
    read_repo_file("Cargo.toml")
}

fn cargo_lock() -> String {
    read_repo_file("Cargo.lock")
}

// ── Tiny structural matchers (std-only, comment-aware) ───────────────────────

/// The first non-comment `[dependencies]`-style manifest line whose key is
/// exactly `name` (i.e. `name = ...`). Returns the trimmed line, or `None`.
///
/// Guards against matching the crate name where it appears inside a `#`
/// provenance comment, and against prefix collisions (e.g. `amplihack-memory`
/// vs a hypothetical `amplihack-memory-foo`) by requiring the key to be
/// followed by whitespace and `=`.
fn manifest_dep_line(contents: &str, name: &str) -> Option<String> {
    contents
        .lines()
        .map(str::trim)
        .find(|l| {
            if l.starts_with('#') {
                return false;
            }
            match l.strip_prefix(name) {
                Some(rest) => rest.trim_start().starts_with('='),
                None => false,
            }
        })
        .map(str::to_string)
}

/// Extract the value of a `key = "value"` field from a single manifest line.
fn field_value(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key} = \"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The `rev = "…"` pin on a git dependency line.
fn dep_rev(contents: &str, name: &str) -> Option<String> {
    field_value(&manifest_dep_line(contents, name)?, "rev")
}

/// The `git = "…"` remote on a git dependency line.
fn dep_git_remote(contents: &str, name: &str) -> Option<String> {
    field_value(&manifest_dep_line(contents, name)?, "git")
}

/// The `source = "…"` string of the `[[package]]` named `name` in Cargo.lock.
fn locked_source(lockfile: &str, name: &str) -> Option<String> {
    let needle = format!("name = \"{name}\"");
    let mut lines = lockfile.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != needle {
            continue;
        }
        for following in lines.by_ref() {
            let t = following.trim();
            if let Some(v) = t.strip_prefix("source = \"") {
                return v.strip_suffix('"').map(str::to_string);
            }
            if t.starts_with("[[package]]") {
                break; // next package started; this one had no source.
            }
        }
    }
    None
}

/// Every distinct locked `version` for a `[[package]]` `name` in Cargo.lock.
fn distinct_locked_versions(lockfile: &str, name: &str) -> BTreeSet<String> {
    let needle = format!("name = \"{name}\"");
    let mut versions = BTreeSet::new();
    let mut lines = lockfile.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != needle {
            continue;
        }
        for following in lines.by_ref() {
            let t = following.trim();
            if let Some(rest) = t.strip_prefix("version = \"") {
                if let Some(end) = rest.find('"') {
                    versions.insert(rest[..end].to_string());
                }
                break;
            }
            if t.starts_with("[[package]]") {
                break;
            }
        }
    }
    versions
}

/// True when `rev` is a full 40-char lowercase hex git SHA (not a branch/tag).
fn is_full_sha(rev: &str) -> bool {
    rev.len() == 40
        && rev
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

// ─────────────────────────────────────────────────────────────────────────────
// Primary contract — Cargo.toml pins the two crates at current upstream main
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cargo_toml_pins_amplihack_agent_eval_to_target_main_rev() {
    let rev = dep_rev(&cargo_toml(), "amplihack-agent-eval")
        .expect("Cargo.toml must declare a git `amplihack-agent-eval` dependency with a `rev`");
    assert_eq!(
        rev, AGENT_EVAL_TARGET_REV,
        "amplihack-agent-eval must be pinned to amplihack-rs `main` HEAD \
         {AGENT_EVAL_TARGET_REV} (#2626 bump). Found `{rev}`."
    );
}

#[test]
fn cargo_toml_pins_amplihack_memory_to_target_main_rev() {
    let rev = dep_rev(&cargo_toml(), "amplihack-memory")
        .expect("Cargo.toml must declare a git `amplihack-memory` dependency with a `rev`");
    assert_eq!(
        rev, MEMORY_TARGET_REV,
        "amplihack-memory must be pinned to amplihack-memory-lib `main` HEAD \
         {MEMORY_TARGET_REV} (#2626 bump). Found `{rev}`."
    );
}

#[test]
fn cargo_toml_moves_off_the_stale_amplihack_revs() {
    // Anti-regression: the exact stale SHAs must no longer pin either
    // dependency. (Historical SHAs may still appear inside provenance comments
    // — this only inspects the live dependency lines.)
    let toml = cargo_toml();
    let agent_rev = dep_rev(&toml, "amplihack-agent-eval").unwrap_or_default();
    let memory_rev = dep_rev(&toml, "amplihack-memory").unwrap_or_default();
    assert_ne!(
        agent_rev, AGENT_EVAL_STALE_REV,
        "amplihack-agent-eval is still on the STALE rev {AGENT_EVAL_STALE_REV}; \
         #2626 requires moving to {AGENT_EVAL_TARGET_REV}."
    );
    assert_ne!(
        memory_rev, MEMORY_STALE_REV,
        "amplihack-memory is still on the STALE rev {MEMORY_STALE_REV}; \
         #2626 requires moving to {MEMORY_TARGET_REV}."
    );
}

#[test]
fn amplihack_pins_are_full_sha_revs_not_floating_refs() {
    // A pin must be an immutable 40-char SHA, never a `branch`/`tag`. A moving
    // ref could silently swap the code the binary links between builds.
    let toml = cargo_toml();
    for name in ["amplihack-agent-eval", "amplihack-memory"] {
        let line = manifest_dep_line(&toml, name)
            .unwrap_or_else(|| panic!("missing `{name}` dependency line in Cargo.toml"));
        assert!(
            !line.contains("branch =") && !line.contains("tag ="),
            "`{name}` must be pinned by an immutable `rev` SHA, not a branch/tag: {line}"
        );
        let rev =
            dep_rev(&toml, name).unwrap_or_else(|| panic!("`{name}` dependency has no `rev` pin"));
        assert!(
            is_full_sha(&rev),
            "`{name}` rev `{rev}` is not a full 40-char lowercase hex git SHA."
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lockfile parity — Cargo.lock git sources must match the bumped pins
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cargo_lock_source_rev_matches_bumped_agent_eval_pin() {
    let source = locked_source(&cargo_lock(), "amplihack-agent-eval")
        .expect("Cargo.lock must contain the amplihack-agent-eval [[package]] source");
    assert!(
        source.contains(AGENT_EVAL_TARGET_REV),
        "Cargo.lock amplihack-agent-eval source must be refreshed to rev \
         {AGENT_EVAL_TARGET_REV} (run `cargo update -p amplihack-agent-eval`). \
         Found `{source}`."
    );
    assert!(
        !source.contains(AGENT_EVAL_STALE_REV),
        "Cargo.lock amplihack-agent-eval source still references the STALE rev \
         {AGENT_EVAL_STALE_REV}; the lockfile was not refreshed."
    );
}

#[test]
fn cargo_lock_source_rev_matches_bumped_memory_pin() {
    let source = locked_source(&cargo_lock(), "amplihack-memory")
        .expect("Cargo.lock must contain the amplihack-memory [[package]] source");
    assert!(
        source.contains(MEMORY_TARGET_REV),
        "Cargo.lock amplihack-memory source must be refreshed to rev \
         {MEMORY_TARGET_REV} (run `cargo update -p amplihack-memory`). \
         Found `{source}`."
    );
    assert!(
        !source.contains(MEMORY_STALE_REV),
        "Cargo.lock amplihack-memory source still references the STALE rev \
         {MEMORY_STALE_REV}; the lockfile was not refreshed."
    );
}

#[test]
fn bumped_crates_stay_on_their_allowlisted_git_remotes() {
    // R1 guard: the bump must NOT swap either crate onto a new git host (a
    // typosquat / deny.toml-allowlist-bypass vector). Both the manifest `git =`
    // and the locked `source` must stay on the known rysweet remotes.
    let toml = cargo_toml();
    let lock = cargo_lock();

    let agent_remote = dep_git_remote(&toml, "amplihack-agent-eval").unwrap_or_default();
    assert_eq!(
        agent_remote, AGENT_EVAL_REMOTE,
        "amplihack-agent-eval must stay on {AGENT_EVAL_REMOTE}, found `{agent_remote}`."
    );
    let memory_remote = dep_git_remote(&toml, "amplihack-memory").unwrap_or_default();
    assert_eq!(
        memory_remote, MEMORY_REMOTE,
        "amplihack-memory must stay on {MEMORY_REMOTE}, found `{memory_remote}`."
    );

    let agent_src = locked_source(&lock, "amplihack-agent-eval").unwrap_or_default();
    assert!(
        agent_src.contains(AGENT_EVAL_REMOTE),
        "Cargo.lock amplihack-agent-eval source must resolve from \
         {AGENT_EVAL_REMOTE}; found `{agent_src}`."
    );
    let memory_src = locked_source(&lock, "amplihack-memory").unwrap_or_default();
    assert!(
        memory_src.contains(MEMORY_REMOTE),
        "Cargo.lock amplihack-memory source must resolve from {MEMORY_REMOTE}; \
         found `{memory_src}`."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Lockstep invariant — exactly one lbug engine links into the final binary
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lbug_resolves_to_exactly_one_version() {
    // The hard lockstep constraint: memory-lib's `persistent` feature and the
    // simard-tui binary must share a single LadybugDB engine + on-disk store
    // format. Two locked `lbug` versions = two engines = a corrupt/split store.
    let versions = distinct_locked_versions(&cargo_lock(), "lbug");
    assert_eq!(
        versions.len(),
        1,
        "Cargo.lock must resolve EXACTLY ONE `lbug` version (single engine / \
         store format). Found {}: {:?}. If the memory-lib bump pulled a new \
         lbug, reconcile Simard's direct `lbug = \"=X\"` pin to match so only \
         one version resolves — do NOT leave two.",
        versions.len(),
        versions
    );
}

#[test]
fn direct_lbug_pin_matches_the_single_locked_version() {
    // Simard's direct `lbug = "=X"` pin must equal whatever single version the
    // memory-lib bump resolves — the pin follows memory-lib, it is never chosen
    // independently. This keeps the manifest honest about the linked engine.
    let toml = cargo_toml();
    let line = manifest_dep_line(&toml, "lbug")
        .expect("Cargo.toml must declare a direct `lbug` dependency (simard-tui goal board)");
    // The pin is a bare string requirement: `lbug = "=0.17.1"`.
    let pin_raw = field_value(&line, "lbug").expect("could not parse the `lbug` version pin");
    let pin = pin_raw.trim_start_matches('=').trim().to_string();

    let locked = distinct_locked_versions(&cargo_lock(), "lbug");
    assert_eq!(
        locked.len(),
        1,
        "expected exactly one locked lbug version before comparing the pin; found {locked:?}"
    );
    let locked_version = locked.iter().next().cloned().unwrap_or_default();
    assert_eq!(
        pin, locked_version,
        "Cargo.toml direct pin `lbug = \"={pin}\"` must equal the single locked \
         lbug version `{locked_version}`. If the memory-lib bump moved lbug, \
         update the direct pin (and its lockstep comments) to `{locked_version}`."
    );
}
