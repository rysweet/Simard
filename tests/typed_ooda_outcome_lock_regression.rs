//! Regression: concurrent typed-OODA outcome persistence + outbox startup
//! recovery must never surface `database is locked` / `SQLITE_BUSY`.
//!
//! Reproduces issue #4483: on a daemon restart, multiple concurrent OODA cycles
//! (each a distinct goal opening its own `CapabilityHandler` on the SAME
//! typed-outcome SQLite file) contend with the outbox startup-recovery path
//! (`drain_pending`). Every handle to the ledger DB must funnel through a single
//! shared, WAL + busy_timeout connection so opens and writes serialize instead
//! of colliding on the file-level write lock.
//!
//! TDD status: RED until every typed-outcome DB handle is routed through the
//! shared connection factory (serialized access + bounded busy retry). Before
//! that fix, each `CapabilityHandler::open` creates an independent connection;
//! when many cycles start at once (as on restart) their first-init +
//! `PRAGMA journal_mode = WAL` acquisitions pile up past the per-connection 5s
//! `busy_timeout`, and at least one `open` / `record_*` / `drain_pending` call
//! returns `typed outcome persistence failed: database is locked`.
//!
//! After the fix there is exactly one shared connection per path, so the
//! concurrent-open count is irrelevant: initialization happens once, every
//! write serializes through the shared mutex, and the test is fast and green.

use std::path::Path;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use simard::typed_ooda::{
    AuthenticatedToolContext, CapabilityGrant, CapabilityHandler, CapabilityPolicy,
    EffectExecutionError, EffectExecutor, EffectJob, EffectResult, OpaqueBytes, OutboxWorker,
    RecordNoActionRequest, RepositoryRef, TerminalRequestIdentity,
};

/// Number of concurrent OODA cycles (distinct goals) opening the ledger at once.
///
/// The journal evidence was a six-goal post-restart burst, but six independent
/// connections are masked by the per-connection 5s `busy_timeout`. To surface
/// the latent file-lock race deterministically we scale the simultaneous-open
/// count well past the point where cumulative first-init serialization exceeds
/// that timeout. With the shared-connection factory in place there is exactly
/// one connection, so this count is irrelevant and the test stays fast + green.
const CONCURRENT_GOALS: usize = 112;

/// Terminal outcomes each cycle persists back-to-back. First-init contention
/// dominates the race, so a small write count keeps the post-fix run fast.
const WRITES_PER_GOAL: usize = 4;

/// Number of concurrent startup-recovery workers draining the outbox while the
/// cycles persist outcomes. Added to the simultaneous-open pressure.
const RECOVERY_WORKERS: usize = 16;

/// Iterations of `drain_pending` each recovery worker performs.
const RECOVERY_PASSES: usize = 4;

struct NoopEffects;

impl EffectExecutor for NoopEffects {
    fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
        Ok(EffectResult::Succeeded {
            evidence: Vec::new(),
        })
    }
}

fn is_lock_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("database is locked") || lowered.contains("sqlite_busy")
}

fn open_handler(path: &Path) -> Result<CapabilityHandler, String> {
    // Opening also runs schema initialization; on a fresh file this includes
    // `PRAGMA journal_mode = WAL` and the migration transaction, which is the
    // real contention point across racing connections.
    CapabilityHandler::open(path, CapabilityPolicy::new("policy-v1"))
        .map_err(|error| error.to_string())
}

/// Pre-build the `(actor, request)` payloads for one goal so that, once the
/// starting barrier releases, each thread's first observable action is
/// `open()` — maximizing the number of connections simultaneously inside the
/// first-init window where the lock race lives.
fn build_no_action_requests(goal: usize) -> Vec<(AuthenticatedToolContext, RecordNoActionRequest)> {
    (0..WRITES_PER_GOAL)
        .map(|iteration| {
            let session_id = format!("session-goal-{goal}");
            let cycle_id = format!("cycle-{goal}-{iteration}");
            let goal_id = format!("goal-{goal}");
            let request_id = format!("request-{goal}-{iteration}");

            let actor = AuthenticatedToolContext::new(
                "goal-session-actor",
                session_id.clone(),
                [CapabilityGrant::RecordNoAction],
            )
            .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
            .bound_to_cycle_goal(cycle_id.clone(), goal_id.clone());

            let request = RecordNoActionRequest {
                identity: TerminalRequestIdentity::new(request_id, session_id, cycle_id, goal_id),
                reason: OpaqueBytes::from(b"no protocol action this cycle".to_vec()),
                raw_semantic: OpaqueBytes::from(vec![0x00, 0xff, b'N']),
                evidence: Vec::new(),
            };
            (actor, request)
        })
        .collect()
}

#[test]
fn concurrent_cycles_and_startup_recovery_never_lock_the_outcome_ledger() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = dir.path().join("outcomes.sqlite3");

    // Every worker opens its OWN handler onto the shared file and they all race
    // from the same starting line — reproducing the post-restart burst where
    // distinct goals and the outbox recovery path open connections at once.
    let barrier = Arc::new(Barrier::new(CONCURRENT_GOALS + RECOVERY_WORKERS));
    let mut handles = Vec::new();

    // Writer threads: each distinct goal is its own OODA cycle with its own
    // handler/connection onto the shared ledger file.
    for goal in 0..CONCURRENT_GOALS {
        let path = ledger_path.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || -> Vec<String> {
            // Build payloads BEFORE synchronizing so the post-barrier hot path
            // is only `open()` + writes.
            let requests = build_no_action_requests(goal);
            let mut lock_errors = Vec::new();

            barrier.wait();
            let handler = match open_handler(&path) {
                Ok(handler) => handler,
                Err(message) => {
                    if is_lock_error(&message) {
                        lock_errors.push(message);
                    }
                    return lock_errors;
                }
            };
            for (actor, request) in requests {
                if let Err(error) = handler.record_no_action(&actor, request) {
                    let message = error.to_string();
                    if is_lock_error(&message) {
                        lock_errors.push(message);
                    }
                }
            }
            lock_errors
        }));
    }

    // Startup-recovery threads: the outbox recovery path (`drain_pending`)
    // contends with the writers, exactly as it does on daemon restart.
    for worker in 0..RECOVERY_WORKERS {
        let path = ledger_path.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || -> Vec<String> {
            let worker_id = format!("startup-recovery-{worker}");
            let mut lock_errors = Vec::new();

            barrier.wait();
            let handler = match open_handler(&path) {
                Ok(handler) => handler,
                Err(message) => {
                    if is_lock_error(&message) {
                        lock_errors.push(message);
                    }
                    return lock_errors;
                }
            };
            let effects = NoopEffects;
            for _ in 0..RECOVERY_PASSES {
                let outbox =
                    OutboxWorker::new(&handler, &effects, &worker_id, Duration::from_secs(300));
                if let Err(error) = outbox.drain_pending(32) {
                    let message = error.to_string();
                    if is_lock_error(&message) {
                        lock_errors.push(message);
                    }
                }
            }
            lock_errors
        }));
    }

    let mut lock_errors: Vec<String> = Vec::new();
    for handle in handles {
        lock_errors.extend(handle.join().expect("worker thread must not panic"));
    }

    assert!(
        lock_errors.is_empty(),
        "concurrent outcome persistence + startup recovery must never report a locked \
         database, but observed {} lock failure(s); first few: {:?}",
        lock_errors.len(),
        lock_errors.iter().take(5).collect::<Vec<_>>(),
    );

    // Every distinct (session, cycle) terminal must be durably persisted: no
    // write may have been silently dropped in the name of avoiding the lock.
    let verifier = open_handler(&ledger_path).expect("open verifier handler");
    for goal in 0..CONCURRENT_GOALS {
        let session_id = format!("session-goal-{goal}");
        for iteration in 0..WRITES_PER_GOAL {
            let cycle_id = format!("cycle-{goal}-{iteration}");
            let terminal = verifier
                .terminal_for_cycle(&session_id, &cycle_id)
                .expect("terminal lookup must not fail");
            assert!(
                terminal.is_some(),
                "expected a durable terminal outcome for {session_id}/{cycle_id}",
            );
        }
    }
}
