//! Canary deployment and handover for self-relaunch.
//!
//! Gate sequence: Smoke -> UnitTest -> GymBaseline -> BridgeHealth.
//! All gates must pass before handover. Failures reject the canary (Pillar 11).
//!
//! For coordinated multi-process handoff with leader election, see
//! [`coordinated_relaunch`] which uses [`self_relaunch_semaphore`].

mod canary;
mod gates;
mod types;

// Re-export all public items so `crate::self_relaunch::X` still works.
pub use canary::{build_canary, coordinated_relaunch, handover};
pub use gates::{all_gates_passed, verify_canary};
pub use types::{GateResult, RelaunchConfig, RelaunchGate, default_gates};
