use std::collections::{BTreeMap, BTreeSet};

use crate::error::{SimardError, SimardResult};
use crate::goals::{GoalStatus, GoalUpdate};
use crate::review::{ImprovementProposal, ReviewArtifact};

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
                        Some(required_improvement_field("selected-base-type", value)?)
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

fn parse_persisted_record_pairs(raw: &str) -> SimardResult<Vec<(&str, &str)>> {
    let mut trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidImprovementRecord {
            field: "record".to_string(),
            reason: "persisted improvement record cannot be empty".to_string(),
        });
    }
    if let Some(stripped) = trimmed.strip_prefix("improvement-curation-record") {
        trimmed = stripped.trim_start();
        if let Some(stripped) = trimmed.strip_prefix('|') {
            trimmed = stripped.trim_start();
        }
    }

    let mut pairs = Vec::new();
    let mut cursor = 0;
    while cursor < trimmed.len() {
        cursor = skip_record_separators(trimmed, cursor);
        if cursor >= trimmed.len() {
            break;
        }

        let key_start = cursor;
        while cursor < trimmed.len() {
            let ch = trimmed[cursor..]
                .chars()
                .next()
                .expect("cursor should remain on a valid char boundary");
            if ch == '=' || ch.is_whitespace() || ch == '|' {
                break;
            }
            cursor += ch.len_utf8();
        }
        if cursor >= trimmed.len() || !trimmed[cursor..].starts_with('=') {
            return Err(SimardError::InvalidImprovementRecord {
                field: "record".to_string(),
                reason: format!(
                    "expected key=value segment near '{}'",
                    trimmed[key_start..].trim()
                ),
            });
        }

        let field = trimmed[key_start..cursor].trim();
        cursor += 1;
        let value_start = cursor;
        let value;
        if trimmed[value_start..].starts_with('[') {
            let (parsed, next_cursor) = read_bracketed_value(trimmed, value_start)?;
            value = parsed;
            cursor = next_cursor;
        } else {
            while cursor < trimmed.len() {
                let ch = trimmed[cursor..]
                    .chars()
                    .next()
                    .expect("cursor should remain on a valid char boundary");
                if ch == '|' {
                    break;
                }
                if ch.is_whitespace() {
                    let next_cursor = skip_spaces(trimmed, cursor);
                    if looks_like_field_start(trimmed, next_cursor) {
                        break;
                    }
                    cursor = next_cursor;
                    continue;
                }
                cursor += ch.len_utf8();
            }
            value = trimmed[value_start..cursor].trim();
        }

        if field.is_empty() {
            return Err(SimardError::InvalidImprovementRecord {
                field: "record".to_string(),
                reason: "persisted improvement record contains an empty field name".to_string(),
            });
        }
        if value.is_empty() {
            return Err(SimardError::InvalidImprovementRecord {
                field: field.to_string(),
                reason: "value cannot be empty".to_string(),
            });
        }
        pairs.push((field, value));
    }

    if pairs.is_empty() {
        return Err(SimardError::InvalidImprovementRecord {
            field: "record".to_string(),
            reason: "persisted improvement record contained no key=value fields".to_string(),
        });
    }

    Ok(pairs)
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

fn parse_bracketed_list(field: &str, raw: &str) -> SimardResult<Vec<String>> {
    let trimmed = raw.trim();
    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "value must use bracketed list syntax".to_string(),
        });
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    let mut current = String::new();
    let mut bracket_depth = 0usize;
    let chars = inner.chars().peekable();
    for ch in chars {
        match ch {
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                if bracket_depth == 0 {
                    return Err(SimardError::InvalidImprovementRecord {
                        field: field.to_string(),
                        reason: "list contains an unexpected closing bracket".to_string(),
                    });
                }
                bracket_depth -= 1;
                current.push(ch);
            }
            '|' if bracket_depth == 0 => {
                let item = current.trim();
                if item.is_empty() {
                    return Err(SimardError::InvalidImprovementRecord {
                        field: field.to_string(),
                        reason: "list contains an empty item".to_string(),
                    });
                }
                items.push(item.to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    if bracket_depth != 0 {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "list contains an unterminated bracket".to_string(),
        });
    }

    let item = current.trim();
    if item.is_empty() {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "list contains an empty item".to_string(),
        });
    }
    items.push(item.to_string());
    Ok(items)
}

fn parse_non_negative_count(field: &str, raw: &str) -> SimardResult<usize> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "value must be a non-negative integer or bracketed list".to_string(),
        })
}

fn read_bracketed_value(raw: &str, start: usize) -> SimardResult<(&str, usize)> {
    let mut cursor = start;
    let mut depth = 0usize;
    while cursor < raw.len() {
        let ch = raw[cursor..]
            .chars()
            .next()
            .expect("cursor should remain on a valid char boundary");
        match ch {
            '[' => depth += 1,
            ']' => {
                depth =
                    depth
                        .checked_sub(1)
                        .ok_or_else(|| SimardError::InvalidImprovementRecord {
                            field: "record".to_string(),
                            reason:
                                "persisted improvement record has an unexpected closing bracket"
                                    .to_string(),
                        })?;
                if depth == 0 {
                    cursor += ch.len_utf8();
                    return Ok((&raw[start..cursor], cursor));
                }
            }
            _ => {}
        }
        cursor += ch.len_utf8();
    }

    Err(SimardError::InvalidImprovementRecord {
        field: "record".to_string(),
        reason: "persisted improvement record has an unterminated bracketed list".to_string(),
    })
}

fn skip_record_separators(raw: &str, mut cursor: usize) -> usize {
    while cursor < raw.len() {
        let ch = raw[cursor..]
            .chars()
            .next()
            .expect("cursor should remain on a valid char boundary");
        if ch == '|' || ch.is_whitespace() {
            cursor += ch.len_utf8();
            continue;
        }
        break;
    }
    cursor
}

fn skip_spaces(raw: &str, mut cursor: usize) -> usize {
    while cursor < raw.len() {
        let ch = raw[cursor..]
            .chars()
            .next()
            .expect("cursor should remain on a valid char boundary");
        if ch.is_whitespace() {
            cursor += ch.len_utf8();
            continue;
        }
        break;
    }
    cursor
}

fn looks_like_field_start(raw: &str, cursor: usize) -> bool {
    let tail = &raw[cursor..];
    let mut seen_any = false;
    for ch in tail.chars() {
        if ch == '=' {
            return seen_any;
        }
        if ch == '|' || ch.is_whitespace() {
            return false;
        }
        if !ch.is_ascii_alphanumeric() && ch != '-' {
            return false;
        }
        seen_any = true;
    }
    false
}

fn required_improvement_field(field: &str, value: impl AsRef<str>) -> SimardResult<String> {
    let trimmed = value.as_ref().trim();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidImprovementRecord {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

fn sanitize_directive_value(value: &str) -> String {
    value
        .replace('\n', " ")
        .replace('|', "/")
        .replace(";;", ";")
        .trim()
        .to_string()
}

fn fallback_value<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{ReviewArtifact, ReviewEvidenceSummary, ReviewTargetKind};

    #[test]
    fn parses_review_context_and_operator_decisions() {
        let raw = "\
review-id: session-1-review\n\
review-target: operator-review\n\
proposal: Capture denser execution evidence | category=evidence-capture | rationale=thin trail | suggested_change=record more phases | evidence=phase-1 ;; phase-2\n\
proposal: Promote this pattern into a repeatable benchmark | category=benchmark-coverage | rationale=one-off session | suggested_change=make a scenario | evidence=target=operator-review\n\
approve: Capture denser execution evidence | priority=1 | status=active | rationale=make this visible now\n\
defer: Promote this pattern into a repeatable benchmark | rationale=wait for the next planning pass";

        let plan = ImprovementPromotionPlan::parse(raw).expect("plan should parse");

        assert_eq!(plan.review_id, "session-1-review");
        assert_eq!(plan.proposals.len(), 2);
        assert_eq!(plan.approvals.len(), 1);
        assert_eq!(plan.deferrals.len(), 1);
        assert_eq!(plan.approvals[0].status, GoalStatus::Active);
    }

    #[test]
    fn rejects_decisions_for_unknown_proposals() {
        let raw = "\
review-id: session-1-review\n\
proposal: Capture denser execution evidence | category=evidence-capture | rationale=thin trail | suggested_change=record more phases | evidence=phase-1\n\
approve: Missing proposal | priority=1 | status=active | rationale=bad";

        let error = ImprovementPromotionPlan::parse(raw).unwrap_err();
        assert_eq!(
            error,
            SimardError::InvalidImprovementRecord {
                field: "decision".to_string(),
                reason: "decision references unknown proposal 'Missing proposal'".to_string(),
            }
        );
    }

    #[test]
    fn renders_review_context_directives_for_operator_curator_sessions() {
        let review = ReviewArtifact {
            review_id: "session-1-review".to_string(),
            reviewed_at_unix_ms: 1,
            target_kind: ReviewTargetKind::Session,
            target_label: "operator-review".to_string(),
            identity_name: "simard-engineer".to_string(),
            session_id: "session-1".to_string(),
            selected_base_type: "local-harness".to_string(),
            topology: "single-process".to_string(),
            objective_metadata: "objective-metadata(chars=10, words=2, lines=1)".to_string(),
            execution_summary: "done".to_string(),
            reflection_summary: "reflect".to_string(),
            summary: "summary".to_string(),
            measurement_notes: Vec::new(),
            evidence_summary: ReviewEvidenceSummary {
                memory_records: 1,
                evidence_records: 1,
                decision_records: 1,
                benchmark_records: 0,
                exported_state: "ready".to_string(),
                session_phase: Some("complete".to_string()),
                failed_signals: Vec::new(),
            },
            proposals: vec![ImprovementProposal {
                category: "evidence-capture".to_string(),
                title: "Capture denser execution evidence".to_string(),
                rationale: "thin trail".to_string(),
                suggested_change: "record more phases".to_string(),
                evidence: vec!["phase-1".to_string(), "phase-2".to_string()],
            }],
        };

        let directives = render_review_context_directives(&review);
        assert!(directives.contains("review-id: session-1-review"));
        assert!(directives.contains("proposal: Capture denser execution evidence"));
        assert!(directives.contains("evidence=phase-1 ;; phase-2"));
    }

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
