//! Read/write the two advisory ignore lists Simard maintains, kept in sync
//! (issue #2741).
//!
//! Two independent gates read two independent ignore lists, and drift between
//! them is exactly what re-breaks CI:
//!
//! - **`deny.toml`** `[advisories] ignore` — inline
//!   `{ id = "…", reason = "… <tracking-url>" }` tables (the [`cargo-deny`] style).
//! - **`.cargo/audit.toml`** `[advisories] ignore` — bare advisory-ID strings
//!   (the [`cargo-audit`] style).
//!
//! Writes are **textual**, inserting/removing entries in place so the carefully
//! curated comment blocks in both files are preserved (a full TOML re-serialise
//! would drop them). The pure string transforms are unit-tested; [`IgnoreFiles`]
//! is the thin file-facing wrapper the execution layer drives.
//!
//! [`cargo-deny`]: https://embarkstudios.github.io/cargo-deny/
//! [`cargo-audit`]: https://github.com/rustsec/rustsec/tree/main/cargo-audit

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

/// The pair of ignore files, addressed relative to a repository root.
pub struct IgnoreFiles {
    deny_path: PathBuf,
    audit_path: PathBuf,
}

impl IgnoreFiles {
    /// Address `deny.toml` and `.cargo/audit.toml` under `root`.
    pub fn at_root(root: &Path) -> Self {
        Self {
            deny_path: root.join("deny.toml"),
            audit_path: root.join(".cargo").join("audit.toml"),
        }
    }

    /// True when `id` is ignored in **both** files (the only state that
    /// actually suppresses the advisory across both gates).
    pub fn is_ignored(&self, id: &str) -> SimardResult<bool> {
        let deny = deny_ignored_ids(&read(&self.deny_path)?);
        let audit = audit_ignored_ids(&read(&self.audit_path)?);
        Ok(deny.contains(id) && audit.contains(id))
    }

    /// True when the two files list the identical set of ignored advisory IDs.
    /// A partial write would break this; the execution layer asserts it after
    /// every mutation.
    pub fn ignored_ids_in_sync(&self) -> SimardResult<bool> {
        let deny = deny_ignored_ids(&read(&self.deny_path)?);
        let audit = audit_ignored_ids(&read(&self.audit_path)?);
        Ok(deny == audit)
    }

    /// Add a justified ignore for `id` to **both** files, embedding
    /// `tracking_url` in the recorded reason.
    ///
    /// HARD RAIL: refuses with
    /// [`SimardError::SupplyChainSuppressionWithoutTracker`] if `tracking_url`
    /// is empty — an advisory can never be suppressed without an open tracker.
    /// Idempotent: an `id` already present in a file is left untouched there.
    pub fn add_justified_ignore(
        &self,
        id: &str,
        reason: &str,
        tracking_url: &str,
    ) -> SimardResult<()> {
        if tracking_url.trim().is_empty() {
            return Err(SimardError::SupplyChainSuppressionWithoutTracker {
                advisory_id: id.to_string(),
            });
        }
        let full_reason = if reason.contains(tracking_url) {
            reason.to_string()
        } else {
            format!("{reason} Tracked: {tracking_url}")
        };

        let deny = read(&self.deny_path)?;
        let deny = insert_deny_ignore(&deny, id, &full_reason)?;
        write(&self.deny_path, &deny)?;

        let audit = read(&self.audit_path)?;
        let audit = insert_audit_ignore(&audit, id, &full_reason)?;
        write(&self.audit_path, &audit)?;
        Ok(())
    }

    /// Remove a (now-stale) ignore for `id` from **both** files. Idempotent:
    /// absent IDs are a no-op. Used to correct an ignore whose upstream fix has
    /// since shipped.
    pub fn remove_ignore(&self, id: &str) -> SimardResult<()> {
        let deny = read(&self.deny_path)?;
        write(&self.deny_path, &remove_ignore_entry(&deny, id))?;
        let audit = read(&self.audit_path)?;
        write(&self.audit_path, &remove_ignore_entry(&audit, id))?;
        Ok(())
    }
}

fn read(path: &Path) -> SimardResult<String> {
    std::fs::read_to_string(path).map_err(|e| SimardError::SupplyChainRemediationFailed {
        reason: format!("read {}: {e}", path.display()),
    })
}

fn write(path: &Path, content: &str) -> SimardResult<()> {
    std::fs::write(path, content).map_err(|e| SimardError::SupplyChainRemediationFailed {
        reason: format!("write {}: {e}", path.display()),
    })
}

// ───────────────────────── pure parsing ─────────────────────────

/// The set of advisory IDs ignored in a `deny.toml`'s `[advisories] ignore`
/// (each entry an inline `{ id = "…", … }` table, or a bare string).
pub fn deny_ignored_ids(deny_toml: &str) -> BTreeSet<String> {
    ignored_ids(deny_toml)
}

/// The set of advisory IDs ignored in a `.cargo/audit.toml`'s
/// `[advisories] ignore` (each entry a bare `"RUSTSEC-…"` string).
pub fn audit_ignored_ids(audit_toml: &str) -> BTreeSet<String> {
    ignored_ids(audit_toml)
}

/// Extract ignored IDs from either file shape via a real TOML parse. An entry
/// contributes its `id` field (inline-table style) or its own string value
/// (bare style). A file that fails to parse yields an empty set (the caller
/// treats "not provably ignored" as "not ignored", the safe default).
fn ignored_ids(toml_str: &str) -> BTreeSet<String> {
    #[derive(serde::Deserialize, Default)]
    struct Doc {
        #[serde(default)]
        advisories: Advisories,
    }
    #[derive(serde::Deserialize, Default)]
    struct Advisories {
        #[serde(default)]
        ignore: Vec<toml::Value>,
    }

    let Ok(doc) = toml::from_str::<Doc>(toml_str) else {
        return BTreeSet::new();
    };
    doc.advisories
        .ignore
        .iter()
        .filter_map(|entry| match entry {
            toml::Value::String(s) => Some(s.clone()),
            toml::Value::Table(t) => t.get("id").and_then(|v| v.as_str()).map(str::to_string),
            _ => None,
        })
        .collect()
}

// ───────────────────────── pure insertion ─────────────────────────

/// Insert a `deny.toml`-style inline ignore for `id` (matching the existing
/// `RUSTSEC-2023-0071` entry: `{ id = "…", reason = "…" }`). No-op if `id` is
/// already present.
pub fn insert_deny_ignore(deny_toml: &str, id: &str, reason: &str) -> SimardResult<String> {
    if deny_ignored_ids(deny_toml).contains(id) {
        return Ok(deny_toml.to_string());
    }
    // Escape `id` for the same defense-in-depth reason `reason` is escaped: it is
    // interpolated into a TOML basic string (and a `#` comment). Today `id` is
    // constrained to the RustSec/CVE/GHSA ID format from the SHA-pinned
    // advisory-db, but escaping keeps this robust if that source ever loosens.
    let id = escape_toml_basic(id);
    let entry = format!(
        "    # {id}: auto-added by the supply-chain steward (#2741) — no fixed \
         upstream release; not reachable in Simard's usage.\n    \
         {{ id = \"{id}\", reason = \"{reason}\" }},",
        reason = escape_toml_basic(reason),
    );
    insert_into_ignore_array(deny_toml, &entry)
}

/// Insert a `.cargo/audit.toml`-style bare-ID ignore for `id`, above a comment
/// carrying the justification + tracking link. No-op if `id` is already present.
pub fn insert_audit_ignore(audit_toml: &str, id: &str, reason: &str) -> SimardResult<String> {
    if audit_ignored_ids(audit_toml).contains(id) {
        return Ok(audit_toml.to_string());
    }
    // Escape `id` before it lands in the TOML bare-string value (see the
    // matching note in `insert_deny_ignore`).
    let id = escape_toml_basic(id);
    let entry = format!(
        "    # {reason}\n    \"{id}\",",
        reason = reason.replace('\n', " ")
    );
    insert_into_ignore_array(audit_toml, &entry)
}

/// Insert `entry` (already indented) immediately before the closing `]` of the
/// first `ignore = [ … ]` array. Handles the multi-line array both files use,
/// and the inline-empty `ignore = []` case.
fn insert_into_ignore_array(content: &str, entry: &str) -> SimardResult<String> {
    let lines: Vec<&str> = content.lines().collect();
    let opener = lines
        .iter()
        .position(|l| is_ignore_opener(l))
        .ok_or_else(|| SimardError::SupplyChainRemediationFailed {
            reason: "no `[advisories] ignore = [` array found to insert into".to_string(),
        })?;

    // Inline `ignore = []` (opener line also closes the array): expand it.
    if lines[opener].contains(']') {
        let expanded = lines[opener].replacen("[]", &format!("[\n{entry}\n]"), 1);
        let result: Vec<String> = lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if i == opener {
                    expanded.clone()
                } else {
                    (*l).to_string()
                }
            })
            .collect();
        return Ok(join_preserving_trailing_newline(content, &result));
    }

    // Multi-line: find the first `]` line after the opener.
    let closer = (opener + 1..lines.len())
        .find(|&j| lines[j].trim() == "]")
        .ok_or_else(|| SimardError::SupplyChainRemediationFailed {
            reason: "unterminated `ignore = [` array (no closing `]`)".to_string(),
        })?;

    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    result.insert(closer, entry.to_string());
    Ok(join_preserving_trailing_newline(content, &result))
}

/// Remove the ignore entry for `id` (and any contiguous comment lines directly
/// above it) from a `deny.toml` or `.cargo/audit.toml`. Idempotent.
pub fn remove_ignore_entry(content: &str, id: &str) -> String {
    let needle = format!("\"{id}\"");
    let lines: Vec<&str> = content.lines().collect();
    let Some(entry_idx) = lines.iter().position(|l| l.contains(&needle)) else {
        return content.to_string();
    };

    // Walk back over contiguous comment lines that form this entry's block.
    let mut start = entry_idx;
    while start > 0 {
        let prev = lines[start - 1].trim_start();
        if prev.starts_with('#') {
            start -= 1;
        } else {
            break;
        }
    }

    let kept: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < start || *i > entry_idx)
        .map(|(_, l)| (*l).to_string())
        .collect();
    join_preserving_trailing_newline(content, &kept)
}

fn is_ignore_opener(line: &str) -> bool {
    let t = line.trim_start();
    (t.starts_with("ignore") && t.contains('=') && t.contains('['))
        && t["ignore".len()..].trim_start().starts_with('=')
}

/// TOML basic-string escaping for the reason field (backslash + double quote).
fn escape_toml_basic(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

/// Re-join lines, restoring the original trailing newline if the source had one.
fn join_preserving_trailing_newline(original: &str, lines: &[String]) -> String {
    let mut out = lines.join("\n");
    if original.ends_with('\n') {
        out.push('\n');
    }
    out
}
