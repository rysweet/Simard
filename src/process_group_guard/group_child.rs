//! [`GroupChild`]: an RAII guard that spawns a child as the leader of its own
//! process group and tears down the **entire** group subtree on every exit
//! path — `Ok`, `Err`, `?`-propagation, timeout, and panic-unwind — unless it
//! was explicitly [`disarm`](GroupChild::disarm)ed or already
//! [`reap`](GroupChild::reap)ed.
//!
//! Cross-links `rysweet/amplihack-rs#964`: the upstream companion fix for the
//! same bug class in `recipe-runner-rs`. This Simard-side guard hardens the
//! analogous nested-subprocess supervision that is editable in this checkout,
//! so a failed/aborted/timed-out/panicking orchestrator run leaves no orphaned
//! children.
//!
//! # Why a group, not just the child
//!
//! The nested agents Simard spawns (`recipe-runner-rs`, `copilot`/`claude`
//! Node processes) fork descendants. `child.kill()` reaps only the immediate
//! child, orphaning the grandchildren (they reparent to init and keep pipes /
//! target dirs open). Spawning with `process_group(0)` makes the child's PGID
//! equal its PID; a single `kill(-pgid, …)` then reaches the whole subtree.
//!
//! # Teardown escalation
//!
//! On drop the guard sends SIGTERM to the group, waits up to a bounded grace
//! window for the group to exit, and only then escalates to SIGKILL — never
//! leading with SIGKILL. All OS signalling goes through the
//! [`ProcessGroupProbe`](super::probe::ProcessGroupProbe) seam so the
//! escalation is unit-tested offline and sleep-free.

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use super::probe::{LibcSignaller, ProcessGroupProbe};

/// Default grace window between SIGTERM and SIGKILL escalation on teardown.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(5);

/// How often the teardown loop polls the group for exit while inside the grace
/// window. Kept small so a group that exits promptly is not SIGKILLed just for
/// being slower than a single check, while never busy-spinning.
const TEARDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// RAII guard owning a child spawned in its own process group.
///
/// Dropping an *armed* guard (not disarmed, not reaped, `pgid > 1`) tears down
/// the whole group. See the module docs for the escalation policy.
pub struct GroupChild {
    /// The spawned child. `None` after [`disarm`](Self::disarm) hands ownership
    /// back to the caller, or in the offline test constructor.
    child: Option<Child>,
    /// The child's process-group id. Equals the child PID because the child is
    /// spawned with `process_group(0)`. The teardown target is `-pgid`.
    pgid: i32,
    /// OS-signalling seam (production: [`LibcSignaller`]; tests: a recording
    /// double).
    signaller: Arc<dyn ProcessGroupProbe>,
    /// Grace window between SIGTERM and SIGKILL.
    grace: Duration,
    /// Set once the child has been waited on; a reaped PID must never be
    /// re-signalled (REQ-V2: avoids signalling a recycled foreign pgid).
    reaped: bool,
    /// Set by [`disarm`](Self::disarm): the caller has taken ownership and the
    /// guard must not tear the group down on drop (the one intentional detached
    /// spawn, e.g. `simard safe-update`).
    disarmed: bool,
}

impl GroupChild {
    /// Spawn `cmd` as the leader of its own process group, guarded for
    /// full-subtree teardown on drop.
    ///
    /// On Unix the child is placed in a new process group via
    /// `process_group(0)`; on other platforms it is spawned normally and the
    /// guard is a no-op (teardown is Unix-only).
    pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
        Self::spawn_with(cmd, Arc::new(LibcSignaller), DEFAULT_GRACE)
    }

    /// Spawn with an injected signaller + grace window. Internal seam for tests
    /// that need a real child but a recording signaller.
    pub fn spawn_with(
        cmd: &mut Command,
        signaller: Arc<dyn ProcessGroupProbe>,
        grace: Duration,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        let child = cmd.spawn()?;
        let pgid = child.id() as i32;
        Ok(Self {
            child: Some(child),
            pgid,
            signaller,
            grace,
            reaped: false,
            disarmed: false,
        })
    }

    /// Offline test constructor: build a guard around a *fake* pgid and injected
    /// signaller with **no** real child. Lets the teardown contract be asserted
    /// without spawning a process.
    #[cfg(test)]
    pub(crate) fn from_parts(
        pgid: i32,
        signaller: Arc<dyn ProcessGroupProbe>,
        grace: Duration,
    ) -> Self {
        Self {
            child: None,
            pgid,
            signaller,
            grace,
            reaped: false,
            disarmed: false,
        }
    }

    /// The child's process-group id (equals its PID).
    pub fn pgid(&self) -> i32 {
        self.pgid
    }

    /// Mutable access to the underlying child (e.g. to take stdio or `try_wait`).
    /// `None` once disarmed or in the offline test constructor.
    pub fn child_mut(&mut self) -> Option<&mut Child> {
        self.child.as_mut()
    }

    /// Wait for the child to exit and mark it reaped, so drop will **not**
    /// re-signal a (possibly recycled) pgid. Returns the exit status, or `None`
    /// when there is no owned child.
    pub fn reap(&mut self) -> io::Result<Option<ExitStatus>> {
        self.reaped = true;
        match self.child.as_mut() {
            Some(child) => child.wait().map(Some),
            None => Ok(None),
        }
    }

    /// Relinquish ownership of the child so it survives the guard's drop
    /// (the single intentional detached spawn). After this, drop performs no
    /// teardown. Returns the raw [`Child`] when one is owned.
    pub fn disarm(&mut self) -> Option<Child> {
        self.disarmed = true;
        self.child.take()
    }
}

impl GroupChild {
    /// Tear the whole group down with the SIGTERM → bounded grace → SIGKILL
    /// escalation. Signals only `-pgid`.
    ///
    /// The grace loop reaps the immediate leader child (non-blocking) as soon as
    /// it exits: `std::process::Child` does not reap on its own, and an
    /// un-reaped leader lingers as a **zombie** that `kill(-pgid, 0)` still
    /// counts as a live group member. Without reaping it here the liveness probe
    /// would stay positive for the whole grace window on *every* teardown — even
    /// one whose group died instantly on SIGTERM — needlessly waiting the full
    /// grace and then escalating to a redundant SIGKILL (with a misleading
    /// "survived SIGTERM" warning). Reaping the leader in-loop lets the graceful
    /// path be detected as soon as the real subtree is gone.
    fn tear_down_group(&mut self) {
        // Graceful first: SIGTERM the whole group. If the group is already gone
        // (ESRCH surfaces as an error), there is nothing to escalate.
        if self
            .signaller
            .signal_group(self.pgid, libc::SIGTERM)
            .is_err()
        {
            return;
        }

        // Wait up to `grace` for the group to exit before escalating. With a
        // zero grace (tests) the loop makes exactly one liveness check and never
        // sleeps. Never lead with SIGKILL.
        let deadline = Instant::now() + self.grace;
        loop {
            // Reap the leader if it has exited, so its zombie does not keep the
            // group-liveness probe positive and force a spurious escalation.
            self.try_reap_leader();
            if !self.signaller.group_alive(self.pgid) {
                // Exited on the graceful signal — do not escalate.
                return;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline.saturating_duration_since(now);
            std::thread::sleep(remaining.min(TEARDOWN_POLL_INTERVAL));
        }

        // Grace elapsed and the group is still alive: escalate to SIGKILL so no
        // descendant of a failed/aborted/timed-out/panicking run is orphaned.
        tracing::warn!(
            pgid = self.pgid,
            grace_ms = self.grace.as_millis() as u64,
            "process group survived SIGTERM; escalating to SIGKILL to avoid orphaned subprocesses"
        );
        let _ = self.signaller.signal_group(self.pgid, libc::SIGKILL);
    }

    /// Non-blocking reap of the immediate leader child. If it has exited, drop
    /// the handle so it is neither waited on twice nor seen as a lingering
    /// zombie by later group-liveness probes. A leader that is still running
    /// (e.g. it trapped SIGTERM) is left owned for the escalation path and the
    /// final blocking reap in [`Drop`].
    fn try_reap_leader(&mut self) {
        let exited = matches!(
            self.child.as_mut().map(|child| child.try_wait()),
            Some(Ok(Some(_status)))
        );
        if exited {
            self.child = None;
        }
    }
}

impl Drop for GroupChild {
    fn drop(&mut self) {
        // Ownership / REQ-V2: a disarmed guard handed its child to the caller,
        // and a reaped guard's pgid may already be recycled onto an unrelated
        // group. Either way, tear nothing down and reap nothing.
        if self.disarmed || self.reaped {
            return;
        }

        // REQ-V1 (fail-closed): only tear the group down for a real child pgid.
        // `-0` targets the caller's own group and `-1` broadcasts to every
        // process; a real child pgid is always > 1. A non-positive pgid skips
        // group signalling but still reaps any owned child below.
        if self.pgid > 1 {
            self.tear_down_group();
        }

        // Reap the immediate leader child so its PID is not leaked as a zombie.
        // The graceful path already reaped it inside `tear_down_group` (leaving
        // `child` as `None` here); this final blocking `wait()` covers the
        // escalation path, where the leader trapped SIGTERM and was only just
        // SIGKILLed, plus the `pgid <= 1` path that skips group teardown. Either
        // way `std::process::Child`'s own `Drop` does NOT `wait()`, so without
        // this the leader would linger as a zombie — one leaked PID/handle per
        // armed teardown, exactly the exhaustion class this guard prevents.
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }
}
