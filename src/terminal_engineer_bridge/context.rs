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
