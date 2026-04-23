//! Source-module → target-repo routing for the stewardship loop.
//!
//! Fail-loud on ambiguity — there is **no default repo**.

use super::types::TargetRepo;
use crate::error::{SimardError, SimardResult};

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
/// Returns [`SimardError::StewardshipRoutingAmbiguous`] when no keyword matches.
pub fn route_failure(source_module: &str) -> SimardResult<TargetRepo> {
    let lc = source_module.to_lowercase();
    if AMPLIHACK_KEYWORDS.iter().any(|kw| lc.contains(kw)) {
        return Ok(TargetRepo::Amplihack);
    }
    if SIMARD_KEYWORDS.iter().any(|kw| lc.contains(kw)) {
        return Ok(TargetRepo::Simard);
    }
    Err(SimardError::StewardshipRoutingAmbiguous {
        source: source_module.to_string(),
    })
}
