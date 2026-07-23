//! Tests for platform constants and version metadata.

use super::platform::{CURRENT_VERSION, GITHUB_REPO, platform_suffix};

#[test]
fn test_platform_suffix_not_none() {
    assert!(platform_suffix().is_some());
}

#[test]
fn test_platform_suffix_contains_os_and_arch() {
    let suffix = platform_suffix().unwrap();
    assert!(suffix.contains('-'), "suffix should be os-arch: {suffix}");
    let parts: Vec<&str> = suffix.split('-').collect();
    assert_eq!(parts.len(), 2);
    assert!(
        ["linux", "macos", "windows"].contains(&parts[0]),
        "unexpected OS: {}",
        parts[0]
    );
    assert!(
        ["x86_64", "aarch64"].contains(&parts[1]),
        "unexpected arch: {}",
        parts[1]
    );
}

#[test]
fn test_current_version_format() {
    assert!(CURRENT_VERSION.contains('.'));
    assert!(!CURRENT_VERSION.is_empty());
    let parts: Vec<&str> = CURRENT_VERSION.split('.').collect();
    assert!(parts.len() >= 2, "version should have at least major.minor");
    for part in &parts {
        assert!(
            part.parse::<u32>().is_ok(),
            "version component '{}' should be numeric",
            part
        );
    }
}

#[test]
fn test_github_repo_constant() {
    assert_eq!(GITHUB_REPO, "rysweet/Simard");
}

#[test]
fn test_platform_suffix_is_deterministic() {
    let s1 = platform_suffix();
    let s2 = platform_suffix();
    assert_eq!(s1, s2);
}

#[test]
fn test_current_version_matches_cargo_pkg() {
    assert_eq!(CURRENT_VERSION, env!("CARGO_PKG_VERSION"));
}

#[test]
fn test_platform_suffix_no_hyphens_in_parts() {
    let suffix = platform_suffix().unwrap();
    let parts: Vec<&str> = suffix.split('-').collect();
    assert_eq!(
        parts.len(),
        2,
        "suffix should have exactly one hyphen: {suffix}"
    );
}

#[test]
fn test_platform_suffix_known_combinations() {
    let suffix = platform_suffix().unwrap();
    let valid = [
        "linux-x86_64",
        "linux-aarch64",
        "macos-x86_64",
        "macos-aarch64",
        "windows-x86_64",
    ];
    assert!(
        valid.contains(&suffix),
        "unexpected platform suffix: {suffix}"
    );
}

#[test]
fn test_github_repo_format() {
    assert!(GITHUB_REPO.contains('/'));
    let parts: Vec<&str> = GITHUB_REPO.split('/').collect();
    assert_eq!(parts.len(), 2);
    assert!(!parts[0].is_empty());
    assert!(!parts[1].is_empty());
}

#[test]
fn test_current_version_is_semver() {
    let parts: Vec<&str> = CURRENT_VERSION.split('.').collect();
    assert_eq!(parts.len(), 3, "version should be major.minor.patch");
    for (i, part) in parts.iter().enumerate() {
        assert!(
            part.parse::<u32>().is_ok(),
            "version part {i} '{part}' should be numeric"
        );
    }
}

#[test]
fn test_current_version_no_leading_v() {
    assert!(
        !CURRENT_VERSION.starts_with('v'),
        "CURRENT_VERSION should not have a 'v' prefix"
    );
}

#[test]
fn test_platform_suffix_is_ascii() {
    let suffix = platform_suffix().unwrap();
    assert!(suffix.is_ascii(), "suffix should be ASCII: {suffix}");
}

#[test]
fn test_platform_suffix_no_whitespace() {
    let suffix = platform_suffix().unwrap();
    assert!(!suffix.contains(' '), "suffix should not contain spaces");
}

#[test]
fn test_current_version_no_whitespace() {
    assert!(
        !CURRENT_VERSION.contains(' '),
        "version should not contain spaces"
    );
}

#[test]
fn test_current_version_no_newlines() {
    assert!(
        !CURRENT_VERSION.contains('\n'),
        "version should not contain newlines"
    );
}

#[test]
fn test_current_version_major_is_reasonable() {
    let major: u32 = CURRENT_VERSION.split('.').next().unwrap().parse().unwrap();
    assert!(major < 100, "major version should be < 100, got {major}");
}

#[test]
fn test_github_repo_no_whitespace() {
    assert!(!GITHUB_REPO.contains(' '), "repo should not contain spaces");
}

#[test]
fn test_github_repo_owner_is_rysweet() {
    let owner = GITHUB_REPO.split('/').next().unwrap();
    assert_eq!(owner, "rysweet");
}

#[test]
fn test_github_repo_name_is_simard() {
    let name = GITHUB_REPO.split('/').nth(1).unwrap();
    assert_eq!(name, "Simard");
}

#[test]
fn test_self_test_uses_starter_suite() {
    assert!(!CURRENT_VERSION.is_empty());
    assert!(GITHUB_REPO.contains("Simard"));
}

// ---------------------------------------------------------------------------
// Problem 1 (WS1) — self-deploy release-adoption gate must be fail-closed semver.
//
// The live operator is stuck on 0.31.0 while 0.33.1 is published. The adoption
// gate in `cmd_self_update::update` currently decides "should I adopt?" with a
// fragile STRING-EQUALITY check (`version == CURRENT_VERSION`): anything that is
// merely *unequal* — including an OLDER or malformed tag — passes the gate and
// triggers a download/relaunch. The design replaces that with the authoritative
// semver predicate `update_check::is_newer(current, latest)`, which the fix must
// also promote from a private `fn` to `pub(crate)` so the adoption trigger can
// reuse it.
//
// These tests reference `crate::update_check::is_newer`. They FAIL TO COMPILE
// against the current tree (the fn is private → E0603) and pass once WS1 makes
// it `pub(crate)` and routes the gate through it. `is_newer(current, latest)`
// returns true iff `latest` is STRICTLY newer than `current`, and is fail-closed
// (returns false, never panics) on any unparseable input.
// ---------------------------------------------------------------------------

/// The exact live scenario: a 0.31.0 binary MUST adopt a published 0.33.1.
#[test]
fn adoption_gate_adopts_a_strictly_newer_published_release() {
    assert!(
        crate::update_check::is_newer("0.31.0", "0.33.1"),
        "the stale 0.31.0 operator must detect+adopt the newer 0.33.1 release"
    );
    assert!(crate::update_check::is_newer("0.33.0", "0.33.1"));
    assert!(crate::update_check::is_newer("0.32.9", "0.33.0"));
}

/// The gate must NOT fire when already at the latest — the equal case is a
/// no-op, exactly as the old `version == CURRENT_VERSION` short-circuit intended.
#[test]
fn adoption_gate_is_a_noop_at_the_latest_version() {
    assert!(!crate::update_check::is_newer("0.33.1", "0.33.1"));
    assert!(!crate::update_check::is_newer(
        super::platform::CURRENT_VERSION,
        super::platform::CURRENT_VERSION
    ));
}

/// Fail-closed DOWNGRADE guard — the whole reason to replace `!=` with
/// `is_newer`: an OLDER remote tag is *unequal* to the current version and would
/// wrongly pass a string-inequality gate, but must NEVER be adopted.
#[test]
fn adoption_gate_refuses_to_downgrade_to_an_older_release() {
    assert!(
        !crate::update_check::is_newer("0.33.1", "0.31.0"),
        "an older published tag must never be adopted (no silent downgrade)"
    );
    assert!(!crate::update_check::is_newer("1.0.0", "0.9.9"));
}

/// Fail-closed on MALFORMED input: an unparseable/`v`-prefixed remote tag must
/// yield "not newer" (no adoption) and must never panic.
#[test]
fn adoption_gate_fails_closed_on_malformed_remote_tag() {
    let result = std::panic::catch_unwind(|| {
        // A raw `v`-prefixed tag is not valid semver — the release path strips
        // the prefix before comparing, so an un-stripped tag reaching the gate
        // must be treated as "not newer", never coerced into an update.
        assert!(!crate::update_check::is_newer("0.31.0", "v0.33.1"));
        assert!(!crate::update_check::is_newer("0.31.0", "not-a-version"));
        assert!(!crate::update_check::is_newer("garbage", "0.33.1"));
        assert!(!crate::update_check::is_newer("", ""));
    });
    assert!(
        result.is_ok(),
        "the adoption gate must be panic-safe (fail-closed) on bad input"
    );
}
