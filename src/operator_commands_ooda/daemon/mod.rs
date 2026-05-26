use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::bridge_launcher::{find_python_dir, launch_gym_bridge, launch_knowledge_bridge};
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::goal_curation::{load_goal_board, persist_board};
use crate::identity::OperatingMode;
use crate::memory_ipc;
use crate::ooda_loop::{
    OodaBridges, OodaConfig, OodaPhase, OodaState, run_ooda_cycle, summarize_cycle_report,
};
use crate::runtime_config::RuntimeConfig;
use crate::session_builder::{LlmProvider, SessionBuilder};

use crate::operator_commands_ooda::persistence::{persist_cycle_report, persist_cycle_to_memory};

mod helpers;
pub use helpers::*;

mod brains;

mod config;
pub use config::DaemonDashboardConfig;

/// Run one or more OODA cycles as a daemon-style loop.
///
/// Launches all bridges, opens a RustyClawd session via [`SessionBuilder`]
/// for real autonomous work, loads the goal board from cognitive memory,
/// and runs OODA cycles until `max_cycles` is reached (0 = infinite).
///
/// When `dashboard.enabled` is true, the dashboard's axum server is spawned
/// as a background tokio task — sharing the same process and restarting
/// automatically when the daemon restarts (via auto-reload or systemd).
///
/// On SIGTERM/SIGINT the current cycle finishes, the session is closed
/// cleanly, and the daemon exits without orphaning PTY subprocesses.
///
/// If no LLM adapter is available (e.g. no API key, no Copilot SDK),
/// the daemon exits with an error — no silent degradation to bridge-only mode.
pub fn run_ooda_daemon(
    max_cycles: u32,
    state_root_override: Option<PathBuf>,
    auto_reload: bool,
    dashboard: DaemonDashboardConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // --- signal handling ------------------------------------------------
    //
    // ctrlc with `termination` feature catches SIGINT, SIGTERM and SIGHUP.
    // Without `termination`, only SIGINT was caught; systemd sends SIGTERM
    // by default, which silently bypassed our cleanup and stranded writes
    // in the WAL (issue #1631).
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&shutdown);
        ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        })
        .expect("failed to install SIGTERM/SIGINT/SIGHUP handler");
    }
    // --------------------------------------------------------------------

    // Auto-ensure runtime dependencies before launching bridges
    if let Err(e) = crate::cmd_ensure_deps::handle_ensure_deps() {
        eprintln!("Warning: some dependencies could not be verified: {e}");
    }

    let state_root = state_root_override.unwrap_or_else(memory_ipc::default_state_root);

    std::fs::create_dir_all(&state_root)?;

    let python_dir = find_python_dir()?;

    // Reap any stale lock file from a prior crashed daemon before we open.
    if let Err(e) = memory_ipc::reap_stale_open_lock(&state_root) {
        eprintln!("[simard] OODA daemon: stale-lock reap failed: {e}");
    }

    let shared_mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(NativeCognitiveMemory::open(&state_root)?);

    // Register the live writer for in-process callers (dashboard, OODA
    // loop, reflection, etc.) so they bypass IPC and disk re-open and
    // share this exact handle. This eliminates the dashboard's
    // hollow-success failure mode where launch_writer_bridge previously
    // fell through to a read-only handle when both IPC and direct open
    // failed (issue #1590 follow-up).
    memory_ipc::register_in_process_writer(state_root.clone(), Arc::clone(&shared_mem));

    // Spawn the memory IPC server so meetings and other clients can share
    // this live DB handle without their own locks conflicting. The socket
    // lives next to the DB it fronts (`socket_path_for(state_root)`), so
    // a TempDir-rooted client can never accidentally connect to this
    // daemon (closes
    // [#1923](https://github.com/rysweet/Simard/issues/1923) /
    // [#1925](https://github.com/rysweet/Simard/issues/1925)).
    let socket_path = memory_ipc::socket_path_for(&state_root);
    let _memory_ipc_server = match memory_ipc::spawn_server(socket_path.clone(), shared_mem.clone())
    {
        Ok(h) => {
            daemon_log(
                &state_root,
                &format!(
                    "[simard] OODA daemon: memory IPC listening at {}",
                    socket_path.display()
                ),
            );
            Some(h)
        }
        Err(e) => {
            daemon_log(
                &state_root,
                &format!(
                    "[simard] OODA daemon: memory IPC server failed to start: {e} \
                     (meetings will fall back to direct open)"
                ),
            );
            None
        }
    };

    let memory: Box<dyn CognitiveMemoryOps> =
        Box::new(memory_ipc::SharedMemory(Arc::clone(&shared_mem)));
    let knowledge = launch_knowledge_bridge(&python_dir)?;
    let gym = launch_gym_bridge(&python_dir)?;

    // One-time bootstrap: snapshot SIMARD_LLM_PROVIDER (if set in env)
    // to <state_root>/config.toml so child processes (engineer subprocesses
    // spawned via tmux, meeting REPLs, etc.) read the same configuration
    // without env-var propagation through every wrapper.
    match RuntimeConfig::bootstrap_from_env(&state_root) {
        Ok(true) => daemon_log(
            &state_root,
            "[simard] OODA daemon: wrote ~/.simard/config.toml from environment",
        ),
        Ok(false) => {}
        Err(e) => daemon_log(
            &state_root,
            &format!("[simard] OODA daemon: config bootstrap failed: {e}"),
        ),
    }

    // Open an LLM session for autonomous work. Required — no silent degradation.
    let provider = LlmProvider::resolve()
        .map_err(|e| format!("OODA daemon: LLM provider not configured: {e}"))?;
    let session = SessionBuilder::new(OperatingMode::Orchestrator, provider)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("ooda")
        .open()
        .map_err(|e| format!("OODA daemon requires LLM session but open() failed: {e}"))?;
    daemon_log(
        &state_root,
        "[simard] OODA daemon: LLM session opened for autonomous work",
    );

    // Compute repo_root early — needed by both brain construction and
    // progress-evidence checker.
    let repo_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let brain = brains::build_act_brain(&state_root, &repo_root);
    let decide_brain = brains::build_decide_brain(&state_root, &repo_root);
    let orient_brain = brains::build_orient_brain(&state_root, &repo_root);

    // After all three brains are constructed, surface the cumulative
    // fallback count in the dashboard. Nonzero == daemon is running in
    // degraded mode (see issues #1711, #1748). Future health endpoints
    // should refuse "healthy" when this is nonzero.
    let degraded = brains::fallback_brain_count();
    if degraded > 0 {
        daemon_log(
            &state_root,
            &format!(
                "[simard] OODA daemon: DEGRADED MODE — {degraded}/3 brains fell back to deterministic (see issues #1711, #1748)"
            ),
        );
    } else {
        daemon_log(
            &state_root,
            "[simard] OODA daemon: all 3 brains LLM-backed (no fallback in use)",
        );
    }

    // Surface where the daemon will look for hot-reloadable prompt assets so
    // operators know where to edit (see `docs/concepts/prompt-driven-brain-iteration.md`).
    {
        let store = crate::ooda_brain::prompt_store::global();
        let dir_str = store
            .resolved_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<embedded only>".to_string());
        daemon_log(
            &state_root,
            &format!(
                "[simard] OODA daemon: prompt_assets dir = {dir_str} (3 prompts hot-reloadable)"
            ),
        );
    }

    // Wire the progress-evidence checker (issue #1967; replaced 2026-05-22
    // per user direction to use an LLM reviewer instead of the original
    // git-shelling state-machine gate). Updated for issue #1971 to prefer
    // recipe-runner-rs backed checker when available.
    //
    // Resolution order:
    //   1. Recipe-runner-rs (if binary + recipe YAML available)
    //   2. Direct LLM (LlmReviewerProgressChecker)
    //   3. NoopProgressEvidenceChecker (fallback)
    //
    // Honors `SIMARD_PROGRESS_EVIDENCE=off` as a kill switch.
    let kill_switch = std::env::var("SIMARD_PROGRESS_EVIDENCE")
        .ok()
        .map(|v| v.eq_ignore_ascii_case("off"))
        .unwrap_or(false);
    let progress_evidence: std::sync::Arc<
        dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker,
    > = if kill_switch {
        daemon_log(
            &state_root,
            "[simard] progress-evidence: DISABLED (NoopProgressEvidenceChecker -- SIMARD_PROGRESS_EVIDENCE=off)",
        );
        std::sync::Arc::new(crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker)
    } else if let Some(recipe_checker) =
        crate::goal_curation::recipe_progress_checker::RecipeProgressChecker::new(&repo_root)
    {
        daemon_log(
            &state_root,
            "[simard] progress-evidence: enabled (RecipeProgressChecker -- recipe-runner-rs backed)",
        );
        std::sync::Arc::new(recipe_checker)
    } else {
        match LlmProvider::resolve() {
            Ok(reviewer_provider) => {
                daemon_log(
                    &state_root,
                    "[simard] progress-evidence: enabled (LlmReviewerProgressChecker -- direct LLM fallback)",
                );
                let reviewer_submitter =
                    crate::ooda_brain::SessionLlmSubmitter::new(reviewer_provider);
                std::sync::Arc::new(
                    crate::goal_curation::progress_reviewer::LlmReviewerProgressChecker::new(
                        reviewer_submitter,
                    ),
                )
            }
            Err(e) => {
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] progress-evidence: NO LLM PROVIDER ({e}); falling back to NoopProgressEvidenceChecker (no gating)"
                    ),
                );
                std::sync::Arc::new(
                    crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker,
                )
            }
        }
    };

    let mut bridges = OodaBridges {
        memory,
        knowledge,
        gym,
        session: Some(session),
        brain,
        decide_brain,
        orient_brain,
        repo_root,
        progress_evidence,
    };

    let board = load_goal_board(&*bridges.memory).unwrap_or_default();
    let mut state = OodaState::new(board);
    let config = OodaConfig::default();

    // Issue #1197: sweep orphaned engineer worktrees from prior crashed
    // daemons before starting the loop, so disk pressure doesn't accumulate.
    if let Ok(parent_repo) = std::env::current_dir() {
        match crate::engineer_worktree::sweep_orphaned_worktrees(&parent_repo, &state_root) {
            Ok(report) => {
                if !report.removed_orphan_dirs.is_empty() {
                    daemon_log(
                        &state_root,
                        &format!(
                            "[simard] OODA daemon: swept {} orphan engineer worktree(s)",
                            report.removed_orphan_dirs.len()
                        ),
                    );
                }
            }
            Err(e) => daemon_log(
                &state_root,
                &format!("[simard] OODA daemon: engineer worktree sweep failed: {e}"),
            ),
        }
    }

    let interval_secs: u64 = std::env::var("SIMARD_OODA_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    daemon_log(
        &state_root,
        &format!("[simard] OODA daemon: cycle interval = {interval_secs}s"),
    );

    // --- embedded dashboard ------------------------------------------------
    // Spawn the dashboard as a background tokio task so both OODA loop and
    // dashboard share a single process. On daemon restart (auto-reload or
    // systemd), the dashboard restarts automatically.
    let _dashboard_rt;
    let _dashboard_handle;
    if dashboard.enabled {
        let (code, loaded) = crate::operator_commands_dashboard::init_auth();
        eprintln!("\n  🌲 Simard Dashboard (embedded in OODA daemon)");
        if loaded {
            eprintln!("  Login code: {code} (loaded from ~/.simard/.dashkey)");
        } else {
            eprintln!("  Login code: {code} (saved to ~/.simard/.dashkey)");
        }
        eprintln!(
            "  Open http://localhost:{} and enter the code\n",
            dashboard.port
        );

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let handle =
            crate::operator_commands_dashboard::spawn_dashboard_task(rt.handle(), dashboard.port);
        _dashboard_rt = Some(rt);
        _dashboard_handle = Some(handle);
    } else {
        _dashboard_rt = None;
        _dashboard_handle = None;
        daemon_log(
            &state_root,
            "[simard] OODA daemon: dashboard disabled (use --no-dashboard to suppress)",
        );
    }
    // -----------------------------------------------------------------------

    // Capture the binary mtime at startup so we can detect in-place upgrades.
    let start_time = exe_mtime().unwrap_or_else(SystemTime::now);

    if auto_reload {
        daemon_log(&state_root, "[simard] OODA daemon: auto-reload enabled");
    }

    // --- periodic DB backup state -----------------------------------------
    // Defaults: 5-minute interval and 24-backup retention give 2 hours of
    // history, which has been sufficient to recover from every observed
    // WAL-corruption incident. Override with the env vars below.
    let db_backup_interval_secs: u64 = std::env::var("SIMARD_DB_BACKUP_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let db_backup_keep: usize = std::env::var("SIMARD_DB_BACKUP_KEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);
    let mut last_db_backup = Instant::now()
        .checked_sub(Duration::from_secs(db_backup_interval_secs))
        .unwrap_or_else(Instant::now);
    let backup_consecutive_failures = AtomicU32::new(0);
    let checkpoint_consecutive_failures = AtomicU32::new(0);
    daemon_log(
        &state_root,
        &format!(
            "[simard] OODA daemon: DB backup interval = {db_backup_interval_secs}s, keep = {db_backup_keep}"
        ),
    );

    // --- periodic disk health check state ---------------------------------
    let disk_health_interval_secs: u64 = std::env::var("SIMARD_DISK_HEALTH_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let mut last_disk_health = Instant::now()
        .checked_sub(Duration::from_secs(disk_health_interval_secs))
        .unwrap_or_else(Instant::now);
    daemon_log(
        &state_root,
        &format!("[simard] OODA daemon: disk health interval = {disk_health_interval_secs}s"),
    );
    // -------------------------------------------------------------------

    let mut cycles_run = 0u32;

    loop {
        // Reap any zombie engineer subprocesses from the previous cycle's
        // spawns before doing anything else. Non-blocking; logs only when
        // a positive count was reaped to keep steady-state logs clean.
        let reaped = crate::agent_supervisor::reap_zombies();
        if reaped > 0 {
            daemon_log(
                &state_root,
                &format!("[simard] reaped {reaped} zombie engineer process(es)"),
            );
        }

        // Check for shutdown signal at the top of each iteration.
        if shutdown.load(Ordering::SeqCst) {
            daemon_log(
                &state_root,
                "[simard] OODA daemon: shutting down gracefully",
            );
            break;
        }

        // Auto-reload: if the on-disk binary is newer, exec into it.
        #[cfg(unix)]
        if auto_reload && binary_changed(start_time) {
            // Close the LLM session before exec so we don't leak resources.
            if let Some(ref mut session) = bridges.session {
                let _ = session.close();
            }
            exec_self_reload()?;
            // exec_self_reload only returns on error — continue running.
        }

        if max_cycles > 0 && cycles_run >= max_cycles {
            daemon_log(
                &state_root,
                &format!("[simard] OODA daemon: completed {cycles_run} cycle(s), exiting"),
            );
            break;
        }

        // --- periodic DB backup (at START of cycle) ------------------------
        if last_db_backup.elapsed() >= Duration::from_secs(db_backup_interval_secs) {
            // Checkpoint first so committed-but-WAL-resident writes are
            // captured by the file copy. Failures here increment the
            // pre_backup_checkpoint_failed counter (issue #1975 G3) and
            // are tracked alongside backup_consecutive_failures so the
            // daemon refuses to declare success after N consecutive
            // checkpoint failures.
            if let Err(e) = shared_mem.checkpoint() {
                crate::cognitive_memory::metrics::increment(
                    "pre_backup_checkpoint_failed",
                    "daemon:periodic_backup",
                );
                let n = checkpoint_consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
                daemon_log(
                    &state_root,
                    &format!("[simard] DB backup: pre-copy checkpoint failed (#{n}): {e}"),
                );
            } else {
                checkpoint_consecutive_failures.store(0, Ordering::SeqCst);
            }
            match NativeCognitiveMemory::create_verified_backup(&state_root) {
                Ok(backup_path) => {
                    let ckpt_fails = checkpoint_consecutive_failures.load(Ordering::SeqCst);
                    if ckpt_fails >= 3 {
                        // G3: refuse to declare healthy backup when the
                        // checkpoint that should have flushed WAL writes
                        // has failed repeatedly — the backup may be stale.
                        daemon_log(
                            &state_root,
                            &format!(
                                "[simard] WARN: DB backup created at {} but \
                                 pre-backup checkpoint has failed {ckpt_fails} \
                                 consecutive times — backup may contain stale data",
                                backup_path.display()
                            ),
                        );
                    } else {
                        daemon_log(
                            &state_root,
                            &format!("[simard] DB backup created: {}", backup_path.display()),
                        );
                    }
                    let prune_outcome =
                        NativeCognitiveMemory::prune_old_backups(&state_root, db_backup_keep);
                    if !prune_outcome.failed.is_empty() {
                        daemon_log(
                            &state_root,
                            &format!(
                                "[simard] WARN: prune_old_backups: {} removed, {} failed",
                                prune_outcome.removed,
                                prune_outcome.failed.len()
                            ),
                        );
                    }
                    backup_consecutive_failures.store(0, Ordering::SeqCst);
                }
                Err(e) => {
                    let n = backup_consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
                    if n >= 3 {
                        daemon_log(
                            &state_root,
                            &format!(
                                "[simard] ERROR: DB backup failed {n} consecutive times \
                                 — last error at {}: {e}",
                                state_root.join("backups").display()
                            ),
                        );
                    } else {
                        daemon_log(
                            &state_root,
                            &format!("[simard] WARN: DB backup failed (#{n}): {e}"),
                        );
                    }
                }
            }
            last_db_backup = Instant::now();
        }
        // ── Disk health check (before spawning engineers) ────────────────
        if last_disk_health.elapsed() >= Duration::from_secs(disk_health_interval_secs) {
            match crate::disk_health::run_disk_health_check(&bridges.repo_root, &state_root) {
                Ok(report) => {
                    daemon_log(&state_root, &format!("[simard] {}", report.summary()));
                    if report.cleanup_performed() {
                        daemon_log(
                            &state_root,
                            &format!("[simard] disk cleanup actions: {:?}", report.actions_taken),
                        );
                    }
                }
                Err(e) => {
                    daemon_log(
                        &state_root,
                        &format!("[simard] WARN: disk health check failed: {e}"),
                    );
                }
            }
            last_disk_health = Instant::now();
        }
        // -------------------------------------------------------------------

        let cycle_start = Instant::now();
        let cycle_start_epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        state.cycle_start_epoch = cycle_start_epoch;

        // Write heartbeat at cycle START so the dashboard never sees "stale"
        // during a long-running cycle.
        {
            let health_dir = dirs::data_local_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
                .join("simard");
            let _ = std::fs::create_dir_all(&health_dir);
            let heartbeat = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "cycle_number": cycles_run + 1,
                "status": "running",
                "cycle_phase": state.current_phase.to_string(),
                "cycle_start_epoch": cycle_start_epoch,
                "interval_secs": interval_secs,
                "actions_taken": format!("Starting cycle #{}", cycles_run + 1),
            });
            let _ = std::fs::write(
                health_dir.join("daemon_health.json"),
                serde_json::to_string_pretty(&heartbeat).unwrap_or_default(),
            );
        }

        match run_ooda_cycle(&mut state, &mut bridges, &config) {
            Ok(report) => {
                let cycle_elapsed = cycle_start.elapsed();
                let summary = summarize_cycle_report(&report);
                state.last_cycle_summary = Some(summary.clone());
                state.last_cycle_duration_secs = Some(cycle_elapsed.as_secs());
                state.current_phase = OodaPhase::Sleep;
                daemon_log(&state_root, &format!("[simard] {summary}"));
                // Persist the cycle report to filesystem for auditability.
                persist_cycle_report(&state_root, &report);
                // Persist the cycle summary to cognitive memory as an episode.
                persist_cycle_to_memory(&bridges, &report);
                // Write daemon health file for dashboard
                {
                    let health_dir = dirs::data_local_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
                        .join("simard");
                    let _ = std::fs::create_dir_all(&health_dir);
                    let health = serde_json::json!({
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                        "cycle_number": cycles_run + 1,
                        "status": "healthy",
                        "cycle_phase": "sleep",
                        "cycle_start_epoch": cycle_start_epoch,
                        "cycle_duration_secs": cycle_elapsed.as_secs(),
                        "interval_secs": interval_secs,
                        "actions_taken": summary.clone(),
                        "last_cycle_summary": summary,
                    });
                    let health_path = health_dir.join("daemon_health.json");
                    if let Err(e) = std::fs::write(
                        &health_path,
                        serde_json::to_string_pretty(&health).unwrap_or_default(),
                    ) {
                        eprintln!("[simard] OODA health: failed to write health file: {e}");
                    }
                }
                // Collect self-improvement metrics at end of each cycle.
                if let Err(e) = crate::self_metrics::collect_and_record_all(cycle_elapsed) {
                    eprintln!("[simard] OODA metrics: failed to record: {e}");
                }
            }
            Err(e) => {
                daemon_log(&state_root, &format!("[simard] OODA cycle error: {e}"));
            }
        }

        cycles_run += 1;

        // Skip the inter-cycle sleep if this was the last requested cycle.
        if max_cycles > 0 && cycles_run >= max_cycles {
            continue;
        }

        // Interruptible sleep — wakes early on SIGTERM/SIGINT instead of
        // blocking for the full interval.
        interruptible_sleep(Duration::from_secs(interval_secs), &shutdown);
    }

    // Final shutdown: flush board, drop in-process writer registration,
    // close session, then drop bridges (triggers Database::drop ->
    // force_checkpoint_on_close). Errors at this point only get warned —
    // we are exiting anyway and cannot recover.
    if let Err(e) = shutdown_daemon(
        &state_root,
        &shared_mem,
        &mut state,
        &mut bridges,
        /* signal_driven */ true,
    ) {
        daemon_log(
            &state_root,
            &format!("[simard] OODA daemon: shutdown sequence reported error: {e}"),
        );
    }

    Ok(())
}

/// Graceful shutdown sequence for the OODA daemon.
///
/// Order matters — see issue #1631 for the WAL-corruption regression
/// this fixes:
///
/// 1. Persist the current `state.active_goals` board through the live
///    writer (so the snapshot survives the restart).
/// 2. Force a `CHECKPOINT;` so all writes (including the persist_board
///    call above) are committed to the main DB file rather than left in
///    the WAL.
/// 3. Close the LLM session cleanly.
/// 4. Clear the in-process writer registration so the global `Weak` no
///    longer holds a path that would prevent the writer Arc from being
///    dropped by name elsewhere.
/// 5. Drop the caller-owned bridges (the daemon's `bridges.memory` Box,
///    other Arc<dyn> references). Once the last strong Arc to the
///    `lbug::Database` drops, `Database::drop` runs
///    `force_checkpoint_on_close` as a defense-in-depth backstop.
///
/// `signal_driven=true` makes errors warnings only (we cannot recover
/// during signal-induced exit). `signal_driven=false` propagates errors
/// so test harnesses and normal exits can assert on them.
fn shutdown_daemon(
    state_root: &std::path::Path,
    shared_mem: &Arc<dyn CognitiveMemoryOps>,
    state: &mut OodaState,
    bridges: &mut OodaBridges,
    signal_driven: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    daemon_log(state_root, "[simard] OODA daemon: shutdown sequence start");

    // 1. Persist the goal board through the live writer.
    if let Err(e) = persist_board(&state.active_goals, &*bridges.memory) {
        let msg = format!("[simard] shutdown: persist_board failed: {e}");
        daemon_log(state_root, &msg);
        if !signal_driven {
            return Err(msg.into());
        }
    }

    // 2. Checkpoint so the persist_board write reaches the main DB file.
    if let Err(e) = shared_mem.checkpoint() {
        let msg = format!("[simard] shutdown: pre-exit checkpoint failed: {e}");
        daemon_log(state_root, &msg);
        if !signal_driven {
            return Err(msg.into());
        }
    }

    // 3. Close the LLM session.
    if let Some(ref mut session) = bridges.session
        && let Err(e) = session.close()
    {
        let msg = format!("[simard] shutdown: session.close failed: {e}");
        daemon_log(state_root, &msg);
        if !signal_driven {
            return Err(msg.into());
        }
    }

    // 4. Clear in-process writer registration so the Weak ref drops.
    memory_ipc::clear_in_process_writer();

    // 5. Bridges (and the daemon-owned strong Arc to NativeCognitiveMemory)
    //    drop on function return — the inherent Database::drop runs
    //    force_checkpoint_on_close as a backstop.
    daemon_log(
        state_root,
        "[simard] OODA daemon: shutdown complete (writer Arc will drop on return)",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::cognitive_memory::CognitiveMemoryOps;
    use crate::goal_curation::GoalBoard;
    use crate::gym_bridge::GymBridge;
    use crate::knowledge_bridge::KnowledgeBridge;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use crate::ooda_loop::{OodaBridges, OodaState};
    use serde_json::json;

    fn mock_memory() -> Box<dyn CognitiveMemoryOps> {
        Box::new(CognitiveMemoryBridge::new(Box::new(
            InMemoryBridgeTransport::new("test-daemon-shutdown", |method, _params| match method {
                "memory.search_facts" => Ok(json!({"facts": []})),
                "memory.store_fact" => Ok(json!({"id": "sem_1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_1"})),
                "memory.get_statistics" => Ok(json!({
                    "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                    "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
                })),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            }),
        )))
    }

    fn mock_shared_mem() -> Arc<dyn CognitiveMemoryOps> {
        Arc::new(CognitiveMemoryBridge::new(Box::new(
            InMemoryBridgeTransport::new("test-daemon-shared", |method, _params| match method {
                "memory.search_facts" => Ok(json!({"facts": []})),
                "memory.store_fact" => Ok(json!({"id": "sem_1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_1"})),
                "memory.get_statistics" => Ok(json!({
                    "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                    "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
                })),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            }),
        )))
    }

    fn mock_knowledge() -> KnowledgeBridge {
        KnowledgeBridge::new(Box::new(InMemoryBridgeTransport::new(
            "test-knowledge",
            |method, _params| match method {
                "knowledge.list_packs" => Ok(json!({"packs": []})),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            },
        )))
    }

    fn mock_gym() -> GymBridge {
        GymBridge::new(Box::new(InMemoryBridgeTransport::new(
            "test-gym",
            |_method, _params| Ok(json!({"suite_id": "test", "success": true})),
        )))
    }

    fn test_bridges() -> OodaBridges {
        OodaBridges {
            memory: mock_memory(),
            knowledge: mock_knowledge(),
            gym: mock_gym(),
            session: None,
            brain: Arc::new(crate::ooda_brain::DeterministicFallbackBrain),
            decide_brain: None,
            orient_brain: None,
            repo_root: std::path::PathBuf::from("."),
            progress_evidence: Arc::new(
                crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker,
            ),
        }
    }

    // ── shutdown_daemon ─────────────────────────────────────────────

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn shutdown_daemon_succeeds_with_empty_state() {
        let hermetic = crate::test_support::HermeticState::new();
        let dir = hermetic.state_root();
        let shared_mem = mock_shared_mem();
        let mut state = OodaState::new(GoalBoard::new());
        let mut bridges = test_bridges();

        let result = shutdown_daemon(dir, &shared_mem, &mut state, &mut bridges, false);
        assert!(
            result.is_ok(),
            "shutdown with empty state must succeed: {result:?}"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn shutdown_daemon_writes_log_lines() {
        let hermetic = crate::test_support::HermeticState::new();
        let dir = hermetic.state_root();
        let shared_mem = mock_shared_mem();
        let mut state = OodaState::new(GoalBoard::new());
        let mut bridges = test_bridges();

        let _ = shutdown_daemon(dir, &shared_mem, &mut state, &mut bridges, true);

        let log = std::fs::read_to_string(dir.join("ooda.log")).unwrap_or_default();
        assert!(
            log.contains("shutdown sequence start"),
            "shutdown must log start marker; got: {log}"
        );
        assert!(
            log.contains("shutdown complete"),
            "shutdown must log completion marker; got: {log}"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn shutdown_daemon_signal_driven_tolerates_persist_errors() {
        let hermetic = crate::test_support::HermeticState::new();
        let dir = hermetic.state_root();
        let shared_mem = mock_shared_mem();
        let mut state = OodaState::new(GoalBoard::new());
        let mut bridges = test_bridges();
        let result = shutdown_daemon(dir, &shared_mem, &mut state, &mut bridges, true);
        assert!(
            result.is_ok(),
            "signal-driven shutdown must not propagate errors: {result:?}"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn shutdown_daemon_with_goals_succeeds() {
        let hermetic = crate::test_support::HermeticState::new();
        let dir = hermetic.state_root();
        let shared_mem = mock_shared_mem();
        let mut board = GoalBoard::new();
        board.active.push(crate::goal_curation::ActiveGoal {
            id: "test-goal-01".to_string(),
            description: "Test goal for shutdown".to_string(),
            priority: 1,
            status: crate::goal_curation::GoalProgress::InProgress { percent: 50 },
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        });
        let mut state = OodaState::new(board);
        let mut bridges = test_bridges();

        let result = shutdown_daemon(dir, &shared_mem, &mut state, &mut bridges, false);
        assert!(
            result.is_ok(),
            "shutdown with active goals must succeed: {result:?}"
        );
    }

    // ── env-var parsing (SIMARD_OODA_INTERVAL_SECS) ─────────────────

    #[test]
    fn ooda_interval_env_var_parsing() {
        // Test the same parsing pattern used in run_ooda_daemon.
        let parse = |val: &str| -> u64 { val.parse().ok().unwrap_or(300) };
        assert_eq!(parse("60"), 60);
        assert_eq!(parse("0"), 0);
        assert_eq!(parse("not-a-number"), 300);
        assert_eq!(parse(""), 300);
    }

    // ── DaemonDashboardConfig coverage from mod.rs perspective ───────

    #[test]
    fn dashboard_config_disabled_skips_dashboard() {
        let cfg = DaemonDashboardConfig {
            enabled: false,
            port: 0,
        };
        assert!(!cfg.enabled);
    }

    #[test]
    fn dashboard_config_enabled_has_port() {
        let cfg = DaemonDashboardConfig {
            enabled: true,
            port: 8080,
        };
        assert!(cfg.enabled);
        assert_eq!(cfg.port, 8080);
    }
}
