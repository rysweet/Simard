//! Parse `cargo audit --json` output into [`Advisory`] records (issue #2741).
//!
//! Only the **security-vulnerability** list is consumed — the `vulnerabilities`
//! array reported against `Cargo.lock`. Informational `warnings`
//! (unmaintained / unsound / yanked) are deliberately ignored here: they follow
//! the existing `deny.toml` policy and are never auto-suppressed by the reasoner
//! (see `docs/reference/supply-chain-advisory-stewardship.md` § Advisory scope).

use crate::error::{SimardError, SimardResult};

use super::types::{Advisory, PatchStatus};

/// Parse the JSON emitted by `cargo audit --json` into the list of
/// security-vulnerability advisories affecting the lockfile.
///
/// Returns an empty vec when `cargo audit` reports no vulnerabilities. Malformed
/// or unexpected JSON yields [`SimardError::SupplyChainAuditParseFailed`].
pub fn parse_audit_json(json: &str) -> SimardResult<Vec<Advisory>> {
    let report: AuditReport =
        serde_json::from_str(json).map_err(|e| SimardError::SupplyChainAuditParseFailed {
            reason: format!("invalid cargo-audit JSON: {e}"),
        })?;

    Ok(report
        .vulnerabilities
        .list
        .into_iter()
        .map(Advisory::from)
        .collect())
}

/// Top-level `cargo audit --json` document (only the fields we consume). Unknown
/// fields are ignored so a future cargo-audit schema addition does not break the
/// parse.
#[derive(serde::Deserialize)]
struct AuditReport {
    #[serde(default)]
    vulnerabilities: Vulnerabilities,
}

#[derive(serde::Deserialize, Default)]
struct Vulnerabilities {
    #[serde(default)]
    list: Vec<RawVulnerability>,
}

#[derive(serde::Deserialize)]
struct RawVulnerability {
    advisory: RawAdvisory,
    #[serde(default)]
    versions: RawVersions,
    package: RawPackage,
}

#[derive(serde::Deserialize)]
struct RawAdvisory {
    id: String,
    #[serde(default)]
    title: String,
    /// `url` is `null` for some advisories; fall back to the canonical rustsec URL.
    #[serde(default)]
    url: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct RawVersions {
    /// Semver requirement strings that are fixed, e.g. `[">= 0.9.20"]`. Empty
    /// when no fixed release exists.
    #[serde(default)]
    patched: Vec<String>,
}

#[derive(serde::Deserialize)]
struct RawPackage {
    name: String,
    version: String,
}

impl From<RawVulnerability> for Advisory {
    fn from(raw: RawVulnerability) -> Self {
        let patched = if raw.versions.patched.is_empty() {
            PatchStatus::None
        } else {
            PatchStatus::Fixed {
                requirement: raw.versions.patched.join(", "),
            }
        };
        let url = raw
            .advisory
            .url
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("https://rustsec.org/advisories/{}", raw.advisory.id));
        Advisory {
            id: raw.advisory.id,
            crate_name: raw.package.name,
            installed: raw.package.version,
            patched,
            title: raw.advisory.title,
            url,
        }
    }
}
