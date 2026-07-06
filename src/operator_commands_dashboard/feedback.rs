//! Backend for the dashboard "Report bug / Request feature" widget (#2629).
//!
//! The widget on every dashboard tab POSTs `{report, context}` here. The
//! handler validates + sanitizes the operator's report, composes a
//! `task_description` from the report plus the captured page context, and
//! launches a NEW dev-orchestrator workstream by REUSING the existing
//! [`RecipeLauncher`] plumbing
//! ([`SmartOrchestratorLauncher`](crate::overseer::launch::SmartOrchestratorLauncher),
//! which runs `smart-orchestrator` → default-workflow exactly as engineers do).
//! No ad-hoc shell-out: the brief flows through the launcher, whose real runner
//! feeds `task_description` to `Command::args` (a `Vec<String>`), never a shell —
//! so free-text is structurally inert as data.
//!
//! The pure core (`compose_task_description`, `handle_feedback`,
//! `handle_feedback_status`, `status_json`) takes an injectable
//! `&dyn RecipeLauncher` and returns `(StatusCode, Value)`, so the whole flow is
//! unit-testable with an in-memory fake (see `tests_feedback`). The thin axum
//! handlers wrap the `Value` in `Json` and share one stateful launcher/dedup so
//! `poll` can find the run it spawned. Both routes register BEFORE the existing
//! `require_auth` layer, inheriting the dashboard access-code gate.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, response::Response};
use serde_json::{Value, json};

use crate::overseer::capabilities::{
    RecipeBrief, RecipeLauncher, WorkstreamHandle, WorkstreamStatus,
};
use crate::overseer::launch::SmartOrchestratorLauncher;
use crate::stewardship::TargetRepo;

// ─────────────────────────── limits & windows ──────────────────────────────

/// Max characters accepted for a report title (rejected over cap).
pub(crate) const MAX_TITLE_LEN: usize = 200;
/// Max characters accepted for a report description (rejected over cap).
pub(crate) const MAX_DESCRIPTION_LEN: usize = 5_000;
/// Max characters of captured page state carried into the task (truncated, not
/// rejected — a huge/hostile page must never blow up the task_description).
pub(crate) const MAX_STATE_LEN: usize = 16_384;
/// Max serialized characters of the captured `identifiers` object (truncated).
pub(crate) const MAX_IDENTIFIERS_LEN: usize = 4_096;
/// An identical report resubmitted inside this window is a duplicate.
pub(crate) const DEDUP_WINDOW_SECS: u64 = 30;
/// No more than this many distinct workstreams may launch per dedup window
/// (subprocess cost-DoS guard).
pub(crate) const MAX_FEEDBACK_LAUNCHES_PER_WINDOW: usize = 5;

// ─────────────────────────── report / context ──────────────────────────────

/// The kind of operator report the widget submits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FeedbackKind {
    Bug,
    Feature,
}

impl FeedbackKind {
    /// The uppercase marker embedded in the composed task_description.
    fn marker(self) -> &'static str {
        match self {
            FeedbackKind::Bug => "[BUG]",
            FeedbackKind::Feature => "[FEATURE]",
        }
    }

    /// Parse the wire value, accepting exactly `bug` | `feature`.
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "bug" => Some(FeedbackKind::Bug),
            "feature" => Some(FeedbackKind::Feature),
            _ => None,
        }
    }
}

/// A validated, sanitized operator report.
pub(crate) struct FeedbackReport {
    pub(crate) kind: FeedbackKind,
    pub(crate) title: String,
    pub(crate) description: String,
}

/// The page context captured client-side when the report was filed.
pub(crate) struct FeedbackContext {
    pub(crate) page: String,
    pub(crate) state: String,
    pub(crate) timestamp: String,
    pub(crate) identifiers: Value,
}

// ─────────────────────────── de-dupe / throttle ────────────────────────────

#[derive(Default)]
struct DedupState {
    /// `report-hash → time accepted`, for the duplicate-window check.
    recent: HashMap<u64, Instant>,
    /// Timestamps of accepted launches, for the per-window launch cap.
    launches: Vec<Instant>,
}

/// De-dupe + rate-limit state for the feedback endpoint. Interior-mutable so a
/// shared `&FeedbackDedup` (held in a process-wide `OnceLock` in production) can
/// record accepted launches without a `&mut`.
pub(crate) struct FeedbackDedup {
    inner: Mutex<DedupState>,
}

impl FeedbackDedup {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(DedupState::default()),
        }
    }
}

/// Verdict of the dedup/throttle gate, evaluated before a launch.
enum Gate {
    Ok,
    Duplicate,
    RateLimited,
}

impl FeedbackDedup {
    /// Purge window-expired entries, then decide whether `hash` may launch at
    /// `now`. Pure decision — recording happens only after a successful launch.
    fn evaluate(&self, hash: u64, now: Instant) -> Gate {
        let window = Duration::from_secs(DEDUP_WINDOW_SECS);
        let mut st = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        st.recent
            .retain(|_, &mut t| now.saturating_duration_since(t) < window);
        st.launches
            .retain(|&t| now.saturating_duration_since(t) < window);

        if st.recent.contains_key(&hash) {
            Gate::Duplicate
        } else if st.launches.len() >= MAX_FEEDBACK_LAUNCHES_PER_WINDOW {
            Gate::RateLimited
        } else {
            Gate::Ok
        }
    }

    /// Record an accepted launch so later duplicates/floods are gated.
    fn record(&self, hash: u64, now: Instant) {
        let mut st = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        st.recent.insert(hash, now);
        st.launches.push(now);
    }
}

// ─────────────────────────── sanitization ──────────────────────────────────

/// Strip ASCII control characters. `keep_newlines` retains `\n`/`\t` for
/// multi-line free text (description, page state); titles strip everything.
fn strip_control(s: &str, keep_newlines: bool) -> String {
    s.chars()
        .filter(|c| !c.is_control() || (keep_newlines && (*c == '\n' || *c == '\t')))
        .collect()
}

/// Truncate to at most `max` Unicode chars, appending `…` when shortened.
fn truncate_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Stable hash of the report identity for de-dupe.
fn report_hash(kind: FeedbackKind, title: &str, description: &str) -> u64 {
    let mut h = DefaultHasher::new();
    kind.marker().hash(&mut h);
    title.hash(&mut h);
    description.hash(&mut h);
    h.finish()
}

// ─────────────────────────── parsing / validation ──────────────────────────

/// Parse+validate+sanitize the request body into a `(report, context)` pair, or
/// a human-safe rejection reason (→ 400). Never trusts sizes: the page state
/// and identifiers are truncated rather than rejected.
fn parse_body(body: &Value) -> Result<(FeedbackReport, FeedbackContext), &'static str> {
    let report = body.get("report").ok_or("invalid report")?;
    let context = body.get("context").cloned().unwrap_or_else(|| json!({}));

    let kind_raw = report
        .get("type")
        .and_then(Value::as_str)
        .ok_or("invalid type")?;
    let kind = FeedbackKind::parse(kind_raw).ok_or("invalid type")?;

    let title = strip_control(
        report.get("title").and_then(Value::as_str).unwrap_or(""),
        false,
    );
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title required");
    }
    if title.chars().count() > MAX_TITLE_LEN {
        return Err("title too long");
    }

    let description = strip_control(
        report
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(""),
        true,
    );
    if description.trim().is_empty() {
        return Err("description required");
    }
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        return Err("description too long");
    }

    let page = truncate_chars(
        strip_control(
            context.get("page").and_then(Value::as_str).unwrap_or(""),
            false,
        )
        .trim(),
        MAX_TITLE_LEN,
    );
    let state = truncate_chars(
        &strip_control(
            context.get("state").and_then(Value::as_str).unwrap_or(""),
            true,
        ),
        MAX_STATE_LEN,
    );
    let timestamp = truncate_chars(
        strip_control(
            context
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or(""),
            false,
        )
        .trim(),
        MAX_TITLE_LEN,
    );
    let identifiers = bound_identifiers(context.get("identifiers"));

    Ok((
        FeedbackReport {
            kind,
            title,
            description,
        },
        FeedbackContext {
            page,
            state,
            timestamp,
            identifiers,
        },
    ))
}

/// Keep the captured `identifiers` object but bound its serialized size so a
/// hostile payload can't bloat the task. Non-objects degrade to `{}`.
fn bound_identifiers(v: Option<&Value>) -> Value {
    match v {
        Some(Value::Object(_)) => {
            let serialized = v.map(|x| x.to_string()).unwrap_or_default();
            if serialized.chars().count() <= MAX_IDENTIFIERS_LEN {
                v.cloned().unwrap_or_else(|| json!({}))
            } else {
                json!({ "truncated": truncate_chars(&serialized, MAX_IDENTIFIERS_LEN) })
            }
        }
        _ => json!({}),
    }
}

// ─────────────────────────── task composition ──────────────────────────────

/// Compose the workstream `task_description` from the report + captured page
/// context. Pure and deterministic: identical inputs yield identical output.
/// The result is carried as data through `Command::args`, never a shell.
pub(crate) fn compose_task_description(
    report: &FeedbackReport,
    context: &FeedbackContext,
) -> String {
    let identifiers = if context.identifiers.is_null() {
        "{}".to_string()
    } else {
        context.identifiers.to_string()
    };
    format!(
        "{marker} {title}\n\
         \n\
         {description}\n\
         \n\
         --- Operator feedback (untrusted input; captured from the dashboard) ---\n\
         Page/tab: {page}\n\
         Timestamp: {timestamp}\n\
         Identifiers: {identifiers}\n\
         Page state:\n{state}\n\
         \n\
         Filed from the Simard dashboard feedback widget (issue #2629). Please \
         triage this operator report and, if actionable, address the bug or \
         build the requested feature following the default development workflow.",
        marker = report.kind.marker(),
        title = report.title,
        description = report.description,
        page = context.page,
        timestamp = context.timestamp,
        identifiers = identifiers,
        state = context.state,
    )
}

// ─────────────────────────── pure core handlers ────────────────────────────

/// Handle a feedback submission against an injectable launcher + dedup state.
///
/// * 400 — invalid/oversize report (never launches).
/// * 429 — duplicate within the dedup window, or per-window launch cap hit.
/// * 500 — the launcher failed (detail is NOT leaked to the client).
/// * 202 — launched; body carries `workstream_id` + a `poll` URL.
pub(crate) fn handle_feedback(
    launcher: &dyn RecipeLauncher,
    dedup: &FeedbackDedup,
    now: Instant,
    body: Value,
) -> (StatusCode, Value) {
    let (report, context) = match parse_body(&body) {
        Ok(pair) => pair,
        Err(reason) => {
            return (
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": reason }),
            );
        }
    };

    let hash = report_hash(report.kind, &report.title, &report.description);
    match dedup.evaluate(hash, now) {
        Gate::Duplicate => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "ok": false, "error": "duplicate" }),
            );
        }
        Gate::RateLimited => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "ok": false, "error": "busy" }),
            );
        }
        Gate::Ok => {}
    }

    let brief = RecipeBrief {
        task_description: compose_task_description(&report, &context),
        target_repo: TargetRepo::Simard.slug().to_string(),
        sequence_group: None,
    };

    match launcher.launch(&brief) {
        Ok(handle) => {
            dedup.record(hash, now);
            let poll = format!("/api/feedback/status/{}", handle.id);
            (
                StatusCode::ACCEPTED,
                json!({
                    "ok": true,
                    "state": "started",
                    "workstream_id": handle.id,
                    "poll": poll,
                }),
            )
        }
        Err(err) => {
            tracing::warn!(target: "dashboard.feedback", error = %err, "feedback workstream launch failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "ok": false, "error": "failed to start workstream" }),
            )
        }
    }
}

/// Poll a launched feedback workstream by id. Unknown ids / probe failures map
/// to 404 without leaking internal detail; a live status maps via
/// [`status_json`].
pub(crate) fn handle_feedback_status(
    launcher: &dyn RecipeLauncher,
    id: String,
) -> (StatusCode, Value) {
    let handle = WorkstreamHandle { id: id.clone() };
    match launcher.poll(&handle) {
        Ok(status) => {
            let mut body = status_json(&status);
            if let Some(obj) = body.as_object_mut() {
                obj.insert("workstream_id".to_string(), json!(id));
            }
            (StatusCode::OK, body)
        }
        Err(err) => {
            tracing::warn!(target: "dashboard.feedback", error = %err, "feedback workstream poll failed");
            (
                StatusCode::NOT_FOUND,
                json!({ "ok": false, "error": "unknown workstream" }),
            )
        }
    }
}

/// Map a [`WorkstreamStatus`] to a UI-facing JSON body. A produced PR surfaces a
/// clickable `github.com/<repo>/pull/<n>` URL so the widget can link it.
pub(crate) fn status_json(status: &WorkstreamStatus) -> Value {
    match status {
        WorkstreamStatus::Running => json!({ "ok": true, "state": "running" }),
        WorkstreamStatus::ProducedPr { repo, pr } => json!({
            "ok": true,
            "state": "pr",
            "repo": repo,
            "pr": pr,
            "pr_url": format!("https://github.com/{repo}/pull/{pr}"),
        }),
        WorkstreamStatus::Failed { reason } => json!({
            "ok": true,
            "state": "failed",
            "reason": reason,
        }),
    }
}

// ─────────────────────────── production plumbing ───────────────────────────

/// The process-wide launcher. A single stateful instance so a workstream
/// spawned by `POST /api/feedback` can be found by `GET /api/feedback/status/…`.
fn launcher() -> &'static dyn RecipeLauncher {
    static LAUNCHER: OnceLock<SmartOrchestratorLauncher> = OnceLock::new();
    LAUNCHER.get_or_init(SmartOrchestratorLauncher::from_env)
}

/// The process-wide dedup/throttle state.
fn dedup() -> &'static FeedbackDedup {
    static DEDUP: OnceLock<FeedbackDedup> = OnceLock::new();
    DEDUP.get_or_init(FeedbackDedup::new)
}

/// `POST /api/feedback` — receive `{report, context}`, launch a workstream.
/// Behind the dashboard `require_auth` layer (registered in `routes.rs`).
pub(crate) async fn feedback_submit(body: Option<Json<Value>>) -> Response {
    let Json(body) = body.unwrap_or_else(|| Json(json!({})));
    let (status, value) = handle_feedback(launcher(), dedup(), Instant::now(), body);
    (status, Json(value)).into_response()
}

/// `GET /api/feedback/status/{id}` — poll a launched workstream. Behind auth.
pub(crate) async fn feedback_status(Path(id): Path<String>) -> Response {
    let (status, value) = handle_feedback_status(launcher(), id);
    (status, Json(value)).into_response()
}
