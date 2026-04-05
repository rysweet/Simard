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
