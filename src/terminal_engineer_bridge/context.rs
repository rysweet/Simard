use std::path::Path;

use crate::error::{SimardError, SimardResult};
use crate::evidence::EvidenceRecord;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::sanitization::sanitize_terminal_text;

use super::artifact::{load_runtime_handoff_snapshot, select_optional_handoff_artifact};
use super::types::{
    COMPATIBILITY_HANDOFF_FILE_NAME, SHARED_DEFAULT_STATE_ROOT_SOURCE,
    SHARED_EXPLICIT_STATE_ROOT_SOURCE, ScopedHandoffMode, TERMINAL_HANDOFF_FILE_NAME,
    TerminalBridgeContext,
};

const BRIDGE_SOURCE_PREFIX: &str = "terminal-continuity-source=";
const BRIDGE_HANDOFF_PREFIX: &str = "terminal-continuity-handoff=";
const BRIDGE_WORKING_DIRECTORY_PREFIX: &str = "terminal-continuity-working-directory=";
const BRIDGE_COMMAND_COUNT_PREFIX: &str = "terminal-continuity-command-count=";
const BRIDGE_WAIT_COUNT_PREFIX: &str = "terminal-continuity-wait-count=";
const BRIDGE_LAST_OUTPUT_LINE_PREFIX: &str = "terminal-continuity-last-output-line=";

impl TerminalBridgeContext {
    pub fn load_from_state_root(
        state_root: &Path,
        continuity_source: &str,
    ) -> SimardResult<Option<Self>> {
        let Some(selection) = select_optional_handoff_artifact(
            state_root,
            ScopedHandoffMode::Terminal,
            "engineer run",
        )?
        else {
            return Ok(None);
        };

        let snapshot = load_runtime_handoff_snapshot(&selection, "engineer run")?;
        Self::from_terminal_handoff(
            &snapshot,
            selection.file_name,
            continuity_source.to_string(),
            "engineer run",
        )
    }

    pub fn from_engineer_evidence(
        evidence_records: &[EvidenceRecord],
    ) -> SimardResult<Option<Self>> {
        let Some(continuity_source) =
            optional_evidence_value(evidence_records, BRIDGE_SOURCE_PREFIX)
        else {
            return Ok(None);
        };
        if continuity_source != SHARED_EXPLICIT_STATE_ROOT_SOURCE
            && continuity_source != SHARED_DEFAULT_STATE_ROOT_SOURCE
        {
            return Err(SimardError::InvalidHandoffSnapshot {
                field: "terminal-continuity-source".to_string(),
                reason: "engineer read requires terminal continuity evidence to use a shipped shared-state-root label".to_string(),
            });
        }

        let handoff_file_name =
            required_evidence_value(evidence_records, BRIDGE_HANDOFF_PREFIX, "engineer read")?;
        if handoff_file_name != TERMINAL_HANDOFF_FILE_NAME
            && handoff_file_name != COMPATIBILITY_HANDOFF_FILE_NAME
        {
            return Err(SimardError::InvalidHandoffSnapshot {
                field: "terminal-continuity-handoff".to_string(),
                reason: "engineer read requires terminal continuity evidence to name latest_terminal_handoff.json or latest_handoff.json".to_string(),
            });
        }

        Ok(Some(Self {
            continuity_source: continuity_source.to_string(),
            handoff_file_name: handoff_file_name.to_string(),
            working_directory: sanitize_terminal_text(required_evidence_value(
                evidence_records,
                BRIDGE_WORKING_DIRECTORY_PREFIX,
                "engineer read",
            )?),
            command_count: sanitize_terminal_text(required_evidence_value(
                evidence_records,
                BRIDGE_COMMAND_COUNT_PREFIX,
                "engineer read",
            )?),
            wait_count: sanitize_terminal_text(
                optional_evidence_value(evidence_records, BRIDGE_WAIT_COUNT_PREFIX).unwrap_or("0"),
            ),
            last_output_line: optional_evidence_value(
                evidence_records,
                BRIDGE_LAST_OUTPUT_LINE_PREFIX,
            )
            .map(sanitize_terminal_text),
        }))
    }

    pub fn engineer_evidence_details(&self) -> Vec<String> {
        let mut details = vec![
            format!("{BRIDGE_SOURCE_PREFIX}{}", self.continuity_source),
            format!("{BRIDGE_HANDOFF_PREFIX}{}", self.handoff_file_name),
            format!(
                "{BRIDGE_WORKING_DIRECTORY_PREFIX}{}",
                self.working_directory
            ),
            format!("{BRIDGE_COMMAND_COUNT_PREFIX}{}", self.command_count),
            format!("{BRIDGE_WAIT_COUNT_PREFIX}{}", self.wait_count),
        ];
        if let Some(last_output_line) = &self.last_output_line {
            details.push(format!(
                "{BRIDGE_LAST_OUTPUT_LINE_PREFIX}{last_output_line}"
            ));
        }
        details
    }

    fn from_terminal_handoff(
        handoff: &RuntimeHandoffSnapshot,
        handoff_file_name: &str,
        continuity_source: String,
        consumer_label: &str,
    ) -> SimardResult<Option<Self>> {
        if !contains_terminal_evidence(&handoff.evidence_records) {
            return Ok(None);
        }

        if handoff.session.is_none() {
            return Err(SimardError::InvalidHandoffSnapshot {
                field: "session".to_string(),
                reason: format!(
                    "{consumer_label} requires {handoff_file_name} to contain a persisted session snapshot before terminal continuity can be bridged"
                ),
            });
        }

        Ok(Some(Self {
            continuity_source,
            handoff_file_name: handoff_file_name.to_string(),
            working_directory: sanitize_terminal_text(required_evidence_value(
                &handoff.evidence_records,
                "terminal-working-directory=",
                consumer_label,
            )?),
            command_count: sanitize_terminal_text(required_evidence_value(
                &handoff.evidence_records,
                "terminal-command-count=",
                consumer_label,
            )?),
            wait_count: sanitize_terminal_text(
                optional_evidence_value(&handoff.evidence_records, "terminal-wait-count=")
                    .unwrap_or("0"),
            ),
            last_output_line: optional_evidence_value(
                &handoff.evidence_records,
                "terminal-last-output-line=",
            )
            .map(sanitize_terminal_text),
        }))
    }
}

fn contains_terminal_evidence(evidence_records: &[EvidenceRecord]) -> bool {
    evidence_records
        .iter()
        .any(|record| record.detail.starts_with("terminal-working-directory="))
}

fn required_evidence_value<'a>(
    evidence_records: &'a [EvidenceRecord],
    prefix: &str,
    consumer_label: &str,
) -> SimardResult<&'a str> {
    optional_evidence_value(evidence_records, prefix).ok_or_else(|| {
        SimardError::InvalidHandoffSnapshot {
            field: prefix.trim_end_matches('=').to_string(),
            reason: format!(
                "{consumer_label} requires persisted evidence '{}' for operator-visible terminal continuity",
                prefix.trim_end_matches('=')
            ),
        }
    })
}

fn optional_evidence_value<'a>(
    evidence_records: &'a [EvidenceRecord],
    prefix: &str,
) -> Option<&'a str> {
    evidence_records
        .iter()
        .rev()
        .find_map(|record| record.detail.strip_prefix(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::EvidenceSource;
    use crate::session::{SessionId, SessionPhase};

    fn make_evidence(detail: &str) -> EvidenceRecord {
        EvidenceRecord {
            id: "ev-1".to_string(),
            session_id: SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap(),
            phase: SessionPhase::Intake,
            detail: detail.to_string(),
            source: EvidenceSource::Runtime,
        }
    }

    #[test]
    fn test_optional_evidence_value_found() {
        let records = vec![make_evidence("terminal-working-directory=/home/user")];
        let val = optional_evidence_value(&records, "terminal-working-directory=");
        assert_eq!(val, Some("/home/user"));
    }

    #[test]
    fn test_optional_evidence_value_not_found() {
        let records = vec![make_evidence("other-key=value")];
        let val = optional_evidence_value(&records, "terminal-working-directory=");
        assert_eq!(val, None);
    }

    #[test]
    fn test_optional_evidence_value_empty_records() {
        let records: Vec<EvidenceRecord> = vec![];
        let val = optional_evidence_value(&records, "anything=");
        assert_eq!(val, None);
    }

    #[test]
    fn test_optional_evidence_value_returns_last_match() {
        let records = vec![
            make_evidence("terminal-working-directory=/first"),
            make_evidence("terminal-working-directory=/second"),
        ];
        let val = optional_evidence_value(&records, "terminal-working-directory=");
        assert_eq!(val, Some("/second"));
    }

    #[test]
    fn test_required_evidence_value_found() {
        let records = vec![make_evidence("terminal-command-count=5")];
        let val = required_evidence_value(&records, "terminal-command-count=", "test");
        assert_eq!(val.unwrap(), "5");
    }

    #[test]
    fn test_required_evidence_value_missing() {
        let records = vec![make_evidence("other=val")];
        let val = required_evidence_value(&records, "terminal-command-count=", "test");
        assert!(val.is_err());
    }

    #[test]
    fn test_contains_terminal_evidence_true() {
        let records = vec![make_evidence("terminal-working-directory=/foo")];
        assert!(contains_terminal_evidence(&records));
    }

    #[test]
    fn test_contains_terminal_evidence_false() {
        let records = vec![make_evidence("something-else=bar")];
        assert!(!contains_terminal_evidence(&records));
    }

    #[test]
    fn test_contains_terminal_evidence_empty() {
        assert!(!contains_terminal_evidence(&[]));
    }

    #[test]
    fn test_engineer_evidence_details_basic() {
        let ctx = TerminalBridgeContext {
            continuity_source: "shared explicit state-root".to_string(),
            handoff_file_name: "latest_terminal_handoff.json".to_string(),
            working_directory: "/home/user/project".to_string(),
            command_count: "5".to_string(),
            wait_count: "2".to_string(),
            last_output_line: None,
        };
        let details = ctx.engineer_evidence_details();
        assert_eq!(details.len(), 5);
        assert!(details[0].starts_with("terminal-continuity-source="));
        assert!(details[1].starts_with("terminal-continuity-handoff="));
        assert!(details[2].starts_with("terminal-continuity-working-directory="));
        assert!(details[3].starts_with("terminal-continuity-command-count="));
        assert!(details[4].starts_with("terminal-continuity-wait-count="));
    }

    #[test]
    fn test_engineer_evidence_details_with_last_output() {
        let ctx = TerminalBridgeContext {
            continuity_source: "shared explicit state-root".to_string(),
            handoff_file_name: "latest_terminal_handoff.json".to_string(),
            working_directory: "/home/user".to_string(),
            command_count: "3".to_string(),
            wait_count: "0".to_string(),
            last_output_line: Some("$ cargo test".to_string()),
        };
        let details = ctx.engineer_evidence_details();
        assert_eq!(details.len(), 6);
        assert!(details[5].starts_with("terminal-continuity-last-output-line="));
    }

    #[test]
    fn test_from_engineer_evidence_returns_none_when_no_source() {
        let records = vec![make_evidence("unrelated=value")];
        let result = TerminalBridgeContext::from_engineer_evidence(&records).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_from_engineer_evidence_rejects_invalid_source() {
        let records = vec![
            make_evidence("terminal-continuity-source=invalid-source"),
            make_evidence("terminal-continuity-handoff=latest_terminal_handoff.json"),
            make_evidence("terminal-continuity-working-directory=/home/user"),
            make_evidence("terminal-continuity-command-count=1"),
        ];
        let result = TerminalBridgeContext::from_engineer_evidence(&records);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_engineer_evidence_valid() {
        let records = vec![
            make_evidence("terminal-continuity-source=shared explicit state-root"),
            make_evidence("terminal-continuity-handoff=latest_terminal_handoff.json"),
            make_evidence("terminal-continuity-working-directory=/home/user"),
            make_evidence("terminal-continuity-command-count=3"),
            make_evidence("terminal-continuity-wait-count=1"),
        ];
        let ctx = TerminalBridgeContext::from_engineer_evidence(&records)
            .unwrap()
            .unwrap();
        assert_eq!(ctx.working_directory, "/home/user");
        assert_eq!(ctx.command_count, "3");
        assert_eq!(ctx.wait_count, "1");
        assert!(ctx.last_output_line.is_none());
    }
}
