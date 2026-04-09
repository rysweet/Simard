//! Platform detection and version constants.

pub(crate) const GITHUB_REPO: &str = "rysweet/Simard";
pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

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
