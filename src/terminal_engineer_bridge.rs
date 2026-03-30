use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};
use crate::evidence::EvidenceRecord;
use crate::handoff::{FileBackedHandoffStore, RuntimeHandoffSnapshot, RuntimeHandoffStore};
use crate::sanitization::sanitize_terminal_text;

pub const COMPATIBILITY_HANDOFF_FILE_NAME: &str = "latest_handoff.json";
pub const TERMINAL_HANDOFF_FILE_NAME: &str = "latest_terminal_handoff.json";
pub const ENGINEER_HANDOFF_FILE_NAME: &str = "latest_engineer_handoff.json";
pub const TERMINAL_MODE_BOUNDARY: &str = "terminal is a bounded local terminal session surface";
pub const ENGINEER_MODE_BOUNDARY: &str = "engineer is a separate repo-grounded bounded loop";
pub const SHARED_EXPLICIT_STATE_ROOT_SOURCE: &str = "shared explicit state-root";
pub const SHARED_DEFAULT_STATE_ROOT_SOURCE: &str = "shared default state-root";

const BRIDGE_SOURCE_PREFIX: &str = "terminal-continuity-source=";
const BRIDGE_HANDOFF_PREFIX: &str = "terminal-continuity-handoff=";
const BRIDGE_WORKING_DIRECTORY_PREFIX: &str = "terminal-continuity-working-directory=";
const BRIDGE_COMMAND_COUNT_PREFIX: &str = "terminal-continuity-command-count=";
const BRIDGE_WAIT_COUNT_PREFIX: &str = "terminal-continuity-wait-count=";
const BRIDGE_LAST_OUTPUT_LINE_PREFIX: &str = "terminal-continuity-last-output-line=";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopedHandoffMode {
    Terminal,
    Engineer,
}

impl ScopedHandoffMode {
    pub fn scoped_file_name(self) -> &'static str {
        match self {
            Self::Terminal => TERMINAL_HANDOFF_FILE_NAME,
            Self::Engineer => ENGINEER_HANDOFF_FILE_NAME,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedHandoffArtifact {
    pub path: PathBuf,
    pub file_name: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalBridgeContext {
    pub continuity_source: String,
    pub handoff_file_name: String,
    pub working_directory: String,
    pub command_count: String,
    pub wait_count: String,
    pub last_output_line: Option<String>,
}

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

pub fn compatibility_handoff_path(state_root: &Path) -> PathBuf {
    state_root.join(COMPATIBILITY_HANDOFF_FILE_NAME)
}

pub fn scoped_handoff_path(state_root: &Path, mode: ScopedHandoffMode) -> PathBuf {
    state_root.join(mode.scoped_file_name())
}

pub fn persist_handoff_artifacts(
    state_root: &Path,
    mode: ScopedHandoffMode,
    snapshot: &RuntimeHandoffSnapshot,
) -> SimardResult<()> {
    FileBackedHandoffStore::try_new(compatibility_handoff_path(state_root))?
        .save(snapshot.clone())?;
    FileBackedHandoffStore::try_new(scoped_handoff_path(state_root, mode))?
        .save(snapshot.clone())?;
    Ok(())
}

pub fn select_handoff_artifact_for_read(
    state_root: &Path,
    mode: ScopedHandoffMode,
    mode_label: &str,
) -> SimardResult<SelectedHandoffArtifact> {
    if let Some(path) = validate_optional_regular_file(
        state_root,
        &scoped_handoff_path(state_root, mode),
        mode.scoped_file_name(),
        mode_label,
    )? {
        return Ok(SelectedHandoffArtifact {
            path,
            file_name: mode.scoped_file_name(),
        });
    }

    Ok(SelectedHandoffArtifact {
        path: require_regular_file(
            state_root,
            &compatibility_handoff_path(state_root),
            COMPATIBILITY_HANDOFF_FILE_NAME,
            mode_label,
        )?,
        file_name: COMPATIBILITY_HANDOFF_FILE_NAME,
    })
}

pub fn select_optional_handoff_artifact(
    state_root: &Path,
    mode: ScopedHandoffMode,
    mode_label: &str,
) -> SimardResult<Option<SelectedHandoffArtifact>> {
    if let Some(path) = validate_optional_regular_file(
        state_root,
        &scoped_handoff_path(state_root, mode),
        mode.scoped_file_name(),
        mode_label,
    )? {
        return Ok(Some(SelectedHandoffArtifact {
            path,
            file_name: mode.scoped_file_name(),
        }));
    }

    if let Some(path) = validate_optional_regular_file(
        state_root,
        &compatibility_handoff_path(state_root),
        COMPATIBILITY_HANDOFF_FILE_NAME,
        mode_label,
    )? {
        return Ok(Some(SelectedHandoffArtifact {
            path,
            file_name: COMPATIBILITY_HANDOFF_FILE_NAME,
        }));
    }

    Ok(None)
}

pub fn load_runtime_handoff_snapshot(
    artifact: &SelectedHandoffArtifact,
    consumer_label: &str,
) -> SimardResult<RuntimeHandoffSnapshot> {
    let store = FileBackedHandoffStore::try_new(&artifact.path).map_err(|error| {
        SimardError::InvalidHandoffSnapshot {
            field: artifact.file_name.to_string(),
            reason: format!(
                "{consumer_label} could not load {} cleanly: {error}",
                artifact.file_name
            ),
        }
    })?;

    store
        .latest()
        .map_err(|error| SimardError::InvalidHandoffSnapshot {
            field: artifact.file_name.to_string(),
            reason: format!(
                "{consumer_label} could not read {} cleanly: {error}",
                artifact.file_name
            ),
        })?
        .ok_or_else(|| SimardError::InvalidHandoffSnapshot {
            field: artifact.file_name.to_string(),
            reason: format!(
                "{consumer_label} requires {} to contain a persisted handoff snapshot",
                artifact.file_name
            ),
        })
}

fn contains_terminal_evidence(evidence_records: &[EvidenceRecord]) -> bool {
    evidence_records
        .iter()
        .any(|record| record.detail.starts_with("terminal-working-directory="))
}

fn validate_optional_regular_file(
    state_root: &Path,
    path: &Path,
    file_name: &str,
    mode_label: &str,
) -> SimardResult<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(SimardError::InvalidStateRoot {
                    path: state_root.to_path_buf(),
                    reason: format!(
                        "{mode_label} requires {file_name} to exist as a regular file, not a symlink"
                    ),
                });
            }
            if metadata.is_file() {
                return Ok(Some(path.to_path_buf()));
            }
            Err(SimardError::InvalidStateRoot {
                path: state_root.to_path_buf(),
                reason: format!("{mode_label} requires {file_name} to exist as a regular file"),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!(
                "{mode_label} requires {file_name} to exist as a regular file: {error}"
            ),
        }),
    }
}

fn require_regular_file(
    state_root: &Path,
    path: &Path,
    file_name: &str,
    mode_label: &str,
) -> SimardResult<PathBuf> {
    validate_optional_regular_file(state_root, path, file_name, mode_label)?.ok_or_else(|| {
        SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires {file_name} to exist as a regular file"),
        }
    })
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
