//! GitHub CLI dispatch helpers used by goal-session actions.

const GH_ARG_MAX_BYTES: usize = 32 * 1024;

fn run_gh(args: &[&str]) -> Result<String, String> {
    for a in args {
        if a.len() > GH_ARG_MAX_BYTES {
            return Err(format!(
                "gh argument exceeds {GH_ARG_MAX_BYTES} bytes (got {})",
                a.len()
            ));
        }
    }
    let output = std::process::Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("failed to execute gh: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "gh exited with status {}: {stderr}",
            output.status.code().unwrap_or(-1)
        ))
    }
}

/// Look up an open issue in `repo` whose title exactly matches `title`.
/// Returns `Ok(Some(number))` if a duplicate exists, `Ok(None)` otherwise.
/// Errors are non-fatal — we treat lookup failure as "no duplicate found"
/// and let the caller proceed (failing closed on dedup would block all
/// issue creation if the gh search API hiccups).
pub(super) fn find_duplicate_open_issue(repo: &str, title: &str) -> Result<Option<u64>, String> {
    let search = format!("\"{}\" in:title", title.replace('"', "\\\""));
    let json = run_gh(&[
        "issue",
        "list",
        "--repo",
        repo,
        "--state",
        "open",
        "--search",
        &search,
        "--json",
        "number,title",
        "--limit",
        "10",
    ])?;
    #[derive(serde::Deserialize)]
    struct Hit {
        number: u64,
        title: String,
    }
    let hits: Vec<Hit> =
        serde_json::from_str(&json).map_err(|e| format!("dedup parse failed: {e}"))?;
    let target = title.trim();
    Ok(hits
        .into_iter()
        .find(|h| h.title.trim() == target)
        .map(|h| h.number))
}

pub(super) fn dispatch_gh_issue_create(
    repo: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<String, String> {
    // Pre-flight dedup: refuse to file a second open issue with the same
    // title. The OODA daemon repeatedly proposed identical titles
    // (#1178-1183 were six dupes of #1177; #1247-1250 were four dupes of
    // each other). Title-hash check is cheap and stops the worst case.
    match find_duplicate_open_issue(repo, title) {
        Ok(Some(existing)) => {
            return Err(format!(
                "duplicate of open issue #{existing}: title \"{title}\" already exists"
            ));
        }
        Ok(None) => {}
        Err(e) => {
            // Non-fatal: log and proceed. We'd rather risk an occasional
            // dupe than block all issue creation on a search-API blip.
            eprintln!("[simard] dedup lookup failed (proceeding): {e}");
        }
    }

    let mut args: Vec<&str> = vec![
        "issue", "create", "--repo", repo, "--title", title, "--body", body,
    ];
    let label_csv;
    let sanitized_labels: Vec<String> = labels
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| is_plausible_label(l))
        .collect();
    if !sanitized_labels.is_empty() {
        label_csv = sanitized_labels.join(",");
        args.push("--label");
        args.push(&label_csv);
    }
    run_gh(&args)
}

/// Filter labels that are obviously bogus (placeholders, ellipses, control chars, empty).
/// Real labels here are short kebab-case-or-spaced strings; LLM occasionally emits
/// `"..."` or `".…"` (literal ellipsis) from truncated examples in the prompt.
pub(crate) fn is_plausible_label(label: &str) -> bool {
    if label.is_empty() || label.len() > 50 {
        return false;
    }
    // Reject pure-punctuation placeholders the LLM tends to emit (`...`, `.…`, `…`).
    if label
        .chars()
        .all(|c| matches!(c, '.' | '…' | '-' | '_' | ' '))
    {
        return false;
    }
    // Require at least one alphanumeric character.
    label.chars().any(|c| c.is_alphanumeric())
}

pub(super) fn dispatch_gh_issue_comment(
    repo: &str,
    issue: u64,
    body: &str,
) -> Result<String, String> {
    let issue_str = issue.to_string();
    run_gh(&[
        "issue", "comment", &issue_str, "--repo", repo, "--body", body,
    ])
}

pub(super) fn dispatch_gh_issue_close(
    repo: &str,
    issue: u64,
    comment: Option<&str>,
) -> Result<(), String> {
    let issue_str = issue.to_string();
    if let Some(body) = comment
        && !body.trim().is_empty()
    {
        let _ = run_gh(&[
            "issue", "comment", &issue_str, "--repo", repo, "--body", body,
        ])?;
    }
    let _ = run_gh(&["issue", "close", &issue_str, "--repo", repo])?;
    Ok(())
}

pub(super) fn dispatch_gh_pr_comment(repo: &str, pr: u64, body: &str) -> Result<String, String> {
    let pr_str = pr.to_string();
    run_gh(&["pr", "comment", &pr_str, "--repo", repo, "--body", body])
}
