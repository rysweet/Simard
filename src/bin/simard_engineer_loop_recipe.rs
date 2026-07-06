//! Thin driver bin that runs the engineer loop via the recipe runner.
//! Replaces direct `run_local_engineer_loop` calls — Phase 2 of the
//! recipes-first Simard rebuild.
//!
//! Usage: simard-engineer-loop-recipe \
//!          --workspace <path> --objective <text> \
//!          --topology <single-process|multi-process|distributed> \
//!          --state-root <path>

use std::env;
use std::process::{Command, ExitCode};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().collect();
    let args = &argv[1..].to_vec();

    let workspace = match arg(args, "--workspace") {
        Some(s) => s,
        None => {
            eprintln!(
                "usage: simard-engineer-loop-recipe --workspace <path> --objective <text> --topology <kebab> --state-root <path>"
            );
            return ExitCode::from(2);
        }
    };
    let objective = match arg(args, "--objective") {
        Some(s) => s,
        None => {
            eprintln!("missing --objective");
            return ExitCode::from(2);
        }
    };
    // Bound the free-text objective before it rides on argv (issues #2640/#2692):
    // a generous 8000-char ceiling defensively closes the E2BIG argv-overflow
    // class (8000 chars ≪ ARG_MAX), and the same pass collapses newlines so a
    // multi-line objective can never break the recipe's YAML interpolation (#2127).
    //
    // A `spawn_payload::recipe_context` file channel (`objective_path`) would make
    // this lossless, but `simard-engineer-loop.yaml` is an EXTERNAL asset
    // (amplihack bundle, not this repo) that reads `{{objective}}` inline with no
    // `{{objective_path}}` support — filing a large value would leave the loop
    // with an empty objective. Bounded-inline stays the safe disposition until
    // that external asset gains a `_path` read.
    let objective = simard::ooda_brain::sanitize::sanitize_context_var(&objective, 8000);
    let topology = arg(args, "--topology").unwrap_or_else(|| "single-process".to_string());
    let state_root = match arg(args, "--state-root") {
        Some(s) => s,
        None => {
            eprintln!("missing --state-root");
            return ExitCode::from(2);
        }
    };

    let recipe_path = env::var("SIMARD_ENGINEER_RECIPE_PATH")
        .unwrap_or_else(|_| "amplifier-bundle/recipes/simard-engineer-loop.yaml".to_string());

    let status = Command::new("amplihack")
        .args([
            "recipe",
            "run",
            &recipe_path,
            "-c",
            &format!("workspace_root={workspace}"),
            "-c",
            &format!("objective={objective}"),
            "-c",
            &format!("topology={topology}"),
            "-c",
            &format!("state_root={state_root}"),
            "--verbose",
        ])
        .status();

    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => {
            eprintln!("amplihack recipe run failed: {s}");
            ExitCode::from(s.code().unwrap_or(1) as u8)
        }
        Err(e) => {
            eprintln!("failed to spawn amplihack: {e}");
            ExitCode::from(2)
        }
    }
}
