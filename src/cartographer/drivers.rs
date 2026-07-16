//! Probes for optional delivery runtimes.
//!
//! Cartographer's primary deliverable — the interactive `dashboard.html` — is
//! generated purely in Rust and served by the built-in static server, so it has
//! no external dependencies. The Streamlit and Observable delivery targets are
//! *optional*: Cartographer always emits their source, and merely records
//! whether a runtime is installed so the operator knows if they are runnable.
//!
//! These probes only ask `command -v` (a POSIX builtin that does not execute
//! the target). Cartographer never spawns a Python interpreter itself.

use std::process::Command;

use serde::{Deserialize, Serialize};

/// Availability of an optional delivery runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReport {
    pub name: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Return true if `bin` resolves on `PATH`. Uses `command -v`, which does not
/// execute the target binary.
pub fn binary_available(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Probe a runtime's availability, tagging it with its delivery role.
pub fn probe(bin: &str, role: &str) -> ToolReport {
    ToolReport {
        name: bin.to_string(),
        available: binary_available(bin),
        role: Some(role.to_string()),
    }
}

/// Probe the delivery runtimes Cartographer can target.
pub fn probe_delivery_runtimes() -> Vec<ToolReport> {
    vec![
        probe("streamlit", "streamlit-app"),
        probe("node", "observable-runtime"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_available_is_true_for_sh() {
        // `sh` itself must resolve for the probe mechanism to work at all.
        assert!(binary_available("sh"));
    }

    #[test]
    fn binary_available_is_false_for_nonsense() {
        assert!(!binary_available("this-binary-does-not-exist-xyzzy"));
    }

    #[test]
    fn probe_reports_name_and_role() {
        let report = probe("sh", "shell");
        assert_eq!(report.name, "sh");
        assert!(report.available);
        assert_eq!(report.role.as_deref(), Some("shell"));
    }

    #[test]
    fn delivery_runtimes_are_probed() {
        let reports = probe_delivery_runtimes();
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().any(|r| r.name == "streamlit"));
    }
}
