//! Stewardship loop — autonomous failure → deduplicated issue routing for
//! Simard (issue #1167).
//!
//! See `Specs/ProductArchitecture.md` § Stewardship Mode and
//! `docs/concepts/stewardship-mode.md`.
//!
//! Pipeline:
//! 1. Validate the [`OrchestratorRunSummary`] (fail-loud on missing fields).
//! 2. Route `source_module` → [`TargetRepo`] (unmatched → default repo).
//! 3. Compute a noise-stripped [`failure_signature`].
//! 4. Search the target repo for an open issue with that signature.
//! 5. If found → [`StewardshipOutcome::MatchedExisting`].
//!    Otherwise → file a new issue → [`StewardshipOutcome::FiledNew`].
//! 6. Return the issue handle without feeding the automation output back into
//!    the goal board.

pub mod dedup;
pub mod gh_client;
pub mod merge_authority;
pub mod merge_judge;
pub mod objective_merge_judge;
pub mod recipe_merge_judge;
pub mod routing;
pub mod types;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_extra;
// TDD (Step 7): failing tests for the P1 objective-merge-judge fallback.
#[cfg(test)]
mod tests_objective_merge_judge;

pub use dedup::{failure_signature, find_existing, normalize};
pub use gh_client::{GhClient, GhIssue, RealGhClient};
pub use merge_authority::{
    BASE_ALLOWLIST_ENV, DEFAULT_BASE_ALLOWLIST, MergeOutcome, OpenPrSummary, PrGhClient,
    PrSnapshot, RealPrGhClient, base_allowlist_from_env, evaluate_objective_gates,
    merge_pr_if_merge_ready, merge_pr_if_merge_ready_with_allowlist,
    merge_pr_if_merge_ready_with_judge, parse_pr_list_json,
};
pub use merge_judge::{
    Blocker, JudgeOutcome, LlmMergeJudge, MergeJudge, MergeJudgeKind, RefusingMergeJudge, Verdict,
    build_merge_judge, resolve_merge_judge_kind,
};
pub use objective_merge_judge::ObjectiveMergeJudge;
pub use recipe_merge_judge::RecipeMergeJudge;
pub use routing::route_failure;
pub use types::{OrchestratorRunSummary, StewardshipOutcome, TargetRepo};

use crate::error::SimardResult;

const ISSUE_TITLE: &str = "[stewardship] Orchestrator failure";
const REDACTED_SECRET: &str = "[redacted secret]";
const SENSITIVE_ASSIGNMENT_KEYS: &[&str] = &[
    "authorization",
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "key",
    "access_key",
    "account_key",
    "accountkey",
    "private_key",
    "sig",
    "credential",
];

/// Process one orchestrator run summary end-to-end. See the module docstring
/// for the pipeline.
pub fn process_orchestrator_run(
    run: &OrchestratorRunSummary,
    gh: &dyn GhClient,
) -> SimardResult<StewardshipOutcome> {
    types::validate(run)?;
    let target = route_failure(&run.source_module)?;
    let repo = target.slug().to_string();
    let signature = failure_signature(&run.failure_kind, &run.error_text);

    let existing = gh.search_issues(&repo, &signature)?;
    if let Some(issue) = find_existing(&existing, &signature) {
        return Ok(StewardshipOutcome::MatchedExisting {
            repo,
            issue_number: issue.number,
            url: issue.url.clone(),
            signature,
        });
    }

    let run_id = sanitize_issue_text(&run.run_id);
    let failed_step = sanitize_issue_text(&run.failed_step);
    let source_module = sanitize_issue_text(&run.source_module);
    let error_text = sanitize_issue_text(&run.error_text);
    let body = format!(
        "filed-by: simard-stewardship\n\
         stewardship-signature: {sig}\n\
         originating-run: {rid}\n\
         failed-step: {step}\n\
         source-module: {src}\n\
         \n\
         ## Error\n\
         {err}\n",
        sig = signature,
        rid = run_id,
        step = failed_step,
        src = source_module,
        err = error_text,
    );
    let new = gh.create_issue(&repo, ISSUE_TITLE, &body)?;
    Ok(StewardshipOutcome::FiledNew {
        repo,
        issue_number: new.number,
        url: new.url,
        signature,
    })
}

fn sanitize_issue_text(input: &str) -> String {
    let scrubbed = crate::journal::scrub_secrets(input);
    let assignments = redact_sensitive_assignments(&scrubbed);
    let bearer_tokens = redact_bearer_tokens(&assignments);
    let jwt_tokens = redact_jwts(&bearer_tokens);
    redact_cloud_access_keys(&jwt_tokens)
}

fn redact_sensitive_assignments(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut index = 0;

    while index < input.len() {
        let Some(key) = SENSITIVE_ASSIGNMENT_KEYS.iter().find(|key| {
            lower[index..].starts_with(**key)
                && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
        }) else {
            index += input[index..]
                .chars()
                .next()
                .expect("non-empty remainder")
                .len_utf8();
            continue;
        };

        let mut separator = index + key.len();
        while separator < input.len() && bytes[separator].is_ascii_whitespace() {
            separator += 1;
        }
        if separator == input.len() || !matches!(bytes[separator], b':' | b'=') {
            index += key.len();
            continue;
        }

        let mut value_start = separator + 1;
        while value_start < input.len()
            && bytes[value_start].is_ascii_whitespace()
            && bytes[value_start] != b'\n'
        {
            value_start += 1;
        }
        if value_start == input.len() || bytes[value_start] == b'\n' {
            index = value_start;
            continue;
        }

        let value_end = if input[value_start..].starts_with(REDACTED_SECRET) {
            value_start + REDACTED_SECRET.len()
        } else if *key == "authorization" {
            input[value_start..]
                .find('\n')
                .map_or(input.len(), |offset| value_start + offset)
        } else {
            credential_value_end(input, value_start)
        };
        output.push_str(&input[copied_through..value_start]);
        output.push_str(REDACTED_SECRET);
        copied_through = value_end;
        index = value_end;
    }

    output.push_str(&input[copied_through..]);
    output
}

fn credential_value_end(input: &str, value_start: usize) -> usize {
    let bytes = input.as_bytes();
    let quote = matches!(bytes[value_start], b'\'' | b'"').then_some(bytes[value_start]);
    let mut end = value_start + usize::from(quote.is_some());
    while end < input.len() {
        if quote.is_some_and(|quote| bytes[end] == quote) {
            return end + 1;
        }
        if quote.is_none() && bytes[end].is_ascii_whitespace() {
            break;
        }
        end += input[end..]
            .chars()
            .next()
            .expect("non-empty remainder")
            .len_utf8();
    }
    end
}

fn redact_bearer_tokens(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied_through = 0;
    let mut index = 0;

    while index < input.len() {
        if !lower[index..].starts_with("bearer")
            || (index > 0 && bytes[index - 1].is_ascii_alphanumeric())
        {
            index += input[index..]
                .chars()
                .next()
                .expect("non-empty remainder")
                .len_utf8();
            continue;
        }
        let mut token_start = index + "bearer".len();
        if token_start == input.len() || !bytes[token_start].is_ascii_whitespace() {
            index = token_start;
            continue;
        }
        while token_start < input.len() && bytes[token_start].is_ascii_whitespace() {
            token_start += 1;
        }
        if token_start == input.len() {
            break;
        }
        let token_end = credential_value_end(input, token_start);
        output.push_str(&input[copied_through..token_start]);
        output.push_str(REDACTED_SECRET);
        copied_through = token_end;
        index = token_end;
    }

    output.push_str(&input[copied_through..]);
    output
}

fn redact_jwts(input: &str) -> String {
    redact_token_shape(input, |candidate| {
        let mut parts = candidate.split('.');
        matches!(
            (parts.next(), parts.next(), parts.next(), parts.next()),
            (Some(header), Some(payload), Some(signature), None)
                if header.len() >= 8 && payload.len() >= 8 && signature.len() >= 8
        )
    })
}

fn redact_cloud_access_keys(input: &str) -> String {
    redact_token_shape(input, |candidate| {
        let aws_access_key = candidate.len() == 20
            && (candidate.starts_with("AKIA") || candidate.starts_with("ASIA"))
            && candidate
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
        let google_api_key = candidate.len() == 39
            && candidate.starts_with("AIza")
            && candidate
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        aws_access_key || google_api_key
    })
}

fn redact_token_shape(input: &str, is_secret: impl Fn(&str) -> bool) -> String {
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        let ch = input[index..].chars().next().expect("non-empty remainder");
        if !ch.is_ascii_alphanumeric() {
            output.push(ch);
            index += ch.len_utf8();
            continue;
        }

        let start = index;
        while index < input.len() {
            let ch = input[index..].chars().next().expect("non-empty remainder");
            if !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')) {
                break;
            }
            index += ch.len_utf8();
        }
        let mut end = index;
        while end > start && input.as_bytes()[end - 1] == b'.' {
            end -= 1;
        }
        let candidate = &input[start..end];
        if is_secret(candidate) {
            output.push_str(REDACTED_SECRET);
            output.push_str(&input[end..index]);
        } else {
            output.push_str(&input[start..index]);
        }
    }
    output
}
