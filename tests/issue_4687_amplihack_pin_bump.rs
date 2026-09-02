//! Failing TDD acceptance tests for issue #4687 — bump Simard's pinned
//! `amplihack-memory` dependency OFF the WAL-corrupting rev and onto the
//! upstream fix commit (Step 7 → Step 8).
//!
//! # The defect this bump lands the fix for
//!
//! The pinned `amplihack-memory` rev `c266e15d…` carries a cognitive-memory
//! WAL crash-consistency defect in `graph::lbug_store`: on EVERY daemon start
//! the LadybugDB WAL replay fails checksum verification ("Checksum
//! verification failed, the WAL file is corrupted"), the tail is silently
//! truncated ("recovered from corrupt WAL (good prefix)") dropping the most
//! recent cognitive-memory writes, and the auto-checkpoint then fails to
//! rename `cognitive.wal → cognitive.wal.checkpoint` ("No such file or
//! directory") so the checkpoint never advances and the WAL keeps
//! re-corrupting. The fix (single-owner checkpointing, fsync-before-advance
//! ordering, existence-guarded rename, explicit-error-on-unrecoverable-loss)
//! lands upstream in `rysweet/amplihack-memory-lib`; Simard adopts it by
//! bumping this pin to the fix's squash-merge commit.
//!
//! # The lockstep invariant (unchanged from #2626)
//!
//! The `persistent` feature of `amplihack-memory` compiles the LadybugDB
//! engine through the `lbug` crate, and the standalone `simard-tui` binary
//! links `lbug` directly. The final binary must link **exactly one** `lbug`
//! (one engine, one on-disk store format v42). The #4687 fix is wrapper-level
//! (in `lbug_store/mod.rs`) with **no engine/format change**, so `lbug` should
//! stay on its current fork rev and, crucially, resolve to exactly ONE version.
//! Only if the fix provably required an engine change would the `lbug` fork rev
//! move in lockstep — this guard enforces the "exactly one engine" invariant
//! either way.
//!
//! # Why these are file-shaped (rg/grep-shaped)
//!
//! They read the raw `Cargo.toml` / `Cargo.lock` with std only — no network,
//! no toolchain, no crate import — so an operator running the equivalent
//! `grep` gets the same answer CI does, and the guard stays decoupled from the
//! heavy `simard` (LadybugDB C++) build. They start **RED** on the un-bumped
//! tree (pin still on the buggy `c266e15d…`) and turn **GREEN** once the pin +
//! lockfile are updated to the fix commit.
//!
//! # Step-8 handoff
//!
//! [`MEMORY_FIX_TARGET_REV`] is a sentinel until the upstream fix PR merges.
//! Step 8 MUST replace it with the 40-char squash-merge SHA (or documented
//! interim fix-branch HEAD per ambiguity A2). Until then the exact-target test
//! enforces the durable half of the contract (moved off the buggy rev, still a
//! full SHA, still on the allowlisted remote); once the sentinel is replaced
//! with a real SHA it hardens into an exact-equality pin guard.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

// ── Target / stale pin constants ────────────────────────────────────────────

/// The buggy rev the bump must move *off of* — the WAL-corrupting
/// `amplihack-memory-lib` commit currently pinned in `Cargo.toml`
/// (anti-regression sentinel).
const MEMORY_BUGGY_REV: &str = "c266e15d1399967c04324370e77cf281990b8be1";

/// The upstream fix commit to pin onto — the single-owner / fsync-durable WAL
/// crash-consistency fix (#4687).
///
/// This is the WAL-only fix commit on branch
/// `fix/issue-4687-wal-only-on-c266e15` (durably anchored by tag
/// `issue-4687-wal-crash-consistency-c266e15`): the same diff as the upstream
/// `main` squash-merge of PR #144, applied onto Simard's current base c266e15 so
/// it stays additive/non-breaking (excludes the out-of-scope #137 multi-writer
/// coordination layer). See the Cargo.toml comment for the full rationale.
const MEMORY_FIX_TARGET_REV: &str = "0031505b911151bf47409694a6c45f8b778d91b9";

/// The only git remote `amplihack-memory` may resolve from. A bump must never
/// introduce a *new* git source (typosquat / allowlist-bypass guard).
const MEMORY_REMOTE: &str = "https://github.com/rysweet/amplihack-memory-lib.git";

/// Simard's direct `lbug` dep resolves from the rysweet/ladybug-rust fork. The
/// #4687 fix is wrapper-level with no engine change, so this remote is
/// unchanged; the hard invariant is that exactly ONE `lbug` version links.
const LBUG_FORK_REMOTE: &str = "https://github.com/rysweet/ladybug-rust";

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

/// The first non-comment manifest line whose key is exactly `name` (i.e.
/// `name = ...`). Guards against matching the crate name inside a `#`
/// provenance comment and against prefix collisions.
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
                break;
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

/// The fix target rev, or `None` while it is still the Step-8 sentinel.
fn resolved_fix_target() -> Option<&'static str> {
    is_full_sha(MEMORY_FIX_TARGET_REV).then_some(MEMORY_FIX_TARGET_REV)
}

// ─────────────────────────────────────────────────────────────────────────────
// Primary contract — the pin moves off the buggy WAL rev onto the fix commit
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cargo_toml_moves_off_the_buggy_wal_rev() {
    // Anti-regression: the exact WAL-corrupting SHA must no longer pin
    // `amplihack-memory`. (It may still appear inside provenance comments — this
    // only inspects the live dependency line.)
    let memory_rev = dep_rev(&cargo_toml(), "amplihack-memory")
        .expect("Cargo.toml must declare a git `amplihack-memory` dependency with a `rev`");
    assert_ne!(
        memory_rev, MEMORY_BUGGY_REV,
        "amplihack-memory is still pinned to the WAL-corrupting rev \
         {MEMORY_BUGGY_REV}; #4687 requires bumping to the upstream fix commit."
    );
}

#[test]
fn cargo_toml_pins_amplihack_memory_to_the_fix_rev() {
    let memory_rev = dep_rev(&cargo_toml(), "amplihack-memory")
        .expect("Cargo.toml must declare a git `amplihack-memory` dependency with a `rev`");
    match resolved_fix_target() {
        Some(target) => assert_eq!(
            memory_rev, target,
            "amplihack-memory must be pinned to the #4687 WAL-fix commit \
             {target}. Found `{memory_rev}`."
        ),
        None => {
            // Sentinel still in place: enforce the durable half of the contract
            // so this test is a real RED→GREEN guard even before the merge SHA
            // is known. Step 8 replaces MEMORY_FIX_TARGET_REV to harden this
            // into an exact-equality pin guard.
            assert_ne!(
                memory_rev, MEMORY_BUGGY_REV,
                "amplihack-memory still on the buggy rev {MEMORY_BUGGY_REV}; \
                 bump to the #4687 fix commit and fill MEMORY_FIX_TARGET_REV."
            );
            assert!(
                is_full_sha(&memory_rev),
                "amplihack-memory rev `{memory_rev}` must be a full 40-char \
                 lowercase hex git SHA (immutable fix-commit pin)."
            );
        }
    }
}

#[test]
fn amplihack_memory_pin_is_a_full_sha_not_a_floating_ref() {
    let toml = cargo_toml();
    let line = manifest_dep_line(&toml, "amplihack-memory")
        .expect("missing `amplihack-memory` dependency line in Cargo.toml");
    assert!(
        !line.contains("branch =") && !line.contains("tag ="),
        "`amplihack-memory` must be pinned by an immutable `rev` SHA, not a \
         branch/tag: {line}"
    );
    let rev = dep_rev(&toml, "amplihack-memory").expect("`amplihack-memory` has no `rev` pin");
    assert!(
        is_full_sha(&rev),
        "`amplihack-memory` rev `{rev}` is not a full 40-char lowercase hex git SHA."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Lockfile parity — Cargo.lock git source must match the bumped pin
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cargo_lock_source_matches_the_bumped_memory_pin() {
    let toml_rev = dep_rev(&cargo_toml(), "amplihack-memory")
        .expect("Cargo.toml must declare an `amplihack-memory` `rev`");
    let source = locked_source(&cargo_lock(), "amplihack-memory")
        .expect("Cargo.lock must contain the amplihack-memory [[package]] source");
    assert!(
        source.contains(&toml_rev),
        "Cargo.lock amplihack-memory source must be refreshed to the Cargo.toml \
         rev {toml_rev} (run `cargo update -p amplihack-memory`). Found `{source}`."
    );
    assert!(
        !source.contains(MEMORY_BUGGY_REV),
        "Cargo.lock amplihack-memory source still references the buggy WAL rev \
         {MEMORY_BUGGY_REV}; the lockfile was not refreshed after the #4687 bump."
    );
}

#[test]
fn memory_crate_stays_on_its_allowlisted_git_remote() {
    // The bump must NOT swap `amplihack-memory` onto a new git host (a
    // typosquat / deny.toml-allowlist-bypass vector).
    let toml = cargo_toml();
    let remote = dep_git_remote(&toml, "amplihack-memory").unwrap_or_default();
    assert_eq!(
        remote, MEMORY_REMOTE,
        "amplihack-memory must stay on {MEMORY_REMOTE}, found `{remote}`."
    );
    let src = locked_source(&cargo_lock(), "amplihack-memory").unwrap_or_default();
    assert!(
        src.contains(MEMORY_REMOTE),
        "Cargo.lock amplihack-memory source must resolve from {MEMORY_REMOTE}; found `{src}`."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Lockstep invariant — exactly one lbug engine links (format stays v42)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lbug_resolves_to_exactly_one_version_after_the_wal_fix() {
    // #4687 is a wrapper-level fix with NO engine/format change (A1). The hard
    // constraint survives the bump unchanged: memory-lib's `persistent` feature
    // and the simard-tui binary must share a single LadybugDB engine + on-disk
    // store format v42. Two locked `lbug` versions = two engines = a split
    // store. If the bump somehow pulled a second lbug, Simard's direct `lbug`
    // pin must be reconciled so only one resolves.
    let versions = distinct_locked_versions(&cargo_lock(), "lbug");
    assert_eq!(
        versions.len(),
        1,
        "Cargo.lock must resolve EXACTLY ONE `lbug` version (single engine / \
         store format v42). Found {}: {:?}.",
        versions.len(),
        versions
    );
}

#[test]
fn direct_lbug_pin_stays_on_the_fork_remote_single_engine() {
    // The direct `lbug` dep (simard-tui goal board) must remain a git dep on the
    // ladybug-rust fork, pinned by a full-SHA `rev`, resolving to the single
    // locked engine. The exact rev is NOT hard-pinned here: A1 permits it to move
    // in lockstep ONLY if the fix required an engine change, so this guard
    // enforces "one engine on the allowlisted fork" without over-constraining.
    let toml = cargo_toml();
    let line = manifest_dep_line(&toml, "lbug")
        .expect("Cargo.toml must declare a direct `lbug` dependency (simard-tui goal board)");

    let remote = field_value(&line, "git")
        .expect("direct `lbug` must be a git dependency on the ladybug-rust fork");
    assert_eq!(
        remote.trim_end_matches(".git"),
        LBUG_FORK_REMOTE.trim_end_matches(".git"),
        "direct `lbug` must resolve from the fork {LBUG_FORK_REMOTE}, found `{remote}`."
    );
    let rev = dep_rev(&toml, "lbug").expect("direct `lbug` git dep must carry a `rev` pin");
    assert!(
        is_full_sha(&rev),
        "direct `lbug` rev `{rev}` must be a full 40-char lowercase hex git SHA."
    );

    let locked = distinct_locked_versions(&cargo_lock(), "lbug");
    assert_eq!(
        locked.len(),
        1,
        "expected exactly one locked lbug version (single engine); found {locked:?}"
    );
    let src = locked_source(&cargo_lock(), "lbug")
        .expect("Cargo.lock must contain the lbug [[package]] source");
    assert!(
        src.contains(LBUG_FORK_REMOTE),
        "Cargo.lock lbug must resolve from the fork {LBUG_FORK_REMOTE}; found `{src}`."
    );
}
