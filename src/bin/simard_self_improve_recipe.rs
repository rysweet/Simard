//! `simard-self-improve-recipe` — thin driver that runs the
//! self-improvement cycle as a recipe-runner workflow.
//!
//! This is the architectural pivot in action: instead of `main()` calling
//! `run_improvement_cycle()` (Rust orchestration), it shells out to
//! `amplihack recipe run simard-self-improve-cycle.yaml` and parses the
//! final ImprovementCycle JSON from the recipe output.
//!
//! The legacy `run_improvement_cycle()` path remains in `simard::self_improve`
//! and is exercised by the existing tests; both paths coexist behind a flag
//! while parity is being proven.

use std::process::Command;

use simard::self_improve::ImprovementCycle;

fn die(msg: &str) -> ! {
    eprintln!("simard-self-improve-recipe: {msg}");
    std::process::exit(2);
}

fn arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == flag {
            return iter.next().map(String::as_str);
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let workspace = arg(&args, "--workspace").unwrap_or(".");
    let suite_id = arg(&args, "--suite-id").unwrap_or("default");
    let proposal = arg(&args, "--proposal").unwrap_or("");
    let weak_threshold = arg(&args, "--weak-threshold").unwrap_or("0.7");
    let target_dim = arg(&args, "--target-dimension").unwrap_or("");
    let recipe = arg(&args, "--recipe").unwrap_or(
        "amplifier-bundle/recipes/simard-self-improve-cycle.yaml",
    );
    let amplihack_home = arg(&args, "--amplihack-home").unwrap_or("/home/azureuser/src/amplihack-rs");

    let recipe_path = if recipe.starts_with('/') {
        recipe.to_string()
    } else {
        format!("{amplihack_home}/{recipe}")
    };

    let mut cmd = Command::new("amplihack");
    cmd.env("AMPLIHACK_HOME", amplihack_home)
        .args(["recipe", "run", &recipe_path])
        .args(["-c", &format!("workspace_path={workspace}")])
        .args(["-c", &format!("suite_id={suite_id}")])
        .args(["-c", &format!("proposal={proposal}")])
        .args(["-c", &format!("weak_threshold={weak_threshold}")])
        .args(["-c", &format!("target_dimension={target_dim}")])
        .args(["-f", "json"]);

    let output = cmd.output().unwrap_or_else(|e| die(&format!("spawn amplihack: {e}")));
    if !output.status.success() {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &output.stderr);
        die(&format!("recipe failed with status {}", output.status));
    }

    // Recipe outputs the final ImprovementCycle JSON from emit-cycle step.
    // For now we just stream stdout through; downstream callers can parse it.
    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");

    // Best-effort validation: try to find a cycle JSON line in the output and
    // round-trip it to confirm shape. Non-fatal — recipe runner output format
    // may evolve.
    for line in stdout.lines() {
        if line.starts_with('{') {
            if let Ok(_cycle) = serde_json::from_str::<ImprovementCycle>(line) {
                return;
            }
        }
    }
}
