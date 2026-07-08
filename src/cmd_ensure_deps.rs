//! Dependency checker: verifies that Simard runtime dependencies are present.
//!
//! Checks for the minimal runtime tools (`git`, `python3`, `gh`) and reports
//! missing ones with actionable guidance rather than auto-installing them —
//! Simard's native Rust modules now cover capabilities that previously required
//! the Python amplihack installation.
//!
//! Cognitive memory is embedded **lbug** (LadybugDB), compiled and linked into
//! the binary via `amplihack-memory-lib`'s `persistent` feature — it is *not*
//! the Python `kuzu` package. There is therefore no `kuzu` probe here; a stale
//! kuzu check was removed (issue #3119) because it was misleading noise. The
//! heavier build/host preconditions (Rust toolchain, `build-essential`, `cmake`,
//! `clang`, `pkg-config`, `libssl-dev`, the `amplihack` CLI + bundle, and the
//! lbug from-source build prerequisites) are the platform installer's preflight
//! doctor's responsibility (`simard platform doctor`), not this minimal runtime
//! check.

use std::process::Command;

/// Summary of a single dependency check.
struct DepCheck {
    name: &'static str,
    status: DepStatus,
}

enum DepStatus {
    Ok(String),
    Missing(String),
}

/// Run all dependency checks and report results.
pub fn handle_ensure_deps() -> Result<(), Box<dyn std::error::Error>> {
    println!("simard ensure-deps: checking runtime dependencies\n");

    let results = vec![
        check_binary("git", &["--version"]),
        check_binary("python3", &["--version"]),
        check_binary("gh", &["--version"]),
    ];

    println!();
    let mut failed = 0;
    for dep in &results {
        let (icon, detail) = match &dep.status {
            DepStatus::Ok(msg) => ("✓", msg.as_str()),
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
        println!("All dependencies satisfied.");
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
}
