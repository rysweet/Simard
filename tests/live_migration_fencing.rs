//! TDD (tests-first) for the LIVE MIGRATION fencing / "exactly one primary"
//! contract (issue #2923, design spike).
//!
//! The migration's hardest correctness requirement is split-brain avoidance:
//! during promote/demote there must be EXACTLY ONE primary acting at a time,
//! and a fenced-off old primary must be rejectable even if it is still alive.
//!
//! These tests characterize the invariants the migration's `primary-lease`
//! component MUST uphold, expressed against the existing `LeaderSemaphore`
//! primitive (PID owner + monotonic `generation` fencing token + heartbeat
//! staleness). They compile and pass today, locking the fencing contract that
//! the role/standby state machine and cutover orchestrator build upon.
//!
//! SCOPE / HONEST LIMITATION (do not misread a green run here): `LeaderSemaphore`
//! is HOST-LOCAL — it observes liveness via `kill(pid, 0)` and a local JSON file,
//! so it can only fence rivals on the SAME host. It is the *local half* of the
//! role gate (same-host promote/demote + a monotonic fencing epoch). It is
//! provably INSUFFICIENT for cross-host "exactly one primary" during a
//! two-host migration; that requires the shared cross-host lease tracked by
//! issue #2725 (candidate: an Azure Blob lease whose lease-id composes with the
//! `generation` epoch, checked at every side-effecting actuator — OODA advance,
//! engineer spawn, git merge/deploy, Signal send). These tests lock the local
//! invariants the cross-host lease will build on and reconcile with; they do
//! NOT claim cross-host safety.
//!
//! Any future migration lease implementation must keep these green.

use simard::LeaderSemaphore;

fn tmp_lock(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "simard-migration-fencing-{}-{}-{}.json",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

/// Write the lock file directly with a heartbeat at epoch 0 (1970) so the
/// recorded owner is UNAMBIGUOUSLY stale for any threshold — making seizure
/// deterministic regardless of ambient PID liveness or whether the suite runs
/// as root (`is_pid_alive` uses `kill(pid,0)`, whose EPERM handling differs by
/// user). `try_acquire`'s block guard is `is_pid_alive(owner) && !is_stale`, so
/// a stale owner short-circuits to "seizable" no matter the liveness result.
/// Uses the documented on-disk format `{"pid","generation","heartbeat_epoch"}`.
fn seed_stale_owner(path: &std::path::Path, pid: u32, generation: u64) {
    std::fs::write(
        path,
        format!(
            r#"{{"pid":{},"generation":{},"heartbeat_epoch":0}}"#,
            pid, generation
        ),
    )
    .expect("seed stale lock file");
}

/// INVARIANT: a live, non-stale primary blocks any other candidate from
/// acquiring the lease. Two live primaries can never hold the lease at once.
#[test]
fn live_primary_blocks_second_acquirer_no_split_brain() {
    let path = tmp_lock("block");
    let sem = LeaderSemaphore::new(&path);

    let me = std::process::id();
    let primary = sem.try_acquire(me).expect("first acquire succeeds");
    assert_eq!(primary.pid, me);
    assert_eq!(primary.generation, 1, "first generation is 1");

    // A different candidate PID must be rejected while the live primary holds
    // a fresh (non-stale) lease. This is the split-brain guard.
    let other_pid = me.wrapping_add(1).max(2);
    let err = sem.try_acquire(other_pid);
    assert!(
        err.is_err(),
        "second acquirer must be rejected while live primary holds the lease"
    );

    // The lease owner/token is unchanged after the rejected attempt.
    let state = sem.read_state().unwrap().expect("state present");
    assert_eq!(state.pid, me, "owner unchanged after rejected acquire");
    assert_eq!(
        state.generation, 1,
        "fencing token unchanged after rejection"
    );

    let _ = std::fs::remove_file(&path);
}

/// INVARIANT: the fencing token (`generation`) is monotonic across seizures.
/// A stale/dead primary can be superseded, and the new primary always carries
/// a STRICTLY GREATER token, so downstream writers can fence the old one out.
///
/// Seizure is forced via `seed_stale_owner` (heartbeat at epoch 0), NOT via the
/// same-second `stale_threshold(0)` timing (seconds granularity means three
/// acquires in one second are NOT stale by elapsed time) and NOT via ambient
/// PID liveness (root vs non-root changes `kill(pid,0)`). This keeps the test
/// deterministic in any CI environment.
#[test]
fn fencing_token_is_monotonic_across_seizures() {
    let path = tmp_lock("monotonic");
    let sem = LeaderSemaphore::new(&path).with_stale_threshold(0);

    // Predecessor at generation 1, deterministically stale.
    seed_stale_owner(&path, 900_001, 1);
    let s2 = sem
        .try_acquire(900_002)
        .expect("seize over stale predecessor");
    assert!(
        s2.generation > 1,
        "generation must strictly increase on seizure (1 -> {})",
        s2.generation
    );

    // Re-stale the freshly written state, then seize again.
    seed_stale_owner(&path, s2.pid, s2.generation);
    let s3 = sem.try_acquire(900_003).expect("seize again over stale");
    assert!(
        s3.generation > s2.generation,
        "generation must keep increasing ({} -> {})",
        s2.generation,
        s3.generation
    );
    assert_eq!(s3.pid, 900_003, "latest owner recorded");

    let _ = std::fs::remove_file(&path);
}

/// CHARACTERIZATION of the host-local LIMITATION (this is WHY cross-host safety
/// needs the shared lease of #2725, not a defect in this primitive): once the
/// current owner reads stale/dead, `try_acquire` lets ANY pid seize it —
/// including a previously-demoted one. On a single host, `kill(pid,0)` liveness
/// bounds this; ACROSS hosts, `kill(new_pid,0)` on the old host is meaningless,
/// so a demoted old primary could reclaim and re-split-brain. The correct
/// cross-host defense is FENCE-AT-WRITE (each resource rejects a stale fencing
/// epoch via CAS), NOT merely holding/observing the lease. This test pins the
/// limitation so it is not mistaken for cross-host safety.
#[test]
fn demoted_owner_can_reclaim_once_owner_is_stale_hostlocal_limitation() {
    let path = tmp_lock("reclaim");
    let sem = LeaderSemaphore::new(&path).with_stale_threshold(0);

    let old_primary = 900_101u32;
    let new_primary = 900_102u32;

    // New primary holds the lease at generation 7, but reads stale.
    seed_stale_owner(&path, new_primary, 7);
    // The demoted old primary can seize it back precisely because the owner is
    // stale — host-local liveness cannot prevent cross-host reclaim.
    let reclaimed = sem
        .try_acquire(old_primary)
        .expect("a stale owner is seizable by anyone on the same host");
    assert_eq!(
        reclaimed.pid, old_primary,
        "old primary reclaimed the lease"
    );
    assert!(
        reclaimed.generation > 7,
        "reclaim still bumps the monotonic token (7 -> {})",
        reclaimed.generation
    );

    let _ = std::fs::remove_file(&path);
}

/// INVARIANT: clean cutover is a lease TRANSFER from the current primary to the
/// standby. It only succeeds for the current owner and bumps the fencing token,
/// modeling PROMOTE(secondary)/DEMOTE(primary) as a single atomic role swap.
#[test]
fn cutover_transfer_promotes_standby_and_bumps_token() {
    let path = tmp_lock("transfer");
    let sem = LeaderSemaphore::new(&path);

    let primary_pid = 201u32;
    let standby_pid = 202u32;

    let acquired = sem.try_acquire(primary_pid).expect("primary acquires");
    let promoted = sem
        .transfer(primary_pid, standby_pid)
        .expect("owner may transfer to standby");

    assert_eq!(promoted.pid, standby_pid, "standby is now primary");
    assert!(
        promoted.generation > acquired.generation,
        "cutover bumps fencing token ({} -> {})",
        acquired.generation,
        promoted.generation
    );

    let _ = std::fs::remove_file(&path);
}

/// INVARIANT: a fenced-off / demoted old primary CANNOT transfer or reclaim the
/// lease. After cutover, only the new primary owns it — no reverse split-brain.
#[test]
fn demoted_primary_cannot_transfer_after_cutover() {
    let path = tmp_lock("fenced");
    let sem = LeaderSemaphore::new(&path);

    let old_primary = 301u32;
    let new_primary = 302u32;
    let stray = 303u32;

    sem.try_acquire(old_primary).expect("old primary acquires");
    sem.transfer(old_primary, new_primary)
        .expect("cutover to new primary");

    // The demoted old primary attempting to hand the lease elsewhere must fail:
    // it no longer owns the semaphore.
    let err = sem.transfer(old_primary, stray);
    assert!(
        err.is_err(),
        "demoted old primary must not be able to transfer the lease"
    );

    let state = sem.read_state().unwrap().expect("state present");
    assert_eq!(
        state.pid, new_primary,
        "new primary remains the single lease owner"
    );

    let _ = std::fs::remove_file(&path);
}

/// INVARIANT: releasing the lease is owner-scoped. A non-owner cannot release
/// the current primary's lease (prevents a standby from evicting the primary).
#[test]
fn release_is_owner_scoped() {
    let path = tmp_lock("release");
    let sem = LeaderSemaphore::new(&path);

    let owner = 401u32;
    let intruder = 402u32;

    sem.try_acquire(owner).expect("owner acquires");
    sem.release(intruder).expect("non-owner release is a no-op");

    let state = sem
        .read_state()
        .unwrap()
        .expect("lease still held after non-owner release");
    assert_eq!(state.pid, owner, "only the owner may release its lease");

    sem.release(owner).expect("owner releases");
    assert!(
        sem.read_state().unwrap().is_none(),
        "lease is cleared after owner release"
    );

    let _ = std::fs::remove_file(&path);
}
