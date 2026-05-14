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

    // Try to extract a JSON object from the remainder for variant-specific
    // fields. We use the same `find('{') .. rfind('}')` extraction the
    // legacy parser uses, so a body that mixes prose and JSON still works.
    let trimmed_rest = rest.trim();
    let json_value: serde_json::Value = if let Some(start) = trimmed_rest.find('{')
        && let Some(end) = trimmed_rest.rfind('}')
        && end >= start
    {
        let candidate = &trimmed_rest[start..=end];
        match serde_json::from_str::<serde_json::Value>(candidate) {
            Ok(v) if v.is_object() => v,
            // Body looks like JSON but isn't valid → treat as no JSON
            // present and fall through to prose-rationale derivation. The
            // marker is authoritative; we don't fail just because the
            // optional body has a syntax error.
            _ => serde_json::Value::Object(serde_json::Map::new()),
        }
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    let mut obj = match json_value {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };

    // Marker wins over any `choice` field in the JSON body.
    obj.insert(
        "choice".to_string(),
        serde_json::Value::String(variant.to_string()),
    );

    // If the body did not provide a rationale, derive one from the prose
    // remainder. Strip a leading `rationale:` prefix if present so
    // `DECISION: continue_skipping\nrationale: hb ok` ends up with
    // `rationale = "hb ok"` rather than `"rationale: hb ok"`.
    if !obj.contains_key("rationale") {
        let prose_rationale = if trimmed_rest.is_empty() {
            "(no rationale provided)".to_string()
        } else {
            // Try to derive prose rationale by stripping JSON body if any.
            // For variants with no JSON body, `trimmed_rest` IS the rationale.
            // For variants with a JSON body that nonetheless omits rationale,
            // we use the trimmed remainder verbatim as a best-effort.
            let candidate = trimmed_rest;
            let stripped = candidate
                .strip_prefix("rationale:")
                .or_else(|| candidate.strip_prefix("Rationale:"))
                .or_else(|| candidate.strip_prefix("RATIONALE:"))
                .unwrap_or(candidate)
                .trim();
            if stripped.is_empty() {
                "(no rationale provided)".to_string()
            } else {
                stripped.to_string()
            }
        };
        obj.insert(
            "rationale".to_string(),
            serde_json::Value::String(prose_rationale),
        );
    }

    serde_json::from_value::<EngineerLifecycleDecision>(serde_json::Value::Object(obj)).map_err(
        |e| {
            format!(
                "brain DECISION marker `{variant}` present but field validation failed: {e}; \
                 raw_response={:?}",
                truncate_for_log(raw)
            )
        },
    )
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

    // Prose-first: try to extract a `DECISION:` marker from the first
    // non-blank line. If present, the marker is authoritative.
    if let Some((variant, rest)) = extract_decision_marker(stripped) {
        return parse_with_marker(variant, rest, raw);
    }

    // Backward-compat: legacy JSON object extraction.
    if let Some(start) = stripped.find('{')
        && let Some(end) = stripped.rfind('}')
        && end >= start
    {
        let candidate = &stripped[start..=end];
        return serde_json::from_str::<EngineerLifecycleDecision>(candidate).map_err(|e| {
            format!(
                "brain JSON parse error: {e}; payload={candidate}; raw_response={:?}",
                truncate_for_log(raw)
            )
        });
    }

    Err(format!(
        "brain response had no DECISION marker and no JSON object; raw_response={:?}",
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
