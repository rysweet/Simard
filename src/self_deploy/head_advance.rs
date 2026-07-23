//! Head-advance + per-SHA dedupe for self-deploy (#4305 / #4387 / #4390).
//!
//! The existing time-based [`crate::self_relaunch`] throttle stops *per-tick*
//! thrash but has no memory of *which* head it last deployed, so a merged head
//! that already landed is re-evaluated every cycle (#4387) and a genuinely new
//! merged head is not reliably advanced onto (#4305). This module adds the
//! small, pure, file-backed layer that closes that gap:
//!
//! * [`is_valid_deploy_sha`] — argv-injection guard: only a 40- or 64-char
//!   lowercase-hex SHA may ever reach `git`/`systemctl`/`gh` argv.
//! * [`DeployHeadState`] — the durable "last head I deployed + its result",
//!   serialised alongside the other self-deploy state (mirrors
//!   `SelfRelaunchState`).
//! * [`should_deploy_target_sha`] — per-SHA dedupe: never redeploy a SHA that
//!   already SUCCEEDED; a FAILED attempt may retry (the time throttle still
//!   guards thrash).
//! * [`needs_head_advance`] — deploy only when the running head differs from
//!   the merged head and the merged head is verifiable.
//! * [`classify_unit_load`] / [`should_reconcile_unit`] — turn a
//!   `systemctl is-enabled` result into a "unit not loaded → reconcile"
//!   decision (mirrors [`crate::self_deploy::restart`]'s present-unit heuristic).
//!
//! All logic here is pure and hermetically unit-tested. It exposes DECISIONS
//! only — the effectful callers (`git rev-parse`, the atomic swap, `systemctl`)
//! in the orchestrator and restart modules are the intended consumers and must
//! invoke these helpers before acting. NOTE: this module is not yet wired into
//! the live self-deploy loop; the runtime integration is tracked as follow-up
//! (#4305 / #4387 / #4390) and lands in a separate, integration-testable change.

use serde::{Deserialize, Serialize};

/// Outcome of the most recent self-deploy attempt for [`DeployHeadState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployResult {
    /// The build-from-source deploy landed and verified live.
    Succeeded,
    /// The deploy attempt failed (and may be retried for the same SHA).
    Failed,
}

/// Durable record of the last head self-deploy acted on. File-backed JSON,
/// mirroring `SelfRelaunchState`, so per-SHA dedupe survives a restart and the
/// daemon does not redeploy an already-succeeded head every tick (#4387).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployHeadState {
    /// The target commit SHA the last deploy attempt was for. `None` before the
    /// first deploy.
    #[serde(default)]
    pub last_deploy_target_sha: Option<String>,
    /// The result of that last attempt. `None` before the first deploy.
    #[serde(default)]
    pub last_deploy_result: Option<DeployResult>,
}

/// Whether `sha` is a full, argv-safe git object id: exactly 40 (SHA-1) or 64
/// (SHA-256) characters, every one a **lowercase** hex digit. This rejects
/// uppercase, wrong-length, non-hex, whitespace-padded, and flag-like (`-…`)
/// values before any is passed to `git`/`systemctl`/`gh` — guarding against
/// argv option-injection. Fail-closed: anything not matching is invalid.
pub fn is_valid_deploy_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64)
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Decide whether to deploy `candidate_sha` given the last deploy state.
///
/// * An argv-unsafe candidate is NEVER deployed (fail-closed).
/// * A SHA that already SUCCEEDED is deduped — no redeploy (#4387).
/// * A different merged head always deploys (#4305 head advance).
/// * A SHA whose prior attempt FAILED may retry (the time throttle still
///   prevents per-tick thrash).
pub fn should_deploy_target_sha(state: &DeployHeadState, candidate_sha: &str) -> bool {
    if !is_valid_deploy_sha(candidate_sha) {
        return false;
    }
    !matches!(
        (state.last_deploy_target_sha.as_deref(), state.last_deploy_result),
        (Some(last), Some(DeployResult::Succeeded)) if last == candidate_sha
    )
}

/// Whether the running head must advance onto the merged head: the merged head
/// must be a verifiable argv-safe SHA AND differ from the running head. An
/// unverifiable / argv-unsafe merged target never triggers an advance
/// (fail-closed).
pub fn needs_head_advance(running_head: &str, merged_head: &str) -> bool {
    is_valid_deploy_sha(merged_head) && running_head != merged_head
}

/// Whether a systemd unit backing the deploy is loaded/known to systemd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitLoadState {
    /// The unit is known to systemd (enabled, static, or disabled).
    Loaded,
    /// systemd does not know the unit (`not found` / `no such unit` / `not
    /// loaded`) — the service-managed deploy path is missing.
    NotLoaded,
}

/// Classify a `systemctl is-enabled <unit>` result into a [`UnitLoadState`].
///
/// * A zero exit (`is_enabled_success == true`) means the unit is enabled →
///   [`UnitLoadState::Loaded`].
/// * A non-zero exit is ambiguous: a KNOWN-but-not-enabled unit (`static`,
///   `disabled`) still exits non-zero but is loaded; only an explicit
///   `not found` / `no such` / `not loaded` output means the unit is absent.
///   Mirrors the `systemd_unit_present` heuristic in
///   [`crate::self_deploy::restart`].
pub fn classify_unit_load(is_enabled_success: bool, output: &str) -> UnitLoadState {
    if is_enabled_success {
        return UnitLoadState::Loaded;
    }
    let lower = output.to_ascii_lowercase();
    if lower.contains("not found") || lower.contains("no such") || lower.contains("not loaded") {
        UnitLoadState::NotLoaded
    } else {
        UnitLoadState::Loaded
    }
}

/// Whether the missing/not-loaded systemd unit path should be reconciled so the
/// deploy becomes service-managed. Only a genuinely absent unit is reconciled;
/// a loaded (even disabled) unit is left alone.
pub fn should_reconcile_unit(state: UnitLoadState) -> bool {
    state == UnitLoadState::NotLoaded
}
