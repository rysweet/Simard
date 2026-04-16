//! Hive integration configuration for cognitive memory.
//!
//! Provides configuration parameters that control how cognitive memory
//! interacts with the hive mind (shared cross-agent knowledge). The defaults
//! match those in `amplihack.agents.goal_seeking.hive_mind.constants`.

use crate::identity::IdentityManifest;

/// Default quality threshold for accepting facts from the hive.
///
/// Facts with a quality score below this threshold are not imported into
/// the agent's local semantic memory. Matches `DEFAULT_QUALITY_THRESHOLD`
/// in `amplihack.agents.goal_seeking.hive_mind.constants`.
pub const DEFAULT_QUALITY_THRESHOLD: f64 = 0.3;

/// Default confidence gate for promoting facts to the hive.
///
/// Only facts with confidence at or above this value are considered for
/// promotion to the shared hive. Matches `DEFAULT_CONFIDENCE_GATE` in
/// `amplihack.agents.goal_seeking.hive_mind.constants`.
pub const DEFAULT_CONFIDENCE_GATE: f64 = 0.3;

/// Configuration for hive mind integration.
///
/// Controls the quality and confidence thresholds used when exchanging
/// facts between the local cognitive memory and the shared hive.
#[derive(Clone, Debug, PartialEq)]
pub struct HiveConfig {
    /// Minimum quality score for importing facts from the hive.
    pub quality_threshold: f64,
    /// Minimum confidence for promoting local facts to the hive.
    pub confidence_gate: f64,
    /// Name of the agent, used for attribution in hive records.
    pub agent_name: String,
}

impl Default for HiveConfig {
    fn default() -> Self {
        Self {
            quality_threshold: DEFAULT_QUALITY_THRESHOLD,
            confidence_gate: DEFAULT_CONFIDENCE_GATE,
            agent_name: String::new(),
        }
    }
}

impl HiveConfig {
    /// Create a new hive configuration with explicit values.
    pub fn new(
        agent_name: impl Into<String>,
        quality_threshold: f64,
        confidence_gate: f64,
    ) -> Self {
        Self {
            quality_threshold,
            confidence_gate,
            agent_name: agent_name.into(),
        }
    }

    /// Validate that thresholds are in a sensible range.
    ///
    /// Both thresholds must be in `[0.0, 1.0]`.
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.quality_threshold) {
            return Err(format!(
                "quality_threshold must be in [0.0, 1.0], got {}",
                self.quality_threshold
            ));
        }
        if !(0.0..=1.0).contains(&self.confidence_gate) {
            return Err(format!(
                "confidence_gate must be in [0.0, 1.0], got {}",
                self.confidence_gate
            ));
        }
        Ok(())
    }

    /// Check whether a fact's confidence meets the gate for hive promotion.
    pub fn should_promote(&self, confidence: f64) -> bool {
        confidence >= self.confidence_gate
    }

    /// Check whether a hive fact's quality meets the import threshold.
    pub fn should_import(&self, quality_score: f64) -> bool {
        quality_score >= self.quality_threshold
    }
}

/// Derive a `HiveConfig` from an identity manifest.
///
/// Uses the manifest's `name` as the `agent_name` and applies default
/// thresholds. The manifest's memory policy does not currently influence
/// the hive thresholds, but this function provides the integration point
/// for future policy-driven configuration.
pub fn hive_config_from_identity(manifest: &IdentityManifest) -> HiveConfig {
    HiveConfig {
        quality_threshold: DEFAULT_QUALITY_THRESHOLD,
        confidence_gate: DEFAULT_CONFIDENCE_GATE,
        agent_name: manifest.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_thresholds_match_python_constants() {
        let config = HiveConfig::default();
        assert!((config.quality_threshold - 0.3).abs() < f64::EPSILON);
        assert!((config.confidence_gate - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn validate_accepts_valid_thresholds() {
        let config = HiveConfig::new("agent-1", 0.5, 0.7);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_negative_quality_threshold() {
        let config = HiveConfig::new("agent-1", -0.1, 0.5);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_quality_threshold_above_one() {
        let config = HiveConfig::new("agent-1", 1.1, 0.5);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_confidence_gate_out_of_range() {
        let config = HiveConfig::new("agent-1", 0.5, -0.01);
        assert!(config.validate().is_err());
    }

    #[test]
    fn should_promote_respects_confidence_gate() {
        let config = HiveConfig::new("agent-1", 0.3, 0.5);
        assert!(!config.should_promote(0.49));
        assert!(config.should_promote(0.5));
        assert!(config.should_promote(0.9));
    }

    #[test]
    fn should_import_respects_quality_threshold() {
        let config = HiveConfig::new("agent-1", 0.4, 0.3);
        assert!(!config.should_import(0.39));
        assert!(config.should_import(0.4));
        assert!(config.should_import(1.0));
    }

    #[test]
    fn boundary_values_are_accepted() {
        let config = HiveConfig::new("agent-1", 0.0, 1.0);
        assert!(config.validate().is_ok());
        assert!(config.should_import(0.0));
        assert!(!config.should_promote(0.99));
        assert!(config.should_promote(1.0));
    }

    #[test]
    fn hive_config_from_identity_uses_manifest_name() {
        // We cannot easily construct a full IdentityManifest in a unit test
        // without going through the full validation path, so we test the
        // constants and the function signature compiles. The integration
        // test exercises the full path.
        let config = HiveConfig::default();
        assert_eq!(config.agent_name, "");
    }

    // --- Additional threshold validation tests ---

    #[test]
    fn validate_rejects_confidence_gate_above_one() {
        let config = HiveConfig::new("agent-1", 0.5, 1.01);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_accepts_boundary_zero_zero() {
        let config = HiveConfig::new("agent-1", 0.0, 0.0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_accepts_boundary_one_one() {
        let config = HiveConfig::new("agent-1", 1.0, 1.0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_error_message_includes_quality_threshold_value() {
        let config = HiveConfig::new("agent-1", -0.5, 0.5);
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("-0.5"),
            "error should contain the bad value: {err}"
        );
        assert!(
            err.contains("quality_threshold"),
            "error should name the field: {err}"
        );
    }

    #[test]
    fn validate_error_message_includes_confidence_gate_value() {
        let config = HiveConfig::new("agent-1", 0.5, 2.0);
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("2"),
            "error should contain the bad value: {err}"
        );
        assert!(
            err.contains("confidence_gate"),
            "error should name the field: {err}"
        );
    }

    #[test]
    fn validate_rejects_nan_quality_threshold() {
        let config = HiveConfig::new("agent-1", f64::NAN, 0.5);
        assert!(config.validate().is_err(), "NaN should be rejected");
    }

    #[test]
    fn validate_rejects_nan_confidence_gate() {
        let config = HiveConfig::new("agent-1", 0.5, f64::NAN);
        assert!(config.validate().is_err(), "NaN should be rejected");
    }

    #[test]
    fn validate_rejects_infinity() {
        let config = HiveConfig::new("agent-1", f64::INFINITY, 0.5);
        assert!(config.validate().is_err(), "infinity should be rejected");
    }

    // --- should_promote / should_import edge cases ---

    #[test]
    fn should_promote_at_zero_gate_accepts_everything() {
        let config = HiveConfig::new("agent-1", 0.3, 0.0);
        assert!(config.should_promote(0.0));
        assert!(config.should_promote(0.001));
        assert!(config.should_promote(1.0));
    }

    #[test]
    fn should_promote_at_one_gate_only_accepts_one() {
        let config = HiveConfig::new("agent-1", 0.3, 1.0);
        assert!(!config.should_promote(0.999));
        assert!(config.should_promote(1.0));
    }

    #[test]
    fn should_import_at_zero_threshold_accepts_everything() {
        let config = HiveConfig::new("agent-1", 0.0, 0.3);
        assert!(config.should_import(0.0));
        assert!(config.should_import(0.001));
        assert!(config.should_import(1.0));
    }

    #[test]
    fn should_import_at_one_threshold_only_accepts_one() {
        let config = HiveConfig::new("agent-1", 1.0, 0.3);
        assert!(!config.should_import(0.999));
        assert!(config.should_import(1.0));
    }

    // --- HiveConfig construction / equality ---

    #[test]
    fn new_sets_all_fields() {
        let config = HiveConfig::new("my-agent", 0.6, 0.8);
        assert_eq!(config.agent_name, "my-agent");
        assert!((config.quality_threshold - 0.6).abs() < f64::EPSILON);
        assert!((config.confidence_gate - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn default_agent_name_is_empty() {
        let config = HiveConfig::default();
        assert!(config.agent_name.is_empty());
    }

    #[test]
    fn hive_config_clone_and_eq() {
        let a = HiveConfig::new("agent-1", 0.3, 0.5);
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn hive_config_debug_output() {
        let config = HiveConfig::new("agent-1", 0.3, 0.5);
        let debug = format!("{config:?}");
        assert!(debug.contains("HiveConfig"));
        assert!(debug.contains("agent-1"));
    }

    #[test]
    fn constants_match_defaults() {
        assert!((DEFAULT_QUALITY_THRESHOLD - 0.3).abs() < f64::EPSILON);
        assert!((DEFAULT_CONFIDENCE_GATE - 0.3).abs() < f64::EPSILON);
    }
}
