//! Operator subcommand `simard status [--json]` — one consolidated operational
//! report assembled from durable, process-agnostic sources (issue #2528).
//!
//! This renders the single [`crate::status::StatusSnapshot`] the dashboard
//! **Status** tab and the TUI **Status** tab also render. It reads the daemon's
//! on-disk telemetry snapshot, the cost ledger, `self_metrics`, `/proc`,
//! `systemctl show`, and `gh` — **never** by grepping journald — so it returns
//! the same numbers whether or not it runs inside the daemon process.
//!
//! The command never panics: individual degraded sources render
//! `unavailable`/`stale` rather than failing the whole report, and assembly
//! always exits zero on success.

use crate::status::{self, provider::AssembleOptions};

pub(super) const STATUS_HELP: &str = "\
Simard status subcommand — one consolidated operational report

Usage:
  simard status [--json]

Assembles a single StatusSnapshot from durable, process-agnostic sources (the
daemon's telemetry snapshot, the cost ledger, self_metrics, memory IPC,
systemctl + /proc, and gh) and renders the operator-approved layout: DAEMON /
UPTIME, RESOURCE SNAPSHOT, LLM USAGE, MEMORY / BRAIN, GYM, GOAL BOARD, ACTIVE
WORKSTREAMS, COMPLETED WORK, SELF-IMPROVEMENT, and TELEMETRY / UNEXPECTED
SIGNALS. Never grep journald.

  --json   Emit the serialized StatusSnapshot (stdout is pipe-safe JSON). Each
           section carries availability + freshness so scripts can tell a real
           0 from unknown.

With no endpoint configured (the production default) all data is read locally;
the state root resolves via $SIMARD_STATE_ROOT then $HOME/.simard.
";

/// Dispatch `simard status [--json]`.
pub fn dispatch_status_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" | "help" => {
                print!("{STATUS_HELP}");
                return Ok(());
            }
            "--json" => json = true,
            other => return Err(format!("unexpected argument '{other}'").into()),
        }
    }

    let snapshot = status::assemble(&AssembleOptions::default());

    if json {
        // stdout must be pipe-safe JSON only (logs go to stderr via init_tracing).
        println!("{}", status::json::to_string_pretty(&snapshot)?);
    } else {
        print!("{}", status::render::to_terminal(&snapshot));
    }
    Ok(())
}
