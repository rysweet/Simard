//! Platform detection and version constants.

pub(crate) const GITHUB_REPO: &str = "rysweet/Simard";
pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// cosign keyless verification identity for release assets (issues #2261 / #2252).
///
/// `simard update` pins the signer of a release tarball to THIS repository's
/// `release.yml` workflow running on `main`. The Fulcio certificate's Subject
/// Alternative Name must match this regexp and be issued by GitHub's OIDC
/// provider below. These two constants MUST stay in lockstep with the
/// `cosign sign-blob` identity produced by `.github/workflows/release.yml`.
pub(crate) const RELEASE_CERT_IDENTITY_REGEXP: &str =
    r"^https://github\.com/rysweet/Simard/\.github/workflows/release\.yml@refs/heads/main$";

/// OIDC issuer for GitHub Actions keyless signing certificates.
pub(crate) const RELEASE_CERT_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

/// Platform suffix for GitHub Release assets.
pub(crate) fn platform_suffix() -> Option<&'static str> {
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("linux-x86_64")
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        Some("linux-aarch64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        Some("macos-x86_64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        Some("macos-aarch64")
    } else if cfg!(target_os = "windows") {
        Some("windows-x86_64")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_is_nonempty() {
        assert!(!CURRENT_VERSION.is_empty());
    }

    #[test]
    fn github_repo_format() {
        assert!(GITHUB_REPO.contains("rysweet"));
        assert!(GITHUB_REPO.contains("Simard"));
    }

    #[test]
    fn platform_suffix_returns_some() {
        let suffix = platform_suffix();
        assert!(suffix.is_some());
        let s = suffix.unwrap();
        assert!(s.contains("linux") || s.contains("macos") || s.contains("windows"));
    }
}
