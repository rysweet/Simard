//! Helper binary for executing individual OODA-cycle phases via JSON IPC.
//!
//! This is the deterministic-phase counterpart to `simard-ooda-cycle.yaml`
//! recipe steps. Pure-data phases (orient, decide, review, curate) round-trip
//! state through `OodaStateSnapshot` JSON; bridge-dependent phases (observe,
//! act, budget-check) are intentionally deferred — the recipe will drop in
//! real implementations once the corresponding bridge-instantiation surface
//! exists.
//!
//! ## Usage
//!
//! ```text
//! simard-ooda-step observe   --state-json <P> --state-root <DIR>
//! simard-ooda-step orient    --state-json <P> --observation-json <P>
//! simard-ooda-step decide    --priorities-json <P> [--config-json <P>]
//! simard-ooda-step act       --state-json <P> --actions-json <P> --state-root <DIR>
//! simard-ooda-step review    --outcomes-json <P> [--act-elapsed-millis <N>]
//! simard-ooda-step curate    --state-json <P>
//! ```
//!
//! All subcommands emit JSON to stdout on success (exit 0) and a JSON error
//! envelope `{ "error": "<msg>" }` to stderr on failure (exit 2). All flags
//! that take a path expect a path to a file containing JSON.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use simard::ooda_actions;
use simard::ooda_loop::{
    ActionOutcome, Observation, OodaConfig, OodaStateSnapshot, PlannedAction, Priority,
    bridges_from_state_root, decide, observe, orient, promote_from_backlog, review_outcomes,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("observe") => parse_flags(&args[2..]).and_then(cmd_observe),
        Some("orient") => parse_flags(&args[2..]).and_then(cmd_orient),
        Some("decide") => parse_flags(&args[2..]).and_then(cmd_decide),
        Some("act") => parse_flags(&args[2..]).and_then(cmd_act),
        Some("review") => parse_flags(&args[2..]).and_then(cmd_review),
        Some("curate") => parse_flags(&args[2..]).and_then(cmd_curate),
        Some(other) => Err(format!("unknown subcommand: {other}")),
        None => Err(
            "missing subcommand: expected one of observe | orient | decide | act | review | curate"
                .to_string(),
        ),
    };
    match result {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(msg) => {
            let envelope = serde_json::json!({ "error": msg });
            eprintln!("{envelope}");
            ExitCode::from(2)
        }
    }
}

/// Parse `--key value` pairs into a HashMap. Emits an error if a flag is
/// missing its value or if a positional argument is found.
fn parse_flags(args: &[String]) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let key = arg
            .strip_prefix("--")
            .ok_or_else(|| format!("expected --flag, got '{arg}'"))?;
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("flag --{key} missing value"))?;
        map.insert(key.to_string(), value.clone());
        i += 2;
    }
    Ok(map)
}

fn require<'a>(flags: &'a HashMap<String, String>, key: &str) -> Result<&'a String, String> {
    flags
        .get(key)
        .ok_or_else(|| format!("missing required flag --{key}"))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|e| format!("failed to parse JSON from {}: {e}", path.display()))
}

fn cmd_orient(flags: HashMap<String, String>) -> Result<String, String> {
    let state_path = Path::new(require(&flags, "state-json")?);
    let obs_path = Path::new(require(&flags, "observation-json")?);
    let snapshot: OodaStateSnapshot = read_json(state_path)?;
    let observation: Observation = read_json(obs_path)?;
    let priorities = orient(
        &observation,
        &snapshot.active_goals,
        &snapshot.goal_failure_counts,
    )
    .map_err(|e| format!("orient phase failed: {e}"))?;
    serde_json::to_string(&priorities).map_err(|e| format!("serialize priorities: {e}"))
}

fn cmd_decide(flags: HashMap<String, String>) -> Result<String, String> {
    let pri_path = Path::new(require(&flags, "priorities-json")?);
    let priorities: Vec<Priority> = read_json(pri_path)?;
    let config: OodaConfig = match flags.get("config-json") {
        Some(p) => read_json(Path::new(p))?,
        None => OodaConfig::default(),
    };
    let actions: Vec<PlannedAction> =
        decide(&priorities, &config).map_err(|e| format!("decide phase failed: {e}"))?;
    serde_json::to_string(&actions).map_err(|e| format!("serialize actions: {e}"))
}

fn cmd_review(flags: HashMap<String, String>) -> Result<String, String> {
    let outcomes_path = Path::new(require(&flags, "outcomes-json")?);
    let outcomes: Vec<ActionOutcome> = read_json(outcomes_path)?;
    let elapsed_ms: u64 = match flags.get("act-elapsed-millis") {
        Some(s) => s
            .parse()
            .map_err(|e| format!("invalid --act-elapsed-millis '{s}': {e}"))?,
        None => 0,
    };
    let directives = review_outcomes(&outcomes, Duration::from_millis(elapsed_ms));
    serde_json::to_string(&directives).map_err(|e| format!("serialize directives: {e}"))
}

fn cmd_curate(flags: HashMap<String, String>) -> Result<String, String> {
    let state_path = Path::new(require(&flags, "state-json")?);
    let snapshot: OodaStateSnapshot = read_json(state_path)?;
    let mut state = snapshot.into_state();
    let archived = simard::goal_curation::archive_completed(&mut state.active_goals);
    promote_from_backlog(&mut state.active_goals);
    let result = serde_json::json!({
        "archived_goal_ids": archived.iter().map(|g| g.id.clone()).collect::<Vec<_>>(),
        "snapshot": OodaStateSnapshot::from(&state),
    });
    serde_json::to_string(&result).map_err(|e| format!("serialize curate result: {e}"))
}

/// Observe phase: bridge-dependent. Loads or builds an `OodaState` snapshot,
/// connects bridges from `--state-root`, runs `observe`, and returns the
/// `Observation` plus the updated snapshot (since `observe` mutates state —
/// it consumes pending review_improvements into the observation).
fn cmd_observe(flags: HashMap<String, String>) -> Result<String, String> {
    let state_path = Path::new(require(&flags, "state-json")?);
    let state_root = PathBuf::from(require(&flags, "state-root")?);
    let snapshot: OodaStateSnapshot = read_json(state_path)?;
    let mut state = snapshot.into_state();
    let bridges =
        bridges_from_state_root(&state_root).map_err(|e| format!("bridge_factory failed: {e}"))?;
    let observation =
        observe(&mut state, &bridges).map_err(|e| format!("observe phase failed: {e}"))?;
    let result = serde_json::json!({
        "observation": observation,
        "snapshot": OodaStateSnapshot::from(&state),
    });
    serde_json::to_string(&result).map_err(|e| format!("serialize observe result: {e}"))
}

/// Act phase: bridge-dependent. Dispatches the supplied planned actions
/// against live bridges and returns one [`ActionOutcome`] per input action.
/// Also returns the post-act snapshot, since dispatch_actions mutates state
/// (e.g. records engineer worktree handles, updates goal progress).
fn cmd_act(flags: HashMap<String, String>) -> Result<String, String> {
    let state_path = Path::new(require(&flags, "state-json")?);
    let actions_path = Path::new(require(&flags, "actions-json")?);
    let state_root = PathBuf::from(require(&flags, "state-root")?);
    let snapshot: OodaStateSnapshot = read_json(state_path)?;
    let actions: Vec<PlannedAction> = read_json(actions_path)?;
    let mut state = snapshot.into_state();
    let mut bridges =
        bridges_from_state_root(&state_root).map_err(|e| format!("bridge_factory failed: {e}"))?;
    let outcomes = ooda_actions::dispatch_actions(&actions, &mut bridges, &mut state)
        .map_err(|e| format!("act phase failed: {e}"))?;
    let result = serde_json::json!({
        "outcomes": outcomes,
        "snapshot": OodaStateSnapshot::from(&state),
    });
    serde_json::to_string(&result).map_err(|e| format!("serialize act result: {e}"))
}
