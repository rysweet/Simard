use std::collections::BTreeSet;

use crate::error::{SimardError, SimardResult};
use crate::goals::GoalStatus;

use super::parsing::{
    parse_bracketed_list, parse_non_negative_count, parse_persisted_record_pairs,
};
use super::types::{
    DeferredImprovement, PersistedImprovementApproval, PersistedImprovementRecord,
    required_improvement_field,
};

impl PersistedImprovementRecord {
    pub fn parse(raw: &str) -> SimardResult<Self> {
        let pairs = parse_persisted_record_pairs(raw)?;
        let mut seen = BTreeSet::new();
        let mut review_id = None;
        let mut review_target = None;
        let mut approved_proposals = None;
        let mut approval_count = None;
        let mut deferred_proposals = None;
        let mut deferral_count = None;
        let mut selected_base_type = None;
        let mut topology = None;
        let mut outcome = None;

        for (field, value) in pairs {
            if !seen.insert(field) {
                return Err(SimardError::InvalidImprovementRecord {
                    field: field.to_string(),
                    reason: "field cannot appear more than once".to_string(),
                });
            }

            match field {
                "review" => review_id = Some(required_improvement_field("review", value)?),
                "target" => review_target = Some(required_improvement_field("target", value)?),
                "approvals" => {
                    if value.trim_start().starts_with('[') {
                        approved_proposals =
                            Some(parse_persisted_approval_list("approvals", value)?);
                    } else {
                        approval_count = Some(parse_non_negative_count("approvals", value)?);
                    }
                }
                "approved_goals" => {
                    approved_proposals =
                        Some(parse_persisted_approval_list("approved_goals", value)?);
                }
                "deferred" => {
                    deferred_proposals = Some(parse_persisted_deferral_list("deferred", value)?);
                }
                "deferrals" => {
                    deferral_count = Some(parse_non_negative_count("deferrals", value)?);
                }
                "selected-base-type" => {
                    selected_base_type =
                        Some(required_improvement_field("selected-base-type", value)?);
                }
                "topology" => topology = Some(required_improvement_field("topology", value)?),
                "outcome" => outcome = Some(required_improvement_field("outcome", value)?),
                other => {
                    return Err(SimardError::InvalidImprovementRecord {
                        field: other.to_string(),
                        reason: "unsupported persisted improvement field".to_string(),
                    });
                }
            }
        }

        let approved_proposals =
            approved_proposals.ok_or_else(|| SimardError::InvalidImprovementRecord {
                field: "approvals".to_string(),
                reason: "approved proposal list is required".to_string(),
            })?;
        let deferred_proposals =
            deferred_proposals.ok_or_else(|| SimardError::InvalidImprovementRecord {
                field: "deferred".to_string(),
                reason: "deferred proposal list is required".to_string(),
            })?;

        if let Some(expected_count) = approval_count
            && expected_count != approved_proposals.len()
        {
            return Err(SimardError::InvalidImprovementRecord {
                field: "approvals".to_string(),
                reason: format!(
                    "approval count {expected_count} does not match approved proposal list length {}",
                    approved_proposals.len()
                ),
            });
        }
        if let Some(expected_count) = deferral_count
            && expected_count != deferred_proposals.len()
        {
            return Err(SimardError::InvalidImprovementRecord {
                field: "deferrals".to_string(),
                reason: format!(
                    "deferral count {expected_count} does not match deferred proposal list length {}",
                    deferred_proposals.len()
                ),
            });
        }

        Ok(Self {
            review_id: review_id.ok_or_else(|| SimardError::InvalidImprovementRecord {
                field: "review".to_string(),
                reason: "review id is required".to_string(),
            })?,
            review_target: review_target.ok_or_else(|| SimardError::InvalidImprovementRecord {
                field: "target".to_string(),
                reason: "review target is required".to_string(),
            })?,
            approved_proposals,
            deferred_proposals,
            selected_base_type,
            topology,
            outcome,
        })
    }

    pub fn concise_record(&self) -> String {
        format!(
            "review={} target={} approvals=[{}] deferred=[{}]",
            self.review_id,
            self.review_target,
            self.approved_proposal_summaries().join(" | "),
            self.deferred_proposal_summaries().join(" | "),
        )
    }

    pub fn approved_proposal_summaries(&self) -> Vec<String> {
        self.approved_proposals
            .iter()
            .map(PersistedImprovementApproval::concise_label)
            .collect()
    }

    pub fn deferred_proposal_summaries(&self) -> Vec<String> {
        self.deferred_proposals
            .iter()
            .map(|deferral| format!("{} ({})", deferral.title, deferral.rationale))
            .collect()
    }
}

fn parse_persisted_approval_list(
    field: &str,
    raw: &str,
) -> SimardResult<Vec<PersistedImprovementApproval>> {
    parse_bracketed_list(field, raw)?
        .into_iter()
        .map(|entry| parse_persisted_approval_entry(field, &entry))
        .collect()
}

fn parse_persisted_approval_entry(
    field: &str,
    raw: &str,
) -> SimardResult<PersistedImprovementApproval> {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix('p') else {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: format!(
                "approval entry '{trimmed}' must start with p<priority> [status] title"
            ),
        });
    };
    let digit_count = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: format!("approval entry '{trimmed}' is missing a numeric priority"),
        });
    }
    let priority =
        rest[..digit_count]
            .parse::<u8>()
            .map_err(|_| SimardError::InvalidImprovementRecord {
                field: field.to_string(),
                reason: format!("approval entry '{trimmed}' has an invalid priority"),
            })?;
    if priority == 0 {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: format!("approval entry '{trimmed}' must use priority 1 or greater"),
        });
    }

    let rest = rest[digit_count..].trim_start();
    let Some(status_rest) = rest.strip_prefix('[') else {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: format!("approval entry '{trimmed}' is missing [status]"),
        });
    };
    let Some((status_raw, title_raw)) = status_rest.split_once(']') else {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: format!("approval entry '{trimmed}' has an unterminated [status]"),
        });
    };
    let status = GoalStatus::parse(status_raw.trim()).ok_or_else(|| {
        SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: format!("approval entry '{trimmed}' uses an unsupported status"),
        }
    })?;

    Ok(PersistedImprovementApproval {
        priority,
        status,
        title: required_improvement_field(field, title_raw)?,
    })
}

fn parse_persisted_deferral_list(field: &str, raw: &str) -> SimardResult<Vec<DeferredImprovement>> {
    parse_bracketed_list(field, raw)?
        .into_iter()
        .map(|entry| parse_persisted_deferral_entry(field, &entry))
        .collect()
}

fn parse_persisted_deferral_entry(field: &str, raw: &str) -> SimardResult<DeferredImprovement> {
    let trimmed = raw.trim();
    let Some(stripped) = trimmed.strip_suffix(')') else {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: format!("deferred entry '{trimmed}' must end with '(rationale)'"),
        });
    };
    let Some((title, rationale)) = stripped.rsplit_once(" (") else {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: format!("deferred entry '{trimmed}' must include a rationale"),
        });
    };
    Ok(DeferredImprovement {
        title: required_improvement_field(field, title)?,
        rationale: required_improvement_field(field, rationale)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_persisted_improvement_record_for_readback() {
        let record = PersistedImprovementRecord::parse(
            "review=session-42-review target=operator-review approvals=[p1 [active] Capture denser execution evidence] deferred=[Promote this pattern into a repeatable benchmark (wait for the next benchmark planning pass)]",
        )
        .expect("persisted improvement record should parse");

        assert_eq!(record.review_id, "session-42-review");
        assert_eq!(record.review_target, "operator-review");
        assert_eq!(record.approved_proposals.len(), 1);
        assert_eq!(
            record.approved_proposals[0].concise_label(),
            "p1 [active] Capture denser execution evidence"
        );
        assert_eq!(
            record.deferred_proposal_summaries(),
            vec![
                "Promote this pattern into a repeatable benchmark (wait for the next benchmark planning pass)"
            ]
        );
        assert_eq!(
            record.concise_record(),
            "review=session-42-review target=operator-review approvals=[p1 [active] Capture denser execution evidence] deferred=[Promote this pattern into a repeatable benchmark (wait for the next benchmark planning pass)]"
        );
    }

    #[test]
    fn rejects_persisted_improvement_record_with_malformed_approvals() {
        let error = PersistedImprovementRecord::parse(
            "review=session-42-review target=operator-review approvals=not-a-list deferred=[]",
        )
        .unwrap_err();

        assert_eq!(
            error,
            SimardError::InvalidImprovementRecord {
                field: "approvals".to_string(),
                reason: "value must be a non-negative integer or bracketed list".to_string(),
            }
        );
    }
}
