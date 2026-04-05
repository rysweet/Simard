use std::fs;
use std::path::Path;

pub(crate) fn render_redacted_objective_metadata(value: &str) -> crate::SimardResult<String> {
    crate::sanitization::normalize_objective_metadata(value).ok_or_else(|| {
        crate::SimardError::InvalidHandoffSnapshot {
            field: "session.objective".to_string(),
            reason: "engineer read requires a trusted handoff artifact to persist objective metadata as objective-metadata(chars=<n>, words=<n>, lines=<n>)".to_string(),
        }
    })
}

pub(crate) fn required_terminal_evidence_value<'a>(
    evidence_records: &'a [crate::EvidenceRecord],
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
                "terminal read requires {handoff_source} to carry persisted terminal evidence '{}' for operator output",
                prefix.trim_end_matches('=')
            ),
        })
}

pub(crate) fn optional_terminal_evidence_value<'a>(
    evidence_records: &'a [crate::EvidenceRecord],
    prefix: &str,
) -> Option<&'a str> {
    evidence_records
        .iter()
        .rev()
        .find_map(|record| record.detail.strip_prefix(prefix))
}

pub(crate) fn terminal_evidence_values(
    evidence_records: &[crate::EvidenceRecord],
    prefix: &str,
) -> Vec<String> {
    evidence_records
        .iter()
        .filter_map(|record| record.detail.split_once('='))
        .filter(|(label, _)| {
            label.starts_with(prefix)
                && label[prefix.len()..]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_digit())
        })
        .map(|(_, value)| value.to_string())
        .collect()
}

pub(crate) fn load_terminal_objective_file(
    path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "terminal objective file '{}' could not be inspected: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "terminal objective file '{}' must be a regular file, not a symlink",
            path.display()
        )
        .into());
    }
    if !metadata.is_file() {
        return Err(format!(
            "terminal objective file '{}' must be a regular file",
            path.display()
        )
        .into());
    }

    fs::read_to_string(path).map_err(|error| {
        format!(
            "terminal objective file '{}' could not be read as UTF-8 text: {error}",
            path.display()
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionId, SessionPhase};
    use std::path::Path;

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn make_evidence(detail: &str) -> crate::EvidenceRecord {
        crate::EvidenceRecord {
            id: s("ev-1"),
            session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            phase: SessionPhase::Execution,
            detail: s(detail),
            source: crate::evidence::EvidenceSource::Runtime,
        }
    }

    #[test]
    fn required_terminal_evidence_value_found() {
        let records = vec![
            make_evidence("terminal-cwd=/home/user"),
            make_evidence("terminal-exit-code=0"),
        ];
        let val = required_terminal_evidence_value(&records, "terminal-exit-code=", "test-handoff")
            .unwrap();
        assert_eq!(val, "0");
    }

    #[test]
    fn required_terminal_evidence_value_not_found() {
        let records = vec![make_evidence("terminal-cwd=/home/user")];
        let err = required_terminal_evidence_value(&records, "terminal-exit-code=", "test-handoff")
            .unwrap_err();
        assert!(err.to_string().contains("terminal-exit-code"));
    }

    #[test]
    fn required_terminal_evidence_value_returns_last_match() {
        let records = vec![
            make_evidence("terminal-exit-code=1"),
            make_evidence("terminal-exit-code=0"),
        ];
        let val = required_terminal_evidence_value(&records, "terminal-exit-code=", "test-handoff")
            .unwrap();
        assert_eq!(val, "0", "should return last (most recent) match");
    }

    #[test]
    fn optional_terminal_evidence_value_found() {
        let records = vec![make_evidence("terminal-cwd=/workspace")];
        assert_eq!(
            optional_terminal_evidence_value(&records, "terminal-cwd="),
            Some("/workspace")
        );
    }

    #[test]
    fn optional_terminal_evidence_value_missing() {
        let records = vec![make_evidence("other-key=value")];
        assert_eq!(
            optional_terminal_evidence_value(&records, "terminal-cwd="),
            None
        );
    }

    #[test]
    fn terminal_evidence_values_collects_indexed_entries() {
        let records = vec![
            make_evidence("checkpoint1=first"),
            make_evidence("checkpoint2=second"),
            make_evidence("unrelated=skip"),
        ];
        let values = terminal_evidence_values(&records, "checkpoint");
        assert_eq!(values, vec!["first", "second"]);
    }

    #[test]
    fn terminal_evidence_values_empty_when_no_match() {
        let records = vec![make_evidence("other=value")];
        let values = terminal_evidence_values(&records, "checkpoint");
        assert!(values.is_empty());
    }

    #[test]
    fn render_redacted_objective_metadata_valid() {
        let result =
            render_redacted_objective_metadata("objective-metadata(chars=10, words=2, lines=1)");
        assert!(result.is_ok());
        let rendered = result.unwrap();
        assert!(rendered.contains("chars=10"));
        assert!(rendered.contains("words=2"));
        assert!(rendered.contains("lines=1"));
    }

    #[test]
    fn render_redacted_objective_metadata_invalid() {
        let result = render_redacted_objective_metadata("not a valid metadata string");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("objective"));
    }

    #[test]
    fn required_terminal_evidence_value_multiple_records_various_prefixes() {
        let records = vec![
            make_evidence("terminal-cwd=/home"),
            make_evidence("terminal-shell=bash"),
            make_evidence("terminal-exit-code=0"),
        ];
        assert_eq!(
            required_terminal_evidence_value(&records, "terminal-shell=", "test").unwrap(),
            "bash"
        );
        assert_eq!(
            required_terminal_evidence_value(&records, "terminal-cwd=", "test").unwrap(),
            "/home"
        );
    }

    #[test]
    fn terminal_evidence_values_ignores_non_prefix_matches() {
        let records = vec![
            make_evidence("step1=do A"),
            make_evidence("step2=do B"),
            make_evidence("other_step1=ignore"),
        ];
        let values = terminal_evidence_values(&records, "step");
        assert_eq!(values, vec!["do A", "do B"]);
    }

    #[test]
    fn terminal_evidence_values_requires_digit_after_prefix() {
        let records = vec![
            make_evidence("stepa=not a match"),
            make_evidence("step1=a match"),
        ];
        let values = terminal_evidence_values(&records, "step");
        assert_eq!(values, vec!["a match"]);
    }

    #[test]
    fn load_terminal_objective_file_reads_valid_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("objective.txt");
        std::fs::write(&file, "Build the widget").unwrap();
        let content = load_terminal_objective_file(&file).unwrap();
        assert_eq!(content, "Build the widget");
    }

    #[test]
    fn load_terminal_objective_file_fails_for_nonexistent() {
        let result = load_terminal_objective_file(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn load_terminal_objective_file_fails_for_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = load_terminal_objective_file(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn optional_terminal_evidence_value_empty_records() {
        let records: Vec<crate::EvidenceRecord> = vec![];
        assert_eq!(
            optional_terminal_evidence_value(&records, "terminal-cwd="),
            None
        );
    }

    #[test]
    fn optional_terminal_evidence_value_returns_last_match() {
        let records = vec![
            make_evidence("terminal-cwd=/first"),
            make_evidence("terminal-cwd=/second"),
        ];
        assert_eq!(
            optional_terminal_evidence_value(&records, "terminal-cwd="),
            Some("/second")
        );
    }

    #[test]
    fn terminal_evidence_values_handles_multi_digit_indices() {
        let records = vec![
            make_evidence("step10=ten"),
            make_evidence("step99=ninety-nine"),
        ];
        let values = terminal_evidence_values(&records, "step");
        assert_eq!(values, vec!["ten", "ninety-nine"]);
    }

    #[test]
    fn render_redacted_objective_metadata_empty_string() {
        assert!(render_redacted_objective_metadata("").is_err());
    }

    #[test]
    fn render_redacted_objective_metadata_partial_format() {
        assert!(render_redacted_objective_metadata("objective-metadata(chars=10").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn load_terminal_objective_file_rejects_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let real_file = dir.path().join("real.txt");
        std::fs::write(&real_file, "content").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real_file, &link).unwrap();
        let result = load_terminal_objective_file(&link);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("symlink"));
    }
}
