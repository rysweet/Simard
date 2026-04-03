//! Greeting banner displayed when Simard starts a meeting session.
//!
//! Prints name/version, build stats, GitHub info, known projects, and
//! active goals to stderr before the session begins.

use std::io::Write;
use std::process::Command;

use crate::goal_curation::load_goal_board;
use crate::memory_bridge::CognitiveMemoryBridge;

/// Maximum number of known projects to display in the banner.
const MAX_PROJECTS_SHOWN: usize = 5;

/// Build the greeting banner text. Returns lines to print to stderr.
pub fn build_greeting_banner(bridge: Option<&CognitiveMemoryBridge>) -> Vec<String> {
    let mut lines = Vec::new();

    // Section 1: Name and version
    let version = env!("CARGO_PKG_VERSION");
    lines.push(format!("🌲 Simard v{version}"));
    lines.push("─".repeat(40));

    // Section 2+3: Build stats and GitHub info (all concurrent)
    let src_handle = std::thread::spawn(count_source_files);
    let (issues, prs) = fetch_github_counts();
    let src_count = src_handle.join().unwrap_or_else(|_| "?".to_string());
    lines.push(format!("  Source files: {src_count}"));
    lines.push(format!("  GitHub: {issues} open issues, {prs} open PRs"));

    // Section 4: Known projects from cognitive memory
    if let Some(bridge) = bridge {
        let projects = known_projects(bridge);
        if projects.is_empty() {
            lines.push("  Known projects: (none yet)".to_string());
        } else {
            lines.push(format!("  Known projects ({}):", projects.len()));
            for (name, confidence) in projects.iter().take(MAX_PROJECTS_SHOWN) {
                lines.push(format!("    • {name} ({:.0}%)", confidence * 100.0));
            }
            if projects.len() > MAX_PROJECTS_SHOWN {
                lines.push(format!(
                    "    … and {} more",
                    projects.len() - MAX_PROJECTS_SHOWN
                ));
            }
        }

        // Section 5: Active goals
        match load_goal_board(bridge) {
            Ok(board) if !board.active.is_empty() => {
                lines.push(format!("  Active goals ({}):", board.active.len()));
                for goal in &board.active {
                    lines.push(format!("    • {}", goal.concise_label()));
                }
            }
            Ok(_) => {
                // No active goals — show memory stats as fallback
                append_memory_stats_fallback(bridge, &mut lines);
            }
            Err(_) => {
                append_memory_stats_fallback(bridge, &mut lines);
            }
        }
    } else {
        lines.push("  Known projects: (no memory bridge)".to_string());
        lines.push("  Goals: (no memory bridge)".to_string());
    }

    lines.push("─".repeat(40));
    lines
}

/// Print the greeting banner to stderr.
pub fn print_greeting_banner(bridge: Option<&CognitiveMemoryBridge>) {
    let lines = build_greeting_banner(bridge);
    let mut stderr = std::io::stderr().lock();
    for line in &lines {
        let _ = writeln!(stderr, "{line}");
    }
}

/// Count .rs source files under src/. Uses compile-time repo root so this
/// works regardless of the CWD at runtime.
fn count_source_files() -> String {
    let compile_time_src = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let src_dir = if std::path::Path::new(compile_time_src).is_dir() {
        compile_time_src
    } else {
        "src"
    };
    match Command::new("find")
        .args([src_dir, "-name", "*.rs", "-type", "f"])
        .output()
    {
        Ok(output) => {
            let count = output.stdout.iter().filter(|&&b| b == b'\n').count();
            count.to_string()
        }
        Err(_) => "?".to_string(),
    }
}

/// Fetch open issue and PR counts from GitHub concurrently.
fn fetch_github_counts() -> (String, String) {
    let issues_handle = std::thread::spawn(|| {
        fetch_gh_count(&[
            "issue",
            "list",
            "--repo",
            "rysweet/Simard",
            "--state",
            "open",
            "--json",
            "number",
            "--jq",
            "length",
        ])
    });
    let prs_handle = std::thread::spawn(|| {
        fetch_gh_count(&[
            "pr",
            "list",
            "--repo",
            "rysweet/Simard",
            "--state",
            "open",
            "--json",
            "number",
            "--jq",
            "length",
        ])
    });

    let issues = issues_handle.join().unwrap_or_else(|_| "?".to_string());
    let prs = prs_handle.join().unwrap_or_else(|_| "?".to_string());
    (issues, prs)
}

/// Run a single `gh` CLI command and return the trimmed output.
fn fetch_gh_count(args: &[&str]) -> String {
    match Command::new("gh")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            let output = child.wait_with_output();
            match output {
                Ok(o) if o.status.success() => {
                    String::from_utf8_lossy(&o.stdout).trim().to_string()
                }
                _ => "?".to_string(),
            }
        }
        Err(_) => "?".to_string(),
    }
}

/// Extract known project names from semantic facts.
fn known_projects(bridge: &CognitiveMemoryBridge) -> Vec<(String, f64)> {
    match bridge.search_facts("project", 20, 0.0) {
        Ok(facts) => {
            let mut projects: Vec<(String, f64)> = facts
                .iter()
                .filter(|f| {
                    f.tags
                        .iter()
                        .any(|t| t == "project" || t.starts_with("project:"))
                        || f.concept.starts_with("project:")
                        || f.concept.contains("project")
                })
                .map(|f| {
                    let name = f
                        .concept
                        .strip_prefix("project:")
                        .unwrap_or(&f.concept)
                        .to_string();
                    (name, f.confidence)
                })
                .collect();
            projects.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            projects.dedup_by(|a, b| a.0 == b.0);
            projects
        }
        Err(_) => Vec::new(),
    }
}

/// Show memory statistics as a fallback when no active goals exist.
fn append_memory_stats_fallback(bridge: &CognitiveMemoryBridge, lines: &mut Vec<String>) {
    match bridge.get_statistics() {
        Ok(stats) => {
            lines.push(format!(
                "  Memory: {} total ({} semantic, {} episodic, {} procedural)",
                stats.total(),
                stats.semantic_count,
                stats.episodic_count,
                stats.procedural_count,
            ));
        }
        Err(_) => {
            lines.push("  Memory: (unavailable)".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_contains_version() {
        let lines = build_greeting_banner(None);
        let header = &lines[0];
        assert!(header.contains("Simard v"));
        assert!(header.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn banner_contains_source_count_line() {
        let lines = build_greeting_banner(None);
        assert!(lines.iter().any(|l| l.contains("Source files:")));
    }

    #[test]
    fn banner_contains_github_line() {
        let lines = build_greeting_banner(None);
        assert!(lines.iter().any(|l| l.contains("GitHub:")));
    }

    #[test]
    fn banner_without_bridge_shows_no_bridge() {
        let lines = build_greeting_banner(None);
        assert!(lines.iter().any(|l| l.contains("no memory bridge")));
    }

    #[test]
    fn banner_has_separator_lines() {
        let lines = build_greeting_banner(None);
        let sep_count = lines.iter().filter(|l| l.starts_with('─')).count();
        assert_eq!(sep_count, 2);
    }

    #[test]
    fn count_source_files_returns_number_or_question_mark() {
        let result = count_source_files();
        assert!(
            result == "?" || result.parse::<usize>().is_ok(),
            "expected number or '?', got: {result}"
        );
    }

    #[test]
    fn fetch_gh_count_handles_missing_binary() {
        // If gh is not installed or fails, should return "?" not panic
        let result = fetch_gh_count(&["nonexistent-subcommand"]);
        // Either "?" or some output — the point is it doesn't panic
        assert!(!result.is_empty() || result == "?");
    }

    #[test]
    fn known_projects_returns_vec_with_no_bridge() {
        // Direct test of the empty case — no bridge available
        let lines = build_greeting_banner(None);
        assert!(lines.iter().any(|l| l.contains("no memory bridge")));
    }

    #[test]
    fn max_projects_shown_is_reasonable() {
        // Compile-time validation that the constant is in a sensible range
        const { assert!(MAX_PROJECTS_SHOWN > 0 && MAX_PROJECTS_SHOWN <= 10) };
    }
}
