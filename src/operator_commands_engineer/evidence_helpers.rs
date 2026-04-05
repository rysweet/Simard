use crate::EvidenceRecord;

pub(super) fn required_engineer_evidence_value<'a>(
    evidence_records: &'a [EvidenceRecord],
    prefix: &str,
    handoff_source: &str,
) -> crate::SimardResult<&'a str> {
    evidence_records
        .iter()
        .rev()
        .find_map(|record| record.detail.strip_prefix(prefix))
        .ok_or_else(|| crate::SimardError::InvalidHandoffSnapshot {
            field: prefix.trim_end_matches('=').to_string(),
            reason: format!(
                "engineer read requires {handoff_source} to carry persisted engineer evidence '{}' for operator output",
                prefix.trim_end_matches('=')
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::EvidenceSource;
    use crate::session::{SessionId, SessionPhase};

    fn make_evidence(detail: &str) -> EvidenceRecord {
        EvidenceRecord {
            id: "ev-test".to_string(),
            session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            phase: SessionPhase::Execution,
            detail: detail.to_string(),
            source: EvidenceSource::Runtime,
        }
    }

    // --- required_engineer_evidence_value ---

    #[test]
    fn required_evidence_returns_value_for_matching_prefix() {
        let records = vec![make_evidence("repo-root=/home/user/project")];
        let result =
            required_engineer_evidence_value(&records, "repo-root=", "test-handoff").unwrap();
        assert_eq!(result, "/home/user/project");
    }

    #[test]
    fn required_evidence_errors_when_no_match() {
        let records = vec![make_evidence("repo-root=/home/user/project")];
        let result =
            required_engineer_evidence_value(&records, "nonexistent-prefix=", "test-handoff");
        assert!(result.is_err());
    }

    #[test]
    fn required_evidence_returns_last_matching_value_via_rev() {
        let records = vec![
            make_evidence("repo-branch=main"),
            make_evidence("repo-branch=feature/new"),
        ];
        let result =
            required_engineer_evidence_value(&records, "repo-branch=", "test-handoff").unwrap();
        assert_eq!(result, "feature/new");
    }

    #[test]
    fn required_evidence_ignores_non_matching_records() {
        let records = vec![
            make_evidence("other-key=value1"),
            make_evidence("repo-head=abc123"),
            make_evidence("another-key=value2"),
        ];
        let result =
            required_engineer_evidence_value(&records, "repo-head=", "test-handoff").unwrap();
        assert_eq!(result, "abc123");
    }

    #[test]
    fn required_evidence_empty_records_errors() {
        let records: Vec<EvidenceRecord> = vec![];
        let result = required_engineer_evidence_value(&records, "repo-root=", "test-handoff");
        assert!(result.is_err());
    }

    #[test]
    fn required_evidence_error_message_contains_field() {
        let records: Vec<EvidenceRecord> = vec![];
        let err =
            required_engineer_evidence_value(&records, "repo-root=", "test-handoff").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("repo-root"),
            "error should mention field name: {msg}"
        );
    }

    #[test]
    fn required_evidence_error_message_contains_handoff_source() {
        let records: Vec<EvidenceRecord> = vec![];
        let err = required_engineer_evidence_value(&records, "repo-root=", "my-handoff.json")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("my-handoff.json"),
            "error should mention handoff source: {msg}"
        );
    }

    #[test]
    fn required_evidence_strips_prefix_correctly() {
        let records = vec![make_evidence("worktree-dirty=false")];
        let result =
            required_engineer_evidence_value(&records, "worktree-dirty=", "test-handoff").unwrap();
        assert_eq!(result, "false");
    }

    #[test]
    fn required_evidence_handles_empty_value_after_prefix() {
        let records = vec![make_evidence("changed-files=")];
        let result =
            required_engineer_evidence_value(&records, "changed-files=", "test-handoff").unwrap();
        assert_eq!(result, "");
    }

    // --- required_engineer_evidence_value with special characters ---

    #[test]
    fn required_evidence_value_with_equals_in_value() {
        let records = vec![make_evidence("repo-root=/home/user=special")];
        let result =
            required_engineer_evidence_value(&records, "repo-root=", "test-handoff").unwrap();
        assert_eq!(result, "/home/user=special");
    }

    #[test]
    fn required_evidence_value_with_spaces_in_value() {
        let records = vec![make_evidence("repo-branch=feature/my branch")];
        let result =
            required_engineer_evidence_value(&records, "repo-branch=", "test-handoff").unwrap();
        assert_eq!(result, "feature/my branch");
    }

    #[test]
    fn required_evidence_prefix_partial_match_does_not_match() {
        let records = vec![make_evidence("repo-root-extra=/value")];
        let result = required_engineer_evidence_value(&records, "repo-root=", "test-handoff");
        assert!(result.is_err(), "partial prefix match should not succeed");
    }

    // --- required_engineer_evidence_value: more patterns ---

    #[test]
    fn required_evidence_multiple_different_prefixes() {
        let records = vec![
            make_evidence("repo-root=/home/user"),
            make_evidence("repo-branch=main"),
            make_evidence("repo-head=abc123"),
            make_evidence("worktree-dirty=true"),
        ];
        assert_eq!(
            required_engineer_evidence_value(&records, "repo-root=", "h").unwrap(),
            "/home/user"
        );
        assert_eq!(
            required_engineer_evidence_value(&records, "repo-branch=", "h").unwrap(),
            "main"
        );
        assert_eq!(
            required_engineer_evidence_value(&records, "repo-head=", "h").unwrap(),
            "abc123"
        );
        assert_eq!(
            required_engineer_evidence_value(&records, "worktree-dirty=", "h").unwrap(),
            "true"
        );
    }

    #[test]
    fn required_evidence_error_mentions_engineer_read() {
        let records: Vec<EvidenceRecord> = vec![];
        let err =
            required_engineer_evidence_value(&records, "selected-action=", "engineer-handoff.json")
                .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("selected-action"),
            "error should mention field: {msg}"
        );
        assert!(
            msg.contains("engineer-handoff.json"),
            "error should mention source: {msg}"
        );
        assert!(
            msg.contains("engineer read"),
            "error should mention context: {msg}"
        );
    }
}
