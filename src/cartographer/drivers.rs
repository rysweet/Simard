//! Drivers for optional external delivery tools.
//!
//! The Cartographer dashboard (`dashboard.html`) is pure client-side Plotly and
//! needs no external tooling. These probes report the *optional* engines used
//! for alternate delivery — Streamlit (`app.py`), Python, and Node/Observable —
//! so the manifest can record what a host can additionally run. Absent tools are
//! reported, never fatal (graceful degradation).

use std::process::Command;

/// Availability + version of an optional external tool.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolReport {
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
}

/// Return true if `bin` resolves on PATH. Uses the POSIX `command -v` builtin,
/// which does not execute the target.
pub fn binary_available(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort first line of `bin --version`.
fn tool_version(bin: &str) -> Option<String> {
    let output = Command::new(bin).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let text = if text.trim().is_empty() {
        String::from_utf8_lossy(&output.stderr).into_owned()
    } else {
        text
    };
    text.lines().next().map(|l| l.trim().to_string())
}

/// Probe a tool's availability and version.
pub fn probe(bin: &str) -> ToolReport {
    let available = binary_available(bin);
    ToolReport {
        name: bin.to_string(),
        available,
        version: if available { tool_version(bin) } else { None },
    }
}

/// Probe the optional delivery engines Cartographer can additionally use.
pub fn probe_delivery_tools() -> Vec<ToolReport> {
    vec![probe("python3"), probe("streamlit"), probe("node")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binary_is_unavailable() {
        let report = probe("definitely-not-a-real-binary-xyzzy");
        assert!(!report.available);
        assert!(report.version.is_none());
    }

    #[test]
    fn sh_itself_is_available() {
        // `sh` is required to be present for the probe mechanism to work.
        assert!(binary_available("sh"));
    }

    #[test]
    fn probe_delivery_tools_reports_three_engines() {
        let reports = probe_delivery_tools();
        assert_eq!(reports.len(), 3);
        assert!(reports.iter().any(|r| r.name == "streamlit"));
    }
}
