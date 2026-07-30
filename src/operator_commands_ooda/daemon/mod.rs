use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::goal_curation::persist_board;
use crate::identity::OperatingMode;
use crate::memory_ipc;
use crate::ooda_loop::{
    OodaClients, OodaConfig, OodaPhase, OodaState, run_ooda_cycle, summarize_cycle_report,
};
use crate::rpc_subprocess_launcher::{launch_gym_client_native, launch_knowledge_client_native};
use crate::runtime_config::RuntimeConfig;
use crate::session_builder::{LlmProvider, SessionBuilder};

use crate::operator_commands_ooda::persistence::{
    persist_cycle_report_timed, persist_cycle_to_memory,
};

mod helpers;
pub use helpers::*;

mod backup;

mod brains;

mod config;
pub use config::DaemonDashboardConfig;

mod signal_embed;
use signal_embed::spawn_embedded_signal_channel;

/// Seed the durable, brain-relative OODA cycle counter for a starting daemon
/// (issue #1).
///
/// Returns `persistent_cycle_count` when it is already authoritative (non-zero)
/// — the steady-state path, where the durable `PersistentGoalState.cycle_count`
/// is the source of truth. When it is `0` — a brain upgraded from a build that
/// never persisted the field — the count is recovered from the highest
/// `cycle_<N>.json` report filename already on disk, so the displayed number
/// does not dip to `#1` for a single deploy after the upgrade. A genuinely
/// fresh brain (no reports) stays at `0`, so its first cycle is `#1`.
///
/// Read-only and idempotent: it only inspects report filenames (never bodies,
/// never the network, never a write), and a single successful `commit_cycle`
/// makes the `== 0` guard permanently false so the backfill never runs again.
fn seed_cycle_count(persistent_cycle_count: u32, state_root: &std::path::Path) -> u32 {
    if persistent_cycle_count != 0 {
        return persistent_cycle_count;
    }
    let latest =
        crate::operator_commands_dashboard::cycle_source::latest_persisted_cycle_number(state_root);
    if latest > 0 {
        // `latest_persisted_cycle_number` returns u64; the counter is u32.
        // Saturate rather than wrap on the (unreachable in practice) overflow.
        u32::try_from(latest).unwrap_or(u32::MAX)
    } else {
        0
    }
}

fn clear_stale_draining_flag_at_boot(state_root: &std::path::Path) {
    // Engineer dispatch checks safe_update::default_state_dir(), which is
    // `<state_root>/state` for the default daemon root; clear that canonical
    // gate even when the daemon was booted with a different state_root override.
    let state_dir = crate::safe_update::default_state_dir();
    clear_stale_draining_flag_at_boot_in(&state_dir, state_root);
}

fn clear_stale_draining_flag_at_boot_in(state_dir: &std::path::Path, log_root: &std::path::Path) {
    let should_clear = match crate::safe_update::state::read_status(state_dir) {
        Ok(None) => true,
        Ok(Some(status)) => status.phase != crate::safe_update::state::UpgradePhase::ExecHandover,
        Err(e) => {
            daemon_log(
                log_root,
                &format!(
                    "[simard] boot: could not read upgrade status; preserving draining.flag: {e}"
                ),
            );
            false
        }
    };
    if !should_clear {
        return;
    }

    let flag = crate::safe_update::draining_flag_path(state_dir);
    if !flag.exists() {
        return;
    }
    match crate::safe_update::drain::unmark_draining(state_dir) {
        Ok(()) => daemon_log(
            log_root,
            "[simard] boot: cleared stale draining.flag (no ExecHandover upgrade in flight) — resuming engineer dispatch",
        ),
        Err(e) => daemon_log(
            log_root,
            &format!("[simard] boot: failed to clear stale draining.flag; continuing boot: {e}"),
        ),
    }
}

fn purge_actor_sessions_on_startup(
    state_root: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let ledger_path = crate::typed_ooda::ledger_path(state_root);
    let ledger_parent = ledger_path
        .parent()
        .ok_or_else(|| std::io::Error::other("typed-OODA ledger path has no parent directory"))?;
    std::fs::create_dir_all(ledger_parent)?;
    let handler = crate::typed_ooda::CapabilityHandler::open(
        &ledger_path,
        crate::typed_ooda::CapabilityPolicy::new("daemon-startup"),
    )?;
    handler.purge_actor_sessions()?;
    Ok(())
}

/// Resolve the identity-scoped cognition (#3125) for the daemon from the
/// environment, **fail-closed**.
///
/// - `SIMARD_IDENTITY` unset/blank → no identity → [`crate::ooda_loop::IdentityCognition::default`]
///   (Simard herself: default seed goals + engineer-dispatching Act phase — unchanged).
/// - `SIMARD_IDENTITY` set and the manifest resolves → project its seed goals,
///   target scope, and write-authority posture via
///   [`crate::ooda_loop::IdentityCognition::from_manifest`].
/// - `SIMARD_IDENTITY` set but the manifest CANNOT be resolved → fail-closed:
///   install a `read-only` posture so the observe-only rail denies every engineer
///   dispatch. A named identity whose posture is uncertain never spawns (operator
///   constraint #3125). There is no wall-clock timeout and no fallback-to-dispatch.
fn resolve_daemon_identity_cognition() -> crate::ooda_loop::IdentityCognition {
    use crate::ooda_loop::IdentityCognition;

    let identity_name = match std::env::var("SIMARD_IDENTITY") {
        Ok(name) if !name.trim().is_empty() => name,
        _ => return IdentityCognition::default(),
    };

    let prompt_root = std::env::var_os("SIMARD_PROMPT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets"));
    let identity_path = std::env::var_os("SIMARD_IDENTITY_PATH").map(PathBuf::from);

    let freshness = match crate::Freshness::now() {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "[simard] OODA daemon: identity cognition FAIL-CLOSED for '{identity_name}': \
                 freshness error: {e}; installing read-only posture (0 engineer dispatch)"
            );
            return fail_closed_identity_cognition(&identity_name);
        }
    };
    let contract = match crate::ManifestContract::new(
        concat!(module_path!(), "::resolve_daemon_identity_cognition"),
        "ooda-daemon -> identity-loader -> identity-cognition",
        vec![format!("identity:{identity_name}")],
        crate::Provenance::runtime(format!("ooda-daemon/identity-cognition/{identity_name}")),
        freshness,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[simard] OODA daemon: identity cognition FAIL-CLOSED for '{identity_name}': \
                 contract error: {e}; installing read-only posture (0 engineer dispatch)"
            );
            return fail_closed_identity_cognition(&identity_name);
        }
    };

    match crate::bootstrap::assembly::load_identity(
        identity_path.as_ref(),
        &prompt_root,
        &crate::IdentityLoadRequest::new(
            identity_name.clone(),
            env!("CARGO_PKG_VERSION"),
            contract,
        ),
    ) {
        Ok(manifest) => {
            let cognition = IdentityCognition::from_manifest(&manifest);
            eprintln!(
                "[simard] OODA daemon: resolved identity cognition for '{}' — posture={}, \
                 {} seed goal(s), targets={:?}",
                manifest.name,
                manifest.authority.posture,
                cognition.seed_goals.len(),
                cognition.target_repos,
            );
            cognition
        }
        Err(e) => {
            eprintln!(
                "[simard] OODA daemon: identity cognition FAIL-CLOSED for '{identity_name}': \
                 manifest load error: {e}; installing read-only posture (0 engineer dispatch)"
            );
            fail_closed_identity_cognition(&identity_name)
        }
    }
}

/// The fail-closed identity cognition: a named identity with a `read-only`
/// posture and no seed-goal override, so the observe-only rail denies every
/// engineer dispatch while leaving Simard's default seeding in place.
fn fail_closed_identity_cognition(identity_name: &str) -> crate::ooda_loop::IdentityCognition {
    crate::ooda_loop::IdentityCognition {
        identity_name: Some(identity_name.to_string()),
        seed_goals: Vec::new(),
        target_repos: Vec::new(),
        authority: Some(crate::identity::IdentityAuthority::read_only()),
    }
}

/// Run one or more OODA cycles as a daemon-style loop.
///
/// Launches all memories, opens a RustyClawd session via [`SessionBuilder`]
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
/// the daemon exits with an error — no silent degradation to memory-only mode.
/// Interval between off-path cognitive-memory statistics-snapshot refreshes
/// (issue #4756). Short enough to keep the rpc-health canary's reading current,
/// long enough to be negligible load (`try_lock` + a cheap fold).
const STATS_SNAPSHOT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// Spawn a lightweight background thread that periodically refreshes the
/// cognitive-memory statistics snapshot OFF the heavy read lock (issue #4756).
///
/// [`CognitiveMemoryOps::refresh_stats_snapshot`] recomputes via `try_lock`, so
/// this thread never blocks on `inner` and can never reintroduce the
/// lock-starvation that timed out the rpc-health gate; it merely bounds how
/// stale the snapshot `get_statistics` serves can get between reads. Holds a
/// [`Weak`] handle so the thread exits on its own once the daemon drops its
/// memory (no lifetime leak in tests or on shutdown). A spawn failure is traced
/// and tolerated: the snapshot is still primed and `get_statistics` refreshes
/// inline on every read, so the canary path stays correct.
fn spawn_stats_snapshot_refresher(memory: std::sync::Weak<dyn CognitiveMemoryOps>) {
    let spawned = std::thread::Builder::new()
        .name("stats-snapshot-refresher".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(STATS_SNAPSHOT_REFRESH_INTERVAL);
                let Some(memory) = memory.upgrade() else {
                    // Daemon dropped its memory handle — nothing left to refresh.
                    break;
                };
                memory.refresh_stats_snapshot();
            }
        });
    if let Err(e) = spawned {
        tracing::error!(
            target: "simard::memory",
            error = %e,
            "failed to spawn stats-snapshot refresher (snapshot still primed; \
             get_statistics refreshes inline on read)",
        );
    }
}

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

    // Auto-ensure runtime dependencies before launching memories
    if let Err(e) = crate::cmd_ensure_deps::handle_ensure_deps() {
        eprintln!("Warning: some dependencies could not be verified: {e}");
    }

    let state_root = state_root_override.unwrap_or_else(memory_ipc::default_state_root);

    std::fs::create_dir_all(&state_root)?;
    purge_actor_sessions_on_startup(&state_root)?;
    clear_stale_draining_flag_at_boot(&state_root);

    // Freshness gate at daemon startup (issue #439): belt-and-suspenders run of
    // `amplihack update` under the same cross-process lock and TTL the per-spawn
    // gate uses, so the very first engineer of the boot already runs on the
    // latest amplihack-rs. A failed update is surfaced (warn/error log +
    // `amplihack_update_failure` metric); the daemon still boots on the
    // last-known-good install (strict mode gates engineer spawns, not boot).
    {
        let outcome = crate::amplihack_freshness_gate::ensure_amplihack_fresh_in(&state_root);
        daemon_log(
            &state_root,
            &format!(
                "[simard] OODA daemon: amplihack freshness gate at startup -> {}",
                outcome.as_str()
            ),
        );
    }

    // Reap any stale lock file from a prior crashed daemon before we open.
    if let Err(e) = memory_ipc::reap_stale_open_lock(&state_root) {
        eprintln!("[simard] OODA daemon: stale-lock reap failed: {e}");
    }

    let shared_mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::open(&state_root)?);

    // Issue #2550: self-heal a corruption-reset store BEFORE anything seeds it.
    // If the library backend had to quarantine a corrupt store and rebuild it
    // empty, restore the newest non-empty verified-backup snapshot so a
    // WAL-corruption reset does not permanently lose memories. A populated store
    // (the normal case) is left untouched. Best-effort: a failure is logged and
    // never aborts daemon startup.
    {
        let backup_config = crate::memory_backup::BackupConfig {
            backup_dir: state_root.join("backups"),
            ..crate::memory_backup::BackupConfig::default()
        };
        match crate::memory_backup::auto_restore_if_empty(shared_mem.as_ref(), &backup_config) {
            Ok(Some(report)) => daemon_log(
                &state_root,
                &format!(
                    "[simard] OODA daemon: store was empty — auto-restored {} memories from {}",
                    report.restored,
                    report.from.display()
                ),
            ),
            Ok(None) => {}
            Err(e) => eprintln!("[simard] OODA daemon: startup auto-restore check failed: {e}"),
        }
    }

    // Register the live writer for in-process callers (dashboard, OODA
    // loop, reflection, etc.) so they bypass IPC and disk re-open and
    // share this exact handle. This eliminates the dashboard's
    // hollow-success failure mode where launch_writer_client previously
    // fell through to a read-only handle when both IPC and direct open
    // failed (issue #1590 follow-up).
    memory_ipc::register_in_process_writer(state_root.clone(), Arc::clone(&shared_mem));

    // Issue #4756: prime the cognitive-memory statistics snapshot synchronously
    // BEFORE the memory IPC socket starts accepting, so the very first canary
    // `memory stats` RPC is served from a populated, in-well-under-a-second
    // snapshot rather than the heavy `inner` lock that starved the rpc-health
    // probe past its 30s window and reddened every self-deploy canary. Then run
    // a lightweight refresher that recomputes the snapshot off the read path
    // (via `try_lock`, never blocking) so its staleness stays bounded.
    shared_mem.refresh_stats_snapshot();
    spawn_stats_snapshot_refresher(Arc::downgrade(&shared_mem));

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
    let knowledge = launch_knowledge_client_native()?;
    let gym = launch_gym_client_native()?;

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
    // Honors `SIMARD_PROGRESS_EVIDENCE=off` as a kill switch, and
    // `SIMARD_PROGRESS_EVIDENCE=direct` as an explicit request to use the
    // direct LLM reviewer instead of the recipe-runner-backed checker.
    let progress_mode = std::env::var("SIMARD_PROGRESS_EVIDENCE").ok();
    let kill_switch = progress_mode
        .as_deref()
        .map(|v| v.eq_ignore_ascii_case("off"))
        .unwrap_or(false);
    let force_direct = progress_mode
        .as_deref()
        .map(|v| v.eq_ignore_ascii_case("direct"))
        .unwrap_or(false);
    let progress_evidence: std::sync::Arc<
        dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker,
    > = if kill_switch {
        daemon_log(
            &state_root,
            "[simard] progress-evidence: DISABLED (NoopProgressEvidenceChecker -- SIMARD_PROGRESS_EVIDENCE=off)",
        );
        std::sync::Arc::new(crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker)
    } else if !force_direct
        && let Some(recipe_checker) =
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
                    if force_direct {
                        "[simard] progress-evidence: enabled (LlmReviewerProgressChecker -- SIMARD_PROGRESS_EVIDENCE=direct)"
                    } else {
                        "[simard] progress-evidence: enabled (LlmReviewerProgressChecker -- direct LLM fallback)"
                    },
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

    // Closed-loop outcome verification (issue #2751). Secure default is ON; the
    // operator kill-switch `SIMARD_OUTCOME_VERIFY=off` restores the artifact-only
    // curate path by leaving the memory pair `None`. The pair is installed only
    // when the reasoning recipe brain is available (recipe file + recipe-runner-rs
    // present); otherwise the daemon logs the degradation and stays on the legacy
    // path (never a silent fallback). `live_signals` is wired iff the brain is.
    let outcome_verify_on = crate::goal_curation::outcome_verify_enabled();
    let outcome_verify_brain: Option<std::sync::Arc<dyn crate::ooda_brain::OodaBrain>> =
        if outcome_verify_on {
            crate::ooda_brain::RecipeBrain::new(
                &repo_root,
                "ooda-goal-outcome-verification.yaml",
                "recipe-outcome-verify-brain",
            )
            .map(|b| std::sync::Arc::new(b) as std::sync::Arc<dyn crate::ooda_brain::OodaBrain>)
        } else {
            None
        };
    if !outcome_verify_on {
        daemon_log(
            &state_root,
            "[simard] outcome-verify: DISABLED (SIMARD_OUTCOME_VERIFY=off) -- artifact-only curate path",
        );
    } else if outcome_verify_brain.is_some() {
        daemon_log(
            &state_root,
            "[simard] outcome-verify: enabled (live outcome verification gates archival)",
        );
    } else {
        daemon_log(
            &state_root,
            "[simard] outcome-verify: recipe brain unavailable (recipe/binary missing) -- artifact-only curate path",
        );
    }
    let live_signals: Option<
        std::sync::Arc<dyn crate::goal_curation::live_signal::LiveSignalSource>,
    > = if outcome_verify_brain.is_some() {
        Some(std::sync::Arc::new(
            crate::goal_curation::DaemonLiveSignals::new(repo_root.clone(), state_root.clone()),
        ))
    } else {
        None
    };

    let mut memories = OodaClients {
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
        outcome_verify_brain,
        live_signals,
    };

    // Issue #1: the authoritative goal board lives in
    // `<state_root>/state/goal_board.json` (a single durable, flock-guarded,
    // read-your-writes store), NOT the cognitive-memory snapshot. On first
    // adoption the current memory snapshot is migrated into the file so no live
    // goal is lost; thereafter the file is the source of truth and the memory
    // snapshot is a derived cache. The persisted `no_progress` tracker is
    // restored into `OodaState` so the no-progress breaker's per-goal counters
    // survive the daemon's periodic restarts (the production bug where the
    // counter reset to zero every ~hour before it could reach the threshold).
    let persistent = crate::goal_board_store::load_or_migrate(&state_root, &*memories.memory)
        .unwrap_or_default();
    let tombstones = crate::ooda_loop::load_tombstones(&state_root);
    let board = crate::goal_board_store::filter_tombstoned(persistent.board, &tombstones);
    // Issue #2589: self-heal any stale [OODA-SAFEGUARD] no-progress block an
    // older daemon build may have parked on a standing/perpetual goal, so a
    // continuous research goal is never left "needs human review" on startup.
    let board = crate::goal_board_store::heal_stale_no_progress_blocks(board);
    let mut state = OodaState::new(board);
    state.no_progress_tracker = persistent.no_progress;
    // #3125: resolve identity-scoped cognition (seed goals / target scope /
    // write-authority posture) once at boot, fail-closed. No identity => Simard
    // unchanged; a read-only identity seeds its own goals and takes the
    // observe-only Act branch.
    state.identity_cognition = resolve_daemon_identity_cognition();
    // Seed the OODA cycle counter from durable brain memory so the cycle number
    // reflects the brain's total lived cognition and CONTINUES across restarts,
    // instead of resetting to 1 on every daemon restart / deploy (issue #1).
    // `OodaState::new` leaves this at 0 (the fresh-brain default); the first
    // cycle's `+= 1` then makes a brand-new brain's first cycle #1. The
    // one-time report backfill (see `seed_cycle_count`) recovers the count from
    // the highest persisted `cycle_<N>.json` for a brain upgraded from a build
    // that never persisted the field, so it never dips to #1 for one deploy.
    state.cycle_count = seed_cycle_count(persistent.cycle_count, &state_root);
    // The daemon is the live production OODA loop, so it opts in to proactive
    // resource cleanup (disk/process reclamation). This side effect walks the
    // real filesystem and must never run under cargo-test, hence it is off in
    // `OodaConfig::default()` and enabled explicitly here. See `OodaConfig`.
    let config = OodaConfig {
        run_resource_cleanup: true,
        ..OodaConfig::default()
    };

    // Issue #1197: sweep orphaned engineer worktrees from prior crashed
    // daemons before starting the loop, so disk pressure doesn't accumulate.
    if let Ok(parent_repo) = std::env::current_dir() {
        match crate::engineer_worktree::sweep_orphaned_worktrees(&parent_repo, &state_root) {
            Ok(report) => {
                if report.is_noteworthy() {
                    daemon_log(
                        &state_root,
                        &format!(
                            "[simard] OODA daemon: swept {} orphan engineer worktree(s) {}",
                            report.removed_orphan_dirs.len(),
                            report.kept_summary(),
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

    // --- embedded Signal operator channel ----------------------------------
    // Fold the Signal channel into THIS daemon instead of a separate
    // `simard-signal.service` process (converge-to-single-daemon). It runs on a
    // dedicated background thread with a supervised reconnect-with-backoff loop,
    // panic-isolated so it can never crash or stall the authoritative OODA
    // cycle. DEFAULT-ON (opt-out via SIMARD_SIGNAL_ENABLED); dormant until a
    // usable `[signal]` config is present. The guard is kept alive for the
    // daemon's lifetime; the thread reads the shared `shutdown` flag.
    let _embedded_signal = spawn_embedded_signal_channel(&state_root, Arc::clone(&shutdown));
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
    // it snapshots the live store through the memory (so it inherently targets
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

    // ── Cognitive-thread scheduler (issue #2419 + #2647) ───────────────
    // ADDITIVE. A `Mind` hosts background cognitive threads on their own
    // cadence, subsuming the ad-hoc periodic-task pattern. OODA itself stays
    // driven by this loop's authoritative inline cycle below so its external
    // cadence and side-effects are byte-for-byte preserved; the scheduler is
    // invoked only AFTER the inline cycle and never gates it.
    //
    // Two INDEPENDENT gates share the one runtime (issue #4845 — both default-ON
    // opt-out now):
    //   * SIMARD_COGNITIVE_THREADS_ENABLED (default-ON, opt-out) owns the
    //     maintenance + engineer-log + ten reflective threads. It is enabled
    //     UNLESS set to an explicit falsy token (0/false/no/off).
    //   * SIMARD_CREATIVE_IDEAS_ENABLED (default-ON, opt-out) owns the Creative
    //     Ideas generator (issue #2647), consistent with the default-ON
    //     Overseer/Journal threads — so it runs on a stock deployment WITHOUT
    //     the generic master switch.
    let cognitive_threads_enabled = crate::cognitive_threads::recipe_rail::env_gate_open(
        std::env::var("SIMARD_COGNITIVE_THREADS_ENABLED")
            .ok()
            .as_deref(),
        None,
    );
    let creative_ideas_cfg = crate::creative_ideas::CreativeIdeasConfig::from_env();
    let creative_ideas_enabled = creative_ideas_cfg.enabled();
    let cognitive_repo_root = memories.repo_root.clone();
    // Build the shared runtime when EITHER gate wants a thread to run.
    let cognitive_runtime = if cognitive_threads_enabled || creative_ideas_enabled {
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
        // Maintenance + engineer-log + the ten reflective threads register under
        // the DEFAULT-ON master gate (issue #4845). Each thread's own per-thread
        // gate (default-ON opt-out) decides whether it actually ticks; the
        // scheduler skips any that report `!enabled()`, so registration is safe
        // and observable even when a thread is individually opted out.
        if cognitive_threads_enabled {
            mind.register(Box::new(
                crate::cognitive_threads::MaintenanceThread::from_env(),
            ));
            mind.register(Box::new(
                crate::cognitive_threads::EngineerLogAnalysisThread::from_env(),
            ));
            // Issue #5 + #4845: the ten reflective threads. Additive and now
            // ENABLED by default behind each thread's own opt-out gate. Recipes
            // over new plumbing.
            crate::cognitive_threads::threads::register_reflective_threads(
                &mut mind,
                &cognitive_repo_root,
                &state_root,
            );
        }
        // Creative Ideas generator thread (issue #2647): a divergent
        // idea-generation background thread, default-ON opt-out via
        // `SIMARD_CREATIVE_IDEAS_ENABLED`, INDEPENDENT of the generic master
        // switch above. The gate seam registers nothing when opted out.
        crate::cognitive_threads::threads::creative_ideas::register_creative_ideas_if_enabled(
            &mut mind,
            &creative_ideas_cfg,
        );
        // NOTE: the Overseer M1 read-only observer sensor that previously
        // registered here (default-OFF, `SIMARD_OVERSEER_ENABLED` truthy) is
        // SUPERSEDED by the acting Overseer periodic task driven below in the
        // main loop (default-ON). Running both would duplicate observations and
        // split the enable-gate, so the
        // acting co-process owns Observe→Orient→Decide→Act now. See
        // `crate::overseer::wiring`.
        daemon_log(
            &state_root,
            &format!(
                "[simard] OODA daemon: cognitive-thread scheduler ENABLED ({} background thread(s))",
                mind.len()
            ),
        );
        // Per-thread startup roster (issue #4845): one line per registered
        // thread so `ooda.log` shows exactly which threads are ENABLED (with
        // their cadence) or DISABLED (operator opt-out) — no thread is invisible.
        for h in mind.health() {
            let line = if h.enabled {
                match h.cadence_secs {
                    Some(secs) => format!(
                        "[simard] OODA daemon: cognitive thread '{}' ENABLED (interval={}s)",
                        h.id, secs
                    ),
                    None => format!(
                        "[simard] OODA daemon: cognitive thread '{}' ENABLED (reactive)",
                        h.id
                    ),
                }
            } else {
                format!(
                    "[simard] OODA daemon: cognitive thread '{}' DISABLED (operator opt-out)",
                    h.id
                )
            };
            daemon_log(&state_root, &line);
        }
    }
    // Creative-ideas startup line (mirrors the Journal thread) — logged
    // unconditionally so the operator can confirm the gate from journalctl.
    let creative_ideas_startup_line = if creative_ideas_enabled {
        format!(
            "[simard] OODA daemon: creative-ideas thread ENABLED (default) \
             (interval = {}s; SIMARD_CREATIVE_IDEAS_ENABLED opt-out)",
            creative_ideas_cfg.interval_secs
        )
    } else {
        "[simard] OODA daemon: creative-ideas thread DISABLED \
         (SIMARD_CREATIVE_IDEAS_ENABLED opt-out)"
            .to_string()
    };
    daemon_log(&state_root, &creative_ideas_startup_line);

    // ── Acting Overseer co-process (issue #2539 wiring) ─────────────────
    // ADDITIVE and DEFAULT-ON: the daemon drives the Overseer's meta-OODA loop
    // (Observe→Orient→Decide→Act) on its own cadence, in THIS process but on a
    // background thread so it never runs inside or blocks the authoritative OODA
    // cycle. The operator opts OUT with `SIMARD_OVERSEER_ENABLED=0`. A panic or
    // error in a tick is caught and logged and never crashes or stalls the
    // daemon (see `crate::overseer::wiring::run_overseer_tick_isolated`). The
    // Overseer runs under a DISTINCT anti-recursion identity so it never
    // verifies/merges/deploys its own PRs and never fights the OODA loop.
    let overseer_acting_enabled = crate::overseer::overseer_acting_enabled();
    let overseer_interval_secs = crate::overseer::overseer_tick_interval_secs();
    // Monotonic origin for the cadence; virtual seconds since daemon start.
    let overseer_epoch = Instant::now();
    let mut overseer_cadence = crate::overseer::OverseerCadence::new(overseer_interval_secs, 0);
    // Gap-scan cadence: run the backlog-coverage survey/act once every N Overseer
    // ticks (default every tick; clamped floor 1). Resolved once at startup; the
    // per-tick index throttles which ticks actually run the gap-scan.
    let overseer_gap_scan_enabled = crate::overseer::gap_scan_enabled();
    let overseer_gap_scan_every_n = crate::overseer::gap_scan_every_n();
    let mut overseer_gap_scan_tick_idx: u64 = 0;
    // Prevents overlapping ticks from stacking up if one runs long.
    let overseer_tick_running = Arc::new(AtomicBool::new(false));
    // #893: running count of CONSECUTIVE transient (self-healable) cycle
    // failures, owned by the daemon across ticks (each tick rebuilds the
    // Overseer on a fresh thread). Reset to 0 on any completed tick; incremented
    // on a transient failure; left unchanged on a fatal one. Bounded by
    // `overseer_transient_ceiling` — beyond it the meta-thread escalates from
    // "backoff" to "erroring" so a hard-down dependency can't hide forever.
    let overseer_consecutive_transient = Arc::new(AtomicU32::new(0));
    let overseer_transient_ceiling = crate::overseer::config::overseer_transient_backoff_ceiling();
    let overseer_repo_root = memories.repo_root.clone();
    daemon_log(
        &state_root,
        &format!(
            "[simard] OODA daemon: acting Overseer {} (interval = {overseer_interval_secs}s; \
             SIMARD_OVERSEER_ENABLED opt-out)",
            if overseer_acting_enabled {
                "ENABLED (default)"
            } else {
                "DISABLED"
            }
        ),
    );

    // ── Daily journal thread (issue #2606) ─────────────────────────────
    // DEFAULT-ON, opt-out via SIMARD_JOURNAL_ENABLED=0. On its own slow cadence
    // (default hourly) the daemon regenerates *today's* narrative engineering &
    // research report from episodic memory and the day's activity — including
    // the day's real code-change proposals pulled from the `gh pr list`
    // PR-readiness service — persisting it in cognitive memory (a
    // `journal:YYYY-MM-DD` fact). Because it touches the network it runs on a
    // background thread (never inline) AFTER the authoritative OODA cycle,
    // panic-isolated and overlap-guarded, so it can never stall or crash the
    // loop. The dashboard Journal tab and the TUI Journal pane read these
    // entries back.
    let journal_thread_enabled = crate::journal::journal_enabled();
    let journal_interval_secs = crate::journal::journal_interval_secs();
    let mut last_journal: Option<Instant> = None;
    // Overlap guard: the journal tick now runs on a background thread (it fetches
    // the day's PRs from the `gh pr list` PR-readiness service), so a slow tick
    // must not stack on top of the previous one.
    let journal_tick_running = Arc::new(AtomicBool::new(false));
    daemon_log(
        &state_root,
        &format!(
            "[simard] OODA daemon: daily journal {} (interval = {journal_interval_secs}s; \
             SIMARD_JOURNAL_ENABLED opt-out)",
            if journal_thread_enabled {
                "ENABLED (default)"
            } else {
                "DISABLED"
            }
        ),
    );

    // ── Cognitive-thread pass overlap guard (issue #5) ──────────────────
    // The reflective threads invoke agentic recipes via `recipe-runner-rs`,
    // which are minutes-long and BLOCKING (`Command::output`). Running the
    // scheduler pass inline on this loop thread would delay the NEXT
    // authoritative OODA cycle by the full recipe duration — the exact stall the
    // Overseer and Journal threads deliberately avoid. So the cognitive pass
    // runs on a background thread (below) AFTER the authoritative OODA cycle, and
    // this guard drops a tick if the previous pass is still running. `Mind` is
    // shared behind a mutex so the background pass can mutate its scheduler
    // bookkeeping while the main loop only ever `try_lock`s it for a read-only,
    // never-blocking health snapshot.
    let mind = Arc::new(std::sync::Mutex::new(mind));
    let cognitive_pass_running = Arc::new(AtomicBool::new(false));

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
            if let Some(ref mut session) = memories.session {
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
                crate::disk_health::emergency_cleanup(&memories.repo_root, &state_root)
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
            // Tier 2: agentic disk-health recipe (moderate pressure). The recipe
            // now *acts* through the `simard disk` tool (which enforces the
            // disk-safety heuristic internally) and prints no envelope — this is
            // a thin exit-status trigger (issue #4722). We log success/failure by
            // the recipe's exit status; there is no report to parse.
            match crate::disk_health::run_disk_health_check(&memories.repo_root, &state_root, None)
            {
                Ok(true) => daemon_log(&state_root, "[simard] disk health recipe: OK"),
                Ok(false) => daemon_log(
                    &state_root,
                    "[simard] WARN: disk health recipe reported failure (non-zero exit)",
                ),
                Err(e) => {
                    daemon_log(
                        &state_root,
                        &format!("[simard] WARN: disk health check failed: {e}"),
                    );
                }
            }
            // Tier 3 (issue #2704): agentic self-healing disk reclamation. A
            // cheap deterministic `df` `%-used` probe gates the launch; only when
            // usage crosses `SIMARD_DISK_RECLAIM_PCT` do we invoke the agentic
            // reclaim capability (an analysis agent proposes candidates, a
            // deterministic Rust executor disposes behind hard safety rails).
            // Ships in dry-run + human-review by default: the daemon only deletes
            // when `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`. This supersedes the
            // ad-hoc per-cycle disk-guard heuristics — no more re-deriving
            // cleanup logic in scheduler prompts.
            {
                use crate::disk_pressure::check::DiskStatProvider as _;
                let reclaim_pct = crate::disk_reclaim::reclaim_pct_from_env();
                let used_now = crate::disk_pressure::RealDiskStatProvider
                    .stat(&state_root)
                    .ok()
                    .and_then(|s| crate::disk_pressure::used_pct(&s))
                    .map(|p| p.round().clamp(0.0, 100.0) as u8);
                match used_now {
                    Some(used) if crate::disk_reclaim::daemon_should_trigger(used, reclaim_pct) => {
                        let mode = crate::disk_reclaim::daemon_apply_from_env();
                        match crate::disk_reclaim::run_disk_reclaim(
                            &memories.repo_root,
                            &state_root,
                            None,
                            mode,
                            reclaim_pct,
                            crate::disk_reclaim::ReclaimSource::Daemon,
                        ) {
                            Ok(report) => {
                                daemon_log(&state_root, &format!("[simard] {}", report.summary()));
                            }
                            Err(e) => daemon_log(
                                &state_root,
                                &format!("[simard] WARN: disk reclaim failed: {e}"),
                            ),
                        }
                    }
                    Some(used) => daemon_log(
                        &state_root,
                        &format!(
                            "[simard] disk reclaim: {used}% used, under threshold ({reclaim_pct}%), no run"
                        ),
                    ),
                    None => daemon_log(
                        &state_root,
                        "[simard] WARN: disk reclaim probe could not read disk usage; skipping",
                    ),
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
                        if report.is_noteworthy() {
                            daemon_log(
                                &state_root,
                                &format!(
                                    "[simard] periodic sweep: removed {} orphan engineer \
                                     worktree(s) {}",
                                    report.removed_orphan_dirs.len(),
                                    report.kept_summary(),
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
                &*memories.memory,
                &memories.repo_root,
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
                &memories.repo_root,
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

        // ── Issue #1: re-sync the authoritative goal board at cycle start ──
        // Load the single source of truth (`goal_board.json`) so operator
        // mutations (`goal add/remove/complete/reprioritize`) and meeting
        // handoffs made since the last cycle take effect and STICK. Restore the
        // persisted no-progress tracker so the breaker's per-goal counters
        // survive the daemon's periodic exec-reload restarts, and overwrite the
        // cognitive-memory snapshot cache so `run_ooda_cycle`'s internal board
        // read sees exactly the authoritative (tombstone-filtered) board.
        {
            let cycle_tombstones = crate::ooda_loop::load_tombstones(&state_root);
            let persistent = crate::goal_board_store::load(&state_root);
            // Issue #2589: self-heal any stale [OODA-SAFEGUARD] no-progress block
            // an older daemon build parked on a standing/perpetual goal. This
            // per-cycle heal is load-bearing: the board is re-read from disk each
            // cycle, so a startup-only heal would be undone here (disk still says
            // Blocked) and a Blocked goal is never dispatched — so the runtime
            // exemption could never fire. Healing before `overwrite_memory_cache`
            // makes the healed board reach the snapshot `run_ooda_cycle` reads;
            // the cleared status is persisted by the next `commit_cycle`.
            state.active_goals = crate::goal_board_store::heal_stale_no_progress_blocks(
                crate::goal_board_store::filter_tombstoned(persistent.board, &cycle_tombstones),
            );
            state.no_progress_tracker = persistent.no_progress;
            if let Err(e) =
                crate::goal_curation::overwrite_memory_cache(&state.active_goals, &*memories.memory)
            {
                daemon_log(
                    &state_root,
                    &format!("[simard] OODA cycle: memory cache sync failed: {e}"),
                );
            }
            // Issue #4232: reap any in-flight engineer whose goal was
            // removed/completed (tombstoned) since the last cycle. Tombstone-
            // gated, never a wall-clock timeout — a healthy engineer whose goal
            // is still on the board is never touched. Reuses `kill_subordinate`
            // (SIGTERM) + the existing worktree/claim cleanup chokepoint. Runs
            // here, inside the block where `cycle_tombstones` is in scope, so it
            // reuses the already-loaded tombstone set. The registry disk-read +
            // JSON parse is gated behind a cheap in-memory check so the common
            // steady-state cycle (nothing tombstoned) pays no extra I/O.
            let reaped_goal_ids = if crate::ooda_actions::advance_goal::has_tombstoned_engineer(
                &state,
                &cycle_tombstones,
            ) {
                let subagent_registry = crate::subagent_sessions::load();
                crate::ooda_actions::advance_goal::reap_engineers_for_tombstoned_goals(
                    &mut state,
                    &cycle_tombstones,
                    &subagent_registry,
                )
            } else {
                Vec::new()
            };
            if !reaped_goal_ids.is_empty() {
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] OODA cycle: reaped {} in-flight engineer(s) for \
                         tombstoned goal(s): {}",
                        reaped_goal_ids.len(),
                        reaped_goal_ids.join(", "),
                    ),
                );
            }
        }
        // Snapshot the pre-cycle active ids so the post-cycle commit can
        // tombstone any goal that left the board (archived / dropped / done).
        let pre_cycle_active_ids: std::collections::HashSet<String> = state
            .active_goals
            .active
            .iter()
            .map(|g| g.id.clone())
            .collect();

        // Write heartbeat at cycle START so the dashboard never sees "stale"
        // during a long-running cycle.
        {
            let health_dir = dirs::data_local_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
                .join("simard");
            let _ = std::fs::create_dir_all(&health_dir);
            let heartbeat = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                // Brain-relative cycle number (issue #1): this runs BEFORE
                // `run_ooda_cycle` does `state.cycle_count += 1`, so the cycle
                // about to run is `state.cycle_count + 1`. Derived from the
                // durable counter so a restart keeps the accumulated number
                // instead of resetting the dashboard to "#1".
                "cycle_number": state.cycle_count + 1,
                "status": "running",
                "cycle_phase": state.current_phase.to_string(),
                // Additive, observability-only (issue #4929): stamp the daemon's
                // own PID so `simard status` can sample /proc/<pid> RSS + CPU
                // instead of rendering "daemon CPU / RSS absent".
                "main_pid": std::process::id(),
                "cycle_start_epoch": cycle_start_epoch,
                "interval_secs": interval_secs,
                "actions_taken": format!("Starting cycle #{}", state.cycle_count + 1),
            });
            let _ = std::fs::write(
                health_dir.join("daemon_health.json"),
                serde_json::to_string_pretty(&heartbeat).unwrap_or_default(),
            );
        }

        match run_ooda_cycle(&mut state, &mut memories, &config) {
            Ok(report) => {
                let cycle_elapsed = cycle_start.elapsed();
                let summary = summarize_cycle_report(&report);
                state.last_cycle_summary = Some(summary.clone());
                state.last_cycle_duration_secs = Some(cycle_elapsed.as_secs());
                state.current_phase = OodaPhase::Sleep;
                daemon_log(&state_root, &format!("[simard] {summary}"));

                // ── Issue #1: commit the post-cycle board authoritatively ──
                // 1. Run the every-cycle done-gate over the WHOLE active board
                //    (cross-repo aware) so a goal whose objective was completed
                //    out-of-band — a merged PR / closed issue on ANY governed
                //    repo — is auto-completed instead of being re-litigated
                //    forever. 2. Tombstone every goal that left the board this
                //    cycle (archived, dropped by the no-progress breaker, or
                //    just-completed by the done-gate) so nothing re-seeds it.
                //    3. Commit the reconciled board + persisted no-progress
                //    tracker to the authoritative store, then regenerate the
                //    memory cache from the committed board.
                {
                    let mut newly_done: Vec<String> = Vec::new();
                    if let Some(evidence) = &memories.completion_evidence {
                        newly_done = crate::goal_board_store::sweep_done_goals(
                            &mut state.active_goals,
                            evidence.as_ref(),
                        );
                        if !newly_done.is_empty() {
                            let done: std::collections::HashSet<&str> =
                                newly_done.iter().map(String::as_str).collect();
                            state
                                .active_goals
                                .active
                                .retain(|g| !done.contains(g.id.as_str()));
                            daemon_log(
                                &state_root,
                                &format!(
                                    "[simard] OODA done-gate: auto-completed {} goal(s) with cross-repo evidence: {}",
                                    newly_done.len(),
                                    newly_done.join(", "),
                                ),
                            );
                        }
                    }

                    let post_active: std::collections::HashSet<&str> = state
                        .active_goals
                        .active
                        .iter()
                        .map(|g| g.id.as_str())
                        .collect();
                    let post_backlog: std::collections::HashSet<&str> = state
                        .active_goals
                        .backlog
                        .iter()
                        .map(|b| b.id.as_str())
                        .collect();
                    let mut tombstones: Vec<String> = pre_cycle_active_ids
                        .iter()
                        .filter(|id| {
                            !post_active.contains(id.as_str())
                                && !post_backlog.contains(id.as_str())
                        })
                        .cloned()
                        .collect();
                    tombstones.extend(newly_done);

                    match crate::goal_board_store::commit_cycle(
                        &state_root,
                        &state.active_goals,
                        &state.no_progress_tracker,
                        state.cycle_count,
                        &tombstones,
                    ) {
                        Ok(committed) => {
                            state.active_goals = committed;
                            if let Err(e) = crate::goal_curation::overwrite_memory_cache(
                                &state.active_goals,
                                &*memories.memory,
                            ) {
                                daemon_log(
                                    &state_root,
                                    &format!(
                                        "[simard] OODA cycle: memory cache refresh failed: {e}"
                                    ),
                                );
                            }
                        }
                        Err(e) => daemon_log(
                            &state_root,
                            &format!("[simard] OODA cycle: authoritative commit failed: {e}"),
                        ),
                    }
                }

                // Persist the cycle report to filesystem for auditability.
                // Record the wall-clock cycle duration so the Cycle History
                // duration-trend chart has real data to render (issue #21).
                persist_cycle_report_timed(&state_root, &report, Some(cycle_elapsed));
                // Persist the cycle summary to cognitive memory as an episode.
                persist_cycle_to_memory(&memories, &report);
                // Write daemon health file for dashboard
                {
                    let health_dir = dirs::data_local_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
                        .join("simard");
                    let _ = std::fs::create_dir_all(&health_dir);
                    let health = serde_json::json!({
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                        // Brain-relative cycle number (issue #1): `run_ooda_cycle`
                        // has already advanced `state.cycle_count`, so this is the
                        // durable count of the cycle just completed — the single
                        // authoritative "Cycle #N" the dashboard renders.
                        "cycle_number": state.cycle_count,
                        "status": "healthy",
                        "cycle_phase": "sleep",
                        // Additive, observability-only (issue #4929): see the
                        // cycle-start heartbeat above.
                        "main_pid": std::process::id(),
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

                // Bounded WAL-retention cadence (issue #4929): checkpoint the
                // cognitive store every N cycles so the LadybugDB WAL is
                // compacted into the main file regularly instead of growing for
                // the daemon's whole uptime (which inflates replay work and the
                // `cognitive.wal.corrupt` rotation surface). Adapter-scoped —
                // `amplihack-memory-lib` is untouched. Failures are surfaced via
                // structured tracing, never silently swallowed.
                {
                    const WAL_CHECKPOINT_EVERY_CYCLES: u32 = 20;
                    if state.cycle_count > 0
                        && state
                            .cycle_count
                            .is_multiple_of(WAL_CHECKPOINT_EVERY_CYCLES)
                    {
                        match shared_mem.checkpoint() {
                            Ok(()) => tracing::debug!(
                                cycle = state.cycle_count,
                                cadence = WAL_CHECKPOINT_EVERY_CYCLES,
                                "cognitive-store WAL checkpoint (bounded retention cadence)"
                            ),
                            Err(e) => tracing::warn!(
                                cycle = state.cycle_count,
                                error = %e,
                                "cognitive-store WAL checkpoint failed; WAL retention not compacted this cadence"
                            ),
                        }
                    }
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
                    if let Ok(stats) = memories.memory.get_statistics() {
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
                    if let Ok(g) = memories.memory.graph_stats() {
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
                        // Emit the durable graph-memory grounding-coverage
                        // self-metric from the SAME snapshot (no extra store
                        // read): fraction of semantic facts connected into the
                        // DERIVES_FROM provenance graph. Turns a grounding
                        // regression — facts entering semantic memory without a
                        // provenance edge — into a comparable, regressable
                        // `metrics.jsonl` series instead of only raw edge-count
                        // gauges. Best-effort; no-op on an empty store.
                        crate::cognitive_memory::metrics::record_provenance_coverage_metric(
                            g.facts_with_provenance,
                            g.facts_total,
                        );
                    }
                    // Flush the metrics snapshot with the per-cycle enrichment
                    // rollup section attached (issue #2942) so the dashboard's
                    // `GET /api/enrichment` live-read sees the attach-rate and
                    // average injected facts/procedures per decision. The rollup
                    // is read (not drained) so the dashboard shows a stable
                    // lifetime figure; reading it never touches the recall corpus.
                    if let Err(e) = telemetry::flush_snapshot_with(
                        &state_root,
                        crate::enrichment_observability::snapshot_section(),
                    ) {
                        eprintln!("[simard] telemetry: failed to flush metrics snapshot: {e}");
                    }
                }
            }
            Err(e) => {
                daemon_log(&state_root, &format!("[simard] OODA cycle error: {e}"));
            }
        }
        // Flush this cycle's aggregated ranked-recall precision@k into the
        // durable `recall_precision_at_k` series once per cycle, regardless of
        // cycle outcome. Draining unconditionally here (not inside the `Ok` arm)
        // ensures a cycle that recalled and then errored cannot bleed its
        // observations into the next successful cycle's emission. Best-effort;
        // no-op when no ranked recall ran this cycle.
        crate::cognitive_memory::metrics::flush_recall_precision_metric();

        cycles_run += 1;

        // ── Cognitive-thread scheduler tick (issue #2419 / #5) ──────────
        // Runs AFTER the authoritative OODA cycle so OODA is never starved, and
        // on a BACKGROUND thread — never inline — because the reflective threads
        // invoke minutes-long blocking agentic recipes; running them on this loop
        // thread would delay the next OODA cycle by the full recipe duration. An
        // overlap guard drops this tick if the previous cognitive pass is still
        // running, so passes never stack. A thread erroring or panicking is
        // caught and backed off inside `run_due`, never taking down the daemon or
        // the OODA loop. No-op unless explicitly enabled.
        if let Some(ref rt) = cognitive_runtime
            && cognitive_pass_running
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            let now_epoch = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let running = Arc::clone(&cognitive_pass_running);
            let mind_for_pass = Arc::clone(&mind);
            let state_root_for_pass = state_root.clone();
            let repo_root_for_pass = cognitive_repo_root.clone();
            let mem_for_pass = Arc::clone(&shared_mem);
            let shutdown_for_pass = Arc::clone(&shutdown);
            let rt_handle = rt.handle().clone();
            let spawn = std::thread::Builder::new()
                .name("cognitive-threads".to_string())
                .spawn(move || {
                    // Always clear the overlap guard, even on panic.
                    struct ClearOnDrop(Arc<AtomicBool>);
                    impl Drop for ClearOnDrop {
                        fn drop(&mut self) {
                            self.0.store(false, Ordering::SeqCst);
                        }
                    }
                    let _clear = ClearOnDrop(running);

                    // Build the context from owned/Arc'd data so it outlives
                    // the pass on this thread (the references point at these
                    // locals). OODA (Critical) is NOT a Mind thread here — it
                    // runs on the authoritative inline cycle above — so every
                    // thread on this Mind is non-critical background work.
                    let mut ctx = crate::cognitive_threads::ThreadContext {
                        state_root: &state_root_for_pass,
                        repo_root: &repo_root_for_pass,
                        memory: mem_for_pass.as_ref(),
                        runtime: rt_handle,
                        shutdown: &shutdown_for_pass,
                        now_epoch,
                        dry_run: false,
                    };
                    // A poisoned lock means a prior pass panicked OUTSIDE
                    // run_due's per-thread catch_unwind (rare); skip this pass
                    // rather than propagate — the next tick retries.
                    let outcomes = match mind_for_pass.lock() {
                        Ok(mut m) => m.run_due(&mut ctx),
                        Err(_) => Vec::new(),
                    };
                    for outcome in outcomes {
                        if outcome.ran {
                            daemon_log(
                                &state_root_for_pass,
                                &format!("[simard] cognitive-thread: {}", outcome.summary),
                            );
                        }
                    }
                });
            if spawn.is_err() {
                // Could not spawn the pass thread; clear the guard so the next
                // tick can retry (the ClearOnDrop guard never armed).
                cognitive_pass_running.store(false, Ordering::SeqCst);
            }
        }
        // else: previous cognitive pass still running — drop this tick so the
        // OODA loop never waits on a long recipe.

        // ── Acting Overseer meta-OODA tick (issue #2539 wiring) ─────────
        // DEFAULT-ON. Fires on its own cadence, AFTER the authoritative OODA
        // cycle so OODA is never starved. Runs on a background thread — never
        // inline — so a long (network-bound) tick cannot block or stall the
        // OODA loop; an overlap guard drops a tick if the previous one is still
        // running. The tick builds a fresh Overseer under the DISTINCT identity
        // and runs panic-isolated, so a panic/error is caught and logged and
        // never crashes the daemon.
        if overseer_acting_enabled {
            let now_secs = overseer_epoch.elapsed().as_secs();
            if overseer_cadence.due(now_secs)
                && overseer_tick_running
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                let running = Arc::clone(&overseer_tick_running);
                let consecutive_transient_counter = Arc::clone(&overseer_consecutive_transient);
                let transient_ceiling = overseer_transient_ceiling;
                let mem_for_tick = Arc::clone(&shared_mem);
                let repo_root_for_tick = overseer_repo_root.clone();
                let state_root_for_tick = state_root.clone();
                // Gap-scan every-N cadence: the first tick (idx 0) always runs the
                // scan, then it runs once every N ticks. Disabled entirely when the
                // gap-scan is off. Computed on the main loop thread so the index
                // persists across ticks (each tick rebuilds the Overseer).
                let gap_scan_due = overseer_gap_scan_enabled
                    && overseer_gap_scan_tick_idx.is_multiple_of(overseer_gap_scan_every_n);
                overseer_gap_scan_tick_idx = overseer_gap_scan_tick_idx.wrapping_add(1);
                // Capture the cognitive-thread heartbeats + feed context on the
                // main loop thread (before the tick spawns) so the Overseer
                // activity feed (#2419) lists every operator/steward thread, not
                // just the Overseer meta-thread. `try_lock` so a long in-flight
                // cognitive pass never blocks this loop — the feed simply omits
                // cognitive heartbeats for that tick (advisory, read-only).
                let thread_healths = mind.try_lock().map(|m| m.health()).unwrap_or_default();
                let feed_cadence_secs = crate::overseer::config::overseer_interval_secs();
                let feed_author_login = crate::overseer::config::overseer_author_login();
                let spawn = std::thread::Builder::new()
                    .name("overseer-tick".to_string())
                    .spawn(move || {
                        // Always clear the overlap guard, even on panic.
                        struct ClearOnDrop(Arc<AtomicBool>);
                        impl Drop for ClearOnDrop {
                            fn drop(&mut self) {
                                self.0.store(false, Ordering::SeqCst);
                            }
                        }
                        let _clear = ClearOnDrop(running);

                        // Apply the gap-scan cadence for THIS tick on top of the
                        // config default build_overseer sets.
                        let overseer = crate::overseer::build_overseer(
                            mem_for_tick,
                            repo_root_for_tick,
                            state_root_for_tick.clone(),
                        );
                        let mut overseer = overseer
                            .with_gap_scan_enabled(gap_scan_due)
                            // Cognitive-thread oversight (#4786): inject this
                            // tick's single-source-of-truth thread registry
                            // (name + purpose + cadence) captured from
                            // `Mind::health()` above, so the deterministic
                            // oversight pass can reason about each thread.
                            .with_thread_registry(thread_healths.clone());
                        let (report, problem_entries) =
                            crate::overseer::run_overseer_tick_isolated_detailed(&mut overseer);
                        daemon_log(
                            &state_root_for_tick,
                            &format!(
                                "[simard] overseer tick: problems={} issues_filed={} \
                                 recipes_launched={} prs_merged={} deploys={} escalations={} \
                                 held={} goals_unblocked={} goals_escalated={} \
                                 memory_recalls={} memory_writes={} memory_errors={} \
                                 workstream_gaps_detected={} workstream_gaps_suppressed={} \
                                 workstream_gaps_reused_existing={} \
                                 errors={} panicked={} cycle_failed={} ({}ms)",
                                report.problems,
                                report.issues_filed,
                                report.recipes_launched,
                                report.prs_merged,
                                report.deploys,
                                report.escalations,
                                report.held,
                                report.goals_unblocked,
                                report.goals_escalated,
                                report.memory_recalls,
                                report.memory_writes,
                                report.memory_errors,
                                report.workstream_gaps_detected,
                                report.workstream_gaps_suppressed,
                                report.workstream_gaps_reused_existing,
                                report.errors,
                                report.panicked,
                                report.cycle_failed,
                                report.duration_ms,
                            ),
                        );

                        // Record this tick into the durable, cross-process
                        // Overseer activity feed (#2419) so the dashboard, TUI,
                        // and `simard status` can surface what the steward has
                        // been doing. Read-only surfacing: this only *records*
                        // the outcome that already happened; it never changes
                        // the Overseer's decisions. Write failure is non-fatal —
                        // the tick already completed — so it is logged and the
                        // daemon continues.
                        let mut feed_threads = Vec::with_capacity(thread_healths.len() + 1);
                        // #893: update the consecutive-transient self-heal
                        // counter from this tick, then derive the meta-thread's
                        // health. Reset on a completed tick; increment on a
                        // transient failure (count INCLUDES this tick); leave
                        // unchanged on a fatal failure.
                        let cycle_failed = report.panicked || report.cycle_failed;
                        let consecutive_transient = if !cycle_failed {
                            consecutive_transient_counter.store(0, Ordering::SeqCst);
                            0
                        } else if report.transient_cycle_failure {
                            consecutive_transient_counter.fetch_add(1, Ordering::SeqCst) + 1
                        } else {
                            consecutive_transient_counter.load(Ordering::SeqCst)
                        };
                        feed_threads.push(
                            crate::overseer::activity::OverseerThreadStatus::overseer_meta(
                                feed_cadence_secs,
                                &report,
                                consecutive_transient,
                                transient_ceiling,
                            ),
                        );
                        for h in &thread_healths {
                            feed_threads.push(
                                crate::overseer::activity::OverseerThreadStatus::from_thread_health(
                                    h,
                                ),
                            );
                        }
                        let feed_record = crate::overseer::activity::OverseerActivityRecord {
                            timestamp: crate::telemetry::snapshot::now_rfc3339(),
                            enabled: true,
                            report,
                            problem_entries,
                        };
                        if let Err(e) = crate::overseer::activity::record_tick(
                            &state_root_for_tick,
                            feed_record,
                            feed_threads,
                            true,
                            feed_cadence_secs,
                            &feed_author_login,
                        ) {
                            daemon_log(
                                &state_root_for_tick,
                                &format!("[simard] WARN: overseer activity feed write failed: {e}"),
                            );
                        }
                    });
                if let Err(e) = spawn {
                    // Spawn failed — clear the guard so the next cadence retries.
                    overseer_tick_running.store(false, Ordering::SeqCst);
                    daemon_log(
                        &state_root,
                        &format!("[simard] WARN: overseer tick thread spawn failed: {e}"),
                    );
                }
            }
        }

        // ── Daily journal rolling tick (issue #2606) ────────────────────
        // Default-on, interval-gated, overlap-guarded, panic-isolated.
        // Regenerates today's diary entry from episodic memory + the day's real
        // code-change proposals and persists it under the day key (idempotent
        // rolling update). The proposal table is fetched from the `gh pr list`
        // PR-readiness service, so the tick runs on a background thread — never
        // inline — and that network fetch can never stall the OODA loop; an
        // overlap guard drops a tick if the previous one is still running, and a
        // panic/error is caught and logged. Fires on the first iteration so a
        // fresh daemon writes the day's entry promptly, then on its slow cadence.
        if journal_thread_enabled {
            let due = last_journal
                .map(|t| t.elapsed().as_secs() >= journal_interval_secs)
                .unwrap_or(true);
            if due
                && journal_tick_running
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                let running = Arc::clone(&journal_tick_running);
                let mem_for_journal = Arc::clone(&shared_mem);
                let state_root_for_journal = state_root.clone();
                let repo_root_for_journal = memories.repo_root.clone();
                let spawn = std::thread::Builder::new()
                    .name("journal-tick".to_string())
                    .spawn(move || {
                        // Always clear the overlap guard, even on panic.
                        struct ClearOnDrop(Arc<AtomicBool>);
                        impl Drop for ClearOnDrop {
                            fn drop(&mut self) {
                                self.0.store(false, Ordering::SeqCst);
                            }
                        }
                        let _clear = ClearOnDrop(running);

                        // Wrap the real `gh pr list` PR-readiness service behind
                        // the journal's PrListSource seam; it degrades honestly
                        // to an empty proposal table on a `gh` failure.
                        let gh = crate::stewardship::RealPrGhClient::new();
                        let repo = crate::stewardship::TargetRepo::Simard.slug();
                        let base_allowlist = crate::stewardship::base_allowlist_from_env();
                        let prs = crate::journal::GhPrListSource::new(&gh, repo, base_allowlist);

                        let tick = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            crate::journal::run_journal_tick_with_prs_in_repo(
                                mem_for_journal.as_ref(),
                                &crate::journal::SystemClock,
                                &prs,
                                &repo_root_for_journal,
                            )
                        }));
                        match tick {
                            Ok(Ok(entry)) => daemon_log(
                                &state_root_for_journal,
                                &format!(
                                    "[simard] journal: entry for {} regenerated ({} proposal(s), quiet={})",
                                    entry.date,
                                    entry.prs.len(),
                                    entry.quiet_day
                                ),
                            ),
                            Ok(Err(e)) => daemon_log(
                                &state_root_for_journal,
                                &format!("[simard] WARN: journal tick failed: {e}"),
                            ),
                            Err(_) => daemon_log(
                                &state_root_for_journal,
                                "[simard] WARN: journal tick panicked (isolated; loop continues)",
                            ),
                        }

                        // ── Past-day merged-PR reconciliation (#4225) ───────
                        // Once a day passes its entry freezes, so a PR that
                        // merged after the day's final tick — or an entry that
                        // froze before the #4140 merged-PR wiring shipped —
                        // leaves the dashboard reporting `merged: 0` forever.
                        // Fold each recent past day's REAL merges back into its
                        // frozen entry through a deliberately merged-only seam
                        // (so a backfill can never graft today's still-open PRs
                        // onto a historical day). Panic-isolated like the tick;
                        // a `gh` blip degrades honestly per day rather than
                        // failing. Today is never touched — that stays the live
                        // tick's job above.
                        let recon = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let merged_src = crate::journal::GhMergedPrSource::new(&gh, repo);
                            crate::journal::reconcile_recent_days(
                                mem_for_journal.as_ref(),
                                &merged_src,
                                chrono::Utc::now().date_naive(),
                                crate::journal::reconcile_lookback_days(),
                            )
                        }));
                        match recon {
                            Ok(Ok(rep)) if rep.days_updated > 0 || rep.days_degraded > 0 => {
                                daemon_log(
                                    &state_root_for_journal,
                                    &format!(
                                        "[simard] journal: reconciled past days (examined={}, updated={}, degraded={})",
                                        rep.days_examined, rep.days_updated, rep.days_degraded
                                    ),
                                )
                            }
                            // Nothing to backfill — stay quiet so the daemon log
                            // is not spammed on every cadence.
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => daemon_log(
                                &state_root_for_journal,
                                &format!("[simard] WARN: journal reconciliation failed: {e}"),
                            ),
                            Err(_) => daemon_log(
                                &state_root_for_journal,
                                "[simard] WARN: journal reconciliation panicked (isolated; loop continues)",
                            ),
                        }
                    });
                if let Err(e) = spawn {
                    // Spawn failed — clear the guard so the next cadence retries.
                    journal_tick_running.store(false, Ordering::SeqCst);
                    daemon_log(
                        &state_root,
                        &format!("[simard] WARN: journal tick thread spawn failed: {e}"),
                    );
                }
                last_journal = Some(Instant::now());
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
    // close session, then drop memories (triggers Database::drop ->
    // force_checkpoint_on_close). Errors at this point only get warned —
    // we are exiting anyway and cannot recover.
    if let Err(e) = shutdown_daemon(
        &state_root,
        &shared_mem,
        &mut state,
        &mut memories,
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
/// 5. Drop the caller-owned memories (the daemon's `memories.memory` Box,
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
    memories: &mut OodaClients,
    signal_driven: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    daemon_log(state_root, "[simard] OODA daemon: shutdown sequence start");

    // 0. Commit the board + persisted no-progress tracker to the authoritative
    //    store (issue #1) so the last live state — including the breaker's
    //    counters — survives the restart. Best-effort; a failure is logged.
    if let Err(e) = crate::goal_board_store::commit_cycle(
        state_root,
        &state.active_goals,
        &state.no_progress_tracker,
        state.cycle_count,
        &[],
    ) {
        daemon_log(
            state_root,
            &format!("[simard] shutdown: authoritative goal-board commit failed: {e}"),
        );
    }

    // 1. Persist the goal board through the live writer.
    if let Err(e) = persist_board(&state.active_goals, &*memories.memory) {
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
    if let Some(ref mut session) = memories.session
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

    // 5. Clients (and the daemon-owned strong Arc to LibraryCognitiveMemory)
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
    use crate::cognitive_memory::CognitiveMemoryOps;
    use crate::goal_curation::GoalBoard;
    use crate::gym_client::GymClient;
    use crate::knowledge_client::KnowledgeClient;
    use crate::memory_client::CognitiveMemoryClient;
    use crate::ooda_loop::{OodaClients, OodaState};
    use crate::rpc::RpcErrorPayload;
    use crate::rpc_transport::InMemoryRpcTransport;
    use serde_json::json;

    fn mock_memory() -> Box<dyn CognitiveMemoryOps> {
        Box::new(CognitiveMemoryClient::new(Box::new(
            InMemoryRpcTransport::new("test-daemon-shutdown", |method, _params| match method {
                "memory.search_facts" => Ok(json!({"facts": []})),
                "memory.store_fact" => Ok(json!({"id": "sem_1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_1"})),
                "memory.get_statistics" => Ok(json!({
                    "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                    "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
                })),
                _ => Err(RpcErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            }),
        )))
    }

    fn mock_shared_mem() -> Arc<dyn CognitiveMemoryOps> {
        Arc::new(CognitiveMemoryClient::new(Box::new(
            InMemoryRpcTransport::new("test-daemon-shared", |method, _params| match method {
                "memory.search_facts" => Ok(json!({"facts": []})),
                "memory.store_fact" => Ok(json!({"id": "sem_1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_1"})),
                "memory.get_statistics" => Ok(json!({
                    "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                    "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
                })),
                _ => Err(RpcErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            }),
        )))
    }

    fn mock_knowledge() -> KnowledgeClient {
        KnowledgeClient::new(Box::new(InMemoryRpcTransport::new(
            "test-knowledge",
            |method, _params| match method {
                "knowledge.list_packs" => Ok(json!({"packs": []})),
                _ => Err(RpcErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            },
        )))
    }

    fn mock_gym() -> GymClient {
        GymClient::new(Box::new(InMemoryRpcTransport::new(
            "test-gym",
            |_method, _params| Ok(json!({"suite_id": "test", "success": true})),
        )))
    }

    fn test_memories() -> OodaClients {
        OodaClients {
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
            outcome_verify_brain: None,
            live_signals: None,
        }
    }

    #[test]
    fn boot_clear_removes_stale_draining_flag_without_exec_handover() {
        let state = tempfile::tempdir().unwrap();
        let log_root = tempfile::tempdir().unwrap();
        crate::safe_update::drain::mark_draining(state.path()).unwrap();

        clear_stale_draining_flag_at_boot_in(state.path(), log_root.path());

        assert!(!crate::safe_update::draining_flag_path(state.path()).exists());
    }

    #[test]
    fn boot_clear_preserves_draining_flag_during_exec_handover() {
        let state = tempfile::tempdir().unwrap();
        let log_root = tempfile::tempdir().unwrap();
        crate::safe_update::drain::mark_draining(state.path()).unwrap();
        let status = crate::safe_update::state::UpgradeStatus::exec_handover(
            Some("new".into()),
            Some("old".into()),
            1,
            60,
        );
        crate::safe_update::state::write_status(state.path(), &status).unwrap();

        clear_stale_draining_flag_at_boot_in(state.path(), log_root.path());

        assert!(crate::safe_update::draining_flag_path(state.path()).exists());
    }

    #[test]
    fn startup_purge_removes_stale_actor_session_but_preserves_live_scope_guard() {
        use crate::typed_ooda::{
            ActionKind, AuthenticatedToolContext, CapabilityErrorCode, CapabilityGrant,
            CapabilityHandler, CapabilityPolicy, RepositoryRef,
        };

        let state = tempfile::tempdir().expect("state root");
        let ledger_path = crate::typed_ooda::ledger_path(state.path());
        std::fs::create_dir_all(ledger_path.parent().expect("ledger parent"))
            .expect("create ledger directory");
        let actor = |cycle_id: &str, observe_only: bool| {
            AuthenticatedToolContext::new(
                "goal-session-actor",
                "ooda-stable-goal-session",
                [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
            )
            .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
            .bound_to_cycle_goal(cycle_id, "goal-perpetual")
            .with_engineer_permissions(["repo_read"])
            .with_observe_only(observe_only)
        };
        let lease = Duration::from_secs(30 * 24 * 60 * 60);

        let prior_process =
            CapabilityHandler::open(&ledger_path, CapabilityPolicy::new("policy-v1"))
                .expect("open prior-process ledger");
        prior_process
            .register_actor_session(
                &actor("cycle-before-restart", false),
                "request-before-restart",
                "cycle-before-restart",
                "goal-perpetual",
                lease,
            )
            .expect("persist future-dated prior-process lease");
        drop(prior_process);

        purge_actor_sessions_on_startup(state.path()).expect("startup purge must succeed");

        let current_process =
            CapabilityHandler::open(&ledger_path, CapabilityPolicy::new("policy-v1"))
                .expect("reopen ledger after startup purge");
        current_process
            .register_actor_session(
                &actor("cycle-after-restart", true),
                "request-after-restart",
                "cycle-after-restart",
                "goal-perpetual",
                lease,
            )
            .expect("startup purge must allow the stable session under its new scope");

        let error = current_process
            .register_actor_session(
                &actor("cycle-live-scope-change", false),
                "request-live-scope-change",
                "cycle-live-scope-change",
                "goal-perpetual",
                lease,
            )
            .expect_err("a live scope change must still be rejected");
        assert_eq!(
            error.code(),
            CapabilityErrorCode::AuthorizationScopeViolation
        );
    }

    // ── shutdown_daemon ─────────────────────────────────────────────

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn shutdown_daemon_succeeds_with_empty_state() {
        let hermetic = crate::test_support::HermeticState::new();
        let dir = hermetic.state_root();
        let shared_mem = mock_shared_mem();
        let mut state = OodaState::new(GoalBoard::new());
        let mut memories = test_memories();

        let result = shutdown_daemon(dir, &shared_mem, &mut state, &mut memories, false);
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
        let mut memories = test_memories();

        let _ = shutdown_daemon(dir, &shared_mem, &mut state, &mut memories, true);

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
        let mut memories = test_memories();
        let result = shutdown_daemon(dir, &shared_mem, &mut state, &mut memories, true);
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
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
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
        let mut memories = test_memories();

        let result = shutdown_daemon(dir, &shared_mem, &mut state, &mut memories, false);
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

    // --- Durable cycle counter: one-time report backfill (issue #1) ---

    #[test]
    fn seed_cycle_count_uses_persisted_value_when_nonzero() {
        // Steady state: the durable field is authoritative; no directory scan,
        // no backfill — the persisted value passes straight through even if
        // stray report files exist on disk.
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("cycle_reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("cycle_9999.json"), "{}").unwrap();

        assert_eq!(seed_cycle_count(1159, tmp.path()), 1159);
    }

    #[test]
    fn seed_cycle_count_backfills_from_highest_report_when_zero() {
        // A brain upgraded from a build that never persisted `cycle_count`:
        // the field defaults to 0, but thousands of `cycle_<N>.json` reports
        // prove a high cumulative count. The seed recovers it so the display
        // never dips to #1 for one deploy.
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("state").join("cycle_reports");
        std::fs::create_dir_all(&reports).unwrap();
        for n in [1157u32, 1158, 1159] {
            std::fs::write(reports.join(format!("cycle_{n}.json")), "{}").unwrap();
        }
        // Ignores non-cycle files.
        std::fs::write(reports.join("summary.json"), "{}").unwrap();

        assert_eq!(
            seed_cycle_count(0, tmp.path()),
            1159,
            "backfill must recover the highest persisted cycle index",
        );
    }

    #[test]
    fn seed_cycle_count_stays_zero_for_fresh_brain() {
        // No reports and no durable value: a genuinely fresh brain stays at 0,
        // so its first cycle increments to and displays #1.
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(seed_cycle_count(0, tmp.path()), 0);
    }

    #[test]
    fn seed_cycle_count_backfill_is_idempotent() {
        // Repeated startup seeds over the same on-disk state yield the same
        // value (read-only; no state mutation).
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("cycle_reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("cycle_42.json"), "{}").unwrap();

        assert_eq!(seed_cycle_count(0, tmp.path()), 42);
        assert_eq!(seed_cycle_count(0, tmp.path()), 42);
    }
}
