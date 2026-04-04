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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeId;
    use crate::evidence::EvidenceSource;
    use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
    use crate::session::{SessionId, SessionPhase};
    use std::collections::HashSet;
    use tempfile::TempDir;

    fn test_session_id() -> SessionId {
        SessionId::from_uuid(uuid::Uuid::nil())
    }

    fn make_evidence(detail: &str) -> EvidenceRecord {
        EvidenceRecord {
            id: "ev-1".to_string(),
            session_id: test_session_id(),
            phase: SessionPhase::Execution,
            detail: detail.to_string(),
            source: EvidenceSource::Runtime,
        }
    }

    fn minimal_snapshot(evidence: Vec<EvidenceRecord>) -> RuntimeHandoffSnapshot {
        let node = RuntimeNodeId::local();
        RuntimeHandoffSnapshot {
            exported_state: RuntimeState::Ready,
            identity_name: "test-identity".to_string(),
            selected_base_type: BaseTypeId::new("test-base"),
            topology: RuntimeTopology::SingleProcess,
            source_runtime_node: node.clone(),
            source_mailbox_address: RuntimeAddress::local(&node),
            session: None,
            memory_records: vec![],
            evidence_records: evidence,
            copilot_submit_audit: None,
        }
    }

    // ── 1. Constants have expected values ──

    #[test]
    fn constants_are_non_empty() {
        let constants: &[&str] = &[
            COMPATIBILITY_HANDOFF_FILE_NAME,
            TERMINAL_HANDOFF_FILE_NAME,
            ENGINEER_HANDOFF_FILE_NAME,
            TERMINAL_MODE_BOUNDARY,
            ENGINEER_MODE_BOUNDARY,
            SHARED_EXPLICIT_STATE_ROOT_SOURCE,
            SHARED_DEFAULT_STATE_ROOT_SOURCE,
        ];
        for c in constants {
            assert!(!c.is_empty(), "constant must not be empty");
        }
    }

    #[test]
    fn handoff_file_name_constants_are_distinct() {
        let names: HashSet<&str> = [
            COMPATIBILITY_HANDOFF_FILE_NAME,
            TERMINAL_HANDOFF_FILE_NAME,
            ENGINEER_HANDOFF_FILE_NAME,
        ]
        .into_iter()
        .collect();
        assert_eq!(
            names.len(),
            3,
            "all three handoff file names must be distinct"
        );
    }

    #[test]
    fn mode_boundary_constants_are_distinct() {
        assert_ne!(TERMINAL_MODE_BOUNDARY, ENGINEER_MODE_BOUNDARY);
    }

    #[test]
    fn state_root_source_constants_are_distinct() {
        assert_ne!(
            SHARED_EXPLICIT_STATE_ROOT_SOURCE,
            SHARED_DEFAULT_STATE_ROOT_SOURCE
        );
    }

    // ── 2. ScopedHandoffMode::scoped_file_name ──

    #[test]
    fn scoped_file_name_terminal() {
        assert_eq!(
            ScopedHandoffMode::Terminal.scoped_file_name(),
            TERMINAL_HANDOFF_FILE_NAME
        );
    }

    #[test]
    fn scoped_file_name_engineer() {
        assert_eq!(
            ScopedHandoffMode::Engineer.scoped_file_name(),
            ENGINEER_HANDOFF_FILE_NAME
        );
    }

    // ── 3. Path construction helpers ──

    #[test]
    fn compatibility_handoff_path_constructs_correctly() {
        let root = Path::new("/state");
        assert_eq!(
            compatibility_handoff_path(root),
            PathBuf::from("/state/latest_handoff.json")
        );
    }

    #[test]
    fn scoped_handoff_path_terminal() {
        let root = Path::new("/state");
        assert_eq!(
            scoped_handoff_path(root, ScopedHandoffMode::Terminal),
            PathBuf::from("/state/latest_terminal_handoff.json")
        );
    }

    #[test]
    fn scoped_handoff_path_engineer() {
        let root = Path::new("/state");
        assert_eq!(
            scoped_handoff_path(root, ScopedHandoffMode::Engineer),
            PathBuf::from("/state/latest_engineer_handoff.json")
        );
    }

    // ── 4. from_engineer_evidence with empty evidence ──

    #[test]
    fn from_engineer_evidence_returns_none_for_empty_evidence() {
        let result = TerminalBridgeContext::from_engineer_evidence(&[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn from_engineer_evidence_returns_none_without_source_prefix() {
        let records = vec![make_evidence("unrelated-detail=hello")];
        let result = TerminalBridgeContext::from_engineer_evidence(&records).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn from_engineer_evidence_rejects_unknown_source() {
        let records = vec![
            make_evidence("terminal-continuity-source=unknown-source"),
            make_evidence("terminal-continuity-handoff=latest_terminal_handoff.json"),
            make_evidence("terminal-continuity-working-directory=/home"),
            make_evidence("terminal-continuity-command-count=5"),
        ];
        let result = TerminalBridgeContext::from_engineer_evidence(&records);
        assert!(result.is_err());
    }

    #[test]
    fn from_engineer_evidence_rejects_unknown_handoff_file() {
        let records = vec![
            make_evidence(&format!(
                "terminal-continuity-source={SHARED_EXPLICIT_STATE_ROOT_SOURCE}"
            )),
            make_evidence("terminal-continuity-handoff=bad_file.json"),
        ];
        let result = TerminalBridgeContext::from_engineer_evidence(&records);
        assert!(result.is_err());
    }

    #[test]
    fn from_engineer_evidence_succeeds_with_valid_records() {
        let records = vec![
            make_evidence(&format!(
                "terminal-continuity-source={SHARED_EXPLICIT_STATE_ROOT_SOURCE}"
            )),
            make_evidence(&format!(
                "terminal-continuity-handoff={TERMINAL_HANDOFF_FILE_NAME}"
            )),
            make_evidence("terminal-continuity-working-directory=/home/user/project"),
            make_evidence("terminal-continuity-command-count=42"),
            make_evidence("terminal-continuity-wait-count=3"),
            make_evidence("terminal-continuity-last-output-line=build succeeded"),
        ];
        let ctx = TerminalBridgeContext::from_engineer_evidence(&records)
            .unwrap()
            .expect("should return Some");
        assert_eq!(ctx.continuity_source, SHARED_EXPLICIT_STATE_ROOT_SOURCE);
        assert_eq!(ctx.handoff_file_name, TERMINAL_HANDOFF_FILE_NAME);
        assert_eq!(ctx.working_directory, "/home/user/project");
        assert_eq!(ctx.command_count, "42");
        assert_eq!(ctx.wait_count, "3");
        assert_eq!(ctx.last_output_line.as_deref(), Some("build succeeded"));
    }

    #[test]
    fn from_engineer_evidence_defaults_wait_count_to_zero() {
        let records = vec![
            make_evidence(&format!(
                "terminal-continuity-source={SHARED_DEFAULT_STATE_ROOT_SOURCE}"
            )),
            make_evidence(&format!(
                "terminal-continuity-handoff={COMPATIBILITY_HANDOFF_FILE_NAME}"
            )),
            make_evidence("terminal-continuity-working-directory=/w"),
            make_evidence("terminal-continuity-command-count=1"),
        ];
        let ctx = TerminalBridgeContext::from_engineer_evidence(&records)
            .unwrap()
            .expect("should return Some");
        assert_eq!(ctx.wait_count, "0");
        assert!(ctx.last_output_line.is_none());
    }

    // ── 5. engineer_evidence_details ──

    #[test]
    fn engineer_evidence_details_without_last_output_line() {
        let ctx = TerminalBridgeContext {
            continuity_source: "src".to_string(),
            handoff_file_name: "file.json".to_string(),
            working_directory: "/wd".to_string(),
            command_count: "10".to_string(),
            wait_count: "2".to_string(),
            last_output_line: None,
        };
        let details = ctx.engineer_evidence_details();
        assert_eq!(details.len(), 5);
        assert_eq!(details[0], "terminal-continuity-source=src");
        assert_eq!(details[1], "terminal-continuity-handoff=file.json");
        assert_eq!(details[2], "terminal-continuity-working-directory=/wd");
        assert_eq!(details[3], "terminal-continuity-command-count=10");
        assert_eq!(details[4], "terminal-continuity-wait-count=2");
    }

    #[test]
    fn engineer_evidence_details_with_last_output_line() {
        let ctx = TerminalBridgeContext {
            continuity_source: "src".to_string(),
            handoff_file_name: "file.json".to_string(),
            working_directory: "/wd".to_string(),
            command_count: "10".to_string(),
            wait_count: "2".to_string(),
            last_output_line: Some("done".to_string()),
        };
        let details = ctx.engineer_evidence_details();
        assert_eq!(details.len(), 6);
        assert_eq!(details[5], "terminal-continuity-last-output-line=done");
    }

    // ── 6. select_optional_handoff_artifact returns None when no files exist ──

    #[test]
    fn select_optional_handoff_artifact_returns_none_for_empty_dir() {
        let dir = TempDir::new().unwrap();
        let result =
            select_optional_handoff_artifact(dir.path(), ScopedHandoffMode::Terminal, "test")
                .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn select_optional_handoff_artifact_returns_none_for_engineer_mode() {
        let dir = TempDir::new().unwrap();
        let result =
            select_optional_handoff_artifact(dir.path(), ScopedHandoffMode::Engineer, "test")
                .unwrap();
        assert!(result.is_none());
    }

    // ── 7. persist + select + load roundtrip ──

    #[test]
    fn persist_and_select_roundtrip_terminal() {
        let dir = TempDir::new().unwrap();
        let snapshot = minimal_snapshot(vec![]);

        persist_handoff_artifacts(dir.path(), ScopedHandoffMode::Terminal, &snapshot).unwrap();

        assert!(scoped_handoff_path(dir.path(), ScopedHandoffMode::Terminal).exists());
        assert!(compatibility_handoff_path(dir.path()).exists());

        let artifact =
            select_handoff_artifact_for_read(dir.path(), ScopedHandoffMode::Terminal, "test")
                .unwrap();
        assert_eq!(artifact.file_name, TERMINAL_HANDOFF_FILE_NAME);
        assert_eq!(
            artifact.path,
            scoped_handoff_path(dir.path(), ScopedHandoffMode::Terminal)
        );

        let loaded = load_runtime_handoff_snapshot(&artifact, "test").unwrap();
        assert_eq!(loaded, snapshot);
    }

    #[test]
    fn persist_and_select_roundtrip_engineer() {
        let dir = TempDir::new().unwrap();
        let snapshot = minimal_snapshot(vec![make_evidence("some-detail")]);

        persist_handoff_artifacts(dir.path(), ScopedHandoffMode::Engineer, &snapshot).unwrap();

        assert!(scoped_handoff_path(dir.path(), ScopedHandoffMode::Engineer).exists());
        assert!(compatibility_handoff_path(dir.path()).exists());

        let artifact =
            select_handoff_artifact_for_read(dir.path(), ScopedHandoffMode::Engineer, "test")
                .unwrap();
        assert_eq!(artifact.file_name, ENGINEER_HANDOFF_FILE_NAME);

        let loaded = load_runtime_handoff_snapshot(&artifact, "test").unwrap();
        assert_eq!(loaded.evidence_records.len(), 1);
        assert_eq!(loaded.evidence_records[0].detail, "some-detail");
    }

    #[test]
    fn select_optional_finds_compatibility_fallback() {
        let dir = TempDir::new().unwrap();
        let snapshot = minimal_snapshot(vec![]);

        persist_handoff_artifacts(dir.path(), ScopedHandoffMode::Terminal, &snapshot).unwrap();

        let artifact =
            select_optional_handoff_artifact(dir.path(), ScopedHandoffMode::Engineer, "test")
                .unwrap()
                .expect("should fall back to compatibility file");
        assert_eq!(artifact.file_name, COMPATIBILITY_HANDOFF_FILE_NAME);

        let loaded = load_runtime_handoff_snapshot(&artifact, "test").unwrap();
        assert_eq!(loaded, snapshot);
    }

    #[test]
    fn select_for_read_falls_back_to_compatibility() {
        let dir = TempDir::new().unwrap();
        let snapshot = minimal_snapshot(vec![]);

        persist_handoff_artifacts(dir.path(), ScopedHandoffMode::Terminal, &snapshot).unwrap();

        fs::remove_file(scoped_handoff_path(dir.path(), ScopedHandoffMode::Terminal)).unwrap();

        let artifact =
            select_handoff_artifact_for_read(dir.path(), ScopedHandoffMode::Terminal, "test")
                .unwrap();
        assert_eq!(artifact.file_name, COMPATIBILITY_HANDOFF_FILE_NAME);
    }

    #[test]
    fn select_for_read_errors_when_no_files_exist() {
        let dir = TempDir::new().unwrap();
        let result =
            select_handoff_artifact_for_read(dir.path(), ScopedHandoffMode::Terminal, "test");
        assert!(result.is_err());
    }

    #[test]
    fn load_from_state_root_returns_none_when_no_files() {
        let dir = TempDir::new().unwrap();
        let result =
            TerminalBridgeContext::load_from_state_root(dir.path(), "test-source").unwrap();
        assert!(result.is_none());
    }
}
