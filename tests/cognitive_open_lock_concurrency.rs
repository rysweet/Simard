//! TDD (Step 7) — FAILING tests for the systemic cognitive-store open-lock
//! crash-loop fix (issue: "OODA-spawned engineers die acquiring the
//! cross-process `cognitive-open-lock` and shut down artifact-less").
//!
//! # What crashes today
//!
//! Concurrent OODA engineers each open a SECOND direct exclusive
//! [`LibraryCognitiveMemory`] handle on the daemon's shared cognitive state
//! root. Only one wins the single-writer `flock(LOCK_EX|LOCK_NB)` on the
//! `cognitive.open.lock` sidecar; the rest back off for
//! `DEFAULT_BUDGET = 15_000ms` and then **FAIL LOUD** with
//! `PersistentStoreIo { reason: "cognitive store is held open by another
//! process ... after waiting 15000ms" }`, tear down their MeterProvider, and
//! exit WITHOUT producing any commit / PR / artifact.
//!
//! # The contract these tests pin (all NOT-YET-IMPLEMENTED — the compile /
//! assert failures are the intended TDD red state)
//!
//! A new caller-role-aware cognition connector lets an engineer route through
//! the daemon memory-IPC client and, on a lost open-lock race, **degrade to
//! deferred/read-only cognition** and STILL finish — while the daemon / any
//! genuine exclusive writer keeps the fail-loud corruption guard unchanged.
//!
//! ```rust
//! // src/ooda_loop/client_factory.rs, re-exported from `simard::ooda_loop`.
//!
//! /// Intent of the caller. An INTERNAL capability token — never derived from
//! /// env / CLI / file input.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum CallerRole {
//!     /// The long-lived daemon / any true exclusive writer. Contended open =>
//!     /// FAIL LOUD (preserves the lbug lock-conflict-as-corruption guard).
//!     Daemon,
//!     /// A short-lived OODA engineer / worktree. Contended open (no IPC) =>
//!     /// degrade to deferred/read-only cognition and keep going.
//!     Engineer,
//! }
//!
//! /// How writes behave on the returned handle.
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum WriteMode {
//!     /// Writes persist to the store (live handle or daemon IPC writer).
//!     Live,
//!     /// Writes are deferred (queued / dropped-with-metric), NEVER silently
//!     /// claimed as persisted. Only reachable for `CallerRole::Engineer`.
//!     Deferred,
//! }
//!
//! /// Resolved cognitive access for a caller.
//! pub struct CognitiveAccess { /* private */ }
//! impl CognitiveAccess {
//!     /// Read/write handle. In `WriteMode::Deferred` the write methods queue /
//!     /// drop-with-metric and return `Ok(..)` — the caller already knows via
//!     /// [`Self::write_mode`] that writes are not persisted (not silent).
//!     pub fn memory(&self) -> &dyn simard::cognitive_memory::CognitiveMemoryOps;
//!     pub fn write_mode(&self) -> WriteMode;
//!     pub fn degraded(&self) -> bool;
//! }
//!
//! /// IPC-first resolution:
//! ///   socket present -> daemon IPC writer/reader (Live, shared).
//! ///   no socket, uncontended direct open -> Live.
//! ///   no socket, CONTENDED direct open ->
//! ///       Engineer: Ok(Deferred, degraded=true) + a
//! ///                 `simard.enrichment.degraded{reason="cognitive_open_lock"}`
//! ///                 increment and a structured WARN under `simard::enrichment`.
//! ///       Daemon:   Err(PersistentStoreIo "held open by another process ...").
//! pub fn connect_memory_for_role(
//!     state_root: &std::path::Path,
//!     role: CallerRole,
//! ) -> simard::error::SimardResult<CognitiveAccess>;
//! ```
//!
//! Plus a new bounded [`DegradeReason`] variant:
//!
//! ```rust
//! // src/enrichment_observability/mod.rs
//! pub enum DegradeReason { MemoryIpc, KnowledgeLaunch, CognitiveOpenLock }
//! // DegradeReason::CognitiveOpenLock.as_str() == "cognitive_open_lock"
//! ```
//!
//! # Out of scope for THIS file (covered by unit tests the implementer adds)
//!
//! * `open_guard::{OpenLockOutcome, try_acquire_classified}` — `pub(crate)`,
//!   unreachable from an integration crate; exercised end-to-end here via the
//!   Engineer-degrades-vs-Daemon-fails-loud behaviour.
//! * `spawn::engineer_worktree_state_root` isolated-root fallback (R3) — a
//!   private fn; covered by a `spawn.rs` unit test.
//! * The memory-ipc broken-pipe reconnect (#2860) — explicitly NOT touched.

#![cfg(unix)]

use std::fmt::Debug;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serial_test::serial;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use simard::error::SimardError;
use simard::ooda_loop::{CallerRole, WriteMode, connect_memory_for_role};

// The sidecar open-lock file name is a stable on-disk contract owned by
// `open_guard::OPEN_LOCK_FILE` (which is `pub(crate)`, so we mirror the literal
// here). A `ForeignHolder` raw-`flock`s it to simulate a DIFFERENT process (the
// daemon) holding the store open — `flock` is per open-file-description, so a
// second FD in this process is blocked by `LOCK_EX` exactly like a foreign PID.
const OPEN_LOCK_FILE: &str = "cognitive.open.lock";

/// Env override for the open-lock acquisition budget (milliseconds). Set low so
/// a contended race resolves to Deferred / fail-loud in milliseconds instead of
/// the 15s production budget.
const BUDGET_ENV: &str = "SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS";

// ── foreign (cross-process-equivalent) open-lock holder ─────────────────────

struct ForeignHolder {
    _file: std::fs::File,
}

impl ForeignHolder {
    /// Hold an exclusive `flock` on `<state_root>/cognitive.open.lock`,
    /// simulating the daemon process holding the store open with NO IPC socket
    /// available to engineers.
    fn hold(state_root: &Path) -> Self {
        std::fs::create_dir_all(state_root).expect("mkdir state_root");
        let lock_path = state_root.join(OPEN_LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&lock_path)
            .expect("open sidecar lock file");
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(ret, 0, "foreign holder must acquire the open-lock flock");
        Self { _file: file }
    }
}

impl Drop for ForeignHolder {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

// ── test env / fs helpers ───────────────────────────────────────────────────

/// A unique temp state root (avoids `TempDir` so the `#[serial]` env-setting
/// tests never race each other on cleanup).
fn temp_state_root(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-open-lock-conc-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&dir).expect("create temp state root");
    dir
}

struct BudgetEnvGuard;
impl BudgetEnvGuard {
    fn set(ms: u64) -> Self {
        unsafe { std::env::set_var(BUDGET_ENV, ms.to_string()) };
        Self
    }
}
impl Drop for BudgetEnvGuard {
    fn drop(&mut self) {
        unsafe { std::env::remove_var(BUDGET_ENV) };
    }
}

// ── tracing capture (thread-scoped, hermetic) ───────────────────────────────

#[derive(Clone, Debug)]
struct CapturedEvent {
    level: String,
    target: String,
    fields: String,
}

#[derive(Default)]
struct FieldVisitor {
    out: String,
}

impl FieldVisitor {
    fn push(&mut self, name: &str, value: &str) {
        use std::fmt::Write;
        let _ = write!(self.out, " {name}={value}");
    }
}

impl Visit for FieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field.name(), &value.to_string());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field.name(), &value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field.name(), &value.to_string());
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field.name(), value);
    }
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.push(field.name(), &format!("{value:?}"));
    }
}

struct CollectLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: Subscriber> Layer<S> for CollectLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        self.events.lock().unwrap().push(CapturedEvent {
            level: meta.level().to_string(),
            target: meta.target().to_string(),
            fields: visitor.out,
        });
    }
}

/// Run `body` under a thread-local capturing subscriber; return only events
/// emitted under the `simard::enrichment` target (the degrade choke point).
fn capture_enrichment_events<F: FnOnce()>(body: F) -> Vec<CapturedEvent> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CollectLayer {
        events: Arc::clone(&events),
    };
    let subscriber = tracing_subscriber::registry::Registry::default().with(layer);
    tracing::subscriber::with_default(subscriber, body);
    let all = events.lock().unwrap().clone();
    all.into_iter()
        .filter(|e| e.target == "simard::enrichment")
        .collect()
}

// ═════════════════════════════════════════════════════════════════════════
// 1. Bounded degrade-reason enum (telemetry cardinality)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn degrade_reason_cognitive_open_lock_maps_to_bounded_enum() {
    // Additive, low-cardinality attribute value on the REUSED
    // `simard.enrichment.degraded` counter (no new counter surface).
    assert_eq!(
        simard::enrichment_observability::DegradeReason::CognitiveOpenLock.as_str(),
        "cognitive_open_lock",
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 2. Happy path — uncontended open is LIVE and round-trips (no regression)
// ═════════════════════════════════════════════════════════════════════════

#[test]
#[serial(cognitive_memory)]
fn engineer_uncontended_open_is_live_and_round_trips() {
    let root = temp_state_root("eng-live");

    let access = connect_memory_for_role(&root, CallerRole::Engineer)
        .expect("uncontended engineer open must succeed (Live)");
    assert_eq!(
        access.write_mode(),
        WriteMode::Live,
        "an uncontended engineer open must be a LIVE writer, not deferred"
    );
    assert!(
        !access.degraded(),
        "an uncontended open must not be flagged degraded"
    );

    // Live writes persist and are immediately recallable through the handle.
    access
        .memory()
        .store_fact("rust", "systems language", 0.9, &[], "open-lock-test")
        .expect("Live store_fact must persist");
    let facts = access
        .memory()
        .search_facts("rust", 10, 0.0)
        .expect("recall after a Live write must succeed");
    assert_eq!(facts.len(), 1, "the Live-written fact must be recallable");
    assert_eq!(facts[0].concept, "rust");

    drop(access);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[serial(cognitive_memory)]
fn daemon_uncontended_open_is_live() {
    let root = temp_state_root("daemon-live");

    let access = connect_memory_for_role(&root, CallerRole::Daemon)
        .expect("uncontended daemon open must succeed (Live)");
    assert_eq!(
        access.write_mode(),
        WriteMode::Live,
        "the daemon always opens a LIVE writer on an uncontended store"
    );
    assert!(
        !access.degraded(),
        "uncontended daemon open is not degraded"
    );

    drop(access);
    let _ = std::fs::remove_dir_all(&root);
}

// ═════════════════════════════════════════════════════════════════════════
// 3. Engineer degrades gracefully on a lost open-lock race (R2 core fix)
// ═════════════════════════════════════════════════════════════════════════

#[test]
#[serial(cognitive_memory)]
fn engineer_contended_open_degrades_to_deferred_read_only_not_fatal() {
    let _budget = BudgetEnvGuard::set(300);
    let root = temp_state_root("eng-contended");
    // The daemon (a different process, no IPC socket for us) holds the store.
    let holder = ForeignHolder::hold(&root);

    let start = Instant::now();
    let access = connect_memory_for_role(&root, CallerRole::Engineer).expect(
        "a lost open-lock race must DEGRADE (Ok), never the 15000ms fail-loud \
         hard-exit that leaves the engineer artifact-less",
    );
    let waited = start.elapsed();

    // Degraded to deferred/read-only cognition — observable, not silent.
    assert!(
        access.degraded(),
        "a contended engineer open must be flagged degraded"
    );
    assert_eq!(
        access.write_mode(),
        WriteMode::Deferred,
        "a contended engineer open must degrade writes to Deferred (read-only cognition)"
    );

    // Bounded: it gives up near the (short) budget, nowhere near 15s.
    assert!(
        waited < Duration::from_secs(3),
        "degrade must resolve near the budget, waited {waited:?}"
    );

    // Reads never error under degradation (served from IPC / last-known /
    // empty snapshot) — the engineer keeps reasoning.
    let recalled = access
        .memory()
        .search_facts("anything", 10, 0.0)
        .expect("degraded reads must return Ok (possibly empty), never a lock error");
    assert!(
        recalled.is_empty(),
        "a degraded read against a store we could not open yields no facts"
    );

    // A deferred write is NOT a busy/lock write failure — it returns Ok
    // (queued / dropped-with-metric), because the caller already knows via
    // write_mode() that it is not persisted. This is the anti-crash contract.
    access
        .memory()
        .store_fact(
            "deferred",
            "not persisted, but not an error",
            0.5,
            &[],
            "eng",
        )
        .expect("a deferred write must NOT surface as a busy/lock write failure");

    drop(holder);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[serial(cognitive_memory)]
fn engineer_contended_degrade_is_warned_and_metered() {
    use simard::telemetry::{self, names};

    let _budget = BudgetEnvGuard::set(300);
    telemetry::reset();
    let root = temp_state_root("eng-metered");
    let holder = ForeignHolder::hold(&root);

    let events = capture_enrichment_events(|| {
        let access = connect_memory_for_role(&root, CallerRole::Engineer)
            .expect("contended engineer open must degrade, not fail loud");
        assert_eq!(access.write_mode(), WriteMode::Deferred);
    });

    // Fail-LOUD-but-graceful: a structured WARN under simard::enrichment
    // carrying the bounded reason — never a silent fallback.
    let warn = events
        .iter()
        .find(|e| e.level == "WARN")
        .expect("a cognitive-open-lock degrade MUST emit a WARN (never a silent fallback)");
    assert!(
        warn.fields.contains("reason=cognitive_open_lock"),
        "the degrade WARN must carry reason=cognitive_open_lock, got: {}",
        warn.fields
    );
    // No raw holder metadata (PID / host / path) may ride the WARN line
    // (log-injection / cardinality defence).
    assert!(
        !warn.fields.to_lowercase().contains("pid=")
            && !warn.fields.contains(root.to_string_lossy().as_ref()),
        "the WARN must not leak holder PID or store path, got: {}",
        warn.fields
    );

    // Metered on the REUSED counter with the bounded reason attribute.
    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DEGRADED,
            &[(names::ATTR_REASON, "cognitive_open_lock")]
        ),
        Some(1),
        "a cognitive-open-lock degrade increments \
         simard.enrichment.degraded{{reason=cognitive_open_lock}}"
    );

    drop(holder);
    let _ = std::fs::remove_dir_all(&root);
}

// ═════════════════════════════════════════════════════════════════════════
// 4. Corruption guard preserved for the daemon / true writer (R4 + security)
// ═════════════════════════════════════════════════════════════════════════

#[test]
#[serial(cognitive_memory)]
fn daemon_contended_open_still_fails_loud() {
    let _budget = BudgetEnvGuard::set(300);
    let root = temp_state_root("daemon-contended");
    let holder = ForeignHolder::hold(&root);

    let err = connect_memory_for_role(&root, CallerRole::Daemon).expect_err(
        "the daemon / true writer must KEEP failing loud on a genuine second \
         concurrent open — the lbug lock-conflict-as-corruption guard is preserved",
    );
    match err {
        SimardError::PersistentStoreIo { reason, .. } => assert!(
            reason.contains("held open by another process"),
            "fail-loud error must explain the contention, got: {reason}"
        ),
        other => panic!("expected PersistentStoreIo fail-loud, got {other:?}"),
    }

    drop(holder);
    let _ = std::fs::remove_dir_all(&root);
}

/// Security invariant: `WriteMode::Deferred` is UNREACHABLE for `CallerRole::Daemon`.
/// A degraded (deferred) writer must never be handed to the daemon / a true
/// exclusive writer, or a genuine second-writer corruption could slip past the
/// guard claiming false persistence. Under contention the daemon errors; it is
/// never `Ok(Deferred)`.
#[test]
#[serial(cognitive_memory)]
fn daemon_role_never_yields_a_deferred_writer() {
    let _budget = BudgetEnvGuard::set(300);
    let root = temp_state_root("daemon-no-defer");
    let holder = ForeignHolder::hold(&root);

    match connect_memory_for_role(&root, CallerRole::Daemon) {
        Ok(access) => panic!(
            "Daemon role must never yield a writer under contention; got \
             write_mode={:?} degraded={}",
            access.write_mode(),
            access.degraded()
        ),
        Err(SimardError::PersistentStoreIo { .. }) => { /* correct: fail loud */ }
        Err(other) => panic!("expected PersistentStoreIo, got {other:?}"),
    }

    drop(holder);
    let _ = std::fs::remove_dir_all(&root);
}

// ═════════════════════════════════════════════════════════════════════════
// 5. R5 regression — N concurrent engineers against ONE shared store all
//    make progress; none exit artifact-less; zero busy/lock write failures;
//    zero 15000ms fatal opens.
// ═════════════════════════════════════════════════════════════════════════

#[test]
#[serial(cognitive_memory)]
fn n_concurrent_engineers_against_shared_store_all_make_progress() {
    // Matches the observed in_flight_engineer_count=7 (simard status "live
    // engineers 7"); N=8 (>= 7) competing for a single cognitive store.
    const N: usize = 8;

    let _budget = BudgetEnvGuard::set(300);
    let shared_root = temp_state_root("n-engineers");
    let artifacts_dir = shared_root.join("artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("mkdir artifacts");

    // The daemon-equivalent holder keeps the shared store open for the whole
    // run (no IPC socket exists for the engineers), so every engineer LOSES the
    // open-lock race — exactly the crash-loop condition.
    let holder = ForeignHolder::hold(&shared_root);

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let root = shared_root.clone();
        let artifacts = artifacts_dir.clone();
        handles.push(std::thread::spawn(move || -> Result<(), String> {
            let start = Instant::now();

            // Route engineer cognition through the may-degrade path. A lost race
            // must degrade (Ok) — NEVER the 15000ms fail-loud hard-exit.
            let access = connect_memory_for_role(&root, CallerRole::Engineer)
                .map_err(|e| format!("engineer {i} hard-exited artifact-less: {e}"))?;

            if start.elapsed() >= Duration::from_secs(10) {
                return Err(format!(
                    "engineer {i} waited on the open-lock far past the short budget"
                ));
            }

            // Degraded, read-only cognition — reads and (deferred) writes must
            // not surface as busy/lock failures.
            access
                .memory()
                .search_facts("goal", 5, 0.0)
                .map_err(|e| format!("engineer {i} degraded read failed: {e}"))?;
            access
                .memory()
                .store_fact("progress", "engineer ran", 0.5, &[], "eng")
                .map_err(|e| format!("engineer {i} deferred write surfaced as a failure: {e}"))?;

            // Produce the artifact marker (stands in for the engineer's
            // commit / PR): the whole point is finishing WITH an artifact.
            std::fs::write(artifacts.join(format!("engineer-{i}.done")), b"ok")
                .map_err(|e| format!("engineer {i} could not write its artifact: {e}"))?;
            Ok(())
        }));
    }

    let mut failures = Vec::new();
    for h in handles {
        match h.join() {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => failures.push(msg),
            Err(_) => failures.push("an engineer thread panicked".to_string()),
        }
    }
    assert!(
        failures.is_empty(),
        "every engineer must make progress without an artifact-less exit; failures: {failures:#?}"
    );

    // No engineer exited artifact-less: exactly N artifact markers on disk.
    let produced = std::fs::read_dir(&artifacts_dir)
        .expect("read artifacts dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".done"))
        .count();
    assert_eq!(
        produced, N,
        "all {N} engineers must produce their artifact marker (none dies artifact-less)"
    );

    drop(holder);
    let _ = std::fs::remove_dir_all(&shared_root);
}
