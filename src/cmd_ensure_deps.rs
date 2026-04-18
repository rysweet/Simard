//! Dependency checker: verifies that Simard runtime dependencies are present.
//!
//! Checks for required tools (python3, gh, git) and Python packages (kuzu).
//! Reports missing dependencies with actionable guidance rather than
//! auto-installing them — Simard's native Rust modules now cover capabilities
//! that previously required the Python amplihack installation.

use std::process::Command;

/// Summary of a single dependency check.
struct DepCheck {
    name: &'static str,
    status: DepStatus,
}

enum DepStatus {
    Ok(String),
    Missing(String),
    Warning(String),
}

/// Run all dependency checks and report results.
pub fn handle_ensure_deps() -> Result<(), Box<dyn std::error::Error>> {
    println!("simard ensure-deps: checking runtime dependencies\n");

    let results = vec![
        check_binary("git", &["--version"]),
        check_binary("python3", &["--version"]),
        check_binary("gh", &["--version"]),
        check_python_package("kuzu"),
    ];

    println!();
    let mut failed = 0;
    let mut warnings = 0;
    for dep in &results {
        let (icon, detail) = match &dep.status {
            DepStatus::Ok(msg) => ("✓", msg.as_str()),
            DepStatus::Warning(msg) => {
                warnings += 1;
                ("⚠", msg.as_str())
            }
            DepStatus::Missing(msg) => {
                failed += 1;
                ("✗", msg.as_str())
            }
        };
        println!("  {icon} {}: {detail}", dep.name);
    }

    println!();
    if failed > 0 {
        Err(format!("{failed} dependency check(s) failed").into())
    } else {
        if warnings > 0 {
            println!("All required dependencies satisfied ({warnings} optional warning(s)).");
        } else {
            println!("All dependencies satisfied.");
        }
        Ok(())
    }
}

fn check_binary(name: &'static str, version_args: &[&str]) -> DepCheck {
    match Command::new(name).args(version_args).output() {
        Ok(output) if output.status.success() => {
            let ver = String::from_utf8_lossy(&output.stdout)
                .trim()
                .lines()
                .next()
                .unwrap_or("ok")
                .to_string();
            DepCheck {
                name,
                status: DepStatus::Ok(ver),
            }
        }
        Ok(_) => DepCheck {
            name,
            status: DepStatus::Missing(format!("{name} found but returned error")),
        },
        Err(_) => DepCheck {
            name,
            status: DepStatus::Missing(format!("{name} not found in PATH")),
        },
    }
}

fn check_python_package(package: &'static str) -> DepCheck {
    let check = Command::new("python3")
        .args(["-c", &format!("import {package}")])
        .output();

    if let Ok(output) = &check
        && output.status.success()
    {
        return DepCheck {
            name: package,
            status: DepStatus::Ok("importable".into()),
        };
    }

    DepCheck {
        name: package,
        status: DepStatus::Warning(format!(
            "not importable; install with: pip install {package}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_binary_finds_git() {
        let result = check_binary("git", &["--version"]);
        assert!(matches!(result.status, DepStatus::Ok(_)));
    }

    #[test]
    fn check_binary_missing_returns_missing() {
        let result = check_binary("nonexistent-binary-xyz", &["--version"]);
        assert!(matches!(result.status, DepStatus::Missing(_)));
    }

    #[test]
    fn check_python_package_reports_status() {
        // A package that definitely doesn't exist should warn
        let result = check_python_package("nonexistent_pkg_xyz_12345");
        assert!(matches!(result.status, DepStatus::Warning(_)));
    }
}
