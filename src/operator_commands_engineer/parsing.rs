use crate::meetings::PersistedMeetingRecord;
use crate::terminal_engineer_bridge::ENGINEER_HANDOFF_FILE_NAME;

// Re-export so siblings can import all parsing helpers from one place.
pub(super) use super::evidence_helpers::required_engineer_evidence_value;

pub(super) fn parse_engineer_summary_list(raw: &str, separator: &str) -> Vec<String> {
    if raw == "<none>" {
        return Vec::new();
    }

    raw.split(separator)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn parse_carried_meeting_decisions(raw: &str) -> crate::SimardResult<Vec<String>> {
    if raw == "<none>" {
        return Ok(Vec::new());
    }

    let persisted_records = raw
        .split(" || ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if persisted_records.is_empty() {
        return Err(crate::SimardError::InvalidHandoffSnapshot {
            field: "carried-meeting-decisions".to_string(),
            reason: format!(
                "engineer read requires {ENGINEER_HANDOFF_FILE_NAME} or {} to carry at least one persisted meeting record or '<none>' for carried-meeting-decisions",
                crate::terminal_engineer_bridge::COMPATIBILITY_HANDOFF_FILE_NAME
            ),
        });
    }

    let mut decisions = Vec::new();
    for persisted_record in persisted_records {
        let record = PersistedMeetingRecord::parse(persisted_record).map_err(|error| {
            crate::SimardError::InvalidHandoffSnapshot {
                field: "carried-meeting-decisions".to_string(),
                reason: format!(
                    "engineer read requires valid persisted meeting records for carried-meeting-decisions: {error}"
                ),
            }
        })?;
        decisions.extend(record.decisions);
    }
    Ok(decisions)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_engineer_summary_list ---

    #[test]
    fn summary_list_returns_empty_for_none_marker() {
        assert!(parse_engineer_summary_list("<none>", ", ").is_empty());
    }

    #[test]
    fn summary_list_parses_comma_separated() {
        let result = parse_engineer_summary_list("goal-a, goal-b, goal-c", ", ");
        assert_eq!(result, vec!["goal-a", "goal-b", "goal-c"]);
    }

    #[test]
    fn summary_list_trims_whitespace() {
        let result = parse_engineer_summary_list("  alpha ,  beta  ", ",");
        assert_eq!(result, vec!["alpha", "beta"]);
    }

    #[test]
    fn summary_list_filters_empty_entries() {
        let result = parse_engineer_summary_list("a, , b", ", ");
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn summary_list_single_item() {
        let result = parse_engineer_summary_list("only-one", ", ");
        assert_eq!(result, vec!["only-one"]);
    }

    #[test]
    fn summary_list_with_pipe_separator() {
        let result = parse_engineer_summary_list("x | y | z", " | ");
        assert_eq!(result, vec!["x", "y", "z"]);
    }

    #[test]
    fn summary_list_all_empty_after_split() {
        let result = parse_engineer_summary_list(", , ", ", ");
        assert!(result.is_empty());
    }

    // --- parse_carried_meeting_decisions ---

    #[test]
    fn carried_decisions_returns_empty_for_none_marker() {
        let result = parse_carried_meeting_decisions("<none>").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn carried_decisions_errors_on_invalid_record_format() {
        let result = parse_carried_meeting_decisions("not-a-valid-record");
        assert!(result.is_err());
    }

    #[test]
    fn carried_decisions_parses_valid_record_with_decisions() {
        let record = "agenda=Sprint review; updates=[Updated A]; decisions=[Use strategy X | Defer Y]; risks=[Risk 1]; next_steps=[Step 1]; open_questions=[Question 1]; goals=[p1:active:Goal title:Goal rationale]";
        let result = parse_carried_meeting_decisions(record).unwrap();
        assert_eq!(result, vec!["Use strategy X", "Defer Y"]);
    }

    #[test]
    fn carried_decisions_multiple_records_merged() {
        let record_a = "agenda=Review A; updates=[U1]; decisions=[D1]; risks=[R1]; next_steps=[N1]; open_questions=[Q1]; goals=[p1:active:G1:Rationale]";
        let record_b = "agenda=Review B; updates=[U2]; decisions=[D2 | D3]; risks=[R2]; next_steps=[N2]; open_questions=[Q2]; goals=[p2:active:G2:Rationale]";
        let combined = format!("{record_a} || {record_b}");
        let result = parse_carried_meeting_decisions(&combined).unwrap();
        assert_eq!(result, vec!["D1", "D2", "D3"]);
    }

    // --- parse_engineer_summary_list extended ---

    #[test]
    fn summary_list_preserves_internal_whitespace() {
        let result = parse_engineer_summary_list("goal with spaces, another goal", ", ");
        assert_eq!(result, vec!["goal with spaces", "another goal"]);
    }

    #[test]
    fn summary_list_exact_none_marker() {
        // Only exactly "<none>" should return empty
        let result = parse_engineer_summary_list("<NONE>", ", ");
        assert_eq!(result, vec!["<NONE>"]);
    }

    #[test]
    fn summary_list_single_separator_only() {
        let result = parse_engineer_summary_list(", ", ", ");
        assert!(result.is_empty());
    }

    // --- parse_carried_meeting_decisions extended ---

    #[test]
    fn carried_decisions_rejects_empty_after_split() {
        // A string that splits into only empty pieces after trim
        let result = parse_carried_meeting_decisions(" || ");
        assert!(result.is_err(), "should reject empty records after split");
    }

    #[test]
    fn carried_decisions_single_valid_record_no_decisions() {
        let record = "agenda=Review; updates=[U]; decisions=[]; risks=[R]; next_steps=[N]; open_questions=[Q]; goals=[p1:active:G:Rationale]";
        let result = parse_carried_meeting_decisions(record).unwrap();
        assert!(
            result.is_empty(),
            "empty decisions list should yield empty vec"
        );
    }

    #[test]
    fn carried_decisions_error_mentions_field() {
        let err = parse_carried_meeting_decisions("garbage input").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("carried-meeting-decisions"),
            "error should mention field: {msg}"
        );
    }

    // --- parse_engineer_summary_list: more edge cases ---

    #[test]
    fn summary_list_empty_string() {
        let result = parse_engineer_summary_list("", ", ");
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn summary_list_separator_at_start() {
        let result = parse_engineer_summary_list(", alpha, beta", ", ");
        assert_eq!(result, vec!["alpha", "beta"]);
    }

    #[test]
    fn summary_list_separator_at_end() {
        let result = parse_engineer_summary_list("alpha, beta, ", ", ");
        assert_eq!(result, vec!["alpha", "beta"]);
    }

    #[test]
    fn summary_list_multiple_separators_in_row() {
        let result = parse_engineer_summary_list("a, , , b", ", ");
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn summary_list_none_marker_case_sensitive() {
        // "<None>" is not the same as "<none>"
        let result = parse_engineer_summary_list("<None>", ", ");
        assert_eq!(result, vec!["<None>"]);
    }

    // --- parse_carried_meeting_decisions: more patterns ---

    #[test]
    fn carried_decisions_none_marker_case_sensitive() {
        let result = parse_carried_meeting_decisions("<None>");
        assert!(
            result.is_err(),
            "<None> is not <none>, should be treated as invalid"
        );
    }

    #[test]
    fn carried_decisions_empty_input_errors() {
        let result = parse_carried_meeting_decisions("");
        assert!(result.is_err());
    }

    #[test]
    fn carried_decisions_valid_record_many_decisions() {
        let record = "agenda=Sprint; updates=[U]; decisions=[D1 | D2 | D3 | D4]; risks=[R]; next_steps=[N]; open_questions=[Q]; goals=[p1:active:G:Rationale]";
        let result = parse_carried_meeting_decisions(record).unwrap();
        assert_eq!(result.len(), 4);
    }
}
