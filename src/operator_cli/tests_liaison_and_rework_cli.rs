//! TDD tests for the agent-facing WRITE tools (design components C10) that back
//! the two capabilities. Both are called by their recipes; humans use them for
//! fixtures. All validation lives in the parsers, which reject contradictory
//! invocations LOUDLY (usage error → exit code 2) rather than writing a
//! contradictory record.
//!
//! ## A. Extended `simard merge record-verdict` (rework loop)
//! `crate::operator_cli::merge::parse_record_verdict_args` gains:
//!   - `--reworkable` → `RecordVerdictArgs.reworkable: bool`. Only valid with
//!     `--verdict hold`.
//!   - `--concern <TEXT>` / `--concern @FILE` → `RecordVerdictArgs.concern:
//!     Option<String>`. Requires `--reworkable`.
//!
//! Contradiction guards (all Err → exit 2): `--verdict merge --reworkable`;
//! `--concern` without `--reworkable`; `--reworkable` without `--concern`;
//! invalid `--repo` slug.
//!
//! ## B. New `simard liaison record-decision` (operator-liaison)
//! `crate::operator_cli::liaison::parse_liaison_record_decision_args` →
//!   `LiaisonRecordDecisionArgs { group_id, message_id, run_token,
//!      reply: Option<String>, directive: Option<DirectiveArgs> }`.
//!   Directive flags (`--directive-recipe`, `--directive-task-path`,
//!   `--directive-repo`, `--directive-context-path`) are ALL-OR-NOTHING.
//!   Guards (all Err → exit 2): a partial directive; neither a reply nor a
//!   complete directive; an invalid `--directive-repo` slug.
//!
//! References not-yet-added fields/module → FAILS TO COMPILE until C10 lands.

use crate::operator_cli::liaison::{LiaisonRecordDecisionArgs, parse_liaison_record_decision_args};
use crate::operator_cli::merge::parse_record_verdict_args;

fn v(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// Write a temp file with `contents`, returning its path as a String.
fn temp_file(tag: &str, contents: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "simard-cli-{tag}-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, contents).unwrap();
    path.to_string_lossy().into_owned()
}

// ════════════════════ A. merge record-verdict --reworkable/--concern ═════════

#[test]
fn record_verdict_reworkable_hold_with_concern_parses() {
    let args = parse_record_verdict_args(v(&[
        "--pr",
        "4931",
        "--repo",
        "rysweet/Simard",
        "--verdict",
        "hold",
        "--reworkable",
        "--concern",
        "Clamp before multiply; add a ceiling test.",
        "--reason",
        "fixable",
        "--run-token",
        "tok",
    ]))
    .expect("a reworkable hold with a concern is valid");
    assert!(args.reworkable, "--reworkable must set the flag true");
    assert_eq!(
        args.concern.as_deref(),
        Some("Clamp before multiply; add a ceiling test.")
    );
}

#[test]
fn record_verdict_concern_at_file_form_is_read_from_disk() {
    let path = temp_file("concern", "The backoff multiplies before clamping.");
    let args = parse_record_verdict_args(v(&[
        "--pr",
        "4931",
        "--repo",
        "rysweet/Simard",
        "--verdict",
        "hold",
        "--reworkable",
        "--concern",
        &format!("@{path}"),
        "--reason",
        "fixable",
        "--run-token",
        "tok",
    ]))
    .expect("@FILE concern form is valid");
    assert_eq!(
        args.concern.as_deref(),
        Some("The backoff multiplies before clamping."),
        "--concern @FILE must be resolved to the file's contents"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn record_verdict_plain_hold_defaults_reworkable_false() {
    let args = parse_record_verdict_args(v(&[
        "--pr",
        "1",
        "--repo",
        "o/r",
        "--verdict",
        "hold",
        "--reason",
        "x",
        "--run-token",
        "t",
    ]))
    .expect("a plain hold is valid");
    assert!(
        !args.reworkable,
        "without --reworkable the flag defaults false"
    );
    assert_eq!(args.concern, None);
}

#[test]
fn record_verdict_merge_with_reworkable_is_usage_error() {
    let err = parse_record_verdict_args(v(&[
        "--pr",
        "1",
        "--repo",
        "o/r",
        "--verdict",
        "merge",
        "--reworkable",
        "--concern",
        "x",
        "--reason",
        "y",
        "--run-token",
        "t",
    ]))
    .expect_err("a merge verdict cannot be reworkable");
    assert!(
        err.to_lowercase().contains("rework") || err.to_lowercase().contains("merge"),
        "error should explain the merge/reworkable contradiction: {err}"
    );
}

#[test]
fn record_verdict_concern_without_reworkable_is_usage_error() {
    parse_record_verdict_args(v(&[
        "--pr",
        "1",
        "--repo",
        "o/r",
        "--verdict",
        "hold",
        "--concern",
        "x",
        "--reason",
        "y",
        "--run-token",
        "t",
    ]))
    .expect_err("a concern without --reworkable is meaningless and must be rejected");
}

#[test]
fn record_verdict_reworkable_without_concern_is_usage_error() {
    parse_record_verdict_args(v(&[
        "--pr",
        "1",
        "--repo",
        "o/r",
        "--verdict",
        "hold",
        "--reworkable",
        "--reason",
        "y",
        "--run-token",
        "t",
    ]))
    .expect_err("--reworkable with no --concern gives the recipe nothing to act on");
}

#[test]
fn record_verdict_still_rejects_bad_repo_slug() {
    parse_record_verdict_args(v(&[
        "--pr",
        "1",
        "--repo",
        "not-a-slug",
        "--verdict",
        "hold",
        "--reworkable",
        "--concern",
        "x",
        "--reason",
        "y",
        "--run-token",
        "t",
    ]))
    .expect_err("an invalid --repo slug must still be rejected");
}

// ════════════════════ B. liaison record-decision ═════════════════════════════

#[test]
fn liaison_reply_only_parses() {
    let reply = temp_file("reply", "On it — kicking off a fix now.");
    let args: LiaisonRecordDecisionArgs = parse_liaison_record_decision_args(v(&[
        "--group-id",
        "grp==",
        "--message-id",
        "1690000000123",
        "--run-token",
        "tok",
        "--reply-path",
        &reply,
    ]))
    .expect("a reply-only decision is valid");
    assert_eq!(
        args.reply.as_deref(),
        Some("On it — kicking off a fix now.")
    );
    assert!(args.directive.is_none());
    let _ = std::fs::remove_file(reply);
}

#[test]
fn liaison_full_directive_parses() {
    let task = temp_file("task", "Investigate and fix the flaky canary.");
    let context = temp_file("ctx", "canary failed 3x in the last hour");
    let args = parse_liaison_record_decision_args(v(&[
        "--group-id",
        "grp==",
        "--message-id",
        "42",
        "--run-token",
        "tok",
        "--directive-recipe",
        "default-workflow",
        "--directive-task-path",
        &task,
        "--directive-repo",
        "rysweet/Simard",
        "--directive-context-path",
        &context,
    ]))
    .expect("a complete directive (no reply) is valid");
    let d = args.directive.expect("directive present");
    assert_eq!(d.recipe, "default-workflow");
    assert_eq!(d.target_repo, "rysweet/Simard");
    assert_eq!(d.task_description, "Investigate and fix the flaky canary.");
    let _ = std::fs::remove_file(task);
    let _ = std::fs::remove_file(context);
}

#[test]
fn liaison_partial_directive_is_usage_error() {
    let task = temp_file("task", "do the thing");
    // Missing --directive-repo and --directive-context-path ⇒ partial ⇒ reject.
    parse_liaison_record_decision_args(v(&[
        "--group-id",
        "grp==",
        "--message-id",
        "42",
        "--run-token",
        "tok",
        "--directive-recipe",
        "default-workflow",
        "--directive-task-path",
        &task,
    ]))
    .expect_err("a partial directive (some but not all flags) must be rejected");
    let _ = std::fs::remove_file(task);
}

#[test]
fn liaison_neither_reply_nor_directive_is_usage_error() {
    parse_liaison_record_decision_args(v(&[
        "--group-id",
        "grp==",
        "--message-id",
        "42",
        "--run-token",
        "tok",
    ]))
    .expect_err("a decision with neither a reply nor a directive is a no-op and must be rejected");
}

#[test]
fn liaison_invalid_directive_repo_slug_is_usage_error() {
    let task = temp_file("task", "x");
    let context = temp_file("ctx", "y");
    parse_liaison_record_decision_args(v(&[
        "--group-id",
        "grp==",
        "--message-id",
        "42",
        "--run-token",
        "tok",
        "--directive-recipe",
        "default-workflow",
        "--directive-task-path",
        &task,
        "--directive-repo",
        "not-a-slug",
        "--directive-context-path",
        &context,
    ]))
    .expect_err("an invalid --directive-repo slug must be rejected (reuses validate_repo_slug)");
    let _ = std::fs::remove_file(task);
    let _ = std::fs::remove_file(context);
}
