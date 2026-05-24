//! LLM-backed brain — submits the rendered prompt via a `LlmSubmitter`
//! abstraction. Production wires the real RustyClawd session; tests wire a
//! canned-response stub.

use super::prompt_store;
use super::{EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain};
use crate::base_types::BaseTypeTurnInput;
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::session_builder::{LlmProvider, SessionBuilder};

/// Embedded prompt — compile-time fallback. The runtime brain reads from
/// disk via [`prompt_store::global`] so prompt edits take effect on the
/// next OODA cycle without restarting the daemon (PR #1474 follow-up).
/// This constant is retained as documentation of the embedded baseline; the
/// authoritative copy lives in [`prompt_store::embedded_fallback`].
pub const PROMPT_NAME: &str = "ooda_brain.md";

const ADAPTER_TAG: &str = "ooda-brain";

/// Thin seam over whatever subprocess/HTTP path the rustyclawd adapter uses.
/// Production wires the real adapter via `RustyClawdSessionSubmitter`; tests
/// wire a canned-response stub without touching production wiring.
pub trait LlmSubmitter: Send + Sync {
    fn submit(&self, rendered_prompt: &str) -> SimardResult<String>;
}

/// LLM-backed brain. Construct via `build_rustyclawd_brain` in production so
/// callers do not need to know the adapter type. The submitter is generic so
/// tests can swap in a stub.
pub struct RustyClawdBrain<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> RustyClawdBrain<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }

    /// Render the prompt with the context. Loads the prompt fresh per call
    /// via [`prompt_store::global`] so on-disk edits take effect on the
    /// next OODA cycle. Falls back to the embedded baseline when no file
    /// exists at the resolved path.
    pub fn render_prompt(&self, ctx: &EngineerLifecycleCtx) -> String {
        let sentinel = ctx
            .sentinel_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "<none>".to_string());
        prompt_store::global()
            .load(PROMPT_NAME)
            .replace("{goal_id}", &ctx.goal_id)
            .replace("{goal_description}", &ctx.goal_description)
            .replace("{cycle_number}", &ctx.cycle_number.to_string())
            .replace(
                "{consecutive_skip_count}",
                &ctx.consecutive_skip_count.to_string(),
            )
            .replace("{failure_count}", &ctx.failure_count.to_string())
            .replace("{worktree_path}", &ctx.worktree_path.display().to_string())
            .replace(
                "{worktree_mtime_secs_ago}",
                &ctx.worktree_mtime_secs_ago.to_string(),
            )
            .replace("{sentinel_pid}", &sentinel)
            .replace("{last_engineer_log_tail}", &ctx.last_engineer_log_tail)
            .replace("{commits_behind}", &ctx.commits_behind.to_string())
            .replace(
                "{in_flight_engineer_count}",
                &ctx.in_flight_engineer_count.to_string(),
            )
            .replace(
                "{minutes_since_last_update_attempt}",
                &if ctx.minutes_since_last_update_attempt == u64::MAX {
                    "never".to_string()
                } else {
                    ctx.minutes_since_last_update_attempt.to_string()
                },
            )
    }
}

impl<S: LlmSubmitter> OodaBrain for RustyClawdBrain<S> {
    fn decide_engineer_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision> {
        let prompt = self.render_prompt(ctx);
        let raw = self.submitter.submit(&prompt)?;
        parse_decision_from_response(&raw).map_err(|reason| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason,
        })
    }
}

/// Closed set of `EngineerLifecycleDecision` variant tags. Kept in sync
/// with the `#[serde(tag = "choice", rename_all = "snake_case")]` enum in
/// `mod.rs`. Used by the prose-first DECISION marker parser to validate
/// the variant token before attempting deserialisation.
const VALID_VARIANTS: &[&str] = &[
    "continue_skipping",
    "reclaim_and_redispatch",
    "deprioritize",
    "open_tracking_issue",
    "mark_goal_blocked",
    "consider_self_update",
];

/// Cap on raw response text embedded in error messages so a runaway model
/// output cannot flood `~/.simard/logs`. 8 KiB matches the limit used by
/// `truncate_for_log` in `ooda_actions/advance_goal/spawn.rs`.
const MAX_RAW_LOG_BYTES: usize = 8 * 1024;

/// Truncate a string to at most `MAX_RAW_LOG_BYTES` bytes for log inclusion.
/// UTF-8 safe: walks `char_indices` so no slice ever splits a codepoint.
/// Appends a marker noting the original size so operators see the response
/// was truncated rather than the model emitting a short response.
fn truncate_for_log(s: &str) -> String {
    if s.len() <= MAX_RAW_LOG_BYTES {
        return s.to_string();
    }
    // Find the largest char boundary <= MAX_RAW_LOG_BYTES so the slice is
    // valid UTF-8. `char_indices` yields each char's start byte index.
    let cutoff = s
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= MAX_RAW_LOG_BYTES)
        .last()
        .unwrap_or(0);
    format!("{}... (truncated from {} bytes)", &s[..cutoff], s.len())
}

/// Sibling-module-visible re-export of [`truncate_for_log`] so the parser
/// helpers in `decide.rs` and `orient.rs` (which share the same lossy
/// `got N bytes` anti-pattern fixed in #1711) can reuse the same
/// truncation policy without duplicating the constant or the
/// UTF-8-safe slicing logic.
pub(super) fn truncate_for_log_pub(s: &str) -> String {
    truncate_for_log(s)
}

/// Extract the prose `DECISION: <variant>` marker from the first non-blank
/// line of `text`. Returns `(variant_token, remainder_after_first_line)`.
///
/// Security: only the **first non-blank line** is inspected. A `DECISION:`
/// token appearing later in the response is ignored — this prevents a
/// malfunctioning or hostile model from injecting a marker mid-response.
///
/// The word `DECISION` itself is matched case-insensitively. The variant
/// token is returned verbatim (whitespace-trimmed) so callers can do the
/// exact-match snake_case validation against [`VALID_VARIANTS`].
fn extract_decision_marker(text: &str) -> Option<(&str, &str)> {
    let first_line = text.lines().find(|l| !l.trim().is_empty())?;
    let trimmed = first_line.trim();
    // Case-insensitive prefix check on "decision:". We compare the lowercased
    // first 9 bytes (the length of "decision:") rather than allocating a full
    // lowercase copy of the whole line.
    if trimmed.len() < "decision:".len() {
        return None;
    }
    let prefix = &trimmed[.."decision:".len()];
    if !prefix.eq_ignore_ascii_case("decision:") {
        return None;
    }
    let after_marker = trimmed["decision:".len()..].trim();
    let variant = after_marker.split_whitespace().next()?;
    // Remainder = everything after the first newline of the original text
    // (preserves the rest of the response verbatim, including subsequent
    // blank lines and any JSON body).
    let remainder = text.split_once('\n').map(|(_, r)| r).unwrap_or("");
    Some((variant, remainder))
}

/// Parse a brain response that begins with a `DECISION:` marker. The variant
/// from the marker is the authoritative source of truth — if the optional
/// JSON body contains a conflicting `choice` field, the marker wins
/// (security: prevents variant smuggling via JSON body).
///
/// For variants requiring extra fields (`open_tracking_issue`,
/// `mark_goal_blocked`, `reclaim_and_redispatch`) the body MUST contain a
/// JSON object with those fields. For marker-only variants
/// (`continue_skipping`, `deprioritize`, `consider_self_update`) the
/// remainder of the response (after stripping an optional `rationale:`
/// prefix) becomes the `rationale` field.
fn parse_with_marker(
    variant: &str,
    rest: &str,
    raw: &str,
) -> Result<EngineerLifecycleDecision, String> {
    if !VALID_VARIANTS.contains(&variant) {
        return Err(format!(
            "brain DECISION marker variant `{variant}` is not one of {VALID_VARIANTS:?}; \
             raw_response={:?}",
            truncate_for_log(raw)
        ));
    }

    // Parse labeled body lines and prose from the remainder (issue #1980).
    // Replaces JSON body parsing — structured variants now use labeled lines
    // (TITLE:, BODY:, REASON:, REDISPATCH_CONTEXT:) instead of JSON objects.
    let trimmed_rest = rest.trim();

    // Extract labeled fields from the body
    let mut fields: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut prose_lines: Vec<&str> = Vec::new();

    for line in trimmed_rest.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Try to extract a labeled field (KEY: value)
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim();
            let val = trimmed[colon_pos + 1..].trim();
            // Only recognize known field labels (case-insensitive)
            let key_upper = key.to_ascii_uppercase();
            match key_upper.as_str() {
                "TITLE" | "BODY" | "REASON" | "REDISPATCH_CONTEXT" | "RATIONALE" => {
                    fields.insert(key_upper, val.to_string());
                    continue;
                }
                _ => {}
            }
        }
        prose_lines.push(trimmed);
    }

    // Also try to extract fields from JSON body if present (backward-compat
    // for the transition period where prompts may still emit JSON)
    if let Some(start) = trimmed_rest.find('{')
        && let Some(end) = trimmed_rest.rfind('}')
        && end >= start
    {
        let candidate = &trimmed_rest[start..=end];
        if let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(candidate)
        {
            for (k, v) in &map {
                let key_upper = k.to_ascii_uppercase();
                if let Some(s) = v.as_str() {
                    fields.entry(key_upper).or_insert_with(|| s.to_string());
                }
            }
        }
    }

    // Derive rationale: explicit RATIONALE field > prose lines > default
    let rationale = if let Some(r) = fields.get("RATIONALE") {
        r.clone()
    } else if !prose_lines.is_empty() {
        prose_lines.join(" ")
    } else {
        "(no rationale provided)".to_string()
    };

    // Build the decision based on variant
    match variant {
        "continue_skipping" => Ok(EngineerLifecycleDecision::ContinueSkipping { rationale }),
        "deprioritize" => Ok(EngineerLifecycleDecision::Deprioritize { rationale }),
        "consider_self_update" => Ok(EngineerLifecycleDecision::ConsiderSelfUpdate { rationale }),
        "reclaim_and_redispatch" => {
            let redispatch_context = fields
                .get("REDISPATCH_CONTEXT")
                .cloned()
                .unwrap_or_default();
            Ok(EngineerLifecycleDecision::ReclaimAndRedispatch {
                rationale,
                redispatch_context,
            })
        }
        "open_tracking_issue" => {
            let title = fields
                .get("TITLE")
                .cloned()
                .unwrap_or_else(|| "(no title)".to_string());
            let body = fields
                .get("BODY")
                .cloned()
                .unwrap_or_else(|| "(no body)".to_string());
            Ok(EngineerLifecycleDecision::OpenTrackingIssue {
                rationale,
                title,
                body,
            })
        }
        "mark_goal_blocked" => {
            let reason = fields
                .get("REASON")
                .cloned()
                .unwrap_or_else(|| "(no reason)".to_string());
            Ok(EngineerLifecycleDecision::MarkGoalBlocked { rationale, reason })
        }
        _ => Err(format!(
            "brain DECISION marker `{variant}` present but not handled; raw_response={:?}",
            truncate_for_log(raw)
        )),
    }
}

/// Parse the brain's response into an [`EngineerLifecycleDecision`]. Accepts
/// three input shapes (see [issue #1711](https://github.com/rysweet/Simard/issues/1711)):
///
/// 1. **Prose-first** (preferred, new in #1711): `DECISION: <variant>` on
///    the first non-blank line, optionally followed by a JSON object
///    carrying variant-specific fields, or by free-form prose used as the
///    `rationale`. Marker is case-insensitive on the word `DECISION`.
/// 2. **Pure JSON** (legacy, still supported): a `{...}` object somewhere
///    in the response — extracted with `find('{') .. rfind('}')` and
///    parsed via `serde_json::from_str`.
/// 3. **JSON wrapped in markdown fences or prose** (legacy, still
///    supported): the same extraction handles `` ```json ... ``` `` and
///    leading/trailing commentary.
///
/// On total parse failure the returned error embeds the **full raw response
/// text** (truncated to [`MAX_RAW_LOG_BYTES`] for log safety) so operators
/// can diagnose the model behaviour without hunting through transcripts.
/// This replaces the legacy `got N bytes` byte-count format that
/// originated the production `improve-amplihack-test-coverage` failure.
fn parse_decision_from_response(raw: &str) -> Result<EngineerLifecycleDecision, String> {
    let stripped = raw.trim();
    if stripped.is_empty() {
        return Err(format!(
            "brain returned an empty response (raw_response={:?})",
            raw
        ));
    }

    // DECISION marker is the only accepted format (issue #1980).
    // JSON fallback removed — it was the source of fallback storms.
    if let Some((variant, rest)) = extract_decision_marker(stripped) {
        return parse_with_marker(variant, rest, raw);
    }

    Err(format!(
        "brain response had no DECISION: marker on first line; raw_response={:?}",
        truncate_for_log(raw)
    ))
}

// ---------------------------------------------------------------------------
// Production constructor
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Production submitter — opens a fresh BaseTypeSession per submit() call.
// ---------------------------------------------------------------------------

/// Production [`LlmSubmitter`]: opens a fresh [`BaseTypeSession`] via
/// [`SessionBuilder`] for each `submit()` call, runs one turn, and returns
/// the LLM response text (`outcome.execution_summary` per the
/// `BaseTypeSession` contract — see `engineer_plan::plan_objective` for the
/// canonical example).
///
/// **Why per-call session:** the engineer-lifecycle skip branch only fires
/// when an engineer is already alive (rare). Per-call session open mirrors
/// `engineer_plan` / `review_pipeline` and avoids threading
/// `Arc<Mutex<Box<dyn BaseTypeSession>>>` through `OodaBridges`. If profiling
/// later shows session-open cost dominating, swap to a cached session
/// without changing the `LlmSubmitter` trait.
///
/// **Resilience:** session-open or `run_turn` failures propagate as
/// `SimardError::AdapterInvocationFailed`. The `dispatch_spawn_engineer`
/// caller (see `ooda_actions/advance_goal/spawn.rs`) maps any brain error
/// to `ContinueSkipping`, preserving pre-#1266 behaviour on transient
/// adapter failure. We deliberately do **not** retry inside the submitter:
/// the OODA loop already iterates once per cycle, so a retry-on-failure
/// loop here would compound latency on every skipped cycle.
pub struct SessionLlmSubmitter {
    provider: LlmProvider,
}

impl SessionLlmSubmitter {
    pub fn new(provider: LlmProvider) -> Self {
        Self { provider }
    }
}

impl LlmSubmitter for SessionLlmSubmitter {
    fn submit(&self, rendered_prompt: &str) -> SimardResult<String> {
        let mut session = SessionBuilder::new(OperatingMode::Orchestrator, self.provider)
            .node_id("ooda-brain")
            .address("ooda-brain://local")
            .adapter_tag(ADAPTER_TAG)
            .open()
            .map_err(|reason| SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!("session open failed: {reason}"),
            })?;

        let outcome = session
            .run_turn(BaseTypeTurnInput::objective_only(
                rendered_prompt.to_string(),
            ))
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!("run_turn failed: {e}"),
            });

        // Best-effort close — never mask the run_turn error if close also
        // fails. Closing on the error path mirrors `engineer_plan`.
        let _ = session.close();

        outcome.map(|o| o.execution_summary)
    }
}

/// Production constructor. Resolves the configured [`LlmProvider`] and
/// returns a [`RustyClawdBrain`] wired to a [`SessionLlmSubmitter`].
///
/// Returns `Err` if no LLM provider is configured (no env var, no
/// `~/.simard/config.toml`, no API credentials). Callers — currently
/// `operator_commands_ooda::daemon` — must fall back to
/// `DeterministicFallbackBrain` so the daemon behaves identically to the
/// pre-#1266 daemon when LLM access is unavailable.
///
/// Provider resolution does **not** open a session here: the
/// `SessionLlmSubmitter` opens one per `submit()` call (see its docstring
/// for rationale). This keeps daemon startup fast and avoids holding an
/// LLM connection open for the rare engineer-lifecycle decision path.
pub fn build_rustyclawd_brain() -> SimardResult<Box<dyn OodaBrain>> {
    let provider = LlmProvider::resolve()?;
    let submitter = SessionLlmSubmitter::new(provider);
    Ok(Box::new(RustyClawdBrain::new(submitter)))
}

// ---------------------------------------------------------------------------
// Inline tests (issue #1979 — per-source-file coverage of the RustyClawd
// bridge: the prose-first marker parser, the legacy JSON-object salvage,
// the UTF-8-safe log truncation, AND the bridge's end-to-end behaviour for
// the four shapes the directive calls out (well-formed JSON, JSON with
// trailing prose, completely unparseable, and a per-shape end-to-end run
// through the bridge with a canned-response submitter).
//
// Sibling `tests.rs` covers higher-level dispatch; these inline tests pin
// the private parser helpers that the bridge depends on. )
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ----- helpers -------------------------------------------------------

    struct StubSubmitter {
        response: SimardResult<String>,
        calls: std::sync::Arc<std::sync::atomic::AtomicU32>,
    }

    impl StubSubmitter {
        fn ok(s: impl Into<String>) -> Self {
            Self {
                response: Ok(s.into()),
                calls: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }

        fn err() -> Self {
            Self {
                response: Err(SimardError::AdapterInvocationFailed {
                    base_type: "stub".into(),
                    reason: "stub-network-down".into(),
                }),
                calls: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }

        fn call_counter(&self) -> std::sync::Arc<std::sync::atomic::AtomicU32> {
            self.calls.clone()
        }
    }

    impl LlmSubmitter for StubSubmitter {
        fn submit(&self, _rendered_prompt: &str) -> SimardResult<String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match &self.response {
                Ok(s) => Ok(s.clone()),
                Err(e) => Err(e.clone()),
            }
        }
    }

    fn ctx() -> EngineerLifecycleCtx {
        EngineerLifecycleCtx {
            goal_id: "g1".into(),
            goal_description: "ship v1".into(),
            cycle_number: 7,
            consecutive_skip_count: 3,
            failure_count: 0,
            worktree_path: PathBuf::from("/tmp/wt"),
            worktree_mtime_secs_ago: 60,
            sentinel_pid: Some(42),
            last_engineer_log_tail: "ok".into(),
            commits_behind: 0,
            in_flight_engineer_count: 1,
            minutes_since_last_update_attempt: u64::MAX,
        }
    }

    // ----- truncate_for_log: UTF-8 safety + truncation marker ------------

    #[test]
    fn truncate_for_log_returns_input_unchanged_when_under_cap() {
        let input = "small body";
        assert_eq!(truncate_for_log(input), input);
    }

    #[test]
    fn truncate_for_log_inserts_marker_when_over_cap() {
        let big = "a".repeat(MAX_RAW_LOG_BYTES + 100);
        let out = truncate_for_log(&big);
        assert!(
            out.contains("truncated from"),
            "must emit `truncated from N bytes` marker"
        );
        assert!(out.len() < big.len(), "must shrink");
    }

    #[test]
    fn truncate_for_log_does_not_split_multibyte_codepoint() {
        // Pad with 4-byte emoji past the cap. The slice must land on a
        // char boundary — if it did not, the format! macro would panic in
        // debug builds and produce invalid UTF-8 in release.
        let prefix = "a".repeat(MAX_RAW_LOG_BYTES - 2);
        let body = format!("{prefix}🦀🦀🦀");
        let out = truncate_for_log(&body);
        // No panic = pass; also assert the marker is present.
        assert!(out.contains("truncated from"));
        assert!(
            std::str::from_utf8(out.as_bytes()).is_ok(),
            "truncated body must be valid UTF-8"
        );
    }

    #[test]
    fn truncate_for_log_pub_matches_truncate_for_log() {
        // Sibling re-export for decide.rs/orient.rs must apply the same policy.
        let big = "x".repeat(MAX_RAW_LOG_BYTES + 50);
        assert_eq!(truncate_for_log(&big), truncate_for_log_pub(&big));
    }

    // ----- extract_decision_marker: first non-blank line only ------------

    #[test]
    fn marker_parses_simple_decision_line() {
        let (variant, rest) =
            extract_decision_marker("DECISION: continue_skipping\nrationale: hb ok")
                .expect("must extract");
        assert_eq!(variant, "continue_skipping");
        assert!(rest.starts_with("rationale:"));
    }

    #[test]
    fn marker_is_case_insensitive_on_word_decision() {
        for line in [
            "decision: continue_skipping",
            "Decision: continue_skipping",
            "DECISION: continue_skipping",
        ] {
            let (v, _) = extract_decision_marker(line).expect("must extract");
            assert_eq!(v, "continue_skipping");
        }
    }

    #[test]
    fn marker_ignores_later_decision_line_injection() {
        // Security: a hostile/buggy model embedding a marker mid-response
        // must NOT be treated as authoritative. Only first non-blank line.
        let body = "the model is thinking\nDECISION: open_tracking_issue";
        let extracted = extract_decision_marker(body);
        assert!(
            extracted.is_none(),
            "mid-body DECISION marker must be ignored, got: {extracted:?}"
        );
    }

    #[test]
    fn marker_returns_none_when_absent() {
        assert!(extract_decision_marker("just prose").is_none());
        assert!(extract_decision_marker("").is_none());
        assert!(extract_decision_marker("   ").is_none());
    }

    #[test]
    fn marker_skips_leading_blank_lines() {
        let (v, _) = extract_decision_marker("\n\nDECISION: deprioritize\n").expect("must extract");
        assert_eq!(v, "deprioritize");
    }

    // ----- parse_with_marker: variant + body interaction -----------------

    #[test]
    fn parse_with_marker_rejects_unknown_variant() {
        let err = parse_with_marker("not_a_real_variant", "", "raw").expect_err("must Err");
        assert!(err.contains("not_a_real_variant"));
        assert!(err.contains("raw_response"));
    }

    #[test]
    fn parse_with_marker_derives_rationale_from_prose_remainder() {
        let d = parse_with_marker("continue_skipping", "rationale: hb ok", "raw").unwrap();
        match d {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                assert_eq!(rationale, "hb ok");
            }
            other => panic!("expected ContinueSkipping, got {other:?}"),
        }
    }

    #[test]
    fn parse_with_marker_uses_default_rationale_when_remainder_empty() {
        let d = parse_with_marker("continue_skipping", "", "raw").unwrap();
        match d {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                assert!(rationale.contains("no rationale"));
            }
            other => panic!("expected ContinueSkipping, got {other:?}"),
        }
    }

    #[test]
    fn parse_with_marker_pulls_required_fields_from_json_body() {
        // open_tracking_issue requires title + body in the JSON body.
        let body = r#"{"title":"engineer wedged","body":"see logs","rationale":"7h idle"}"#;
        let d = parse_with_marker("open_tracking_issue", body, "raw").unwrap();
        match d {
            EngineerLifecycleDecision::OpenTrackingIssue {
                title,
                body,
                rationale,
            } => {
                assert_eq!(title, "engineer wedged");
                assert_eq!(body, "see logs");
                assert_eq!(rationale, "7h idle");
            }
            other => panic!("expected OpenTrackingIssue, got {other:?}"),
        }
    }

    #[test]
    fn parse_with_marker_wins_over_conflicting_json_choice() {
        // Security: prevent JSON-body variant smuggling. Marker is authoritative.
        let body = r#"{"choice":"mark_goal_blocked","reason":"x","rationale":"y"}"#;
        let d = parse_with_marker("continue_skipping", body, "raw").unwrap();
        assert!(matches!(
            d,
            EngineerLifecycleDecision::ContinueSkipping { .. }
        ));
    }

    // ----- parse_decision_from_response: DECISION marker only (issue #1980) -

    // (a) DECISION marker with rationale — the supported format.
    #[test]
    fn parse_decision_marker_continue_skipping() {
        let raw = "DECISION: continue_skipping\nhealthy heartbeat";
        let d = parse_decision_from_response(raw).expect("must parse");
        match d {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                assert!(rationale.contains("healthy"));
            }
            other => panic!("expected ContinueSkipping, got {other:?}"),
        }
    }

    #[test]
    fn parse_decision_marker_deprioritize() {
        let raw = "DECISION: deprioritize\nchronic failure pattern";
        let d = parse_decision_from_response(raw).expect("must parse");
        assert!(matches!(d, EngineerLifecycleDecision::Deprioritize { .. }));
    }

    // (b) JSON-only input is now REJECTED (issue #1980 — JSON fallback removed).
    #[test]
    fn parse_decision_json_only_is_rejected() {
        let raw = r#"{"choice":"continue_skipping","rationale":"healthy"}"#;
        let err = parse_decision_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("DECISION:"),
            "JSON without DECISION marker must be rejected (issue #1980): {err}"
        );
    }

    #[test]
    fn parse_decision_json_in_prose_is_rejected() {
        let raw = "Here's the call:\n```json\n{\"choice\":\"deprioritize\",\"rationale\":\"chronic\"}\n```";
        let err = parse_decision_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("DECISION:"),
            "JSON-in-prose without DECISION marker must be rejected (issue #1980): {err}"
        );
    }

    // (c) Completely unparseable surfaces a structured error, never panics.
    #[test]
    fn parse_decision_unparseable_returns_err_with_raw_body() {
        let raw = "no json no marker just words";
        let err = parse_decision_from_response(raw).expect_err("must Err");
        assert!(
            err.contains(raw),
            "raw must be embedded (issue #1711): {err}"
        );
        assert!(
            err.contains("DECISION:"),
            "error must explain missing marker: {err}"
        );
    }

    #[test]
    fn parse_decision_empty_returns_empty_err() {
        let err = parse_decision_from_response("").expect_err("must Err");
        assert!(err.to_lowercase().contains("empty"), "got: {err}");
    }

    #[test]
    fn parse_decision_prefers_marker_over_embedded_json() {
        // Mixed shape: prose marker + a body JSON. Marker is authoritative.
        let raw =
            "DECISION: mark_goal_blocked\n{\"reason\":\"needs API key\",\"rationale\":\"human\"}";
        let d = parse_decision_from_response(raw).expect("must parse");
        match d {
            EngineerLifecycleDecision::MarkGoalBlocked { reason, .. } => {
                assert_eq!(reason, "needs API key");
            }
            other => panic!("expected MarkGoalBlocked, got {other:?}"),
        }
    }

    // Structured variants with labeled body lines instead of JSON
    #[test]
    fn parse_decision_open_tracking_issue_with_labeled_body() {
        let raw = "DECISION: open_tracking_issue\nTITLE: engineer panicked\nBODY: see logs for details\nrationale: repeated panic in loop";
        let d = parse_decision_from_response(raw).expect("must parse");
        match d {
            EngineerLifecycleDecision::OpenTrackingIssue {
                title,
                body,
                rationale,
            } => {
                assert_eq!(title, "engineer panicked");
                assert_eq!(body, "see logs for details");
                assert!(rationale.contains("panic"));
            }
            other => panic!("expected OpenTrackingIssue, got {other:?}"),
        }
    }

    #[test]
    fn parse_decision_mark_goal_blocked_with_labeled_body() {
        let raw = "DECISION: mark_goal_blocked\nREASON: needs API key from operator\nrationale: external dependency";
        let d = parse_decision_from_response(raw).expect("must parse");
        match d {
            EngineerLifecycleDecision::MarkGoalBlocked { reason, rationale } => {
                assert_eq!(reason, "needs API key from operator");
                assert!(rationale.contains("external"));
            }
            other => panic!("expected MarkGoalBlocked, got {other:?}"),
        }
    }

    #[test]
    fn parse_decision_reclaim_with_labeled_body() {
        let raw = "DECISION: reclaim_and_redispatch\nREDISPATCH_CONTEXT: focus on the persistence layer\nrationale: stuck for 7 hours";
        let d = parse_decision_from_response(raw).expect("must parse");
        match d {
            EngineerLifecycleDecision::ReclaimAndRedispatch {
                rationale,
                redispatch_context,
            } => {
                assert!(redispatch_context.contains("persistence"));
                assert!(rationale.contains("stuck"));
            }
            other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
        }
    }

    // ----- (d) RustyClawdBrain bridge: end-to-end with stub submitter ---

    #[test]
    fn bridge_returns_decision_on_marker_response() {
        let stub = StubSubmitter::ok("DECISION: continue_skipping\nhb ok");
        let brain = RustyClawdBrain::new(stub);
        let d = brain.decide_engineer_lifecycle(&ctx()).expect("must Ok");
        assert!(matches!(
            d,
            EngineerLifecycleDecision::ContinueSkipping { .. }
        ));
    }

    #[test]
    fn bridge_rejects_json_only_response() {
        let stub = StubSubmitter::ok(r#"{"choice":"continue_skipping","rationale":"hb ok"}"#);
        let brain = RustyClawdBrain::new(stub);
        let err = brain
            .decide_engineer_lifecycle(&ctx())
            .expect_err("JSON-only must now be rejected (issue #1980)");
        match err {
            SimardError::AdapterInvocationFailed { reason, .. } => {
                assert!(
                    reason.contains("DECISION:"),
                    "error should mention missing DECISION marker: {reason}"
                );
            }
            other => panic!("wrong error variant: {other:?}"),
        }
    }

    #[test]
    fn bridge_rejects_json_in_prose_response() {
        let stub = StubSubmitter::ok(
            "Some thinking...\n```json\n{\"choice\":\"deprioritize\",\"rationale\":\"chronic\"}\n```\nDone.",
        );
        let brain = RustyClawdBrain::new(stub);
        let err = brain
            .decide_engineer_lifecycle(&ctx())
            .expect_err("JSON-in-prose must now be rejected (issue #1980)");
        match err {
            SimardError::AdapterInvocationFailed { .. } => {} // expected
            other => panic!("wrong error variant: {other:?}"),
        }
    }

    #[test]
    fn bridge_surfaces_adapter_error_on_unparseable_response() {
        let stub = StubSubmitter::ok("totally not json at all");
        let brain = RustyClawdBrain::new(stub);
        let err = brain
            .decide_engineer_lifecycle(&ctx())
            .expect_err("must Err so caller can fall back");
        match err {
            SimardError::AdapterInvocationFailed { base_type, reason } => {
                assert_eq!(base_type, ADAPTER_TAG);
                assert!(
                    reason.contains("totally not json at all"),
                    "adapter error must embed raw body for diagnosis: {reason}"
                );
            }
            other => panic!("expected AdapterInvocationFailed, got {other:?}"),
        }
    }

    #[test]
    fn bridge_propagates_submitter_error_without_panic() {
        let stub = StubSubmitter::err();
        let brain = RustyClawdBrain::new(stub);
        let err = brain
            .decide_engineer_lifecycle(&ctx())
            .expect_err("must Err");
        // Network-style failure surfaces directly (not parsed as a brain
        // response). The stub's own AdapterInvocationFailed bubbles up.
        match err {
            SimardError::AdapterInvocationFailed { base_type, .. } => {
                assert_eq!(base_type, "stub");
            }
            other => panic!("expected AdapterInvocationFailed, got {other:?}"),
        }
    }

    #[test]
    fn bridge_calls_submitter_exactly_once_per_decision() {
        let stub = StubSubmitter::ok("DECISION: continue_skipping\nok");
        let counter = stub.call_counter();
        let brain = RustyClawdBrain::new(stub);
        let _ = brain.decide_engineer_lifecycle(&ctx()).unwrap();
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "bridge must call submitter exactly once per decision"
        );
    }

    #[test]
    fn bridge_renders_prompt_with_context_fields() {
        let stub = StubSubmitter::ok("DECISION: continue_skipping\nok");
        let brain = RustyClawdBrain::new(stub);
        let prompt = brain.render_prompt(&EngineerLifecycleCtx {
            goal_id: "marker-goal".into(),
            goal_description: "marker-desc".into(),
            cycle_number: 99,
            consecutive_skip_count: 5,
            failure_count: 2,
            worktree_path: PathBuf::from("/tmp/marker-wt"),
            worktree_mtime_secs_ago: 0,
            sentinel_pid: Some(12345),
            last_engineer_log_tail: "marker-log-tail".into(),
            commits_behind: 3,
            in_flight_engineer_count: 1,
            minutes_since_last_update_attempt: u64::MAX,
        });
        assert!(prompt.contains("marker-goal"));
        assert!(prompt.contains("marker-desc"));
        assert!(prompt.contains("/tmp/marker-wt"));
        assert!(prompt.contains("marker-log-tail"));
        assert!(prompt.contains("12345"));
    }

    #[test]
    fn bridge_renders_sentinel_none_as_placeholder() {
        let stub = StubSubmitter::ok("DECISION: continue_skipping\nok");
        let brain = RustyClawdBrain::new(stub);
        let prompt = brain.render_prompt(&EngineerLifecycleCtx {
            sentinel_pid: None,
            ..ctx()
        });
        // None must render as the documented sentinel rather than panic.
        assert!(prompt.contains("<none>"));
    }

    // ----- VALID_VARIANTS audit ------------------------------------------
    #[test]
    fn valid_variants_matches_decision_enum_tags() {
        // Anti-drift: every variant the wire enum accepts must be in
        // VALID_VARIANTS, or the marker parser will reject perfectly valid
        // brain output as "invalid variant".
        for tag in [
            "continue_skipping",
            "reclaim_and_redispatch",
            "deprioritize",
            "open_tracking_issue",
            "mark_goal_blocked",
            "consider_self_update",
        ] {
            assert!(
                VALID_VARIANTS.contains(&tag),
                "VALID_VARIANTS must include `{tag}`"
            );
        }
    }
}
