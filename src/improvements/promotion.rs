use std::collections::{BTreeMap, BTreeSet};

use crate::error::{SimardError, SimardResult};
use crate::goals::{GoalStatus, GoalUpdate};
use crate::review::{ImprovementProposal, ReviewArtifact};

use super::types::{
    DeferredImprovement, ImprovementDirective, ImprovementPromotionPlan, ImprovementProposalRecord,
    fallback_value, required_improvement_field, sanitize_directive_value,
};

impl ImprovementPromotionPlan {
    pub fn parse(raw: &str) -> SimardResult<Self> {
        let mut review_id = String::new();
        let mut review_target = String::new();
        let mut proposals = Vec::new();
        let mut approvals = Vec::new();
        let mut deferrals = Vec::new();

        for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let Some((label, value)) = line.split_once(':') else {
                continue;
            };
            let label = label.trim().to_ascii_lowercase();
            let value = value.trim();
            if value.is_empty() {
                continue;
            }

            match label.as_str() {
                "review-id" => review_id = required_improvement_field("review-id", value)?,
                "review-target" => {
                    review_target = required_improvement_field("review-target", value)?
                }
                "proposal" => proposals.push(parse_proposal_record(value)?),
                "approve" | "promote" => approvals.push(parse_approval_directive(
                    value,
                    (approvals.len() + 1) as u8,
                )?),
                "defer" => deferrals.push(parse_deferral_directive(value)?),
                _ => {}
            }
        }

        if review_id.is_empty() {
            return Err(SimardError::InvalidImprovementRecord {
                field: "review-id".to_string(),
                reason: "a review-id line is required".to_string(),
            });
        }
        if proposals.is_empty() {
            return Err(SimardError::InvalidImprovementRecord {
                field: "proposal".to_string(),
                reason: "at least one proposal line is required".to_string(),
            });
        }
        if approvals.is_empty() && deferrals.is_empty() {
            return Err(SimardError::InvalidImprovementRecord {
                field: "decision".to_string(),
                reason: "at least one approve: or defer: line is required".to_string(),
            });
        }

        let proposal_titles = proposals
            .iter()
            .map(|proposal| proposal.title.clone())
            .collect::<BTreeSet<_>>();
        let mut decided_titles = BTreeSet::new();
        for title in approvals
            .iter()
            .map(|directive| directive.title.clone())
            .chain(deferrals.iter().map(|directive| directive.title.clone()))
        {
            if !proposal_titles.contains(&title) {
                return Err(SimardError::InvalidImprovementRecord {
                    field: "decision".to_string(),
                    reason: format!("decision references unknown proposal '{title}'"),
                });
            }
            if !decided_titles.insert(title.clone()) {
                return Err(SimardError::InvalidImprovementRecord {
                    field: "decision".to_string(),
                    reason: format!("proposal '{title}' cannot be decided more than once"),
                });
            }
        }

        Ok(Self {
            review_id,
            review_target,
            proposals,
            approvals,
            deferrals,
        })
    }

    pub fn approved_goal_updates(&self) -> SimardResult<Vec<GoalUpdate>> {
        let proposals = self
            .proposals
            .iter()
            .map(|proposal| (proposal.title.clone(), proposal))
            .collect::<BTreeMap<_, _>>();

        self.approvals
            .iter()
            .map(|directive| {
                let proposal = proposals.get(&directive.title).ok_or_else(|| {
                    SimardError::InvalidImprovementRecord {
                        field: "approve".to_string(),
                        reason: format!(
                            "approved proposal '{}' was not supplied in the review context",
                            directive.title
                        ),
                    }
                })?;
                GoalUpdate::new(
                    proposal.title.clone(),
                    format!(
                        "{}; review={} target={} category={} suggested_change={}",
                        directive.rationale,
                        self.review_id,
                        fallback_value(&self.review_target, "unknown-target"),
                        proposal.category,
                        proposal.suggested_change
                    ),
                    directive.status,
                    directive.priority,
                )
            })
            .collect()
    }

    pub fn approval_summaries(&self) -> Vec<String> {
        self.approvals
            .iter()
            .map(|approval| {
                format!(
                    "p{} [{}] {}",
                    approval.priority, approval.status, approval.title
                )
            })
            .collect()
    }

    pub fn deferral_summaries(&self) -> Vec<String> {
        self.deferrals
            .iter()
            .map(|deferral| format!("{} ({})", deferral.title, deferral.rationale))
            .collect()
    }
}

pub fn render_review_context_directives(review: &ReviewArtifact) -> String {
    let mut lines = vec![
        format!("review-id: {}", sanitize_directive_value(&review.review_id)),
        format!(
            "review-target: {}",
            sanitize_directive_value(&review.target_label)
        ),
    ];
    for proposal in &review.proposals {
        lines.push(render_proposal_directive(proposal));
    }
    lines.join("\n")
}

fn render_proposal_directive(proposal: &ImprovementProposal) -> String {
    let evidence = if proposal.evidence.is_empty() {
        "none".to_string()
    } else {
        proposal
            .evidence
            .iter()
            .map(|item| sanitize_directive_value(item))
            .collect::<Vec<_>>()
            .join(" ;; ")
    };
    format!(
        "proposal: {} | category={} | rationale={} | suggested_change={} | evidence={}",
        sanitize_directive_value(&proposal.title),
        sanitize_directive_value(&proposal.category),
        sanitize_directive_value(&proposal.rationale),
        sanitize_directive_value(&proposal.suggested_change),
        evidence
    )
}

fn parse_proposal_record(raw: &str) -> SimardResult<ImprovementProposalRecord> {
    let mut segments = raw
        .split('|')
        .map(str::trim)
        .filter(|segment| !segment.is_empty());
    let title = required_improvement_field(
        "proposal.title",
        segments
            .next()
            .ok_or_else(|| SimardError::InvalidImprovementRecord {
                field: "proposal".to_string(),
                reason: "proposal entries must include a title before attributes".to_string(),
            })?,
    )?;
    let mut category = String::new();
    let mut rationale = String::new();
    let mut suggested_change = String::new();
    let mut evidence = Vec::new();

    for segment in segments {
        let (key, value) =
            segment
                .split_once('=')
                .ok_or_else(|| SimardError::InvalidImprovementRecord {
                    field: "proposal".to_string(),
                    reason: format!("proposal attribute '{segment}' must look like key=value"),
                })?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "category" => category = required_improvement_field("proposal.category", value)?,
            "rationale" => rationale = required_improvement_field("proposal.rationale", value)?,
            "suggested_change" | "suggested-change" => {
                suggested_change = required_improvement_field("proposal.suggested_change", value)?
            }
            "evidence" => {
                evidence = value
                    .split(";;")
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            _ => {
                return Err(SimardError::InvalidImprovementRecord {
                    field: key,
                    reason: "unsupported proposal attribute".to_string(),
                });
            }
        }
    }

    Ok(ImprovementProposalRecord {
        category: required_improvement_field("proposal.category", &category)?,
        title,
        rationale: required_improvement_field("proposal.rationale", &rationale)?,
        suggested_change: required_improvement_field(
            "proposal.suggested_change",
            &suggested_change,
        )?,
        evidence,
    })
}

fn parse_approval_directive(raw: &str, default_priority: u8) -> SimardResult<ImprovementDirective> {
    let mut segments = raw
        .split('|')
        .map(str::trim)
        .filter(|segment| !segment.is_empty());
    let title = required_improvement_field(
        "approve.title",
        segments
            .next()
            .ok_or_else(|| SimardError::InvalidImprovementRecord {
                field: "approve".to_string(),
                reason: "approve entries must include a proposal title".to_string(),
            })?,
    )?;
    let mut priority = default_priority.max(1);
    let mut status = GoalStatus::Proposed;
    let mut rationale =
        "operator approved this improvement proposal for durable tracking".to_string();

    for segment in segments {
        let (key, value) =
            segment
                .split_once('=')
                .ok_or_else(|| SimardError::InvalidImprovementRecord {
                    field: "approve".to_string(),
                    reason: format!("approve attribute '{segment}' must look like key=value"),
                })?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "priority" => {
                priority =
                    value
                        .parse::<u8>()
                        .map_err(|_| SimardError::InvalidImprovementRecord {
                            field: "approve.priority".to_string(),
                            reason: "priority must be a positive integer".to_string(),
                        })?;
                if priority == 0 {
                    return Err(SimardError::InvalidImprovementRecord {
                        field: "approve.priority".to_string(),
                        reason: "priority must be at least 1".to_string(),
                    });
                }
            }
            "status" => {
                status = GoalStatus::parse(value).ok_or_else(|| {
                    SimardError::InvalidImprovementRecord {
                        field: "approve.status".to_string(),
                        reason: "status must be active, proposed, paused, or completed".to_string(),
                    }
                })?
            }
            "rationale" => rationale = required_improvement_field("approve.rationale", value)?,
            _ => {
                return Err(SimardError::InvalidImprovementRecord {
                    field: key,
                    reason: "unsupported approve attribute".to_string(),
                });
            }
        }
    }

    Ok(ImprovementDirective {
        title,
        priority,
        status,
        rationale,
    })
}

fn parse_deferral_directive(raw: &str) -> SimardResult<DeferredImprovement> {
    let mut segments = raw
        .split('|')
        .map(str::trim)
        .filter(|segment| !segment.is_empty());
    let title = required_improvement_field(
        "defer.title",
        segments
            .next()
            .ok_or_else(|| SimardError::InvalidImprovementRecord {
                field: "defer".to_string(),
                reason: "defer entries must include a proposal title".to_string(),
            })?,
    )?;
    let mut rationale = "operator deferred this proposal for later review".to_string();

    for segment in segments {
        let (key, value) =
            segment
                .split_once('=')
                .ok_or_else(|| SimardError::InvalidImprovementRecord {
                    field: "defer".to_string(),
                    reason: format!("defer attribute '{segment}' must look like key=value"),
                })?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "rationale" => rationale = required_improvement_field("defer.rationale", value)?,
            _ => {
                return Err(SimardError::InvalidImprovementRecord {
                    field: key,
                    reason: "unsupported defer attribute".to_string(),
                });
            }
        }
    }

    Ok(DeferredImprovement { title, rationale })
}
