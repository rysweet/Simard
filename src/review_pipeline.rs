//! LLM-driven code review pipeline for the engineer loop.
//!
//! Provides [`ReviewFinding`] with [`FindingCategory`] and [`Severity`],
//! [`ReviewSession`] for managing LLM interactions, [`review_diff`] for
//! LLM-based code review, [`should_commit`] as a commit gate, and
//! [`summarize_review`] for human-readable output.

use serde::{Deserialize, Serialize};

use crate::base_types::BaseTypeTurnInput;
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::session_builder::{LlmProvider, SessionBuilder};

/// Severity level of a review finding, ordered from least to most severe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// Category of a review finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingCategory {
    Bug,
    Style,
    Architecture,
    Security,
}

impl FindingCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Style => "style",
            Self::Architecture => "architecture",
            Self::Security => "security",
        }
    }
}

/// A single finding produced by the LLM code review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub category: FindingCategory,
    pub severity: Severity,
    pub description: String,
    pub file_path: String,
    pub line_range: Option<(usize, usize)>,
}

/// Wraps a `BaseTypeSession` for LLM-driven code review.
///
/// Mirrors the `SessionBuilder` pattern from [`crate::engineer_plan`]:
/// open via `SessionBuilder`, call `run_turn`, parse structured JSON.
pub struct ReviewSession {
    session: Box<dyn crate::base_types::BaseTypeSession>,
}

impl ReviewSession {
    /// Open a review LLM session. Returns an error if the adapter is unavailable.
    pub fn open() -> SimardResult<Self> {
        let provider = LlmProvider::resolve().map_err(|e| SimardError::ReviewUnavailable {
            reason: format!("LLM provider unavailable for review pipeline: {e}"),
        })?;
        let session = SessionBuilder::new(OperatingMode::Engineer, provider)
            .node_id("review-pipeline")
            .address("review-pipeline://local")
            .adapter_tag("review-pipeline")
            .open()
            .map_err(|e| SimardError::ReviewUnavailable {
                reason: format!("review session open() failed: {e}"),
            })?;
        Ok(Self { session })
    }

    /// Close the underlying LLM session.
    pub fn close(mut self) -> SimardResult<()> {
        self.session.close()
    }
}

const REVIEW_INSTRUCTIONS: &str = include_str!("../prompt_assets/simard/review_pipeline.md");

fn build_review_prompt(diff_text: &str, philosophy_guidelines: &str) -> String {
    format!(
        "{}\n\nPhilosophy guidelines:\n{philosophy_guidelines}\n\n\
         Diff to review:\n{diff_text}",
        REVIEW_INSTRUCTIONS.trim(),
    )
}

fn parse_review_response(text: &str) -> SimardResult<Vec<ReviewFinding>> {
    let trimmed = text.trim();
    let json_text = if trimmed.starts_with("```") {
        let inner = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        inner.strip_suffix("```").unwrap_or(inner).trim()
    } else {
        trimmed
    };
    serde_json::from_str(json_text).map_err(|e| SimardError::ReviewUnavailable {
        reason: format!("failed to parse LLM review response: {e}"),
    })
}

/// Ask the LLM to review a diff against philosophy guidelines.
///
/// Builds a prompt, calls `run_turn` on the session, and parses the
/// structured JSON response into [`ReviewFinding`]s.
pub fn review_diff(
    session: &mut ReviewSession,
    diff_text: &str,
    philosophy_guidelines: &str,
) -> SimardResult<Vec<ReviewFinding>> {
    let prompt = build_review_prompt(diff_text, philosophy_guidelines);
    let outcome = session
        .session
        .run_turn(BaseTypeTurnInput::objective_only(prompt))
        .map_err(|e| SimardError::ReviewUnavailable {
            reason: format!("LLM turn failed: {e}"),
        })?;
    // Read execution_summary (the actual LLM text), not plan (adapter
    // telemetry). See engineer_plan::plan_objective for context.
    parse_review_response(&outcome.execution_summary)
}

/// Commit gate: returns `false` if any Bug or Security finding has
/// severity >= High, meaning the commit should be blocked.
pub fn should_commit(findings: &[ReviewFinding]) -> bool {
    !findings.iter().any(|f| {
        matches!(f.category, FindingCategory::Bug | FindingCategory::Security)
            && f.severity >= Severity::High
    })
}

/// Produce a human-readable review summary from findings.
pub fn summarize_review(findings: &[ReviewFinding]) -> String {
    if findings.is_empty() {
        return "Review passed: no findings.".to_string();
    }
    let mut lines = vec![format!("Review complete: {} finding(s).", findings.len())];
    for (i, f) in findings.iter().enumerate() {
        let range = match f.line_range {
            Some((start, end)) => format!(" (lines {start}-{end})"),
            None => String::new(),
        };
        lines.push(format!(
            "  {}. [{}/{}] {}{}: {}",
            i + 1,
            f.category.as_str(),
            f.severity.as_str(),
            f.file_path,
            range,
            f.description
        ));
    }
    if !should_commit(findings) {
        lines.push(
            "BLOCKED: commit should not proceed due to high-severity bug or security finding(s)."
                .to_string(),
        );
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bug(severity: Severity) -> ReviewFinding {
        ReviewFinding {
            category: FindingCategory::Bug,
            severity,
            description: "null pointer dereference".into(),
            file_path: "src/main.rs".into(),
            line_range: Some((10, 15)),
        }
    }

    fn security(severity: Severity) -> ReviewFinding {
        ReviewFinding {
            category: FindingCategory::Security,
            severity,
            description: "unsanitized input".into(),
            file_path: "src/api.rs".into(),
            line_range: Some((42, 42)),
        }
    }

    fn style() -> ReviewFinding {
        ReviewFinding {
            category: FindingCategory::Style,
            severity: Severity::Low,
            description: "inconsistent naming".into(),
            file_path: "src/lib.rs".into(),
            line_range: None,
        }
    }

    fn architecture() -> ReviewFinding {
        ReviewFinding {
            category: FindingCategory::Architecture,
            severity: Severity::High,
            description: "tight coupling between modules".into(),
            file_path: "src/engine.rs".into(),
            line_range: Some((1, 100)),
        }
    }

    #[test]
    fn finding_construction_and_severity_ordering() {
        let f = bug(Severity::Medium);
        assert_eq!(f.category, FindingCategory::Bug);
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.file_path, "src/main.rs");
        assert_eq!(f.line_range, Some((10, 15)));

        assert!(Severity::Low < Severity::Medium);
        assert!(Severity::Medium < Severity::High);
        assert!(Severity::High < Severity::Critical);
    }
    #[test]
    fn severity_as_str() {
        assert_eq!(Severity::Low.as_str(), "low");
        assert_eq!(Severity::Medium.as_str(), "medium");
        assert_eq!(Severity::High.as_str(), "high");
        assert_eq!(Severity::Critical.as_str(), "critical");
    }
    #[test]
    fn finding_category_as_str() {
        assert_eq!(FindingCategory::Bug.as_str(), "bug");
        assert_eq!(FindingCategory::Style.as_str(), "style");
        assert_eq!(FindingCategory::Architecture.as_str(), "architecture");
        assert_eq!(FindingCategory::Security.as_str(), "security");
    }
    #[test]
    fn should_commit_allows_low_severity() {
        assert!(should_commit(&[]));
        assert!(should_commit(&[bug(Severity::Low)]));
        assert!(should_commit(&[bug(Severity::Medium)]));
        assert!(should_commit(&[security(Severity::Low)]));
        assert!(should_commit(&[security(Severity::Medium)]));
        assert!(should_commit(&[style()]));
        assert!(should_commit(&[architecture()]));
    }
    #[test]
    fn should_commit_blocks_high_severity_bug() {
        assert!(!should_commit(&[bug(Severity::High)]));
        assert!(!should_commit(&[bug(Severity::Critical)]));
    }
    #[test]
    fn should_commit_blocks_high_severity_security() {
        assert!(!should_commit(&[security(Severity::High)]));
        assert!(!should_commit(&[security(Severity::Critical)]));
    }
    #[test]
    fn should_commit_mixed_findings() {
        let findings = vec![style(), bug(Severity::Low), security(Severity::High)];
        assert!(!should_commit(&findings));
    }
    #[test]
    fn parse_review_response_valid_json() {
        let json = r#"[{"category":"bug","severity":"high","description":"off-by-one","file_path":"src/lib.rs","line_range":[10,12]}]"#;
        let findings = parse_review_response(json).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, FindingCategory::Bug);
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].description, "off-by-one");
        assert_eq!(findings[0].line_range, Some((10, 12)));
    }
    #[test]
    fn parse_review_response_with_fences() {
        let json = "```json\n[{\"category\":\"style\",\"severity\":\"low\",\"description\":\"naming\",\"file_path\":\"a.rs\",\"line_range\":null}]\n```";
        let findings = parse_review_response(json).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, FindingCategory::Style);
        assert!(findings[0].line_range.is_none());
    }
    #[test]
    fn parse_review_response_empty_array() {
        assert!(parse_review_response("[]").unwrap().is_empty());
    }
    #[test]
    fn parse_review_response_invalid_json() {
        match parse_review_response("not json").unwrap_err() {
            SimardError::ReviewUnavailable { reason } => {
                assert!(reason.contains("failed to parse"));
            }
            other => panic!("expected ReviewUnavailable, got: {other}"),
        }
    }
    #[test]
    fn build_review_prompt_contains_context() {
        let prompt = build_review_prompt("diff --git a/foo", "keep it simple");
        assert!(prompt.contains("diff --git a/foo"));
        assert!(prompt.contains("keep it simple"));
        assert!(prompt.contains("JSON array"));
        assert!(prompt.contains("category"));
    }
    #[test]
    fn summarize_review_empty() {
        assert_eq!(summarize_review(&[]), "Review passed: no findings.");
    }
    #[test]
    fn summarize_review_with_findings() {
        let findings = vec![bug(Severity::Low), style()];
        let summary = summarize_review(&findings);
        assert!(summary.contains("2 finding(s)"));
        assert!(summary.contains("[bug/low]"));
        assert!(summary.contains("src/main.rs (lines 10-15)"));
        assert!(summary.contains("[style/low]"));
        assert!(summary.contains("src/lib.rs:"));
        assert!(!summary.contains("BLOCKED"));
    }
    #[test]
    fn summarize_review_blocked() {
        let findings = vec![security(Severity::Critical)];
        let summary = summarize_review(&findings);
        assert!(summary.contains("BLOCKED"));
        assert!(summary.contains("high-severity"));
    }
    #[test]
    fn finding_serialization_round_trip() {
        let f = bug(Severity::High);
        let json = serde_json::to_string(&f).unwrap();
        let deserialized: ReviewFinding = serde_json::from_str(&json).unwrap();
        assert_eq!(f, deserialized);
    }
    #[test]
    fn finding_null_line_range_serialization() {
        let f = style();
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("null"));
        let deserialized: ReviewFinding = serde_json::from_str(&json).unwrap();
        assert_eq!(f, deserialized);
    }
    #[test]
    fn review_session_open_without_api_key_does_not_panic() {
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
        // Default provider (Copilot) may or may not open; no panic is the invariant.
        let _ = ReviewSession::open();
    }
    #[test]
    fn parse_review_response_multiple_findings() {
        let json = r#"[
            {"category":"bug","severity":"critical","description":"crash","file_path":"a.rs","line_range":[1,5]},
            {"category":"security","severity":"high","description":"injection","file_path":"b.rs","line_range":null},
            {"category":"style","severity":"low","description":"fmt","file_path":"c.rs","line_range":[3,3]}
        ]"#;
        let findings = parse_review_response(json).unwrap();
        assert_eq!(findings.len(), 3);
        assert_eq!(findings[0].severity, Severity::Critical);
        assert_eq!(findings[1].category, FindingCategory::Security);
        assert!(findings[1].line_range.is_none());
    }
    #[test]
    fn should_commit_architecture_high_does_not_block() {
        assert!(should_commit(&[architecture()]));
    }
}
