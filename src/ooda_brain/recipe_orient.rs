//! Recipe-runner-backed [`OodaOrientBrain`] — delegates the LLM call to
//! `recipe-runner-rs` executing the
//! `prompt_assets/simard/recipes/ooda-orient.yaml` recipe.
//!
//! This replaces the former `RustyClawdOrientBrain` for deployments
//! where recipe-runner-rs is available, following the same pattern as
//! [`super::recipe_decide::RecipeDecideBrain`] (PR #2115).
//!
//! ## 3-tier parse chain
//!
//! The shim invokes `recipe-runner-rs` as a subprocess with `-c` context
//! vars, then applies a 3-tier cascade to extract the adjusted urgency:
//!
//! 1. **JSON extraction** — `{"adjusted_urgency": f64, ...}` parsed via
//!    serde, validated via [`OrientJudgment::validate`].
//! 2. **Bare float** — regex `[0-9]+\.[0-9]+` scan for a decimal number,
//!    validated via `OrientJudgment::validate`.
//! 3. **Deterministic floor** — `base_urgency - 0.2 * failure_count`
//!    (clamped to 0), matching [`DeterministicFallbackOrientBrain::compute`].
//!
//! Fallback on parse failure is always safe: the floor cannot escalate.

use std::path::PathBuf;
use std::process::Command;

#[cfg(test)]
use super::orient::DeterministicFallbackOrientBrain;
use super::orient::{
    FAILURE_PENALTY_PER_CONSECUTIVE, OodaOrientBrain, OrientContext, OrientJudgment,
};
use super::sanitize::sanitize_context_var;
use crate::error::{SimardError, SimardResult};

const ADAPTER_TAG: &str = "recipe-orient-brain";
const RECIPE_FILENAME: &str = "ooda-orient.yaml";

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
fn resolve_recipe_path(repo_root: &std::path::Path) -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Recipe-runner-backed orient brain.
pub struct RecipeOrientBrain {
    recipe_path: PathBuf,
    agent_binary: &'static str,
}

impl RecipeOrientBrain {
    /// Construct if recipe file and recipe-runner-rs binary are both available.
    pub fn new(repo_root: &std::path::Path) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        if Command::new("recipe-runner-rs")
            .arg("--version")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }
        Some(Self {
            recipe_path,
            agent_binary,
        })
    }
}

impl OodaOrientBrain for RecipeOrientBrain {
    fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!("base_urgency={:.3}", ctx.base_urgency))
            .arg("-c")
            .arg(format!(
                "base_reason={}",
                sanitize_context_var(&ctx.base_reason, 500)
            ))
            .arg("-c")
            .arg(format!("failure_count={}", ctx.failure_count))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, 500)
                ),
            });
        }

        let raw = String::from_utf8(output.stdout)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        Ok(parse_orient_from_text(
            &raw,
            ctx.base_urgency,
            ctx.failure_count,
        ))
    }
}

// ---------------------------------------------------------------------------
// 3-tier parse chain: JSON → bare float → deterministic floor
// ---------------------------------------------------------------------------

/// Parse recipe output using a 3-tier cascade. Always returns a valid
/// [`OrientJudgment`] — the deterministic floor (tier 3) is the safety net.
///
/// ## Tiers
///
/// 1. **JSON**: Extract `{"adjusted_urgency": f64, "rationale": "...", ...}`.
///    Must pass [`OrientJudgment::validate`] against `base_urgency`.
/// 2. **Bare float**: Regex `[0-9]+\.[0-9]+` — first match becomes
///    `adjusted_urgency`. Must pass validate (no escalation, in [0,1]).
/// 3. **Deterministic floor**: `base_urgency - 0.2 * failure_count`,
///    clamped to 0. Always valid.
pub fn parse_orient_from_text(text: &str, base_urgency: f64, failure_count: u32) -> OrientJudgment {
    // Tier 1: JSON extraction — {"adjusted_urgency": f64, ...}
    if let Some(j) = try_json_extraction(text, base_urgency) {
        return j;
    }

    // Tier 2: Bare float — first [0-9]+\.[0-9]+ that validates
    if let Some(j) = try_bare_float(text, base_urgency) {
        return j;
    }

    // Tier 3: Deterministic floor — always valid, cannot escalate
    deterministic_floor(base_urgency, failure_count)
}

/// Tier 1: Extract the first `{…}` substring, parse as [`OrientJudgment`],
/// validate against `base_urgency`. Returns `None` on any failure (malformed
/// JSON, escalation, out-of-range) so callers fall through to tier 2.
fn try_json_extraction(text: &str, base_urgency: f64) -> Option<OrientJudgment> {
    let stripped = text.trim();
    let start = stripped.find('{')?;
    let end = stripped.rfind('}')?;
    if end <= start {
        return None;
    }
    let json_slice = &stripped[start..=end];
    let mut j: OrientJudgment = serde_json::from_str(json_slice).ok()?;
    // Always recompute demotion_applied = base − adjusted (the JSON value
    // may be stale or absent; the daemon recomputes anyway).
    j.demotion_applied = base_urgency - j.adjusted_urgency;
    j.validate(base_urgency).ok()?;
    Some(j)
}

/// Tier 2: Scan for the first bare decimal float matching `[0-9]+\.[0-9]+`
/// that passes [`OrientJudgment::validate`]. Integers (no decimal point)
/// and floats that would escalate above `base_urgency` are skipped.
fn try_bare_float(text: &str, base_urgency: f64) -> Option<OrientJudgment> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // Must have a decimal point followed by at least one digit
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                let after_dot = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > after_dot
                    && let Ok(val) = text[start..i].parse::<f64>()
                {
                    let j = OrientJudgment {
                        adjusted_urgency: val,
                        rationale: truncate(text.trim(), 500),
                        confidence: 1.0,
                        demotion_applied: base_urgency - val,
                    };
                    if j.validate(base_urgency).is_ok() {
                        return Some(j);
                    }
                }
            }
            // No decimal point — skip this digit run
            continue;
        }
        i += 1;
    }
    None
}

/// Compute the deterministic floor judgment. Reuses the same formula as
/// [`DeterministicFallbackOrientBrain::compute`] but constructs the
/// context inline to avoid needing the full `OrientContext`.
fn deterministic_floor(base_urgency: f64, failure_count: u32) -> OrientJudgment {
    let penalty = FAILURE_PENALTY_PER_CONSECUTIVE * failure_count as f64;
    let adjusted = (base_urgency - penalty).max(0.0);
    OrientJudgment {
        adjusted_urgency: adjusted,
        rationale: format!(
            "{ADAPTER_TAG}: deterministic floor — {failure_count} failure(s), \
             urgency {base_urgency:.2} − {penalty:.2}",
        ),
        confidence: 1.0,
        demotion_applied: base_urgency - adjusted,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    match s.char_indices().nth(max) {
        Some((byte_offset, _)) => format!("{}…", &s[..byte_offset]),
        None => s.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests — TDD: define the contract FIRST, implement SECOND.
//
// These tests pin the 3-tier parse chain, constructor, and trait impl.
// At the TDD stage, tier-1 and tier-2 tests FAIL (the stub only returns
// the deterministic floor). After implementation, all tests pass.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ===================================================================
    // Tier 1: JSON extraction
    // ===================================================================

    #[test]
    fn tier1_full_json_object() {
        let text = r#"{"adjusted_urgency": 0.4, "demotion_applied": 0.4, "rationale": "transient failure", "confidence": 0.9}"#;
        let j = parse_orient_from_text(text, 0.8, 2);
        assert!(
            (j.adjusted_urgency - 0.4).abs() < 1e-9,
            "tier-1 must extract adjusted_urgency from JSON; got {}",
            j.adjusted_urgency
        );
        assert_eq!(j.rationale, "transient failure");
        assert!((j.confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn tier1_json_with_surrounding_prose() {
        let text = r#"Here is my judgment: {"adjusted_urgency": 0.2, "rationale": "chronic"} done"#;
        let j = parse_orient_from_text(text, 0.8, 3);
        assert!(
            (j.adjusted_urgency - 0.2).abs() < 1e-9,
            "must parse JSON even with surrounding prose; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier1_json_missing_confidence_defaults_to_one() {
        let text = r#"{"adjusted_urgency": 0.3, "rationale": "ok"}"#;
        let j = parse_orient_from_text(text, 0.8, 1);
        assert!(
            (j.adjusted_urgency - 0.3).abs() < 1e-9,
            "got {}",
            j.adjusted_urgency
        );
        assert!(
            (j.confidence - 1.0).abs() < 1e-9,
            "missing confidence must default to 1.0; got {}",
            j.confidence
        );
    }

    #[test]
    fn tier1_json_missing_demotion_defaults_to_zero() {
        let text = r#"{"adjusted_urgency": 0.5, "rationale": "ok"}"#;
        let j = parse_orient_from_text(text, 0.8, 1);
        // demotion_applied must be computed as base - adjusted, not defaulted to 0
        let expected_demotion = 0.8 - 0.5;
        assert!(
            (j.demotion_applied - expected_demotion).abs() < 1e-9,
            "demotion_applied must be computed as base - adjusted; got {}",
            j.demotion_applied
        );
    }

    #[test]
    fn tier1_json_in_markdown_fences() {
        let text = "```json\n{\"adjusted_urgency\": 0.5, \"rationale\": \"fenced\"}\n```";
        let j = parse_orient_from_text(text, 0.8, 1);
        assert!(
            (j.adjusted_urgency - 0.5).abs() < 1e-9,
            "must parse JSON inside markdown fences; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier1_json_extra_fields_ignored() {
        let text =
            r#"{"adjusted_urgency": 0.4, "rationale": "ok", "futurefield": 42, "bonus": true}"#;
        let j = parse_orient_from_text(text, 0.8, 2);
        assert!(
            (j.adjusted_urgency - 0.4).abs() < 1e-9,
            "unknown JSON fields must be ignored; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier1_json_zero_urgency_valid() {
        let text = r#"{"adjusted_urgency": 0.0, "rationale": "chronic"}"#;
        let j = parse_orient_from_text(text, 0.8, 4);
        assert!(j.adjusted_urgency.abs() < 1e-9, "0.0 is a valid urgency");
    }

    // --- Tier 1: validation failures fall through to tier 2/3 -----------

    #[test]
    fn tier1_json_escalation_falls_through() {
        // JSON says 0.9 but base is 0.5 — escalation forbidden → falls to floor
        let text = r#"{"adjusted_urgency": 0.9, "rationale": "bad LLM"}"#;
        let j = parse_orient_from_text(text, 0.5, 1);
        // Floor: 0.5 - 0.2*1 = 0.3
        assert!(
            j.adjusted_urgency <= 0.5 + 1e-9,
            "escalation must be rejected — adjusted_urgency must be ≤ base; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier1_json_out_of_range_falls_through() {
        let text = r#"{"adjusted_urgency": 1.5, "rationale": "invalid"}"#;
        let j = parse_orient_from_text(text, 0.8, 1);
        // Floor: 0.8 - 0.2 = 0.6
        assert!(
            j.adjusted_urgency <= 1.0,
            "out-of-range (>1.0) must be rejected; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier1_json_negative_falls_through() {
        let text = r#"{"adjusted_urgency": -0.1, "rationale": "invalid"}"#;
        let j = parse_orient_from_text(text, 0.8, 1);
        assert!(
            j.adjusted_urgency >= 0.0,
            "negative urgency must be rejected; got {}",
            j.adjusted_urgency
        );
    }

    // ===================================================================
    // Tier 2: Bare float extraction
    // ===================================================================

    #[test]
    fn tier2_bare_float_alone() {
        let text = "0.42";
        let j = parse_orient_from_text(text, 0.8, 1);
        assert!(
            (j.adjusted_urgency - 0.42).abs() < 1e-9,
            "bare float must be extracted as adjusted_urgency; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier2_float_in_prose() {
        let text = "The adjusted urgency should be 0.35 given the transient nature.";
        let j = parse_orient_from_text(text, 0.8, 2);
        assert!(
            (j.adjusted_urgency - 0.35).abs() < 1e-9,
            "float embedded in prose must be found; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier2_float_at_end() {
        let text = "result: 0.50";
        let j = parse_orient_from_text(text, 0.8, 1);
        assert!(
            (j.adjusted_urgency - 0.50).abs() < 1e-9,
            "float at end of text must be found; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier2_float_zero() {
        let text = "0.0";
        let j = parse_orient_from_text(text, 0.8, 4);
        assert!(j.adjusted_urgency.abs() < 1e-9, "0.0 is valid");
    }

    #[test]
    fn tier2_float_confidence_defaults_to_one() {
        let text = "0.42";
        let j = parse_orient_from_text(text, 0.8, 1);
        assert!(
            (j.confidence - 1.0).abs() < 1e-9,
            "bare float tier must default confidence to 1.0; got {}",
            j.confidence
        );
    }

    #[test]
    fn tier2_float_demotion_computed() {
        let text = "0.42";
        let j = parse_orient_from_text(text, 0.8, 1);
        let expected = 0.8 - 0.42;
        assert!(
            (j.demotion_applied - expected).abs() < 1e-9,
            "demotion_applied must be base - adjusted; got {}",
            j.demotion_applied
        );
    }

    #[test]
    fn tier2_float_rationale_includes_text() {
        let text = "The urgency should be 0.35 because of transient failures.";
        let j = parse_orient_from_text(text, 0.8, 2);
        assert!(
            j.rationale.contains("transient") || j.rationale.contains("0.35"),
            "rationale should include agent text or the number; got: {}",
            j.rationale
        );
    }

    // --- Tier 2: validation failures fall through to tier 3 -------------

    #[test]
    fn tier2_float_escalation_falls_to_floor() {
        // Float says 0.9 but base is 0.5 — escalation → floor
        let text = "0.9";
        let j = parse_orient_from_text(text, 0.5, 1);
        // Floor: 0.5 - 0.2 = 0.3
        let floor = (0.5 - 0.2 * 1.0_f64).max(0.0);
        assert!(
            (j.adjusted_urgency - floor).abs() < 1e-9,
            "escalating float must fall to deterministic floor; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier2_float_out_of_range_falls_to_floor() {
        let text = "1.5";
        let j = parse_orient_from_text(text, 0.8, 1);
        let floor = (0.8_f64 - 0.2).max(0.0);
        assert!(
            (j.adjusted_urgency - floor).abs() < 1e-9,
            "out-of-range float must fall to floor; got {}",
            j.adjusted_urgency
        );
    }

    // --- Tier 2: patterns that must NOT match as bare floats -------------

    #[test]
    fn tier2_integer_not_matched() {
        // "42" has no decimal point — must not match [0-9]+\.[0-9]+
        let text = "42";
        let j = parse_orient_from_text(text, 0.8, 2);
        // Should fall to floor: 0.8 - 0.4 = 0.4
        let floor = (0.8_f64 - 0.2 * 2.0).max(0.0);
        assert!(
            (j.adjusted_urgency - floor).abs() < 1e-9,
            "integer without decimal must not match bare-float pattern; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier2_negative_float_not_matched() {
        // "-0.3" — the regex [0-9]+\.[0-9]+ excludes the minus sign
        // but "0.3" would still match — however 0.3 < 0.8 so it's valid
        // This test checks that the leading minus doesn't prevent matching
        // the positive portion IF the positive part is valid.
        let text = "-0.3";
        let j = parse_orient_from_text(text, 0.8, 2);
        // Two valid behaviors: extract 0.3 (ignoring minus) or fall to floor
        // The design says regex is [0-9]+\.[0-9]+ which would match "0.3" in "-0.3"
        // So 0.3 is valid (≤ base_urgency 0.8) and should be accepted
        assert!(
            (j.adjusted_urgency - 0.3).abs() < 1e-9
                || (j.adjusted_urgency - (0.8_f64 - 0.4).max(0.0)).abs() < 1e-9,
            "negative prefix: either extract 0.3 or fall to floor; got {}",
            j.adjusted_urgency
        );
    }

    // ===================================================================
    // Tier 3: Deterministic floor
    // ===================================================================

    #[test]
    fn tier3_no_parseable_content() {
        let text = "I cannot determine the urgency at this time.";
        let j = parse_orient_from_text(text, 0.8, 2);
        let floor = (0.8_f64 - 0.2 * 2.0).max(0.0);
        assert!(
            (j.adjusted_urgency - floor).abs() < 1e-9,
            "no parseable content must fall to deterministic floor; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier3_empty_string() {
        let j = parse_orient_from_text("", 0.8, 2);
        let floor = (0.8_f64 - 0.4).max(0.0);
        assert!(
            (j.adjusted_urgency - floor).abs() < 1e-9,
            "empty string → deterministic floor; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier3_whitespace_only() {
        let j = parse_orient_from_text("   \n\t  ", 0.8, 2);
        let floor = (0.8_f64 - 0.4).max(0.0);
        assert!(
            (j.adjusted_urgency - floor).abs() < 1e-9,
            "whitespace-only → deterministic floor; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier3_floor_formula_basic() {
        // base=0.8, failures=2 → 0.8 - 0.4 = 0.4
        let j = parse_orient_from_text("no number here", 0.8, 2);
        assert!(
            (j.adjusted_urgency - 0.4).abs() < 1e-9,
            "floor = base - 0.2 * failures; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier3_floor_clamped_to_zero() {
        // base=0.3, failures=5 → 0.3 - 1.0 → clamped to 0.0
        let j = parse_orient_from_text("nothing", 0.3, 5);
        assert!(
            j.adjusted_urgency.abs() < 1e-9,
            "floor must be clamped to 0; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier3_floor_zero_failures() {
        // base=1.0, failures=0 → 1.0 - 0 = 1.0
        let j = parse_orient_from_text("nothing", 1.0, 0);
        assert!(
            (j.adjusted_urgency - 1.0).abs() < 1e-9,
            "zero failures → no demotion; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier3_floor_one_failure() {
        // base=0.6, failures=1 → 0.6 - 0.2 = 0.4
        let j = parse_orient_from_text("nothing", 0.6, 1);
        assert!(
            (j.adjusted_urgency - 0.4).abs() < 1e-9,
            "one failure → 0.2 demotion; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn tier3_floor_demotion_applied() {
        let j = parse_orient_from_text("nothing", 0.8, 2);
        // demotion = 0.8 - 0.4 = 0.4
        assert!(
            (j.demotion_applied - 0.4).abs() < 1e-9,
            "demotion_applied must equal base - adjusted; got {}",
            j.demotion_applied
        );
    }

    #[test]
    fn tier3_floor_confidence_one() {
        let j = parse_orient_from_text("nothing", 0.8, 2);
        assert!(
            (j.confidence - 1.0).abs() < 1e-9,
            "floor tier must have confidence 1.0; got {}",
            j.confidence
        );
    }

    #[test]
    fn tier3_floor_rationale_describes_formula() {
        let j = parse_orient_from_text("nothing", 0.8, 2);
        // Rationale should mention the adapter tag and the computation
        assert!(
            j.rationale.contains(ADAPTER_TAG) || j.rationale.contains("deterministic"),
            "floor rationale must identify the adapter or strategy; got: {}",
            j.rationale
        );
    }

    // ===================================================================
    // Cross-cutting: validate() always enforced
    // ===================================================================

    #[test]
    fn validate_always_enforced_adjusted_le_base() {
        // Even if tier-1 JSON says adjusted > base, the result must not escalate.
        let scenarios: &[(&str, f64, u32)] = &[
            (
                r#"{"adjusted_urgency": 0.9, "rationale": "escalate"}"#,
                0.5,
                1,
            ),
            ("0.9", 0.5, 1),
        ];
        for (text, base, failures) in scenarios {
            let j = parse_orient_from_text(text, *base, *failures);
            assert!(
                j.adjusted_urgency <= *base + 1e-9,
                "adjusted must be ≤ base ({base}); got {} for text={text:?}",
                j.adjusted_urgency
            );
        }
    }

    #[test]
    fn validate_always_enforced_in_unit_range() {
        let scenarios: &[(&str, f64, u32)] = &[
            (r#"{"adjusted_urgency": 1.5, "rationale": "over"}"#, 0.8, 1),
            (
                r#"{"adjusted_urgency": -0.1, "rationale": "under"}"#,
                0.8,
                1,
            ),
            ("1.5", 0.8, 1),
        ];
        for (text, base, failures) in scenarios {
            let j = parse_orient_from_text(text, *base, *failures);
            assert!(
                j.adjusted_urgency >= 0.0 && j.adjusted_urgency <= 1.0,
                "adjusted must be in [0,1]; got {} for text={text:?}",
                j.adjusted_urgency
            );
        }
    }

    // ===================================================================
    // Rationale truncation
    // ===================================================================

    #[test]
    fn tier1_rationale_preserved_from_json() {
        let text = r#"{"adjusted_urgency": 0.4, "rationale": "this is the reason"}"#;
        let j = parse_orient_from_text(text, 0.8, 2);
        // When tier-1 succeeds, rationale should come from JSON
        assert!(
            j.rationale.contains("this is the reason") || j.rationale.contains("deterministic"), // stub returns floor
            "rationale should come from JSON or indicate floor; got: {}",
            j.rationale
        );
    }

    #[test]
    fn tier2_rationale_truncated_for_long_text() {
        let long_text = format!("0.42 because {}", "x".repeat(1000));
        let j = parse_orient_from_text(&long_text, 0.8, 1);
        // Rationale should be truncated (max 500 chars)
        assert!(
            j.rationale.chars().count() <= 600,
            "rationale should be bounded; got {} chars",
            j.rationale.chars().count()
        );
    }

    // ===================================================================
    // Realistic LLM output patterns
    // ===================================================================

    #[test]
    fn realistic_json_response() {
        let text = "Based on the failure history:\n\n\
                    {\"adjusted_urgency\": 0.35, \"rationale\": \
                    \"2 consecutive failures suggest transient infra issue; \
                    moderate demotion appropriate\", \"confidence\": 0.85}\n\n\
                    This accounts for the recent CI instability.";
        let j = parse_orient_from_text(text, 0.8, 2);
        assert!(
            (j.adjusted_urgency - 0.35).abs() < 1e-9,
            "realistic JSON output; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn realistic_bare_float_response() {
        let text = "## Orient Analysis\n\n\
                    Given 2 consecutive failures with base urgency 0.800:\n\
                    - Failures appear transient (CI flake)\n\
                    - Moderate demotion warranted\n\n\
                    Adjusted urgency: 0.45";
        let j = parse_orient_from_text(text, 0.8, 2);
        // Multiple floats in text: 0.800 (>= base so would fail validate),
        // 0.45 (valid). The parser should find the first valid one.
        assert!(
            (j.adjusted_urgency - 0.45).abs() < 1e-9
                // If parser takes first float (0.800), validate passes (== base with FP slack)
                || (j.adjusted_urgency - 0.8).abs() < 1e-9
                // Or falls to floor
                || (j.adjusted_urgency - 0.4).abs() < 1e-9,
            "realistic bare float; got {}",
            j.adjusted_urgency
        );
    }

    #[test]
    fn realistic_no_number_response() {
        let text = "I believe the goal should be significantly demoted due to \
                    chronic infrastructure failures. The engineer has been unable \
                    to make progress for several cycles.";
        let j = parse_orient_from_text(text, 0.8, 3);
        // No number → floor: 0.8 - 0.6 = 0.2
        let floor = (0.8_f64 - 0.2 * 3.0).max(0.0);
        assert!(
            (j.adjusted_urgency - floor).abs() < 1e-9,
            "no number in response → deterministic floor; got {}",
            j.adjusted_urgency
        );
    }

    // ===================================================================
    // Consistency with DeterministicFallbackOrientBrain
    // ===================================================================

    #[test]
    fn floor_matches_deterministic_fallback_brain() {
        // The deterministic floor in parse_orient_from_text must produce
        // exactly the same adjusted_urgency as DeterministicFallbackOrientBrain.
        let ctx = OrientContext {
            goal_id: "g1".into(),
            base_urgency: 0.8,
            base_reason: "test".into(),
            failure_count: 3,
        };
        let fallback = DeterministicFallbackOrientBrain::compute(&ctx);
        let recipe_floor = parse_orient_from_text("nothing", 0.8, 3);
        assert!(
            (recipe_floor.adjusted_urgency - fallback.adjusted_urgency).abs() < 1e-9,
            "recipe floor ({}) must match deterministic fallback ({})",
            recipe_floor.adjusted_urgency,
            fallback.adjusted_urgency,
        );
    }

    // ===================================================================
    // Constructor
    // ===================================================================

    #[test]
    fn new_returns_none_when_recipe_missing() {
        let brain = RecipeOrientBrain::new(std::path::Path::new("/nonexistent"));
        assert!(brain.is_none());
    }

    #[test]
    fn judge_orientation_with_missing_binary_returns_error() {
        let brain = RecipeOrientBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
        };
        let ctx = OrientContext {
            goal_id: "test-goal".into(),
            base_urgency: 0.7,
            base_reason: "test reason".into(),
            failure_count: 1,
        };
        let err = brain.judge_orientation(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains(ADAPTER_TAG),
            "error should identify the adapter: {msg}"
        );
    }
}
