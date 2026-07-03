use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::bridge_launcher::{launch_gym_bridge_native, launch_knowledge_bridge_native};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
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

mod backup;

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

    // Reap any stale lock file from a prior crashed daemon before we open.
    if let Err(e) = memory_ipc::reap_stale_open_lock(&state_root) {
        eprintln!("[simard] OODA daemon: stale-lock reap failed: {e}");
    }

    let shared_mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::open(&state_root)?);

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
    let knowledge = launch_knowledge_bridge_native()?;
    let gym = launch_gym_bridge_native()?;

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

    // Mint per-thread LLM sessions for concurrent AdvanceGoal dispatch so the
    // slow goal-action `run_turn` calls run in parallel (one engineer per
    // uncovered goal per round) instead of serializing on the single shared
    // `session`. Bounded by the AIMD cap in the Act phase. Falls back to the
    // shared `session` only if this factory is ever `None`.
    let session_factory: std::sync::Arc<dyn crate::ooda_loop::OrchestratorSessionFactory> =
        std::sync::Arc::new(crate::session_builder::ProviderSessionFactory::new(
            provider, "ooda",
        ));

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

    // Deploy-aware done-gate (issue #2419). Honors the `SIMARD_COMPLETION_EVIDENCE=off`
    // kill switch; otherwise a completed goal is archived only with hard
    // evidence (merged PR + closed issue + — for self-affecting changes — a
    // verified deploy), resolved via `gh` and the reconciliation detector.
    let completion_evidence: Option<
        std::sync::Arc<dyn crate::goal_curation::completion_gate::EvidenceSource>,
    > = if crate::goal_curation::completion_evidence_enabled() {
        daemon_log(
            &state_root,
            "[simard] completion-evidence: enabled (GhCliEvidenceSource -- merged+closed+deployed gate)",
        );
        Some(std::sync::Arc::new(
            crate::goal_curation::GhCliEvidenceSource::new(repo_root.clone()),
        ))
    } else {
        daemon_log(
            &state_root,
            "[simard] completion-evidence: DISABLED (legacy archive -- SIMARD_COMPLETION_EVIDENCE=off)",
        );
        None
    };

    let mut bridges = OodaBridges {
        memory,
        knowledge,
        gym,
        session: Some(session),
        session_factory: Some(session_factory),
        brain,
        decide_brain,
        orient_brain,
        repo_root,
        progress_evidence,
        completion_evidence,
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

    let interval_secs: u64 =
        ooda_interval_secs_from_env(std::env::var("SIMARD_OODA_INTERVAL_SECS").ok().as_deref());

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

    // Pin the running image's CONTENT hash at startup too. The reload gate
    // (`binary_changed`) now relaunches only when the on-disk image is a
    // genuinely different binary — not on a byte-identical rebuild that merely
    // bumped the mtime, which was the ~40–45 min self-restart churn trigger
    // (2026-07-02 operator-review #2; see
    // docs/reference/ooda-binary-identity-reload-gate.md). Pinning it here
    // (rather than lazily on the first check) narrows the window where an
    // in-place replace between exec and this call could be mistaken for the
    // running image (the hash is read from the on-disk path, so a replace that
    // lands before this line still poisons the pinned identity — but the effect
    // is bounded: the next genuinely-different rebuild bumps mtime+hash and
    // reloads).
    capture_running_image_hash();

    if auto_reload {
        // Make the LOGGED state match reality: if we could not hash our own
        // image at startup, the content-identity gate fails closed and
        // self-reload is disabled for this whole process. Say so, rather than
        // logging "enabled" while silently never relaunching.
        if running_image_hash().is_some() {
            daemon_log(&state_root, "[simard] OODA daemon: auto-reload enabled");
        } else {
            daemon_log(
                &state_root,
                "[simard] OODA daemon: WARNING auto-reload requested but the running-image hash \
                 is unavailable — self-reload is DISABLED for this process (fail-closed)",
            );
        }
    }
    let self_relaunch_interval = crate::self_deploy::restart::self_relaunch_min_interval_from_env(
        std::env::var(crate::self_deploy::restart::SELF_RELAUNCH_MIN_INTERVAL_ENV)
            .ok()
            .as_deref(),
    );
    daemon_log(
        &state_root,
        &format!(
            "[simard] self-relaunch: min interval = {} ({}; 0/off disables interval-only relaunches; real binary hash changes bypass the interval)",
            self_relaunch_interval.label(),
            crate::self_deploy::restart::SELF_RELAUNCH_MIN_INTERVAL_ENV,
        ),
    );

    // De-fork Phase 2b (issue #2307): the native lbug-WAL file-copy backup was
    // removed. Issue #2420 reintroduces a periodic **verified** backup below —
    // it snapshots the live store through the bridge (so it inherently targets
    // the migrated `state_root/cognitive` path), verifies the backup re-opens
    // before pruning, and is best-effort (a failure WARNs, never aborts the
    // cycle). The library backend still owns its own WAL durability.

    // --- periodic verified backup state (issue #2420) ---------------------
    let backup_interval_secs: u64 = backup::backup_interval_secs_from_env(
        std::env::var("SIMARD_BACKUP_INTERVAL_SECS").ok().as_deref(),
    );
    // `None` == "never backed up yet" so the FIRST cycle always runs a backup,
    // regardless of host uptime. (A `checked_sub` back-date would silently
    // defer the first backup a full interval on any host whose monotonic clock
    // — boot-relative on Linux — is younger than the interval, i.e. every
    // freshly rebooted/deployed host. That is exactly the post-restart window
    // this fix exists to protect.)
    let mut last_backup: Option<Instant> = None;
    daemon_log(
        &state_root,
        &format!("[simard] OODA daemon: verified backup interval = {backup_interval_secs}s"),
    );
    // -------------------------------------------------------------------

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

    // --- periodic engineer worktree sweep state (issue #2167) -----------
    let worktree_sweep_interval_secs: u64 = std::env::var("SIMARD_WORKTREE_SWEEP_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1800); // every 30 minutes by default
    let mut last_worktree_sweep = Instant::now(); // startup sweep just ran
    daemon_log(
        &state_root,
        &format!("[simard] OODA daemon: worktree sweep interval = {worktree_sweep_interval_secs}s"),
    );
    // -------------------------------------------------------------------

    // --- periodic brain introspection + memory hygiene state (issue #2419) ---
    let brain_introspection_interval_secs: u64 = crate::brain_introspection::interval_secs_from_env(
        std::env::var("SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS")
            .ok()
            .as_deref(),
    );
    // NOT back-dated like disk-health: the first introspection runs one full
    // interval after start (nothing useful to say at t=0; baseline is empty).
    let mut last_brain_introspection = Instant::now();
    daemon_log(
        &state_root,
        &format!(
            "[simard] OODA daemon: brain introspection interval = {brain_introspection_interval_secs}s"
        ),
    );
    // -------------------------------------------------------------------

    // --- periodic monthly self-quality-audit state (issue #2419) ---------
    // Reuses the sibling periodic-task infra, but persists last-run to DISK
    // (every sibling gates on an in-process `Instant`, which resets on reboot —
    // fine at 24h, wrong at 30d). On startup: load the persisted epoch; if the
    // marker is absent or unparseable, initialize it to NOW and persist, so the
    // heavy five-wave audit fires ~one interval later rather than on every fresh
    // deploy/restart. Env `SIMARD_SELF_AUDIT_INTERVAL` (seconds; 0 disables).
    let self_audit_interval_secs: u64 = crate::self_quality_audit::interval_secs_from_env(
        std::env::var("SIMARD_SELF_AUDIT_INTERVAL").ok().as_deref(),
    );
    let self_audit_last_run_path = state_root.join(crate::self_quality_audit::LAST_RUN_FILENAME);
    let mut self_audit_last_run: u64 =
        match crate::self_quality_audit::read_last_run(&self_audit_last_run_path) {
            Some(epoch) => epoch,
            None => {
                let now = crate::self_quality_audit::now_epoch_secs();
                let _ = crate::self_quality_audit::write_last_run(&self_audit_last_run_path, now);
                now
            }
        };
    daemon_log(
        &state_root,
        &format!("[simard] OODA daemon: self quality-audit interval = {self_audit_interval_secs}s"),
    );
    // -------------------------------------------------------------------

    // ── Cognitive-thread scheduler (issue #2419) ───────────────────────
    // ADDITIVE and OFF by default. When SIMARD_COGNITIVE_THREADS_ENABLED is
    // truthy, a `Mind` hosts NEW background cognitive threads (safe
    // maintenance/cleanup + engineer-log analysis) on their own cadence,
    // subsuming the ad-hoc periodic-task pattern for these threads. OODA
    // itself stays driven by this loop's authoritative inline cycle below so
    // its external cadence and side-effects are byte-for-byte preserved; the
    // scheduler is invoked only AFTER the inline cycle and never gates it.
    let cognitive_threads_enabled = std::env::var("SIMARD_COGNITIVE_THREADS_ENABLED")
        .ok()
        .map(|s| matches!(s.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false);
    let cognitive_repo_root = bridges.repo_root.clone();
    let cognitive_runtime = if cognitive_threads_enabled {
        match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
        {
            Ok(rt) => Some(rt),
            Err(e) => {
                daemon_log(
                    &state_root,
                    &format!("[simard] WARN: cognitive-thread runtime build failed: {e}"),
                );
                None
            }
        }
    } else {
        None
    };
    let mut mind = crate::cognitive_threads::Mind::new();
    if cognitive_runtime.is_some() {
        mind.register(Box::new(
            crate::cognitive_threads::MaintenanceThread::from_env(),
        ));
        mind.register(Box::new(
            crate::cognitive_threads::EngineerLogAnalysisThread::from_env(),
        ));
        daemon_log(
            &state_root,
            &format!(
                "[simard] OODA daemon: cognitive-thread scheduler ENABLED ({} background thread(s))",
                mind.len()
            ),
        );
    }

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

        // Auto-reload: if the on-disk binary is a genuinely different image
        // (content hash differs from the running one), exec into it.
        #[cfg(unix)]
        if auto_reload && binary_changed(start_time) {
            daemon_log(
                &state_root,
                "[simard] OODA daemon: on-disk binary is a genuinely different image (content hash changed) — reloading via exec()",
            );
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

        // ── Periodic verified backup of the LIVE cognitive store (#2420) ──
        // Best-effort: snapshot the live store the daemon opened, verify the
        // backup re-opens, then prune. A failure WARNs and skips the prune so
        // prior good backups survive; it never aborts the OODA cycle.
        if backup::should_run_backup(last_backup, backup_interval_secs) {
            match backup::run_verified_backup(shared_mem.as_ref(), &state_root) {
                Ok(manifest) => daemon_log(
                    &state_root,
                    &format!(
                        "[simard] verified backup OK: {} facts + {} procedures + {} records -> {}",
                        manifest.cognitive_facts_count,
                        manifest.cognitive_procedures_count,
                        manifest.memory_records_count,
                        manifest.backup_dir.display()
                    ),
                ),
                Err(e) => daemon_log(
                    &state_root,
                    &format!("[simard] WARN: verified backup FAILED, prune skipped: {e}"),
                ),
            }
            last_backup = Some(Instant::now());
        }
        // -------------------------------------------------------------------

        // ── Disk health check (before spawning engineers) ────────────────
        if last_disk_health.elapsed() >= Duration::from_secs(disk_health_interval_secs) {
            // Tier 1: deterministic emergency cleanup (no LLM, no recipe)
            if let Some(emergency_report) =
                crate::disk_health::emergency_cleanup(&bridges.repo_root, &state_root)
            {
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] EMERGENCY disk cleanup: {}% -> freed {} bytes",
                        emergency_report.disk_used_pct, emergency_report.freed_bytes
                    ),
                );
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] emergency actions: {:?}",
                        emergency_report.actions_taken
                    ),
                );
            }
            // Tier 2: recipe-based LLM cleanup (moderate pressure, nuanced decisions)
            match crate::disk_health::run_disk_health_check(&bridges.repo_root, &state_root, None) {
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

        // ── RSS health check (issue #2167) / memory shedding (issue #2183) ─
        if let Some(report) = crate::rss_health::check_rss_health() {
            let rss_str = crate::rss_health::format_rss(report.rss_bytes);
            if report.critical {
                daemon_log(
                    &state_root,
                    &format!("[simard] CRITICAL: RSS = {rss_str} — exceeds hard threshold"),
                );
            } else if report.warn {
                daemon_log(
                    &state_root,
                    &format!("[simard] WARN: RSS = {rss_str} — exceeds warn threshold"),
                );
            } else {
                daemon_log(&state_root, &format!("[simard] RSS health: {rss_str}"));
            }

            // Emergency memory shedding when RSS exceeds the elevated
            // threshold (default 8 GiB, env SIMARD_RSS_ELEVATED_BYTES).
            if report.rss_bytes >= crate::memory_health::elevated_threshold_bytes() {
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] RSS {} exceeds elevated threshold — running emergency shed",
                        rss_str
                    ),
                );
                let shed =
                    crate::memory_health::run_emergency_shed(shared_mem.as_ref(), &state_root);
                daemon_log(&state_root, &format!("[simard] {}", shed.summary()));
            }
        }
        // ── Periodic engineer worktree sweep (issue #2167) ──────────────
        if last_worktree_sweep.elapsed() >= Duration::from_secs(worktree_sweep_interval_secs) {
            if let Ok(parent_repo) = std::env::current_dir() {
                match crate::engineer_worktree::sweep_orphaned_worktrees(&parent_repo, &state_root)
                {
                    Ok(report) => {
                        if !report.removed_orphan_dirs.is_empty() {
                            daemon_log(
                                &state_root,
                                &format!(
                                    "[simard] periodic sweep: removed {} orphan engineer worktree(s)",
                                    report.removed_orphan_dirs.len()
                                ),
                            );
                        }
                    }
                    Err(e) => daemon_log(
                        &state_root,
                        &format!("[simard] periodic worktree sweep failed: {e}"),
                    ),
                }
            }
            last_worktree_sweep = Instant::now();
        }
        // -------------------------------------------------------------------

        // ── Periodic brain introspection + memory hygiene (issue #2419) ──
        // Higher-level self-examination pass: safe RPC-backed memory hygiene
        // (expired-sensory prune + additive consolidation) plus an agentic
        // recipe that surfaces brain-health/patterns, recommends bounded prunes,
        // and writes findings to a dedup'd GitHub issue. Best-effort: a recipe
        // failure WARNs and the safe hygiene still ran.
        if crate::brain_introspection::should_run_introspection(
            last_brain_introspection.elapsed(),
            brain_introspection_interval_secs,
        ) {
            match crate::brain_introspection::run_brain_introspection(
                &*bridges.memory,
                &bridges.repo_root,
                &state_root,
                None,
            ) {
                Ok(report) => {
                    daemon_log(&state_root, &format!("[simard] {}", report.summary()));
                }
                Err(e) => daemon_log(
                    &state_root,
                    &format!("[simard] WARN: brain introspection failed: {e}"),
                ),
            }
            last_brain_introspection = Instant::now();
        }
        // -------------------------------------------------------------------

        // ── Periodic monthly self-quality-audit (issue #2419) ────────────
        // Fires ~monthly on a DISK-persisted gate (survives restarts). Drives
        // five SEEK→VALIDATE→FIX quality-audit waves over Simard's OWN repo,
        // each resulting PR gated by a bounded crusty-old-engineer proxy review,
        // then self-merges crusty-approved + CI-green PRs. No-fallback: a recipe
        // failure WARNs; last-run is persisted regardless of outcome so a
        // failing recipe cannot hot-loop for a full interval.
        let self_audit_elapsed = Duration::from_secs(
            crate::self_quality_audit::now_epoch_secs().saturating_sub(self_audit_last_run),
        );
        if crate::self_quality_audit::should_run_self_audit(
            self_audit_elapsed,
            self_audit_interval_secs,
        ) {
            daemon_log(
                &state_root,
                "[simard] self quality-audit: firing 5-wave crusty-gated self-audit of rysweet/Simard",
            );
            match crate::self_quality_audit::run_self_quality_audit(
                &bridges.repo_root,
                &state_root,
                None,
            ) {
                Ok(report) => {
                    daemon_log(&state_root, &format!("[simard] {}", report.summary()));
                }
                Err(e) => daemon_log(
                    &state_root,
                    &format!("[simard] WARN: self quality-audit failed: {e}"),
                ),
            }
            // Persist last-run on BOTH Ok and Err to prevent hot-looping a
            // failing recipe every cycle for a full interval.
            self_audit_last_run = crate::self_quality_audit::now_epoch_secs();
            let _ = crate::self_quality_audit::write_last_run(
                &self_audit_last_run_path,
                self_audit_last_run,
            );
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

                // Issue #2528: emit unified cycle telemetry (OTel + in-process
                // registry) and push per-cycle gauges, then flush the metrics
                // snapshot so `simard status` and the TUI — separate processes —
                // read live daemon metrics with no external OTLP collector. All
                // best-effort: a telemetry hiccup never disrupts the cycle.
                {
                    use crate::telemetry::{self, names};
                    telemetry::counter_add(names::DAEMON_CYCLE, 1, &[]);
                    telemetry::histogram_record(
                        names::DAEMON_CYCLE_DURATION_SECONDS,
                        cycle_elapsed.as_secs_f64(),
                        &[],
                    );
                    telemetry::gauge_set(
                        names::GOAL_ACTIVE,
                        state.active_goals.active.len() as i64,
                        &[],
                    );
                    if let Ok(stats) = bridges.memory.get_statistics() {
                        telemetry::gauge_set(
                            names::MEMORY_NODES,
                            stats.episodic_count as i64,
                            &[(names::ATTR_TYPE, "episodic")],
                        );
                        telemetry::gauge_set(
                            names::MEMORY_NODES,
                            stats.semantic_count as i64,
                            &[(names::ATTR_TYPE, "semantic")],
                        );
                        telemetry::gauge_set(
                            names::MEMORY_NODES,
                            stats.prospective_count as i64,
                            &[(names::ATTR_TYPE, "prospective")],
                        );
                        telemetry::gauge_set(
                            names::MEMORY_NODES,
                            stats.working_count as i64,
                            &[(names::ATTR_TYPE, "working")],
                        );
                        telemetry::gauge_set(
                            names::MEMORY_NODES,
                            stats.procedural_count as i64,
                            &[(names::ATTR_TYPE, "procedural")],
                        );
                        telemetry::gauge_set(
                            names::MEMORY_NODES,
                            stats.sensory_count as i64,
                            &[(names::ATTR_TYPE, "sensory")],
                        );
                    }
                    if let Ok(g) = bridges.memory.graph_stats() {
                        telemetry::gauge_set(
                            names::MEMORY_EDGES,
                            g.derives_from_edges as i64,
                            &[(names::ATTR_TYPE, "DERIVES_FROM")],
                        );
                        telemetry::gauge_set(
                            names::MEMORY_EDGES,
                            g.similar_to_edges as i64,
                            &[(names::ATTR_TYPE, "SIMILAR_TO")],
                        );
                        telemetry::gauge_set(
                            names::MEMORY_EDGES,
                            g.supersedes_edges as i64,
                            &[(names::ATTR_TYPE, "SUPERSEDES")],
                        );
                    }
                    if let Err(e) = telemetry::flush_snapshot(&state_root) {
                        eprintln!("[simard] telemetry: failed to flush metrics snapshot: {e}");
                    }
                }
            }
            Err(e) => {
                daemon_log(&state_root, &format!("[simard] OODA cycle error: {e}"));
            }
        }

        cycles_run += 1;

        // ── Cognitive-thread scheduler tick (issue #2419) ───────────────
        // Runs AFTER the authoritative OODA cycle so OODA is never starved.
        // Background threads (maintenance, engineer-log analysis) run on their
        // own cadence under the `Mind`'s priority/failure-isolation budget; a
        // thread erroring or panicking is caught and backed off, never taking
        // down the daemon or the OODA loop. No-op unless explicitly enabled.
        if let Some(ref rt) = cognitive_runtime {
            let now_epoch = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mut ctx = crate::cognitive_threads::ThreadContext {
                state_root: &state_root,
                repo_root: &cognitive_repo_root,
                memory: shared_mem.as_ref(),
                runtime: rt.handle().clone(),
                shutdown: &shutdown,
                now_epoch,
                dry_run: false,
            };
            for outcome in mind.run_due(&mut ctx) {
                if outcome.ran {
                    daemon_log(
                        &state_root,
                        &format!("[simard] cognitive-thread: {}", outcome.summary),
                    );
                }
            }
        }

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

    // 5. Bridges (and the daemon-owned strong Arc to LibraryCognitiveMemory)
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
            session_factory: None,
            brain: Arc::new(crate::ooda_brain::DeterministicLifecycleBrain),
            decide_brain: None,
            orient_brain: None,
            repo_root: std::path::PathBuf::from("."),
            progress_evidence: Arc::new(
                crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker,
            ),
            completion_evidence: None,
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
            parent_goal_id: None,
            repo: None,
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
        // Exercises the REAL helper (not a copy of the inline logic).
        assert_eq!(ooda_interval_secs_from_env(Some("60")), 60);
        // A `0` interval must clamp to the default — a zero-delay loop busy-spins
        // the daemon (interruptible_sleep no-ops on Duration::ZERO). This is the
        // Wave 4 fix; the prior test asserted `0 -> 0`, codifying the bug.
        assert_eq!(
            ooda_interval_secs_from_env(Some("0")),
            DEFAULT_OODA_INTERVAL_SECS
        );
        assert_eq!(
            ooda_interval_secs_from_env(Some("not-a-number")),
            DEFAULT_OODA_INTERVAL_SECS
        );
        assert_eq!(
            ooda_interval_secs_from_env(Some("")),
            DEFAULT_OODA_INTERVAL_SECS
        );
        assert_eq!(
            ooda_interval_secs_from_env(None),
            DEFAULT_OODA_INTERVAL_SECS
        );
        // Whitespace is trimmed; a large value is honoured (a long sleep is harmless).
        assert_eq!(ooda_interval_secs_from_env(Some("  120  ")), 120);
        assert_eq!(ooda_interval_secs_from_env(Some("86400")), 86_400);
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
