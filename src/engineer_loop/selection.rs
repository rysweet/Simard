use crate::error::{SimardError, SimardResult};

use super::types::{
    AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind, GitCommitRequest,
    OpenIssueRequest, RepoInspection, SelectedEngineerAction, ShellCommandRequest,
    StructuredEditRequest, extract_command_from_objective, extract_file_path_from_objective,
    is_prose_fragment, parse_structured_edit_request, validate_repo_relative_path,
};

use super::SHELL_COMMAND_ALLOWLIST;

pub(crate) fn carry_forward_note(inspection: &RepoInspection) -> String {
    if inspection.carried_meeting_decisions.is_empty() {
        String::new()
    } else {
        format!(
            " Shared state root also carries {} meeting decision record{}, so the engineer loop keeps that handoff visible while choosing the next safe repo-native action.",
            inspection.carried_meeting_decisions.len(),
            if inspection.carried_meeting_decisions.len() == 1 {
                ""
            } else {
                "s"
            }
        )
    }
}

pub(crate) fn select_structured_edit(
    inspection: &RepoInspection,
    edit_request: StructuredEditRequest,
    note: &str,
) -> SimardResult<SelectedEngineerAction> {
    if inspection.worktree_dirty {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: "structured text replacement objectives require a clean git worktree so Simard does not overwrite unrelated local changes".to_string(),
        });
    }
    let relative_path = validate_repo_relative_path(&edit_request.relative_path)?;
    let verify_contains = edit_request.verify_contains.clone();
    Ok(SelectedEngineerAction {
        label: "structured-text-replace".to_string(),
        rationale: format!(
            "Objective includes explicit edit-file/replace/with/verify-contains directives, so the next honest bounded engineer action is to update '{}' once, then verify the requested text is present and visible through git state.{note}",
            relative_path
        ),
        argv: vec![
            "simard-structured-edit".to_string(),
            relative_path.clone(),
            "replace-once".to_string(),
        ],
        plan_summary: format!(
            "Inspect the clean repo, replace the requested text once in '{}', then verify the file content and git state reflect exactly that bounded local change.",
            relative_path
        ),
        verification_steps: vec![
            format!("confirm '{}' contains '{}'", relative_path, verify_contains),
            format!(
                "confirm git status reports '{}' as the only changed file",
                relative_path
            ),
            "confirm carried meeting decisions and active goals stayed stable".to_string(),
        ],
        expected_changed_files: vec![relative_path.clone()],
        kind: EngineerActionKind::StructuredTextReplace(StructuredEditRequest {
            relative_path,
            ..edit_request
        }),
    })
}

pub(crate) fn extract_content_body(objective: &str) -> String {
    objective
        .lines()
        .skip_while(|l| !l.to_lowercase().starts_with("content:"))
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn select_create_file(
    objective: &str,
    note: &str,
) -> Option<SimardResult<SelectedEngineerAction>> {
    let path = extract_file_path_from_objective(objective)?;
    let relative_path = match validate_repo_relative_path(&path) {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    let content = extract_content_body(objective);
    Some(Ok(SelectedEngineerAction {
        label: "create-file".to_string(),
        rationale: format!("Objective requests creating a new file at '{relative_path}'.{note}"),
        argv: vec!["simard-create-file".to_string(), relative_path.clone()],
        plan_summary: format!(
            "Create file '{}' with the specified content, then verify the file exists.",
            relative_path
        ),
        verification_steps: vec![
            format!("confirm '{}' exists", relative_path),
            "confirm file content matches request".to_string(),
        ],
        expected_changed_files: vec![relative_path.clone()],
        kind: EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path,
            content,
        }),
    }))
}

pub(crate) fn select_append_to_file(
    objective: &str,
    note: &str,
) -> Option<SimardResult<SelectedEngineerAction>> {
    let path = extract_file_path_from_objective(objective)?;
    let relative_path = match validate_repo_relative_path(&path) {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    let content = extract_content_body(objective);
    Some(Ok(SelectedEngineerAction {
        label: "append-to-file".to_string(),
        rationale: format!("Objective requests appending content to '{relative_path}'.{note}"),
        argv: vec!["simard-append-file".to_string(), relative_path.clone()],
        plan_summary: format!(
            "Append content to '{}', then verify the file contains the appended text.",
            relative_path
        ),
        verification_steps: vec![format!(
            "confirm '{}' contains appended content",
            relative_path
        )],
        expected_changed_files: vec![relative_path.clone()],
        kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
            relative_path,
            content,
        }),
    }))
}

pub(crate) fn select_shell_command(objective: &str, note: &str) -> Option<SelectedEngineerAction> {
    let argv = extract_command_from_objective(objective)?;
    select_shell_command_from_argv(argv, note, "Objective")
}

/// Build a shell-command action from an explicit argv vector.
///
/// Used when the LLM-produced `PlanStep.target` already contains the
/// concrete command, so we do not need to re-extract it from a prose
/// objective. The allowlist is enforced identically.
pub(crate) fn select_shell_command_from_argv(
    argv: Vec<String>,
    note: &str,
    source_label: &str,
) -> Option<SelectedEngineerAction> {
    if argv.is_empty() {
        return None;
    }
    let first = argv.first().cloned().unwrap_or_default();
    if !SHELL_COMMAND_ALLOWLIST.contains(&first.as_str()) {
        return None;
    }
    Some(SelectedEngineerAction {
        label: "run-shell-command".to_string(),
        rationale: format!(
            "{source_label} requests running '{}', which is in the shell allowlist.{note}",
            argv.join(" ")
        ),
        argv: argv.clone(),
        plan_summary: format!("Execute '{}' and capture output.", argv.join(" ")),
        verification_steps: vec![format!("confirm '{}' exits with status 0", argv.join(" "))],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::RunShellCommand(ShellCommandRequest { argv }),
    })
}

/// Tokenise an LLM-supplied target string into argv.
///
/// LLMs emit shell-command targets in two common forms:
///   * Plain space-separated argv (`"gh issue view 915"`).
///   * Backtick- or quote-wrapped (`"`gh issue view 915`"`).
///
/// We strip surrounding backticks/quotes and split on whitespace. This
/// is intentionally simple — the allowlist gate in
/// `select_shell_command_from_argv` rejects anything that does not begin
/// with a vetted binary, so adversarial parsing is not required here.
pub(crate) fn tokenise_target_argv(target: &str) -> Vec<String> {
    let trimmed = target.trim();
    let unwrapped = trimmed
        .trim_matches('`')
        .trim_matches('\'')
        .trim_matches('"')
        .trim();
    unwrapped.split_whitespace().map(String::from).collect()
}

pub(crate) fn select_git_commit(objective: &str, note: &str) -> SelectedEngineerAction {
    let raw_message = {
        let lower = objective.to_lowercase();
        if let Some(idx) = lower.find("commit ") {
            objective[idx + 7..].trim().to_string()
        } else {
            objective.to_string()
        }
    };
    let message = sanitize_commit_message(&raw_message);
    SelectedEngineerAction {
        label: "git-commit".to_string(),
        rationale: format!(
            "Objective requests committing changes with message: '{}'.{note}",
            message
        ),
        argv: vec![
            "git".to_string(),
            "commit".to_string(),
            "-m".to_string(),
            message.clone(),
        ],
        plan_summary: "Stage all changes and create a git commit.".to_string(),
        verification_steps: vec!["confirm HEAD changed (new commit created)".to_string()],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::GitCommit(GitCommitRequest { message }),
    }
}

/// Maximum length for a GitHub issue title created by the engineer loop.
pub(super) const MAX_ISSUE_TITLE_LEN: usize = 256;

/// Maximum digits to scan for an issue number. u64::MAX is 20 digits;
/// 21+ digits cannot fit and are rejected as overflow without `from_str` cost.
const MAX_ISSUE_DIGITS: usize = 20;

/// Scan an objective string for a reference to an existing GitHub issue
/// number. Returns the earliest non-zero issue number found across two
/// patterns:
///
/// - Pattern A: `#<digits>` — but rejected when the `#` is preceded by `&`
///   (HTML numeric character reference like `&#915;`) or when the digit run
///   is followed by an ASCII alphanumeric character.
/// - Pattern B: `issue` (case-insensitive) followed by ASCII whitespace,
///   then optionally `#`, `number `, or `id `, then `<digits>` with the
///   same word-boundary guard at the end.
///
/// The scanner is single-pass linear — no regex, no backtracking. Digit
/// runs are capped at `MAX_ISSUE_DIGITS` to bound input cost. Zero is
/// rejected because GitHub issue numbers start at 1.
pub(crate) fn extract_existing_issue_number(objective: &str) -> Option<u64> {
    let bytes = objective.as_bytes();
    let mut earliest: Option<(usize, u64)> = None;

    let mut consider = |start: usize, n: u64| {
        if earliest.map(|(s, _)| start < s).unwrap_or(true) {
            earliest = Some((start, n));
        }
    };

    // Pattern A: `#<digits>` with HTML-entity guard and trailing word-boundary.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            // HTML numeric character reference guard: reject `&#...`.
            let preceded_by_amp = i > 0 && bytes[i - 1] == b'&';
            if !preceded_by_amp
                && let Some((n, end)) = parse_digits(bytes, i + 1)
                && !is_alnum_byte(bytes.get(end).copied())
                && n != 0
            {
                consider(i, n);
            }
        }
        i += 1;
    }

    // Pattern B: `issue` (case-insensitive) + ws + optional `#` / `number ` / `id ` + digits.
    let lower = objective.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let needle = b"issue";
    let mut search_from = 0;
    while let Some(rel) = find_subslice(&lower_bytes[search_from..], needle) {
        let start = search_from + rel;
        let end_word = start + needle.len();
        // Require a leading word-boundary: previous byte must not be alnum.
        let leading_ok = start == 0 || !is_alnum_byte(Some(bytes[start - 1]));
        // Require ASCII whitespace immediately after `issue`.
        let trailing_ok = bytes
            .get(end_word)
            .map(|&b| b.is_ascii_whitespace())
            .unwrap_or(false);
        if leading_ok && trailing_ok {
            // Skip the whitespace run.
            let mut p = end_word;
            while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                p += 1;
            }
            // Optional `#`, `number `, or `id ` qualifier.
            if p < bytes.len() && bytes[p] == b'#' {
                p += 1;
            } else if try_consume_keyword(lower_bytes, p, b"number") {
                p += b"number".len();
                while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                    p += 1;
                }
            } else if try_consume_keyword(lower_bytes, p, b"id") {
                p += b"id".len();
                while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                    p += 1;
                }
            }
            if let Some((n, dend)) = parse_digits(bytes, p)
                && !is_alnum_byte(bytes.get(dend).copied())
                && n != 0
            {
                consider(start, n);
            }
        }
        search_from = start + needle.len();
    }

    earliest.map(|(_, n)| n)
}

fn is_alnum_byte(b: Option<u8>) -> bool {
    matches!(b, Some(c) if c.is_ascii_alphanumeric())
}

fn parse_digits(bytes: &[u8], start: usize) -> Option<(u64, usize)> {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() && (end - start) < MAX_ISSUE_DIGITS {
        end += 1;
    }
    if end == start {
        return None;
    }
    // If the digit run is bounded by MAX_ISSUE_DIGITS but more digits follow,
    // treat as overflow and reject.
    if end < bytes.len() && bytes[end].is_ascii_digit() {
        return None;
    }
    let s = std::str::from_utf8(&bytes[start..end]).ok()?;
    let n: u64 = s.parse().ok()?;
    Some((n, end))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// True when `lower_bytes[at..]` begins with `kw` AND the byte after `kw`
/// is ASCII whitespace (so `number` matches but `numbered` does not).
fn try_consume_keyword(lower_bytes: &[u8], at: usize, kw: &[u8]) -> bool {
    if at + kw.len() > lower_bytes.len() {
        return false;
    }
    if &lower_bytes[at..at + kw.len()] != kw {
        return false;
    }
    lower_bytes
        .get(at + kw.len())
        .map(|&b| b.is_ascii_whitespace())
        .unwrap_or(false)
}

pub(crate) fn select_open_issue(
    objective: &str,
    note: &str,
) -> SimardResult<SelectedEngineerAction> {
    // If the objective references an existing issue number, do NOT create a
    // new issue. Instead emit a read-only `gh issue view <N>` step so the
    // engineer loop verifies the issue exists and proceeds to implementation.
    if let Some(n) = extract_existing_issue_number(objective) {
        let n_str = n.to_string();
        return Ok(SelectedEngineerAction {
            label: "verify-existing-issue".to_string(),
            rationale: format!(
                "Objective references existing issue #{n}; verifying instead of creating.{note}"
            ),
            argv: vec![
                "gh".to_string(),
                "issue".to_string(),
                "view".to_string(),
                n_str,
            ],
            plan_summary: format!("Verify existing GitHub issue #{n} via gh CLI."),
            verification_steps: vec![format!(
                "confirm `gh issue view {n}` returns issue metadata (exit 0)"
            )],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::OpenIssue(OpenIssueRequest {
                title: format!("verify existing issue #{n}"),
                body: String::new(),
                labels: Vec::new(),
            }),
        });
    }

    let title = sanitize_issue_title(objective);

    if title.is_empty() {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: "refusing to create a GitHub issue with an empty title".to_string(),
        });
    }

    Ok(SelectedEngineerAction {
        label: "open-issue".to_string(),
        rationale: format!("Objective requests opening a GitHub issue.{note}"),
        argv: vec![
            "gh".to_string(),
            "issue".to_string(),
            "create".to_string(),
            "--title".to_string(),
            title.clone(),
        ],
        plan_summary: "Create a GitHub issue via gh CLI.".to_string(),
        verification_steps: vec!["confirm issue URL is returned in stdout".to_string()],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::OpenIssue(OpenIssueRequest {
            title,
            body: String::new(),
            labels: Vec::new(),
        }),
    })
}

/// Strip characters that are dangerous in shell arguments or would produce
/// malformed CLI commands when embedded in `--title` / `-m` values.
fn strip_shell_unsafe(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '`' | '$'
                    | '\\'
                    | '"'
                    | '\''
                    | '|'
                    | ';'
                    | '&'
                    | '<'
                    | '>'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '!'
                    | '\0'
            )
        })
        .collect()
}

/// Sanitize an objective string for use as a GitHub issue title.
///
/// Strips newlines, removes shell-unsafe characters, collapses whitespace,
/// and truncates to a reasonable length.
pub(crate) fn sanitize_issue_title(raw: &str) -> String {
    let cleaned = strip_shell_unsafe(raw);
    let single_line: String = cleaned
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if single_line.len() <= MAX_ISSUE_TITLE_LEN {
        single_line
    } else {
        let truncated = &single_line[..MAX_ISSUE_TITLE_LEN];
        // Cut at the last word boundary to avoid mid-word truncation.
        match truncated.rfind(' ') {
            Some(pos) if pos > MAX_ISSUE_TITLE_LEN / 2 => format!("{}…", &truncated[..pos]),
            _ => format!("{truncated}…"),
        }
    }
}

/// Sanitize a commit message: strip newlines, remove shell-unsafe characters,
/// collapse whitespace, and truncate.
pub(super) const MAX_COMMIT_MESSAGE_LEN: usize = 256;

pub(crate) fn sanitize_commit_message(raw: &str) -> String {
    let cleaned = strip_shell_unsafe(raw);
    let single_line: String = cleaned
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if single_line.len() <= MAX_COMMIT_MESSAGE_LEN {
        single_line
    } else {
        let truncated = &single_line[..MAX_COMMIT_MESSAGE_LEN];
        match truncated.rfind(' ') {
            Some(pos) if pos > MAX_COMMIT_MESSAGE_LEN / 2 => format!("{}…", &truncated[..pos]),
            _ => format!("{truncated}…"),
        }
    }
}

pub(crate) fn select_cargo_action(objective: &str, note: &str) -> SelectedEngineerAction {
    let obj_lower = objective.to_lowercase();
    if obj_lower.contains("cargo test")
        || obj_lower.contains("run tests")
        || obj_lower.contains("test suite")
        || obj_lower.contains("run the tests")
    {
        return SelectedEngineerAction {
            label: "cargo-test".to_string(),
            rationale: format!(
                "Objective explicitly requests running tests and a Cargo.toml is present, so the next bounded action is to run the test suite and report results.{note}"
            ),
            argv: vec![
                "cargo".to_string(),
                "test".to_string(),
                "--all-features".to_string(),
                "--locked".to_string(),
            ],
            plan_summary:
                "Run the full Rust test suite, capture results, and verify the build is healthy."
                    .to_string(),
            verification_steps: vec![
                "confirm cargo test exits with status 0".to_string(),
                "confirm test result line reports 0 failures".to_string(),
                "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
            ],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::CargoTest,
        };
    }
    if obj_lower.contains("cargo check")
        || obj_lower.contains("compilation check")
        || obj_lower.contains("check compilation")
        || obj_lower.contains("cargo build")
    {
        return SelectedEngineerAction {
            label: "cargo-check".to_string(),
            rationale: format!(
                "Objective mentions build/check and a Cargo.toml is present, so the next bounded action is to run cargo check and report compilation status.{note}"
            ),
            argv: vec![
                "cargo".to_string(),
                "check".to_string(),
                "--all-targets".to_string(),
                "--all-features".to_string(),
            ],
            plan_summary: "Run cargo check to verify the codebase compiles cleanly.".to_string(),
            verification_steps: vec![
                "confirm cargo check exits with status 0".to_string(),
                "confirm no compilation errors in output".to_string(),
                "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
            ],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::CargoCheck,
        };
    }
    SelectedEngineerAction {
        label: "cargo-metadata-scan".to_string(),
        rationale: format!(
            "Detected a Rust workspace via Cargo.toml, so the next honest v1 action is a local argv-only cargo metadata scan that inspects the workspace graph without pretending remote orchestration exists.{note}"
        ),
        argv: vec!["cargo".to_string(), "metadata".to_string(), "--format-version".to_string(), "1".to_string(), "--no-deps".to_string()],
        plan_summary: "Inspect the repo, query Cargo metadata without mutating files, and verify repo grounding stayed stable.".to_string(),
        verification_steps: vec![
            "confirm cargo metadata returns valid workspace JSON".to_string(),
            "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
            "confirm carried meeting decisions and active goals stayed stable".to_string(),
        ],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::ReadOnlyScan,
    }
}

/// Check whether a keyword-analyzed action is achievable given the current
/// objective and context. Returns `true` when the action is safe to proceed
/// with, `false` when it should be demoted to a safe default.
#[allow(dead_code)] // retained for tests + back-compat callers
pub(crate) fn is_keyword_action_achievable(action: &AnalyzedAction, objective: &str) -> bool {
    is_action_achievable(action, objective, None)
}

/// Like [`is_keyword_action_achievable`] but also considers the LLM's
/// explicit `PlanStep.target` field. RunShellCommand is achievable when
/// the target tokenises to an allowlisted command, even if the objective
/// itself is prose.
pub(crate) fn is_action_achievable(
    action: &AnalyzedAction,
    objective: &str,
    llm_target: Option<&str>,
) -> bool {
    match action {
        // These are always safe — they don't mutate the repo.
        AnalyzedAction::ReadOnlyScan | AnalyzedAction::CargoTest => true,
        // OpenIssue: only valid when the objective explicitly asks to *create* an issue
        // via a known prefix, OR references an existing issue number (which routes to
        // the verify path in `select_open_issue`). Bare prose like "Report a bug" or
        // "fix the issue" no longer qualifies.
        AnalyzedAction::OpenIssue => {
            if extract_existing_issue_number(objective).is_some() {
                return true;
            }
            let lower = objective.trim_start().to_lowercase();
            lower.starts_with("track ")
                || lower.starts_with("file an issue for")
                || lower.starts_with("create an issue")
        }
        // GitCommit: only valid when the objective is specifically about committing,
        // and the text after "commit" is not a prose fragment.
        AnalyzedAction::GitCommit => {
            let lower = objective.to_lowercase();
            if !(lower.contains("commit") || lower.contains("save changes")) {
                return false;
            }
            // Reject if the message portion is prose (e.g. "commit -m and open PR against #890.")
            if let Some(idx) = lower.find("commit ") {
                let after_commit = &objective[idx + 7..];
                if is_prose_fragment(after_commit) {
                    return false;
                }
            }
            true
        }
        // CreateFile / AppendToFile: need a discernible file path in the objective
        // OR in the LLM's target field.
        AnalyzedAction::CreateFile | AnalyzedAction::AppendToFile => {
            if let Some(target) = llm_target
                && !target.trim().is_empty()
            {
                return true;
            }
            extract_file_path_from_objective(objective).is_some()
        }
        // StructuredTextReplace: needs edit-like directives.
        AnalyzedAction::StructuredTextReplace => {
            let lower = objective.to_lowercase();
            lower.contains("replace") || lower.contains("edit-file") || lower.contains("update")
        }
        // RunShellCommand: achievable when either the LLM provided a target
        // whose first token is allowlisted, or the objective contains an
        // extractable allowlisted command.
        AnalyzedAction::RunShellCommand => {
            if let Some(target) = llm_target {
                let argv = tokenise_target_argv(target);
                if let Some(first) = argv.first()
                    && SHELL_COMMAND_ALLOWLIST.contains(&first.as_str())
                {
                    return true;
                }
            }
            extract_command_from_objective(objective).is_some()
        }
    }
}

pub fn select_engineer_action(
    inspection: &RepoInspection,
    objective: &str,
) -> SimardResult<SelectedEngineerAction> {
    let note = carry_forward_note(inspection);

    if let Some(edit_request) = parse_structured_edit_request(objective)? {
        return select_structured_edit(inspection, edit_request, &note);
    }

    // LLM-based planning is the ONLY planner. Per the project's no-fallback
    // rule, keyword analysis is no longer used as a backstop: a parser
    // failure or an LLM unavailability used to silently produce
    // "verify-existing-issue #N" fake-success cycles (issue #1062). Any
    // failure here propagates as PlanningUnavailable so the cycle reports a
    // real failure instead of fabricating progress.
    let plan = crate::engineer_plan::plan_objective(objective, inspection)?;
    let first_step =
        plan.steps()
            .first()
            .cloned()
            .ok_or_else(|| SimardError::PlanningUnavailable {
                reason: format!("LLM plan returned zero steps for objective: {objective}"),
            })?;
    let analyzed = first_step.action.clone();
    if !is_action_achievable(&analyzed, objective, Some(&first_step.target)) {
        return Err(SimardError::PlanningUnavailable {
            reason: format!(
                "LLM plan selected action {:?} with target {:?} which is not achievable for objective: {}",
                analyzed, first_step.target, objective
            ),
        });
    }

    match analyzed {
        AnalyzedAction::CreateFile => {
            if let Some(result) = select_create_file(objective, &note) {
                return result;
            }
        }
        AnalyzedAction::AppendToFile => {
            if let Some(result) = select_append_to_file(objective, &note) {
                return result;
            }
        }
        AnalyzedAction::RunShellCommand => {
            // Prefer the LLM's explicit target field — it already contains
            // the concrete argv. Fall back to objective extraction only if
            // the target is empty (older planner outputs).
            let argv = tokenise_target_argv(&first_step.target);
            if !argv.is_empty()
                && let Some(action) = select_shell_command_from_argv(argv, &note, "LLM plan step")
            {
                return Ok(action);
            }
            if let Some(action) = select_shell_command(objective, &note) {
                return Ok(action);
            }
            return Err(SimardError::PlanningUnavailable {
                reason: format!(
                    "LLM plan step selected RunShellCommand but target {:?} is empty or not in the shell allowlist (objective: {})",
                    first_step.target, objective
                ),
            });
        }
        AnalyzedAction::GitCommit => return Ok(select_git_commit(objective, &note)),
        AnalyzedAction::OpenIssue => return select_open_issue(objective, &note),
        _ => {}
    }

    if inspection.repo_root.join("Cargo.toml").is_file() {
        return Ok(select_cargo_action(objective, &note));
    }

    if inspection.repo_root.join(".git").exists() {
        return Ok(SelectedEngineerAction {
            label: "git-tracked-file-scan".to_string(),
            rationale: format!(
                "No repo-native language manifest was detected, so the loop falls back to a local argv-only scan of tracked files instead of inventing unsupported tooling.{note}"
            ),
            argv: vec!["git".to_string(), "ls-files".to_string(), "--cached".to_string()],
            plan_summary: "Inspect the repo, enumerate tracked files without mutating content, and verify repo grounding stayed stable.".to_string(),
            verification_steps: vec![
                "confirm at least one tracked file is reported".to_string(),
                "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
                "confirm carried meeting decisions and active goals stayed stable".to_string(),
            ],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::ReadOnlyScan,
        });
    }

    Err(SimardError::UnsupportedEngineerAction {
        reason: format!(
            "workspace '{}' is repo-grounded but exposes no supported local-first action policy",
            inspection.repo_root.display()
        ),
    })
}
