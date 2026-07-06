//! TDD contract for the dashboard feedback endpoint (issue #2629).
//!
//! These tests are written BEFORE `super::feedback` exists. They fail to
//! compile until Step 8 creates `src/operator_commands_dashboard/feedback.rs`
//! with the API surface exercised below, then fail on assertions until the
//! behaviour is implemented. They pin the behaviour of the "Report bug /
//! Request feature" widget's backend without spawning a subprocess or touching
//! the network — the `RecipeLauncher` seam (`crate::overseer::capabilities`) is
//! replaced with an in-memory [`FakeLauncher`].
//!
//! Contract that `super::feedback` MUST provide (all `pub(crate)`):
//!
//! ```ignore
//! use std::time::Instant;
//! use axum::http::StatusCode;
//! use serde_json::Value;
//! use crate::overseer::capabilities::{RecipeLauncher, WorkstreamStatus};
//!
//! pub(crate) enum FeedbackKind { Bug, Feature }
//! pub(crate) struct FeedbackReport { kind: FeedbackKind, title: String, description: String }
//! pub(crate) struct FeedbackContext { page: String, state: String, timestamp: String, identifiers: Value }
//! pub(crate) struct FeedbackDedup { /* OnceLock<Mutex<..>> in production */ }
//! impl FeedbackDedup { fn new() -> Self }
//!
//! pub(crate) const MAX_TITLE_LEN: usize;
//! pub(crate) const MAX_DESCRIPTION_LEN: usize;
//! pub(crate) const MAX_STATE_LEN: usize;
//! pub(crate) const DEDUP_WINDOW_SECS: u64;
//! pub(crate) const MAX_FEEDBACK_LAUNCHES_PER_WINDOW: usize;
//!
//! pub(crate) fn compose_task_description(report: &FeedbackReport, context: &FeedbackContext) -> String;
//! pub(crate) fn handle_feedback(launcher: &dyn RecipeLauncher, dedup: &FeedbackDedup, now: Instant, body: Value) -> (StatusCode, Value);
//! pub(crate) fn handle_feedback_status(launcher: &dyn RecipeLauncher, id: String) -> (StatusCode, Value);
//! pub(crate) fn status_json(status: &WorkstreamStatus) -> Value;
//! ```
//!
//! The pure core returns `(StatusCode, serde_json::Value)`; the thin axum
//! handlers wrap the `Value` in `Json` and register BOTH routes before the
//! existing `require_auth` layer, so this endpoint inherits the dashboard
//! access-code gate.

#![cfg(test)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::overseer::capabilities::{
    OverseerError, RecipeBrief, RecipeLauncher, WorkstreamHandle, WorkstreamStatus,
};
use crate::stewardship::TargetRepo;

use super::feedback::{
    DEDUP_WINDOW_SECS, FeedbackContext, FeedbackDedup, FeedbackKind, FeedbackReport,
    MAX_DESCRIPTION_LEN, MAX_FEEDBACK_LAUNCHES_PER_WINDOW, MAX_STATE_LEN, MAX_TITLE_LEN,
    compose_task_description, handle_feedback, handle_feedback_status, status_json,
};

// ─────────────────────────── fake launcher seam ────────────────────────────

/// In-memory [`RecipeLauncher`] that records every launched brief and returns
/// canned results. It NEVER spawns a process or opens a socket, so every test
/// below is hermetic: the injection-safety guarantee is structural (the real
/// runner feeds `brief.task_description` to `Command::args`, never a shell).
struct FakeLauncher {
    launched: Mutex<Vec<RecipeBrief>>,
    handle_id: String,
    /// When `Some`, `launch` fails with this error (drives the 500 test).
    fail_launch: Option<OverseerError>,
    /// Result returned by `poll` (drives the status-route tests).
    poll: Result<WorkstreamStatus, OverseerError>,
}

impl FakeLauncher {
    fn accepting() -> Self {
        Self {
            launched: Mutex::new(Vec::new()),
            handle_id: "ws-42".to_string(),
            fail_launch: None,
            poll: Ok(WorkstreamStatus::Running),
        }
    }

    fn launched(&self) -> Vec<RecipeBrief> {
        self.launched.lock().unwrap().clone()
    }

    fn launched_count(&self) -> usize {
        self.launched.lock().unwrap().len()
    }
}

impl RecipeLauncher for FakeLauncher {
    fn launch(&self, brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
        if let Some(e) = &self.fail_launch {
            return Err(e.clone());
        }
        self.launched.lock().unwrap().push(brief.clone());
        Ok(WorkstreamHandle {
            id: self.handle_id.clone(),
        })
    }

    fn poll(&self, _handle: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
        self.poll.clone()
    }
}

// ─────────────────────────── request builders ──────────────────────────────

fn valid_body() -> Value {
    body_with(
        "bug",
        "Costs panel shows stale total",
        "The daily total did not refresh after midnight.",
    )
}

fn body_with(kind: &str, title: &str, description: &str) -> Value {
    json!({
        "report": { "type": kind, "title": title, "description": description },
        "context": {
            "page": "costs",
            "state": "{\"daily_total_usd\": 12.5}",
            "timestamp": "2026-07-06T03:00:00Z",
            "identifiers": { "hash": "#costs", "active_goal_id": "reduce-spend" }
        }
    })
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

// ─────────────────────────── happy path ────────────────────────────────────

#[test]
fn handle_feedback_launches_workstream_with_composed_task_description() {
    let fake = FakeLauncher::accepting();
    let dedup = FeedbackDedup::new();

    let (status, resp) = handle_feedback(&fake, &dedup, Instant::now(), valid_body());

    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "a valid report must be accepted (202)"
    );
    assert_eq!(resp.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        str_field(&resp, "workstream_id"),
        "ws-42",
        "the launched workstream id must be surfaced so the UI can poll it"
    );
    assert_eq!(str_field(&resp, "state"), "started");
    assert!(
        str_field(&resp, "poll").contains("ws-42"),
        "response must include a poll URL carrying the workstream id, got {resp}"
    );

    let launched = fake.launched();
    assert_eq!(launched.len(), 1, "exactly one workstream must be launched");
    let brief = &launched[0];
    assert_eq!(
        brief.target_repo,
        TargetRepo::Simard.slug(),
        "feedback about the Simard dashboard must target Simard's own repo"
    );
    assert_eq!(
        brief.sequence_group, None,
        "an operator report is not part of a mechanical sweep sequence group"
    );
    // The task_description is composed from report + captured page context.
    let td = &brief.task_description;
    assert!(
        td.contains("Costs panel shows stale total"),
        "td missing title: {td}"
    );
    assert!(
        td.contains("The daily total did not refresh after midnight."),
        "td missing description: {td}"
    );
    assert!(td.contains("costs"), "td missing captured page id: {td}");
    assert!(
        td.contains("2026-07-06T03:00:00Z"),
        "td missing captured timestamp: {td}"
    );
}

// ─────────────────────────── compose_task_description ───────────────────────

fn sample_report(kind: FeedbackKind) -> FeedbackReport {
    FeedbackReport {
        kind,
        title: "Widget overlaps clock".to_string(),
        description: "The feedback button covers the header clock on narrow screens.".to_string(),
    }
}

fn sample_context() -> FeedbackContext {
    FeedbackContext {
        page: "overview".to_string(),
        state: "{\"cycle\": 1680}".to_string(),
        timestamp: "2026-07-06T03:09:41Z".to_string(),
        identifiers: json!({ "hash": "#overview", "active_goal_id": "polish-ui" }),
    }
}

#[test]
fn compose_task_description_includes_report_and_context() {
    let report = sample_report(FeedbackKind::Bug);
    let context = sample_context();

    let td = compose_task_description(&report, &context);

    assert!(td.contains("Widget overlaps clock"), "missing title: {td}");
    assert!(
        td.contains("The feedback button covers the header clock on narrow screens."),
        "missing description: {td}"
    );
    assert!(td.contains("overview"), "missing captured page: {td}");
    assert!(
        td.contains("2026-07-06T03:09:41Z"),
        "missing timestamp: {td}"
    );
    assert!(
        td.contains("{\"cycle\": 1680}") || td.contains("cycle"),
        "missing captured page state: {td}"
    );

    // Deterministic: same inputs → identical task_description.
    let again = compose_task_description(&sample_report(FeedbackKind::Bug), &sample_context());
    assert_eq!(
        td, again,
        "compose_task_description must be pure/deterministic"
    );
}

#[test]
fn compose_task_description_distinguishes_bug_from_feature() {
    let bug = compose_task_description(&sample_report(FeedbackKind::Bug), &sample_context());
    let feature =
        compose_task_description(&sample_report(FeedbackKind::Feature), &sample_context());

    assert!(
        bug.contains("[BUG]"),
        "bug reports must carry a [BUG] marker, got: {bug}"
    );
    assert!(
        feature.contains("[FEATURE]"),
        "feature requests must carry a [FEATURE] marker, got: {feature}"
    );
    assert_ne!(
        bug, feature,
        "the report type must change the composed task"
    );
}

// ─────────────────────────── validation matrix ─────────────────────────────

/// Assert `body` is rejected with 400 and that NO workstream is launched.
fn assert_rejected(body: Value, why: &str) -> Value {
    let fake = FakeLauncher::accepting();
    let dedup = FeedbackDedup::new();
    let (status, resp) = handle_feedback(&fake, &dedup, Instant::now(), body);
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{why}: expected 400, got {resp}"
    );
    assert_eq!(
        resp.get("ok").and_then(Value::as_bool),
        Some(false),
        "{why}: rejection body must set ok=false"
    );
    assert_eq!(
        fake.launched_count(),
        0,
        "{why}: a rejected report must NEVER launch a workstream"
    );
    resp
}

#[test]
fn rejects_unknown_report_type() {
    assert_rejected(
        body_with("spam", "t", "d"),
        "type must be exactly bug|feature",
    );
}

#[test]
fn rejects_missing_report_type() {
    let mut body = valid_body();
    body["report"]["type"] = Value::Null;
    assert_rejected(body, "missing type");
}

#[test]
fn rejects_empty_title() {
    assert_rejected(body_with("bug", "   ", "a real description"), "empty title");
}

#[test]
fn rejects_overlong_title() {
    let long = "T".repeat(MAX_TITLE_LEN + 1);
    assert_rejected(body_with("feature", &long, "desc"), "title over cap");
}

#[test]
fn rejects_empty_description() {
    assert_rejected(body_with("bug", "a title", "   "), "empty description");
}

#[test]
fn rejects_overlong_description() {
    let long = "D".repeat(MAX_DESCRIPTION_LEN + 1);
    assert_rejected(body_with("bug", "a title", &long), "description over cap");
}

// ─────────────────────────── sanitization ──────────────────────────────────

#[test]
fn truncates_oversized_context_state_but_still_launches() {
    // A malicious/huge page state must not blow up the task_description.
    let huge_state = "x".repeat(MAX_STATE_LEN + 5_000);
    let body = json!({
        "report": { "type": "bug", "title": "Overview panel wedged", "description": "Panel not refreshing" },
        "context": {
            "page": "overview",
            "state": huge_state,
            "timestamp": "2026-07-06T03:00:00Z",
            "identifiers": {}
        }
    });

    let fake = FakeLauncher::accepting();
    let dedup = FeedbackDedup::new();
    let (status, _resp) = handle_feedback(&fake, &dedup, Instant::now(), body);

    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "oversized state is truncated, not rejected"
    );
    let launched = fake.launched();
    assert_eq!(launched.len(), 1);
    // Only the state contributes 'x' chars; it must be bounded by MAX_STATE_LEN.
    let x_count = launched[0].task_description.matches('x').count();
    assert!(
        x_count <= MAX_STATE_LEN,
        "captured state must be truncated to <= {MAX_STATE_LEN} chars, got {x_count}"
    );
}

#[test]
fn injection_payload_is_carried_as_inert_data() {
    // A shell-injection attempt in free text must be carried verbatim as data.
    // Structural guarantee: brief.task_description flows to Command::args (a
    // Vec<String>), never `sh -c`, so this string can never be executed.
    let payload = "; rm -rf / # $(reboot) `whoami`";
    let body = body_with("bug", "malicious title", payload);

    let fake = FakeLauncher::accepting();
    let dedup = FeedbackDedup::new();
    let (status, _resp) = handle_feedback(&fake, &dedup, Instant::now(), body);

    assert_eq!(status, StatusCode::ACCEPTED);
    let launched = fake.launched();
    assert_eq!(launched.len(), 1);
    assert!(
        launched[0].task_description.contains(payload),
        "the payload must be carried verbatim as data (proving it is not shell-interpreted)"
    );
}

// ─────────────────────────── de-dupe + throttle ────────────────────────────

#[test]
fn duplicate_report_within_window_is_rate_limited() {
    let fake = FakeLauncher::accepting();
    let dedup = FeedbackDedup::new();
    let t0 = Instant::now();

    let (s1, _) = handle_feedback(&fake, &dedup, t0, valid_body());
    assert_eq!(s1, StatusCode::ACCEPTED, "first submission is accepted");

    let (s2, r2) = handle_feedback(&fake, &dedup, t0, valid_body());
    assert_eq!(
        s2,
        StatusCode::TOO_MANY_REQUESTS,
        "an identical resubmission inside the {DEDUP_WINDOW_SECS}s window is a duplicate"
    );
    assert_eq!(r2.get("ok").and_then(Value::as_bool), Some(false));
    assert_eq!(str_field(&r2, "error"), "duplicate");

    // After the window elapses, the same report is allowed again.
    let (s3, _) = handle_feedback(
        &fake,
        &dedup,
        t0 + Duration::from_secs(DEDUP_WINDOW_SECS + 1),
        valid_body(),
    );
    assert_eq!(s3, StatusCode::ACCEPTED, "the dedup window must expire");

    assert_eq!(
        fake.launched_count(),
        2,
        "only the two non-duplicate submissions launch a workstream"
    );
}

#[test]
fn distinct_reports_are_throttled_after_launch_cap() {
    // De-dup alone won't stop a flood of DISTINCT reports; a per-window launch
    // cap protects against subprocess cost-DoS.
    let fake = FakeLauncher::accepting();
    let dedup = FeedbackDedup::new();
    let t0 = Instant::now();

    for i in 0..MAX_FEEDBACK_LAUNCHES_PER_WINDOW {
        let (status, _) = handle_feedback(
            &fake,
            &dedup,
            t0,
            body_with("bug", &format!("distinct {i}"), "d"),
        );
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "submission {i} within cap must succeed"
        );
    }

    let (status, resp) = handle_feedback(&fake, &dedup, t0, body_with("bug", "one too many", "d"));
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the {}-th distinct launch in the window must be throttled",
        MAX_FEEDBACK_LAUNCHES_PER_WINDOW + 1
    );
    assert_eq!(resp.get("ok").and_then(Value::as_bool), Some(false));
    assert_eq!(
        fake.launched_count(),
        MAX_FEEDBACK_LAUNCHES_PER_WINDOW,
        "no more than {MAX_FEEDBACK_LAUNCHES_PER_WINDOW} launches per window"
    );
}

// ─────────────────────────── status route ──────────────────────────────────

#[test]
fn status_json_maps_running_pr_and_failed() {
    let running = status_json(&WorkstreamStatus::Running);
    assert_eq!(str_field(&running, "state"), "running");

    let pr = status_json(&WorkstreamStatus::ProducedPr {
        repo: "rysweet/Simard".to_string(),
        pr: 2601,
    });
    assert_eq!(str_field(&pr, "state"), "pr");
    assert_eq!(
        str_field(&pr, "pr_url"),
        "https://github.com/rysweet/Simard/pull/2601",
        "a produced PR must surface a clickable github PR URL"
    );

    let failed = status_json(&WorkstreamStatus::Failed {
        reason: "recipe finished but produced no PR".to_string(),
    });
    assert_eq!(str_field(&failed, "state"), "failed");
}

#[test]
fn feedback_status_returns_pr_for_known_workstream() {
    let mut fake = FakeLauncher::accepting();
    fake.poll = Ok(WorkstreamStatus::ProducedPr {
        repo: "rysweet/Simard".to_string(),
        pr: 2601,
    });

    let (status, resp) = handle_feedback_status(&fake, "ws-42".to_string());
    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&resp, "state"), "pr");
    assert_eq!(
        str_field(&resp, "pr_url"),
        "https://github.com/rysweet/Simard/pull/2601"
    );
}

#[test]
fn feedback_status_unknown_workstream_is_404_without_leaking_detail() {
    let internal = "unknown workstream ws-x at /home/azureuser/.simard/secret";
    let mut fake = FakeLauncher::accepting();
    fake.poll = Err(OverseerError::Capability {
        what: "recipe.probe",
        detail: internal.to_string(),
    });

    let (status, resp) = handle_feedback_status(&fake, "ws-x".to_string());
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "polling an unknown workstream id must be 404"
    );
    assert_eq!(resp.get("ok").and_then(Value::as_bool), Some(false));
    assert!(
        !resp.to_string().contains("/home/azureuser/.simard/secret"),
        "the status error must not leak internal paths/details: {resp}"
    );
}

// ─────────────────────────── error handling ────────────────────────────────

#[test]
fn launcher_error_maps_to_generic_500_without_leaking_detail() {
    let internal = "/home/azureuser/.simard/secret spawn amplihack: No such file";
    let fake = FakeLauncher {
        launched: Mutex::new(Vec::new()),
        handle_id: "ws-42".to_string(),
        fail_launch: Some(OverseerError::Capability {
            what: "recipe.spawn",
            detail: internal.to_string(),
        }),
        poll: Ok(WorkstreamStatus::Running),
    };
    let dedup = FeedbackDedup::new();

    let (status, resp) = handle_feedback(&fake, &dedup, Instant::now(), valid_body());
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a launcher failure must be a 500"
    );
    assert_eq!(resp.get("ok").and_then(Value::as_bool), Some(false));
    assert!(
        !resp.to_string().contains("/home/azureuser/.simard/secret"),
        "the 500 body must not leak internal paths/details: {resp}"
    );
}
