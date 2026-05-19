//! One-shot restore helper: insert a missing production active goal into
//! the live goal board via the merge-on-write API. Intended for occasional
//! operator use when a known-good goal was deleted by a regression.
//!
//! Usage:
//!   cargo run --release --example restore_active_goal -- \
//!     <id> <priority> <description>
//!
//! Example:
//!   cargo run --release --example restore_active_goal -- \
//!     improve-simard-dashboard 2 \
//!     "Improve Simard dashboard — surface merge-judge and per-PR readiness (#1880, #1893, #1894)"

use std::process::ExitCode;

use simard::goal_curation::{
    ActiveGoal, GoalProgress, load_goal_board, save_goal_board_with_removals,
};
use simard::memory_ipc::launch_writer_bridge;
use simard::state_root::simard_state_root;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "usage: {} <id> <priority> <description> [--remove <id>]...",
            args.first()
                .map(String::as_str)
                .unwrap_or("restore_active_goal")
        );
        return ExitCode::from(2);
    }
    let id = args[1].clone();
    let priority: u32 = match args[2].parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("priority must be a non-negative integer");
            return ExitCode::from(2);
        }
    };
    let description = args[3].clone();

    let mut remove_ids: Vec<String> = Vec::new();
    let mut i = 4;
    while i < args.len() {
        if args[i] == "--remove" && i + 1 < args.len() {
            remove_ids.push(args[i + 1].clone());
            i += 2;
        } else {
            eprintln!("unknown argument: {}", args[i]);
            return ExitCode::from(2);
        }
    }

    let state_root = simard_state_root();
    let bridge = match launch_writer_bridge(&state_root) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to open cognitive memory writer bridge: {e}");
            return ExitCode::from(1);
        }
    };

    let mut board = match load_goal_board(bridge.ops()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to load goal board: {e}");
            return ExitCode::from(1);
        }
    };

    if board.active.iter().any(|g| g.id == id) {
        eprintln!("goal already present on active board: {id}; no-op-add");
    } else {
        board.active.push(ActiveGoal {
            id: id.clone(),
            description,
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: Vec::new(),
        });
        println!("queued add of active goal: {id}");
    }

    // Apply removals at the same time so the save_goal_board_with_removals
    // post-merge filter prevents stale entries (e.g. placeholder
    // `Goal <id>` fixtures) from resurrecting via merge-on-write.
    if let Err(e) = save_goal_board_with_removals(&board, &remove_ids, bridge.ops()) {
        eprintln!("failed to save goal board: {e}");
        return ExitCode::from(1);
    }
    println!(
        "saved goal board: add={id}; removed={}",
        if remove_ids.is_empty() {
            "<none>".to_string()
        } else {
            remove_ids.join(",")
        }
    );
    ExitCode::SUCCESS
}
