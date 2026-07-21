//! Process-group orphan guard: an RAII wrapper that guarantees no nested
//! subprocess subtree is orphaned when an orchestrator run fails, aborts, times
//! out, or panics.
//!
//! Cross-links `rysweet/amplihack-rs#964` (the same leak-on-failure bug class
//! in the upstream `recipe-runner-rs`). This Simard-side hardening is the
//! deliverable; the amplihack-rs fix is the upstream companion.
//!
//! See:
//! * `docs/reference/process-group-guard-api.md`
//! * `docs/concepts/nested-subprocess-orphan-guard.md`
//! * `docs/howto/add-a-process-group-guarded-spawn.md`

mod group_child;
mod probe;

pub use group_child::{DEFAULT_GRACE, GroupChild};
pub use probe::{LibcSignaller, ProcessGroupProbe};

#[cfg(test)]
mod tests;
