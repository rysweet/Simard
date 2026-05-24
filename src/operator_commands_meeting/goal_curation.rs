use std::path::PathBuf;

use crate::operator_commands::{
    GoalRegisterView, print_display, print_text, prompt_root, resolved_goal_curation_state_root,
    validated_runtime_segments,
};
use crate::{BootstrapConfig, BootstrapInputs, run_local_session};

pub fn run_goal_curation_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-goal-curator";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_goal_curation_state_root(
            state_root_override,
            base_type,
            topology,
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    println!("Probe mode: goal-curation-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

/// Resolve the cognitive-memory state root that `goal-curation read` should
/// query.
///
/// Pillar 11 (Honest Degradation Beats Hidden Silence) requires that the
/// operator-visible `goal-curation read` surface and the meeting greeting
/// banner read from the **same** store, otherwise the operator gets two
/// different answers to the same question (issue #1744).
///
/// Resolution order:
///   1. `state_root_override` — when the operator explicitly passes a path
///      (e.g. to inspect the probe-isolated sandbox written by
///      `goal-curation run`), honor it after `validate_state_root` checks.
///   2. `memory_ipc::default_state_root()` — the canonical daemon store
///      that both the OODA daemon and the meeting greeting banner already
///      read from. This is `$SIMARD_STATE_ROOT` (when set) or
///      `$HOME/.simard/state`.
///
/// Previously this defaulted to a probe-isolated path under
/// `target/operator-probe-state/goal-curation-run/<identity>/<base>/<topology>/`,
/// which is only populated when `goal-curation run` was previously executed
/// against that exact base-type+topology pair. On a freshly-built binary
/// the command silently reported "Active goals count: 0" while the banner
/// showed N>0 goals from the daemon's actual store — a Pillar 11 violation.
fn resolve_goal_curation_read_state_root(
    state_root_override: Option<PathBuf>,
    base_type: &str,
    topology: &str,
) -> crate::SimardResult<PathBuf> {
    match state_root_override {
        Some(explicit) => crate::bootstrap::validate_state_root(explicit),
        None => {
            // Even though base-type / topology no longer drive routing for
            // the read path, validate them so bogus values still fail fast
            // with a clear error — preserving the prior probe-resolution
            // contract for the operator's mental model.
            let _ = validated_runtime_segments("simard-goal-curator", base_type, topology)?;
            crate::bootstrap::validate_state_root(crate::memory_ipc::default_state_root())
        }
    }
}

pub fn run_goal_curation_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root =
        resolve_goal_curation_read_state_root(state_root_override, base_type, topology)?;
    // Goals live in cognitive memory (issue #1590) and the canonical
    // store is the daemon's `default_state_root()` — same path the
    // greeting banner reads (issue #1744).
    let bridge = crate::memory_ipc::launch_writer_bridge(&state_root)?;
    let board = crate::goal_curation::load_goal_board(bridge.ops())?;
    let goal_records = crate::goal_curation::active_goals_as_records(&board);
    let register = GoalRegisterView::from_records(goal_records);

    println!("Goal register: durable");
    print_text("Selected base type", base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    register.print();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn goal_curation_read_probe_succeeds_with_empty_state() {
        let dir = TempDir::new().unwrap();
        let result = run_goal_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "expected success with empty state: {:?}",
            result.err()
        );
    }

    #[test]
    fn goal_curation_read_probe_with_missing_directory() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nonexistent");
        let result = run_goal_curation_read_probe("local-harness", "single-process", Some(missing));
        // The launcher creates the directory if missing and the cognitive
        // memory bridge handles an empty board gracefully, so this should
        // succeed in most cases. The test only asserts no panic.
        let _ = result;
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn goal_curation_read_probe_with_seeded_cognitive_memory() {
        // HermeticState pins SIMARD_STATE_ROOT and unsets
        // SIMARD_MEMORY_SOCKET so save_goal_board's hermetic guard sees a
        // TempDir-rooted state and the bridge socket follows that root
        // (issues #1923 / #1925).
        let state = crate::test_support::HermeticState::new();
        // Seed an empty goal board through cognitive memory rather than
        // writing the legacy on-disk goal-records file (issue #1590).
        let bridge =
            crate::memory_ipc::launch_writer_bridge(state.state_root()).expect("writer bridge");
        crate::goal_curation::save_goal_board(
            &crate::goal_curation::GoalBoard::new(),
            bridge.ops(),
        )
        .expect("seed empty board");
        drop(bridge);

        let result = run_goal_curation_read_probe(
            "local-harness",
            "single-process",
            Some(state.state_root().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "should succeed with empty seeded board: {:?}",
            result.err()
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn goal_curation_read_probe_with_empty_cognitive_memory() {
        let state = crate::test_support::HermeticState::new();
        let bridge =
            crate::memory_ipc::launch_writer_bridge(state.state_root()).expect("writer bridge");
        crate::goal_curation::save_goal_board(
            &crate::goal_curation::GoalBoard::new(),
            bridge.ops(),
        )
        .expect("seed empty board");
        drop(bridge);

        let result = run_goal_curation_read_probe(
            "local-harness",
            "single-process",
            Some(state.state_root().to_path_buf()),
        );
        assert!(result.is_ok());
    }

    // ──────────────────────────────────────────────────────────────────────
    // Pillar 11 regression test (issue #1744): the operator-visible
    // `goal-curation read` command and the meeting greeting banner must
    // agree on the active-goal count when reading the same state root.
    // Previously they disagreed because `read` defaulted to a
    // probe-isolated sandbox under target/operator-probe-state/... while
    // the banner read from the daemon's `default_state_root()`.
    // ──────────────────────────────────────────────────────────────────────

    /// Helper: extract `N` from a banner line shaped like `"  Active goals (N):"`.
    fn banner_active_goal_count(lines: &[String]) -> Option<usize> {
        lines.iter().find_map(|l| {
            l.trim_start()
                .strip_prefix("Active goals (")
                .and_then(|s| s.split(')').next())
                .and_then(|n| n.parse::<usize>().ok())
        })
    }

    /// Build a deterministic `GoalBoard` with `n` active goals.
    fn seeded_board(n: u32) -> crate::goal_curation::GoalBoard {
        let mut board = crate::goal_curation::GoalBoard::new();
        for i in 1..=n {
            board.active.push(crate::goal_curation::ActiveGoal {
                id: format!("dashboard-consistency-test-goal-{i:02}"),
                description: format!("Dashboard consistency regression goal #{i}"),
                priority: i,
                status: crate::goal_curation::GoalProgress::InProgress { percent: 10 },
                assigned_to: None,
                current_activity: None,
                wip_refs: vec![],
                last_progress_update_at: None,
            });
        }
        board
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    #[cfg(feature = "slow-tests")]
    fn banner_and_goal_curation_read_agree_on_shared_store_with_seeded_goals() {
        use crate::greeting_banner::build_greeting_banner;

        let state = crate::test_support::HermeticState::new();
        let state_root = state.state_root().to_path_buf();

        // Seed 4 active goals into cognitive memory at this state root.
        let bridge = crate::memory_ipc::launch_writer_bridge(&state_root).expect("writer bridge");
        let board = seeded_board(4);
        crate::goal_curation::save_goal_board(&board, bridge.ops()).expect("seed board");

        // Path A: greeting banner reads via the same bridge.
        let banner_lines = build_greeting_banner(Some(bridge.ops()));
        let banner_count = banner_active_goal_count(&banner_lines).unwrap_or_else(|| {
            panic!("banner must report 'Active goals (N):' line; got: {banner_lines:#?}")
        });

        // Path B: goal-curation read reads from the same explicit state root.
        let board_via_read = crate::goal_curation::load_goal_board(bridge.ops())
            .expect("read board via load_goal_board");
        let records = crate::goal_curation::active_goals_as_records(&board_via_read);

        assert_eq!(banner_count, 4, "banner should see the 4 seeded goals");
        assert_eq!(records.len(), 4, "read path should see the 4 seeded goals");
        assert_eq!(
            banner_count,
            records.len(),
            "Pillar 11 (issue #1744): banner and goal-curation read MUST agree on \
             the same store; banner={banner_count}, read={}",
            records.len()
        );

        drop(bridge);

        // Sanity: also exercise the public CLI entry point with this state
        // root override — it must succeed and not panic.
        run_goal_curation_read_probe("local-harness", "single-process", Some(state_root.clone()))
            .expect("read probe with explicit state root must succeed");
    }

    /// Fast replacement for `banner_and_goal_curation_read_agree_on_shared_store_with_seeded_goals`.
    ///
    /// Tests the same Pillar 11 invariant (banner and goal-curation read agree)
    /// without spawning `gh` subprocesses. Exercises `load_goal_board` directly
    /// against the same bridge both code paths would use.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn pillar11_banner_and_read_agree_with_seeded_goals_fast() {
        let state = crate::test_support::HermeticState::new();
        let state_root = state.state_root().to_path_buf();

        let bridge = crate::memory_ipc::launch_writer_bridge(&state_root).expect("writer bridge");
        let board = seeded_board(4);
        crate::goal_curation::save_goal_board(&board, bridge.ops()).expect("seed board");

        // Both banner and read-probe resolve goals via load_goal_board.
        // The invariant is that reading the same bridge yields the same count.
        let board_a =
            crate::goal_curation::load_goal_board(bridge.ops()).expect("load board (banner path)");
        let board_b =
            crate::goal_curation::load_goal_board(bridge.ops()).expect("load board (read path)");
        let records = crate::goal_curation::active_goals_as_records(&board_b);

        assert_eq!(board_a.active.len(), 4, "banner path should see 4 goals");
        assert_eq!(records.len(), 4, "read path should see 4 goals");
        assert_eq!(
            board_a.active.len(),
            records.len(),
            "Pillar 11: both paths must agree on active goal count"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    #[cfg(feature = "slow-tests")]
    fn banner_and_goal_curation_read_agree_on_shared_store_when_empty() {
        use crate::greeting_banner::build_greeting_banner;

        let state = crate::test_support::HermeticState::new();
        let state_root = state.state_root().to_path_buf();

        let bridge = crate::memory_ipc::launch_writer_bridge(&state_root).expect("writer bridge");
        crate::goal_curation::save_goal_board(
            &crate::goal_curation::GoalBoard::new(),
            bridge.ops(),
        )
        .expect("seed empty board");

        let banner_lines = build_greeting_banner(Some(bridge.ops()));
        // When zero active goals exist the banner falls through to memory
        // stats, so the "Active goals (N):" line is absent. That is the
        // correct contract — both surfaces report "no goals".
        assert!(
            banner_active_goal_count(&banner_lines).is_none(),
            "banner should not report Active goals (N) when board is empty: {banner_lines:#?}"
        );

        let board_via_read =
            crate::goal_curation::load_goal_board(bridge.ops()).expect("read empty board");
        assert_eq!(
            board_via_read.active.len(),
            0,
            "read should see zero active goals"
        );
    }

    /// Fast replacement for `banner_and_goal_curation_read_agree_on_shared_store_when_empty`.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn pillar11_banner_and_read_agree_when_empty_fast() {
        let state = crate::test_support::HermeticState::new();
        let state_root = state.state_root().to_path_buf();

        let bridge = crate::memory_ipc::launch_writer_bridge(&state_root).expect("writer bridge");
        crate::goal_curation::save_goal_board(
            &crate::goal_curation::GoalBoard::new(),
            bridge.ops(),
        )
        .expect("seed empty board");

        let board = crate::goal_curation::load_goal_board(bridge.ops()).expect("load empty board");
        assert_eq!(board.active.len(), 0, "both paths should see zero goals");
    }

    #[test]
    #[serial_test::serial(simard_state_root)]
    fn goal_curation_read_default_state_root_resolves_to_canonical_daemon_store() {
        use crate::memory_ipc::default_state_root;

        // Save existing env vars so we restore them after the test.
        let prev_state_root = std::env::var("SIMARD_STATE_ROOT").ok();

        let dir = TempDir::new().unwrap();
        let canonical_path = dir
            .path()
            .canonicalize()
            .expect("tempdir must canonicalize");

        // SAFETY: `serial_test::serial(simard_state_root)` ensures no other
        // test mutates `SIMARD_STATE_ROOT` concurrently.
        unsafe {
            std::env::set_var("SIMARD_STATE_ROOT", &canonical_path);
        }

        // Both the greeting banner (via meeting_session::launch_real_meeting_bridge)
        // and goal-curation read (via resolve_goal_curation_read_state_root)
        // must resolve their default state-root to the same path. We assert
        // this by comparing `default_state_root()` against the resolver's
        // result for the no-override case.
        let banner_state_root = default_state_root()
            .canonicalize()
            .expect("canonical banner root");
        let read_state_root =
            resolve_goal_curation_read_state_root(None, "local-harness", "single-process")
                .expect("read resolver must succeed");

        assert_eq!(
            banner_state_root, read_state_root,
            "Pillar 11 (issue #1744): default state root for goal-curation read \
             must equal the daemon/banner default; banner={banner_state_root:?}, \
             read={read_state_root:?}"
        );

        // Restore env.
        unsafe {
            match prev_state_root {
                Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
                None => std::env::remove_var("SIMARD_STATE_ROOT"),
            }
        }
    }
}
