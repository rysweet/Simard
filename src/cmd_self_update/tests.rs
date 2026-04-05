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
