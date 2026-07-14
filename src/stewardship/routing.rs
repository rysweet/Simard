//! Source-module → target-repo routing for the stewardship loop.
//!
//! Routing is **total**: a matched keyword pins the repo, and any
//! `source_module` that matches no keyword falls back to
//! [`DEFAULT_TARGET_REPO`] (`rysweet/Simard`), logged via `tracing::warn!`.
//! Routing therefore never errors and never drops a failure on the floor.

use super::types::TargetRepo;
use crate::error::SimardResult;

/// The one named source of truth for the default target repo. An unmatched
/// `source_module` routes here rather than erroring, so gap/issue briefs whose
/// source has no keyword (e.g. the Overseer's `"overseer"` workstream-gap
/// briefs) always land in a real repo instead of failing every tick.
const DEFAULT_TARGET_REPO: TargetRepo = TargetRepo::Simard;

/// Keywords that pin a failure to the amplihack workflow runtime. Checked
/// first so an `amplihack::engineer_loop` source pins to amplihack.
const AMPLIHACK_KEYWORDS: &[&str] = &["amplihack", "recipe-runner", "orchestrator", "recipe::"];

/// Keywords that pin a failure to Simard's own subsystems.
const SIMARD_KEYWORDS: &[&str] = &[
    "engineer_loop",
    "base_type",
    "self_improve",
    "goal_curation",
    "agent_loop",
    "session_builder",
    "simard::",
];

/// Route a `source_module` string (e.g. `"simard::engineer_loop"`) to the
/// target repo for issue filing.
///
/// Total over all inputs. Keyword sets are checked first (amplihack before
/// Simard). If **no** keyword matches, the source falls back to
/// [`DEFAULT_TARGET_REPO`] (`rysweet/Simard`) and the fallback is recorded with
/// `tracing::warn!` — routing never returns an error and never drops a source.
pub fn route_failure(source_module: &str) -> SimardResult<TargetRepo> {
    let lc = source_module.to_lowercase();
    if AMPLIHACK_KEYWORDS.iter().any(|kw| lc.contains(kw)) {
        return Ok(TargetRepo::Amplihack);
    }
    if SIMARD_KEYWORDS.iter().any(|kw| lc.contains(kw)) {
        return Ok(TargetRepo::Simard);
    }
    tracing::warn!(
        default = DEFAULT_TARGET_REPO.slug(),
        "stewardship routing: no keyword match, routing to default repo"
    );
    Ok(DEFAULT_TARGET_REPO)
}
