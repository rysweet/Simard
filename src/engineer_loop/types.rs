use std::path::{Component, Path};
use std::time::Duration;

use crate::error::{SimardError, SimardResult};
use crate::goals::GoalRecord;
use crate::terminal_engineer_bridge::TerminalBridgeContext;

use std::path::PathBuf;

/// Serialize/deserialize Duration as u64 milliseconds for JSON IPC between
/// recipe steps. Lossy below 1ms, but engineer-loop phases are coarse-grained.
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis().min(u128::from(u64::MAX)) as u64)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let ms = u64::deserialize(d)?;
        Ok(Duration::from_millis(ms))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RepoInspection {
    pub workspace_root: PathBuf,
    pub repo_root: PathBuf,
    pub branch: String,
    pub head: String,
    pub worktree_dirty: bool,
    pub changed_files: Vec<String>,
    pub active_goals: Vec<GoalRecord>,
    pub carried_meeting_decisions: Vec<String>,
    pub architecture_gap_summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StructuredEditRequest {
    pub relative_path: String,
    pub search: String,
    pub replacement: String,
    pub verify_contains: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateFileRequest {
    pub relative_path: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppendToFileRequest {
    pub relative_path: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShellCommandRequest {
    pub argv: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GitCommitRequest {
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpenIssueRequest {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum EngineerActionKind {
    ReadOnlyScan,
    StructuredTextReplace(StructuredEditRequest),
    CargoTest,
    CargoCheck,
    CreateFile(CreateFileRequest),
    AppendToFile(AppendToFileRequest),
    RunShellCommand(ShellCommandRequest),
    GitCommit(GitCommitRequest),
    OpenIssue(OpenIssueRequest),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectedEngineerAction {
    pub label: String,
    pub rationale: String,
    pub argv: Vec<String>,
    pub plan_summary: String,
    pub verification_steps: Vec<String>,
    pub expected_changed_files: Vec<String>,
    pub kind: EngineerActionKind,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExecutedEngineerAction {
    pub selected: SelectedEngineerAction,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub changed_files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerificationReport {
    pub status: String,
    pub summary: String,
    pub checks: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PhaseOutcome {
    Success,
    Failed(String),
    Skipped(String),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PhaseTrace {
    pub name: String,
    #[serde(with = "duration_millis")]
    pub duration: Duration,
    pub outcome: PhaseOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EngineerLoopRun {
    pub state_root: PathBuf,
    pub execution_scope: String,
    pub inspection: RepoInspection,
    pub action: ExecutedEngineerAction,
    pub verification: VerificationReport,
    pub terminal_bridge_context: Option<TerminalBridgeContext>,
    #[serde(with = "duration_millis")]
    pub elapsed_duration: Duration,
    pub phase_traces: Vec<PhaseTrace>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzedAction {
    CreateFile,
    AppendToFile,
    RunShellCommand,
    GitCommit,
    OpenIssue,
    StructuredTextReplace,
    CargoTest,
    ReadOnlyScan,
}

/// Classify an objective string into an action category using keyword matching.
/// Case-insensitive. More specific compound patterns are checked before single
/// keywords so that "run tests" maps to `CargoTest` rather than `RunShellCommand`.
pub fn analyze_objective(objective: &str) -> AnalyzedAction {
    let lower = objective.to_lowercase();

    // Issue/bug patterns before "create" — "create a feature request" is an issue, not a file
    if lower.contains("issue") || lower.contains("bug report") || lower.contains("feature request")
    {
        AnalyzedAction::OpenIssue
    } else if lower.contains("new file") || lower.contains("create") || lower.contains("add file") {
        AnalyzedAction::CreateFile
    } else if lower.contains("append") || lower.contains("add to") {
        AnalyzedAction::AppendToFile
    } else if lower.contains("commit") || lower.contains("save changes") {
        AnalyzedAction::GitCommit
    } else if lower.contains("cargo test")
        || lower.contains("run tests")
        || lower.contains("test suite")
        || lower.contains("run the tests")
    {
        AnalyzedAction::CargoTest
    } else if lower.contains("run") || lower.contains("execute") || lower.contains("check") {
        AnalyzedAction::RunShellCommand
    } else if lower.contains("fix")
        || lower.contains("change")
        || lower.contains("update")
        || lower.contains("replace")
    {
        AnalyzedAction::StructuredTextReplace
    } else if lower.contains("test") {
        AnalyzedAction::CargoTest
    } else {
        AnalyzedAction::ReadOnlyScan
    }
}

/// Words that indicate the extracted text is natural-language prose rather
/// than a real shell command.  Checked as whole whitespace-delimited tokens.
const PROSE_SIGNAL_WORDS: &[&str] = &[
    "and",
    "or",
    "but",
    "then",
    "also",
    "should",
    "would",
    "could",
    "please",
    "the",
    "this",
    "that",
    "with",
    "from",
    "into",
    "about",
    "against",
    "after",
    "before",
    "because",
    "since",
    "while",
    "although",
    "however",
    "therefore",
    "furthermore",
    "additionally",
    "shall",
    "will",
    "might",
    "must",
];

/// Returns `true` when `text` looks like a natural-language prose fragment
/// rather than a structured shell command.
pub(crate) fn is_prose_fragment(text: &str) -> bool {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return true;
    }

    // Sentence-ending punctuation anywhere in the tokens is a strong signal.
    if tokens
        .iter()
        .any(|t| t.ends_with('.') || t.ends_with('?') || t.ends_with('!'))
    {
        return true;
    }

    // If more than a third of the tokens are prose signal words, it's prose.
    let prose_count = tokens
        .iter()
        .filter(|t| PROSE_SIGNAL_WORDS.contains(&t.to_lowercase().as_str()))
        .count();
    if tokens.len() >= 3 && prose_count * 3 >= tokens.len() {
        return true;
    }

    // Issue/PR references like "#890" in the middle of a "command" are prose.
    if tokens
        .iter()
        .skip(1)
        .any(|t| t.starts_with('#') && t.len() > 1)
    {
        return true;
    }

    false
}

pub(crate) fn extract_command_from_objective(objective: &str) -> Option<Vec<String>> {
    let lower = objective.to_lowercase();
    let rest = if let Some(idx) = lower.find("run ") {
        &objective[idx + 4..]
    } else if let Some(idx) = lower.find("execute ") {
        &objective[idx + 8..]
    } else {
        return None;
    };
    let argv: Vec<String> = rest.split_whitespace().map(String::from).collect();
    if argv.is_empty() {
        return None;
    }

    // Reject if the extracted text looks like prose rather than a command.
    if is_prose_fragment(rest) {
        return None;
    }

    Some(argv)
}

pub(crate) fn extract_file_path_from_objective(objective: &str) -> Option<String> {
    objective
        .split_whitespace()
        .find(|w| w.contains('/') || (w.contains('.') && w.len() > 2))
        .map(|s| s.to_string())
}

pub(crate) fn parse_structured_edit_request(
    objective: &str,
) -> SimardResult<Option<StructuredEditRequest>> {
    let mut relative_path = None;
    let mut search = None;
    let mut replacement = None;
    let mut verify_contains = None;
    let mut saw_edit_directive = false;

    for line in objective.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("edit-file:") {
            saw_edit_directive = true;
            relative_path = Some(non_empty_objective_value("edit-file", value)?);
        } else if let Some(value) = trimmed.strip_prefix("replace:") {
            saw_edit_directive = true;
            search = Some(unescape_edit_value(&non_empty_objective_value(
                "replace", value,
            )?));
        } else if let Some(value) = trimmed.strip_prefix("with:") {
            saw_edit_directive = true;
            replacement = Some(unescape_edit_value(&non_empty_objective_value(
                "with", value,
            )?));
        } else if let Some(value) = trimmed.strip_prefix("verify-contains:") {
            saw_edit_directive = true;
            verify_contains = Some(unescape_edit_value(&non_empty_objective_value(
                "verify-contains",
                value,
            )?));
        }
    }

    if !saw_edit_directive {
        return Ok(None);
    }

    match (relative_path, search, replacement, verify_contains) {
        (Some(relative_path), Some(search), Some(replacement), Some(verify_contains)) => {
            Ok(Some(StructuredEditRequest {
                relative_path,
                search,
                replacement,
                verify_contains,
            }))
        }
        _ => Err(SimardError::UnsupportedEngineerAction {
            reason: "structured edit objectives must include non-empty edit-file:, replace:, with:, and verify-contains: lines".to_string(),
        }),
    }
}

pub(crate) fn non_empty_objective_value(field: &str, value: &str) -> SimardResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: format!("structured edit objective field '{field}' cannot be empty"),
        });
    }
    Ok(trimmed.to_string())
}

pub(crate) fn unescape_edit_value(value: &str) -> String {
    value.replace("\\n", "\n").replace("\\t", "\t")
}

pub(crate) fn validate_repo_relative_path(relative_path: &str) -> SimardResult<String> {
    let path = Path::new(relative_path);
    if path.is_absolute() {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: "structured edit target paths must stay relative to the selected repo"
                .to_string(),
        });
    }

    let mut normalized = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SimardError::UnsupportedEngineerAction {
                    reason: "structured edit target paths must not escape the selected repo"
                        .to_string(),
                });
            }
        }
    }

    if normalized.is_empty() {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: "structured edit target paths must identify a file under the selected repo"
                .to_string(),
        });
    }

    Ok(normalized.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_objective_create_file() {
        assert_eq!(
            analyze_objective("create a new file"),
            AnalyzedAction::CreateFile
        );
        assert_eq!(
            analyze_objective("add file to project"),
            AnalyzedAction::CreateFile
        );
    }

    #[test]
    fn analyze_objective_append() {
        assert_eq!(
            analyze_objective("append to the log"),
            AnalyzedAction::AppendToFile
        );
    }

    #[test]
    fn analyze_objective_commit() {
        assert_eq!(
            analyze_objective("commit the changes"),
            AnalyzedAction::GitCommit
        );
        assert_eq!(analyze_objective("save changes"), AnalyzedAction::GitCommit);
    }

    #[test]
    fn analyze_objective_issue() {
        assert_eq!(
            analyze_objective("open a new issue"),
            AnalyzedAction::OpenIssue
        );
        assert_eq!(
            analyze_objective("file a bug report"),
            AnalyzedAction::OpenIssue
        );
        assert_eq!(
            analyze_objective("create a feature request"),
            AnalyzedAction::OpenIssue
        );
    }

    #[test]
    fn analyze_objective_cargo_test() {
        assert_eq!(analyze_objective("cargo test"), AnalyzedAction::CargoTest);
        assert_eq!(analyze_objective("run tests"), AnalyzedAction::CargoTest);
        assert_eq!(analyze_objective("test suite"), AnalyzedAction::CargoTest);
    }

    #[test]
    fn analyze_objective_shell() {
        assert_eq!(
            analyze_objective("run ls -la"),
            AnalyzedAction::RunShellCommand
        );
        assert_eq!(
            analyze_objective("execute the script"),
            AnalyzedAction::RunShellCommand
        );
    }

    #[test]
    fn analyze_objective_structured_edit() {
        assert_eq!(
            analyze_objective("fix the typo"),
            AnalyzedAction::StructuredTextReplace
        );
        assert_eq!(
            analyze_objective("update the version"),
            AnalyzedAction::StructuredTextReplace
        );
        assert_eq!(
            analyze_objective("replace old with new"),
            AnalyzedAction::StructuredTextReplace
        );
    }

    #[test]
    fn analyze_objective_readonly_default() {
        assert_eq!(
            analyze_objective("inspect the workspace layout"),
            AnalyzedAction::ReadOnlyScan
        );
    }

    #[test]
    fn extract_command_from_objective_run() {
        let argv = extract_command_from_objective("run cargo test --all").unwrap();
        assert_eq!(argv, vec!["cargo", "test", "--all"]);
    }

    #[test]
    fn extract_command_from_objective_execute() {
        let argv = extract_command_from_objective("execute git status").unwrap();
        assert_eq!(argv, vec!["git", "status"]);
    }

    #[test]
    fn extract_command_from_objective_no_match() {
        assert!(extract_command_from_objective("just some text").is_none());
    }

    #[test]
    fn extract_command_rejects_prose_with_period() {
        // Issue #912: prose fragments like "git commit -m and open PR against #890."
        // should not be treated as shell commands.
        assert!(
            extract_command_from_objective("run git commit -m and open PR against #890.").is_none()
        );
    }

    #[test]
    fn extract_command_rejects_prose_with_conjunctions() {
        assert!(extract_command_from_objective("run the migration and then deploy").is_none());
    }

    #[test]
    fn extract_command_rejects_prose_with_issue_ref() {
        assert!(
            extract_command_from_objective("execute the fix for #123 in the planner").is_none()
        );
    }

    #[test]
    fn extract_command_accepts_real_commands() {
        let argv = extract_command_from_objective("run cargo test --all").unwrap();
        assert_eq!(argv, vec!["cargo", "test", "--all"]);
        let argv = extract_command_from_objective("run git status").unwrap();
        assert_eq!(argv, vec!["git", "status"]);
    }

    #[test]
    fn is_prose_fragment_detects_sentence_ending() {
        assert!(is_prose_fragment("commit -m and open PR against #890."));
        assert!(is_prose_fragment("what should we do?"));
        assert!(is_prose_fragment("stop the process!"));
    }

    #[test]
    fn is_prose_fragment_detects_conjunctions() {
        assert!(is_prose_fragment("the migration and then deploy"));
    }

    #[test]
    fn is_prose_fragment_detects_issue_refs() {
        assert!(is_prose_fragment("the fix for #123 in the planner"));
    }

    #[test]
    fn is_prose_fragment_allows_real_commands() {
        assert!(!is_prose_fragment("cargo test --all"));
        assert!(!is_prose_fragment("git status"));
        assert!(!is_prose_fragment("gh issue list"));
    }

    #[test]
    fn is_prose_fragment_empty_is_prose() {
        assert!(is_prose_fragment(""));
        assert!(is_prose_fragment("   "));
    }

    #[test]
    fn extract_file_path_from_objective_finds_path() {
        let path = extract_file_path_from_objective("create src/main.rs with content").unwrap();
        assert_eq!(path, "src/main.rs");
    }

    #[test]
    fn extract_file_path_from_objective_finds_dotfile() {
        let path = extract_file_path_from_objective("update Cargo.toml").unwrap();
        assert_eq!(path, "Cargo.toml");
    }

    #[test]
    fn extract_file_path_from_objective_none_when_no_path() {
        assert!(extract_file_path_from_objective("do something").is_none());
    }

    #[test]
    fn validate_repo_relative_path_valid() {
        assert_eq!(
            validate_repo_relative_path("src/main.rs").unwrap(),
            "src/main.rs"
        );
    }

    #[test]
    fn validate_repo_relative_path_strips_curdir() {
        assert_eq!(
            validate_repo_relative_path("./src/main.rs").unwrap(),
            "src/main.rs"
        );
    }

    #[test]
    fn validate_repo_relative_path_rejects_absolute() {
        assert!(validate_repo_relative_path("/etc/passwd").is_err());
    }

    #[test]
    fn validate_repo_relative_path_rejects_parent() {
        assert!(validate_repo_relative_path("../secret").is_err());
    }

    #[test]
    fn validate_repo_relative_path_rejects_empty() {
        assert!(validate_repo_relative_path("").is_err());
    }

    #[test]
    fn unescape_edit_value_newlines_and_tabs() {
        assert_eq!(unescape_edit_value("line1\\nline2"), "line1\nline2");
        assert_eq!(unescape_edit_value("col1\\tcol2"), "col1\tcol2");
    }

    #[test]
    fn parse_structured_edit_request_complete() {
        let objective =
            "edit-file: src/lib.rs\nreplace: old_fn\nwith: new_fn\nverify-contains: new_fn";
        let request = parse_structured_edit_request(objective).unwrap().unwrap();
        assert_eq!(request.relative_path, "src/lib.rs");
        assert_eq!(request.search, "old_fn");
        assert_eq!(request.replacement, "new_fn");
        assert_eq!(request.verify_contains, "new_fn");
    }

    #[test]
    fn parse_structured_edit_request_missing_field_errors() {
        let objective = "edit-file: src/lib.rs\nreplace: old_fn";
        let result = parse_structured_edit_request(objective);
        assert!(result.is_err());
    }

    #[test]
    fn parse_structured_edit_request_no_directives_returns_none() {
        let result = parse_structured_edit_request("just regular text").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn non_empty_objective_value_trims() {
        assert_eq!(
            non_empty_objective_value("field", "  hello  ").unwrap(),
            "hello"
        );
    }

    #[test]
    fn non_empty_objective_value_empty_errors() {
        assert!(non_empty_objective_value("field", "   ").is_err());
    }

    #[test]
    fn phase_outcome_variants() {
        let success = PhaseOutcome::Success;
        let failed = PhaseOutcome::Failed("reason".into());
        let skipped = PhaseOutcome::Skipped("why".into());
        assert_eq!(success, PhaseOutcome::Success);
        assert_ne!(success, failed);
        assert_ne!(failed, skipped);
    }
}
