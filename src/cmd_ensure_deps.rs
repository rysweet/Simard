//! Dependency auto-installer: ensures all Simard runtime dependencies are present.
//!
//! Checks for required tools (python3, gh, amplihack, git), Python packages
//! (kuzu, amplihack-memory), and the amplihack-memory-lib source tree. Installs
//! or clones anything missing so Simard can operate without pre-existing setup.

use std::path::PathBuf;
use std::process::Command;

/// Summary of a single dependency check.
struct DepCheck {
    name: &'static str,
    status: DepStatus,
}

enum DepStatus {
    Ok(String),
    Missing(String),
    Installed(String),
    Failed(String),
}

/// Run all dependency checks and install anything missing.
pub fn handle_ensure_deps() -> Result<(), Box<dyn std::error::Error>> {
    println!("simard ensure-deps: checking runtime dependencies\n");

    let results = vec![
        check_binary("git", &["--version"]),
        check_binary("python3", &["--version"]),
        check_binary("gh", &["--version"]),
        check_amplihack(),
        ensure_python_package("kuzu"),
        ensure_memory_lib(),
    ];

    println!();
    let mut failed = 0;
    for dep in &results {
        let (icon, detail) = match &dep.status {
            DepStatus::Ok(msg) => ("✓", msg.as_str()),
            DepStatus::Installed(msg) => ("⟳", msg.as_str()),
            DepStatus::Missing(msg) => {
                failed += 1;
                ("✗", msg.as_str())
            }
            DepStatus::Failed(msg) => {
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

fn check_amplihack() -> DepCheck {
    if let Ok(output) = Command::new("amplihack").arg("--version").output()
        && output.status.success()
    {
        let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return DepCheck {
            name: "amplihack",
            status: DepStatus::Ok(ver),
        };
    }

    // Try installing via cargo
    println!("  installing amplihack via cargo...");
    match Command::new("cargo")
        .args(["install", "amplihack"])
        .output()
    {
        Ok(output) if output.status.success() => DepCheck {
            name: "amplihack",
            status: DepStatus::Installed("installed via cargo".into()),
        },
        _ => DepCheck {
            name: "amplihack",
            status: DepStatus::Missing("not found; cargo install amplihack failed".into()),
        },
    }
}

fn ensure_python_package(package: &'static str) -> DepCheck {
    // Check if importable
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

    // Try pip install
    println!("  installing {package} via pip...");
    let install = Command::new("python3")
        .args([
            "-m",
            "pip",
            "install",
            "--break-system-packages",
            "--quiet",
            package,
        ])
        .output();

    match install {
        Ok(output) if output.status.success() => DepCheck {
            name: package,
            status: DepStatus::Installed(format!("installed {package}")),
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            DepCheck {
                name: package,
                status: DepStatus::Failed(format!("pip install failed: {}", stderr.trim())),
            }
        }
        Err(e) => DepCheck {
            name: package,
            status: DepStatus::Failed(format!("pip not available: {e}")),
        },
    }
}

fn memory_lib_candidates() -> Vec<PathBuf> {
    let home = dirs_or_home();
    vec![
        home.join("src/amplirusty/amplihack-memory-lib/src"),
        home.join("amplirusty/amplihack-memory-lib/src"),
    ]
}

fn dirs_or_home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/usr/local"), PathBuf::from)
}

fn ensure_memory_lib() -> DepCheck {
    let name = "amplihack-memory-lib";

    // Check if already importable
    if let Ok(output) = Command::new("python3")
        .args(["-c", "import amplihack_memory"])
        .output()
        && output.status.success()
    {
        return DepCheck {
            name,
            status: DepStatus::Ok("importable".into()),
        };
    }

    // Check candidate paths
    for candidate in memory_lib_candidates() {
        if candidate.join("amplihack_memory/__init__.py").exists() {
            return DepCheck {
                name,
                status: DepStatus::Ok(format!("found at {}", candidate.display())),
            };
        }
    }

    // Clone amplihack repo to get the memory lib
    let target = dirs_or_home().join("src/amplirusty");
    if !target.exists() {
        println!("  cloning amplihack repo for memory lib...");
        let result = Command::new("git")
            .args([
                "clone",
                "--depth=1",
                "https://github.com/rysweet/amplihack.git",
                &target.to_string_lossy(),
            ])
            .output();

        match result {
            Ok(output) if output.status.success() => {
                // Verify the memory lib exists in the clone
                let lib_path = target.join("amplihack-memory-lib/src");
                if lib_path.join("amplihack_memory/__init__.py").exists() {
                    return DepCheck {
                        name,
                        status: DepStatus::Installed(format!("cloned to {}", lib_path.display())),
                    };
                }
            }
            _ => {}
        }
    }

    // Check if it exists in the clone even if we didn't just clone
    let lib_path = target.join("amplihack-memory-lib/src");
    if lib_path.join("amplihack_memory/__init__.py").exists() {
        return DepCheck {
            name,
            status: DepStatus::Ok(format!("found at {}", lib_path.display())),
        };
    }

    DepCheck {
        name,
        status: DepStatus::Failed("could not locate or install amplihack-memory-lib".into()),
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
    fn memory_lib_candidates_are_absolute() {
        for path in memory_lib_candidates() {
            assert!(
                path.is_absolute(),
                "candidate should be absolute: {}",
                path.display()
            );
        }
    }

    #[test]
    fn dirs_or_home_returns_absolute() {
        let home = dirs_or_home();
        assert!(home.is_absolute());
    }
}
