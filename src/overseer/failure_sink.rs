//! Bounded, process-global sink for step-failure diagnoses (issue #2640, PART 2).
//!
//! The diagnosis of a failed decision-cycle / engineer / terminal-shell step is
//! produced deep in the execution path ([`crate::terminal_session`]), far from
//! the Overseer's Observe pass. This sink is the seam between them: the failure
//! site [`record_step_failure`]s a structured [`FailureDiagnosis`]; the acting
//! Overseer [`drain_recent`]s them once per Observe pass and lifts each into a
//! corrective `Signal::StepFailureDiagnosed`. That is what makes "diagnose the
//! WHY, then drive a fix" mechanical rather than a silent log line.
//!
//! The buffer is bounded ([`STEP_FAILURE_SINK_CAPACITY`]) so a burst of failures
//! can never grow memory without limit; overflow evicts the OLDEST, keeping the
//! most recent diagnoses for the next Observe pass.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use crate::overseer::diagnosis::FailureDiagnosis;

/// Maximum number of step-failure diagnoses retained between Observe passes.
/// A cycle drains the whole buffer, so this bounds only the burst window.
pub const STEP_FAILURE_SINK_CAPACITY: usize = 64;

/// The process-global ring buffer. Lazily initialised; a poisoned lock is
/// recovered (the buffer is plain data) so a panic elsewhere never wedges
/// failure recording.
fn sink() -> &'static Mutex<VecDeque<FailureDiagnosis>> {
    static SINK: OnceLock<Mutex<VecDeque<FailureDiagnosis>>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(VecDeque::with_capacity(STEP_FAILURE_SINK_CAPACITY)))
}

/// Record a structured step-failure diagnosis for the next Observe pass to act
/// on. The bounded buffer evicts the oldest entry on overflow. This is the
/// "diagnose, don't just log" seam — callers pair it with the existing
/// diagnostic log so the failure is BOTH visible and actionable.
pub fn record_step_failure(diagnosis: FailureDiagnosis) {
    let mut buf = sink().lock().unwrap_or_else(|poison| poison.into_inner());
    if buf.len() >= STEP_FAILURE_SINK_CAPACITY {
        buf.pop_front();
    }
    buf.push_back(diagnosis);
}

/// Drain and return every recorded diagnosis (oldest first), emptying the sink.
/// Called once per Observe pass by the acting Overseer's `run_cycle`.
pub fn drain_recent() -> Vec<FailureDiagnosis> {
    let mut buf = sink().lock().unwrap_or_else(|poison| poison.into_inner());
    buf.drain(..).collect()
}
