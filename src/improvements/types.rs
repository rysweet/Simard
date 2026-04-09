use crate::error::SimardResult;
use crate::goals::GoalStatus;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImprovementDirective {
    pub title: String,
    pub priority: u8,
    pub status: GoalStatus,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredImprovement {
    pub title: String,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImprovementProposalRecord {
    pub category: String,
    pub title: String,
    pub rationale: String,
    pub suggested_change: String,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImprovementPromotionPlan {
    pub review_id: String,
    pub review_target: String,
    pub proposals: Vec<ImprovementProposalRecord>,
    pub approvals: Vec<ImprovementDirective>,
    pub deferrals: Vec<DeferredImprovement>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedImprovementApproval {
    pub priority: u8,
    pub status: GoalStatus,
    pub title: String,
}

impl PersistedImprovementApproval {
    pub fn concise_label(&self) -> String {
        format!("p{} [{}] {}", self.priority, self.status, self.title)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedImprovementRecord {
    pub review_id: String,
    pub review_target: String,
    pub approved_proposals: Vec<PersistedImprovementApproval>,
    pub deferred_proposals: Vec<DeferredImprovement>,
    pub selected_base_type: Option<String>,
    pub topology: Option<String>,
    pub outcome: Option<String>,
}

pub(super) fn required_improvement_field(
    field: &str,
    value: impl AsRef<str>,
) -> SimardResult<String> {
    let trimmed = value.as_ref().trim();
    if trimmed.is_empty() {
        return Err(crate::error::SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

pub(super) fn sanitize_directive_value(value: &str) -> String {
    value
        .replace('\n', " ")
        .replace('|', "/")
        .replace(";;", ";")
        .trim()
        .to_string()
}

pub(super) fn fallback_value<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_approval_concise_label() {
        let approval = PersistedImprovementApproval {
            priority: 1,
            status: GoalStatus::Active,
            title: "optimize cache".to_string(),
        };
        let label = approval.concise_label();
        assert_eq!(label, "p1 [active] optimize cache");
    }

    #[test]
    fn required_improvement_field_valid() {
        let result = required_improvement_field("title", "hello");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn required_improvement_field_trims_whitespace() {
        let result = required_improvement_field("title", "  hello  ");
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn required_improvement_field_empty_returns_error() {
        let result = required_improvement_field("title", "");
        assert!(result.is_err());
    }

    #[test]
    fn required_improvement_field_whitespace_only_returns_error() {
        let result = required_improvement_field("title", "   ");
        assert!(result.is_err());
    }

    #[test]
    fn sanitize_directive_value_replaces_newlines() {
        assert_eq!(sanitize_directive_value("a\nb"), "a b");
    }

    #[test]
    fn sanitize_directive_value_replaces_pipes() {
        assert_eq!(sanitize_directive_value("a|b"), "a/b");
    }

    #[test]
    fn sanitize_directive_value_replaces_double_semicolons() {
        assert_eq!(sanitize_directive_value("a;;b"), "a;b");
    }

    #[test]
    fn sanitize_directive_value_trims() {
        assert_eq!(sanitize_directive_value("  hello  "), "hello");
    }

    #[test]
    fn fallback_value_uses_value_when_non_empty() {
        assert_eq!(fallback_value("hello", "default"), "hello");
    }

    #[test]
    fn fallback_value_uses_fallback_when_empty() {
        assert_eq!(fallback_value("", "default"), "default");
    }

    #[test]
    fn fallback_value_uses_fallback_when_whitespace() {
        assert_eq!(fallback_value("   ", "default"), "default");
    }

    #[test]
    fn improvement_directive_construction() {
        let d = ImprovementDirective {
            title: "t".to_string(),
            priority: 3,
            status: GoalStatus::Proposed,
            rationale: "r".to_string(),
        };
        assert_eq!(d.priority, 3);
        assert_eq!(d.status, GoalStatus::Proposed);
    }

    #[test]
    fn improvement_promotion_plan_construction() {
        let plan = ImprovementPromotionPlan {
            review_id: "r1".to_string(),
            review_target: "target".to_string(),
            proposals: vec![],
            approvals: vec![],
            deferrals: vec![],
        };
        assert!(plan.proposals.is_empty());
        assert!(plan.approvals.is_empty());
        assert!(plan.deferrals.is_empty());
    }
}
