//! `simard creative-ideas` operator subcommands (issue #2925).
//!
//! `consolidate [--apply]` — the one-time, idempotent SEMANTIC-duplication
//! cleanup of the existing Creative Ideas pool. DRY-RUN by default: it clusters
//! the pool by MEANING via the `creative-ideas-consolidation.yaml` recipe (an
//! agentic reasoner, **not** a Rust heuristic) and reports how many clusters /
//! canonicals / redundant ideas WOULD be merged, writing nothing. `--apply`
//! strengthens each cluster's canonical idea and transitions the redundant ideas
//! to `Rejected` via the fail-closed [`crate::cognitive_memory::creative_idea`]
//! state machine — **no hard deletes**, so every collapsed idea stays auditable.
//!
//! Fail-closed: when the consolidation reasoner is unavailable (no
//! recipe-runner-rs / agent binary / recipe asset) the command errors loudly
//! rather than silently reporting "nothing to consolidate".

use std::error::Error;

use crate::cognitive_memory::creative_idea::ProspectiveCreativeIdeaStore;
use crate::creative_ideas::dedup_gate::{ConsolidationReport, consolidate_existing};
use crate::goal_curation::simard_state_root;
use crate::memory_ipc::launch_writer_client;

pub(super) const CREATIVE_IDEAS_HELP: &str = "\
simard creative-ideas — Creative Ideas maintenance (#2925)

Usage:
  simard creative-ideas consolidate [--apply]

  consolidate   Cluster the existing idea pool by SEMANTIC duplication and merge
                each cluster into one canonical idea (redundant ideas are
                transitioned to Rejected — no hard deletes). DRY-RUN by default
                (reports the plan, writes nothing); pass --apply to write. The
                clustering is done by an agentic reasoner (recipe), not a Rust
                heuristic. Idempotent: re-running after --apply is a no-op.
";

/// Dispatch a `simard creative-ideas <subcommand>` invocation.
pub(super) fn dispatch_creative_ideas_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    let subcommand = args.next().unwrap_or_default();
    match subcommand.as_str() {
        "" | "--help" | "-h" | "help" => {
            print!("{CREATIVE_IDEAS_HELP}");
            Ok(())
        }
        "consolidate" => run_consolidate(args),
        other => Err(format!("unknown creative-ideas subcommand '{other}'; try --help").into()),
    }
}

fn run_consolidate(args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let mut apply = false;
    for arg in args {
        match arg.as_str() {
            "--apply" => apply = true,
            "--help" | "-h" => {
                print!("{CREATIVE_IDEAS_HELP}");
                return Ok(());
            }
            other => {
                return Err(
                    format!("unexpected argument '{other}' to consolidate; try --help").into(),
                );
            }
        }
    }

    let state_root = simard_state_root();
    let writer = launch_writer_client(&state_root)?;
    let store = ProspectiveCreativeIdeaStore::new(writer.ops());

    // Build the consolidation reasoner. Fail-CLOSED: no reasoner ⇒ a hard error,
    // never a silent no-op that would masquerade as "nothing to consolidate".
    let repo_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let brain = crate::ooda_brain::RecipeBrain::new(
        &repo_root,
        "creative-ideas-consolidation.yaml",
        "recipe-idea-consolidation-brain",
    )
    .ok_or_else(|| -> Box<dyn Error> {
        "[simard] creative-ideas consolidate: the consolidation reasoner is unavailable \
         (recipe-runner-rs / agent binary / creative-ideas-consolidation.yaml missing) — \
         refusing to run rather than silently doing nothing (#2925)"
            .into()
    })?;

    let report = consolidate_existing(&store, &brain, apply)?;
    print_report(&report);
    Ok(())
}

fn print_report(report: &ConsolidationReport) {
    let mode = if report.dry_run {
        "DRY-RUN (no writes)"
    } else {
        "APPLIED"
    };
    println!(
        "[simard] creative-ideas consolidate [{mode}]: clusters={} canonical_strengthened={} \
         redundant_rejected={}",
        report.clusters, report.canonical, report.rejected,
    );
    if report.dry_run {
        println!(
            "[simard] re-run with --apply to strengthen canonicals and transition redundant ideas \
             to Rejected (no hard deletes)."
        );
    }
}
