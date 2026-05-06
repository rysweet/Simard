//! Helper bin for the recipe-driven engineer loop.
//! Each subcommand corresponds to one phase of the Simard engineer loop.
//!
//! Subcommands:
//!   inspect       --workspace <path> --state-root <path>
//!   agent-spawn   --inspection-json <json> --objective <text> --workspace <path>
//!   review        --inspection-json <json> --action-json <json>
//!   persist       --state-root <path> --topology <kebab> --objective <text>
//!                 --inspection-json <json> --action-json <json>
//!                 --verification-json <json>
//!                 [--terminal-bridge-json <json>]
//!
//! On success: prints the phase output as JSON to stdout, exit 0.
//! On error: writes the error to stderr, exits 2.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;

use simard::engineer_loop::{
    ExecutedEngineerAction, RepoInspection, VerificationReport, inspect_workspace,
    persist_engineer_loop_artifacts, run_optional_review, spawn_agent_for_goal,
};
use simard::runtime::RuntimeTopology;
use simard::terminal_engineer_bridge::TerminalBridgeContext;

fn die(msg: impl AsRef<str>) -> ExitCode {
    eprintln!("simard-engineer-step: {}", msg.as_ref());
    ExitCode::from(2)
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn require(args: &[String], flag: &str) -> Result<String, String> {
    arg(args, flag).ok_or_else(|| format!("missing required flag {flag}"))
}

fn print_json<T: Serialize>(v: &T) -> Result<(), String> {
    let s = serde_json::to_string(v).map_err(|e| format!("serialize: {e}"))?;
    println!("{s}");
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().collect();
    let subcommand = match argv.get(1) {
        Some(s) => s.clone(),
        None => {
            return die(
                "usage: simard-engineer-step <inspect|select|execute|verify|review|persist> [flags...]",
            );
        }
    };
    let args = &argv[2..].to_vec();

    let result = match subcommand.as_str() {
        "inspect" => cmd_inspect(args),
        "agent-spawn" => cmd_agent_spawn(args),
        "review" => cmd_review(args),
        "persist" => cmd_persist(args),
        other => return die(format!("unknown subcommand '{other}'")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => die(e),
    }
}

fn cmd_inspect(args: &[String]) -> Result<(), String> {
    let workspace = PathBuf::from(require(args, "--workspace")?);
    let state_root = PathBuf::from(require(args, "--state-root")?);
    let inspection = inspect_workspace(&workspace, &state_root)
        .map_err(|e| format!("inspect_workspace failed: {e}"))?;
    print_json(&inspection)
}

fn cmd_agent_spawn(args: &[String]) -> Result<(), String> {
    let inspection_json = require(args, "--inspection-json")?;
    let objective = require(args, "--objective")?;
    let workspace = PathBuf::from(require(args, "--workspace")?);
    let inspection: RepoInspection = serde_json::from_str(&inspection_json)
        .map_err(|e| format!("parse inspection-json: {e}"))?;
    let outcome_summary = spawn_agent_for_goal(&objective, &inspection, &workspace)
        .map_err(|e| format!("spawn_agent_for_goal failed: {e}"))?;
    let result = serde_json::json!({
        "status": "completed",
        "outcome_summary": outcome_summary
    });
    println!("{result}");
    Ok(())
}

fn cmd_review(args: &[String]) -> Result<(), String> {
    let inspection_json = require(args, "--inspection-json")?;
    let action_json = require(args, "--action-json")?;
    let inspection: RepoInspection = serde_json::from_str(&inspection_json)
        .map_err(|e| format!("parse inspection-json: {e}"))?;
    let action: ExecutedEngineerAction =
        serde_json::from_str(&action_json).map_err(|e| format!("parse action-json: {e}"))?;
    run_optional_review(&inspection, &action)
        .map_err(|e| format!("run_optional_review failed: {e}"))?;
    println!("{{\"status\":\"ok\"}}");
    Ok(())
}

fn cmd_persist(args: &[String]) -> Result<(), String> {
    let state_root = PathBuf::from(require(args, "--state-root")?);
    let topology_str = require(args, "--topology")?;
    let objective = require(args, "--objective")?;
    let inspection_json = require(args, "--inspection-json")?;
    let action_json = require(args, "--action-json")?;
    let verification_json = require(args, "--verification-json")?;

    let topology: RuntimeTopology = serde_json::from_str(&format!("\"{topology_str}\""))
        .map_err(|e| format!("parse topology '{topology_str}': {e}"))?;
    let inspection: RepoInspection = serde_json::from_str(&inspection_json)
        .map_err(|e| format!("parse inspection-json: {e}"))?;
    let action: ExecutedEngineerAction =
        serde_json::from_str(&action_json).map_err(|e| format!("parse action-json: {e}"))?;
    let verification: VerificationReport = serde_json::from_str(&verification_json)
        .map_err(|e| format!("parse verification-json: {e}"))?;
    let bridge_context: Option<TerminalBridgeContext> = match arg(args, "--terminal-bridge-json") {
        Some(s) if !s.is_empty() && s != "null" => {
            Some(serde_json::from_str(&s).map_err(|e| format!("parse terminal-bridge-json: {e}"))?)
        }
        _ => None,
    };

    persist_engineer_loop_artifacts(
        &state_root,
        topology,
        &objective,
        &inspection,
        &action,
        &verification,
        bridge_context.as_ref(),
    )
    .map_err(|e| format!("persist_engineer_loop_artifacts failed: {e}"))?;
    println!("{{\"status\":\"persisted\"}}");
    Ok(())
}
