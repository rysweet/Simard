//! M1 configuration: the `SIMARD_OVERSEER_*` flag-gate (default **OFF**) and the
//! single-sourced daily-budget knob.
//!
//! Everything here is pure and env-**injectable** (`impl Fn(&str) -> Option<String>`),
//! so the gating and budget-resolution logic is unit-tested with zero process
//! environment mutation (no global state, no `set_var` races between parallel
//! tests). The `*_env` production entry points are the only functions that read
//! the real `std::env`.
//!
//! Two operator hard-gates live here:
//! - The Overseer is **additive and default-off**: absent/empty/unrecognised
//!   `SIMARD_OVERSEER_ENABLED` keeps the daemon behaving exactly as before.
//! - The daily budget is **single-sourced** from `SIMARD_DAILY_BUDGET_USD`
//!   (crusty risk #6) so the Overseer's [`BudgetGate`](crate::overseer::BudgetGate)
//!   can never drift from the OODA loop's ceiling.

/// Master flag that gates the Overseer on. Default **OFF**: while unset (or set
/// to a non-truthy value) nothing constructs or schedules an `Overseer` and the
/// daemon's behaviour is unchanged.
pub const OVERSEER_ENABLED_ENV: &str = "SIMARD_OVERSEER_ENABLED";

/// Daily LLM-budget ceiling, shared with the OODA loop. Single-sourcing this
/// (rather than hardcoding a duplicate) keeps the Overseer's budget gate and the
/// daemon's spend ceiling from silently diverging.
pub const DAILY_BUDGET_ENV: &str = "SIMARD_DAILY_BUDGET_USD";

/// Cadence (seconds) of the M1 read-only observer sensor. Clamped to a floor so
/// self-tuning (M4) can never drive the observer into a hot loop.
pub const OVERSEER_INTERVAL_ENV: &str = "SIMARD_OVERSEER_INTERVAL_SECS";

/// Opt-out flag for the **Simard Whisperer**. The Overseer's lightweight
/// steering channel is ON by default whenever the acting Overseer runs; an
/// explicit falsey value (`0`/`false`/`no`/`off`) disables it. Whispering only
/// makes sense while the Overseer runs, so a disabled Overseer forces the
/// whisperer off regardless of this flag.
pub const SIMARD_OVERSEER_WHISPER_ENV: &str = "SIMARD_OVERSEER_WHISPER";

/// Opt-out flag for **goal-board health handling** (the self-heal + escalate
/// paths). ON by default whenever the acting Overseer runs; an explicit falsey
/// value (`0`/`false`/`no`/`off`) disables it. Goal-board health only makes
/// sense while the Overseer runs, so a disabled Overseer forces it off.
pub const SIMARD_OVERSEER_GOAL_HEALTH_ENV: &str = "SIMARD_OVERSEER_GOAL_HEALTH";

/// Opt-out flag for the Overseer's **cognitive-memory recall** (issue #2628):
/// bounded read access to Simard's memory graph in Observe/Orient plus one
/// deliberate, de-duplicated episodic write-back. ON by default whenever the
/// acting Overseer runs; an explicit falsey value (`0`/`false`/`no`/`off`)
/// disables it. Recall only makes sense while the Overseer runs, so a disabled
/// Overseer forces it off regardless of this flag.
pub const SIMARD_OVERSEER_MEMORY_RECALL_ENV: &str = "SIMARD_OVERSEER_MEMORY_RECALL";

/// Opt-out flag for the recurring **backlog-coverage gap-scan** (the "WHAT
/// WORKSTREAMS ARE WE MISSING?" survey). ON by default whenever the acting
/// Overseer runs; an explicit falsey value (`0`/`false`/`no`/`off`) disables it.
/// The gap-scan only makes sense while the Overseer runs, so a disabled Overseer
/// forces it off regardless of this flag.
pub const SIMARD_OVERSEER_GAP_SCAN_ENV: &str = "SIMARD_OVERSEER_GAP_SCAN";

/// Cadence divisor for the gap-scan: run the survey once every N Overseer ticks.
/// Default `1` (every tick); clamped to a floor of `1` so a bad value never
/// disables the scan by stealth nor divides by zero.
pub const SIMARD_OVERSEER_GAP_SCAN_EVERY_N_ENV: &str = "SIMARD_OVERSEER_GAP_SCAN_EVERY_N";

/// Opt-out flag for the agentic **Overseer health-review** rail: each due tick a
/// reasoning step reads the OODA journal + `simard status` + `simard goal list`,
/// detects crash-loops / shared failure signatures, and drives remediation
/// through `LaunchRecipe` / `EscalateBlockedGoal`. ON by default whenever the
/// acting Overseer runs; an explicit falsey value (`0`/`false`/`no`/`off`)
/// disables it. Health-review only makes sense while the Overseer runs, so a
/// disabled Overseer forces it off regardless of this flag.
pub const SIMARD_OVERSEER_HEALTH_REVIEW_ENV: &str = "SIMARD_OVERSEER_HEALTH_REVIEW";

/// Override for the systemd `--user` unit whose journal the health-review recipe
/// reads. Defaults to the OODA daemon unit (`simard-ooda.service`); an operator
/// may point it at a differently-named unit. Unset/empty falls back to the
/// default — a blank value never yields an empty `-u` argument.
pub const SIMARD_OVERSEER_HEALTH_REVIEW_UNIT_ENV: &str = "SIMARD_OVERSEER_HEALTH_REVIEW_UNIT";

/// Opt-out switch for the periodic stale-engineer-claim reaper (issue #4099).
/// The reaper is ENABLED by default; only an explicit falsey value here disables
/// it. Distinct from the acting-Overseer gate: the reaper is a safety mechanism
/// that closes the `engineer_claims` leak, so it defaults ON even in a
/// conservative deployment.
pub const SIMARD_CLAIM_REAP_ENABLED_ENV: &str = "SIMARD_CLAIM_REAP_ENABLED";

/// Idle-staleness threshold (seconds) beyond which a claim's worktree is judged
/// stale and reclaimed. Newest-file mtime idle age, NOT a run-duration cap.
/// Unset/empty/unparseable ⇒ [`DEFAULT_CLAIM_REAP_STALE_SECS`] (fail-safe).
pub const SIMARD_CLAIM_REAP_STALE_SECS_ENV: &str = "SIMARD_CLAIM_REAP_STALE_SECS";

/// Default reaper staleness threshold: 30 minutes. Generous on purpose so a
/// long-but-alive engineer (slow compile/test) that keeps writing is never
/// reaped (fail-closed, no wall-clock kill).
pub const DEFAULT_CLAIM_REAP_STALE_SECS: u64 = 1800;

/// GitHub login the acting Overseer authors its own workstreams under. Sourced
/// here so the daemon and the merge/recursion path agree on ONE stable, DISTINCT
/// identity (never the human operator's login). Defaults to
/// [`DEFAULT_OVERSEER_AUTHOR_LOGIN`] when unset.
pub const OVERSEER_AUTHOR_LOGIN_ENV: &str = "SIMARD_OVERSEER_AUTHOR_LOGIN";
/// The Overseer's well-known bot login, distinct from the engineer/OODA
/// identity. Used by the anti-recursion guard so the Overseer never
/// verifies/merges/deploys its OWN PRs and never re-opens its own goals.
pub const DEFAULT_OVERSEER_AUTHOR_LOGIN: &str = "simard-overseer[bot]";

/// Opt-in flag (default OFF, truthy-required) for the native Overseer Signal
/// operator-liaison (issue #4911, Deliverable 1). Master-gated: an explicitly
/// disabled Overseer forces it off regardless.
pub const SIMARD_OVERSEER_SIGNAL_LIAISON_ENV: &str = "SIMARD_OVERSEER_SIGNAL_LIAISON";

/// Opt-in flag (default OFF, truthy-required) for the autonomous PR rework loop
/// (issue #4911, Deliverable 2). When OFF, a fixable hold stays held-with-reason.
/// Master-gated.
pub const SIMARD_OVERSEER_REWORK_ENV: &str = "SIMARD_OVERSEER_REWORK";

/// Per-PR rework attempt cap (issue #4911). Parsed as an integer and clamped to
/// `1..=10`; unset/empty/unparseable ⇒ [`DEFAULT_REWORK_MAX_ATTEMPTS`].
pub const SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS_ENV: &str = "SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS";

/// Default per-PR rework attempt cap: a small bound so a self-fighting loop
/// escalates to a human quickly rather than churning.
pub const DEFAULT_REWORK_MAX_ATTEMPTS: u32 = 3;

/// Operator E.164 the liaison accepts messages from (mirrors
/// `SIMARD_OVERSEER_EMAIL_TO`). Required for Deliverable 1 to act.
pub const SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER_ENV: &str =
    "SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER";

/// Operator Signal group id the liaison acts on. Required for Deliverable 1 to
/// act (both operator number AND group id must be configured).
pub const SIMARD_OVERSEER_SIGNAL_GROUP_ID_ENV: &str = "SIMARD_OVERSEER_SIGNAL_GROUP_ID";

/// Default observer cadence: 15 minutes — frequent enough to catch churn, far
/// below any launch/merge cadence (M1 files at most one deduped issue per
/// recurring signature, so a tighter interval adds no writes).
pub const DEFAULT_OVERSEER_INTERVAL_SECS: u64 = 900;

/// Hard floor on the observer cadence (crusty risk #4 / M4 clamp): never tick
/// more often than once a minute regardless of env or self-tuning.
pub const MIN_OVERSEER_INTERVAL_SECS: u64 = 60;

/// Ultimate fallback when `SIMARD_DAILY_BUDGET_USD` is unset, empty, unparseable,
/// or non-positive. Mirrors the OODA loop's historical default.
pub const DEFAULT_DAILY_BUDGET_USD: f64 = 500.0;

/// Env var (#893) that overrides how many *consecutive* transient cycle failures
/// the `overseer` meta-thread will self-heal through (mapping to `"backoff"`)
/// before it escalates to `"erroring"`. Unset/empty/unparseable/zero/negative
/// all fall back to [`DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING`] — a bad value
/// can never disable self-healing.
pub const OVERSEER_TRANSIENT_BACKOFF_CEILING_ENV: &str =
    "SIMARD_OVERSEER_TRANSIENT_BACKOFF_CEILING";

/// Default consecutive-transient self-heal ceiling (#893). Bounded so a
/// hard-down dependency that fails transiently forever cannot hide behind an
/// infinite backoff (SR-2) — after this many consecutive transient failures the
/// meta-thread escalates to `"erroring"`. Must be `>= 1` so at least one
/// self-healing backoff is always permitted.
pub const DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING: u32 = 3;

/// Env var (Problem 1 / issue #4186) setting the BASE suppression window
/// (seconds) of the gap-scan dedup [`crate::overseer::guardrails::BackoffGate`].
/// Unset/empty/unparseable/zero/negative all fall back to
/// [`DEFAULT_OVERSEER_BACKOFF_BASE_SECS`] — a bad base can never collapse the
/// window to `0` and let every duplicate coverage relaunch through.
pub const SIMARD_OVERSEER_BACKOFF_BASE_SECS_ENV: &str = "SIMARD_OVERSEER_BACKOFF_BASE_SECS";

/// Env var (Problem 1 / issue #4186) setting the exponential GROWTH multiplier of
/// the gap-scan dedup [`crate::overseer::guardrails::BackoffGate`]. Must be `> 1`
/// so the window genuinely grows; unset/empty/unparseable/`<= 1` all fall back to
/// [`DEFAULT_OVERSEER_BACKOFF_MULTIPLIER`].
pub const SIMARD_OVERSEER_BACKOFF_MULTIPLIER_ENV: &str = "SIMARD_OVERSEER_BACKOFF_MULTIPLIER";

/// Env var (Problem 1 / issue #4186) setting the hard CAP (seconds) on the
/// gap-scan dedup [`crate::overseer::guardrails::BackoffGate`] window, bounding
/// suppression so a genuinely-recurring gap is never silenced longer than this.
/// Unset/empty/unparseable/zero/negative all fall back to
/// [`DEFAULT_OVERSEER_BACKOFF_MAX_SECS`].
pub const SIMARD_OVERSEER_BACKOFF_MAX_SECS_ENV: &str = "SIMARD_OVERSEER_BACKOFF_MAX_SECS";

/// Default base suppression window (15 min) for the gap-scan dedup backoff — the
/// same window the sibling [`WhisperGate`]-based dedup rails use.
pub const DEFAULT_OVERSEER_BACKOFF_BASE_SECS: i64 = 900;

/// Default exponential growth multiplier for the gap-scan dedup backoff.
pub const DEFAULT_OVERSEER_BACKOFF_MULTIPLIER: i64 = 2;

/// Default hard cap (24 h) on the gap-scan dedup backoff window: suppression is
/// bounded so a real recurring gap always resurfaces within a day.
pub const DEFAULT_OVERSEER_BACKOFF_MAX_SECS: i64 = 86_400;

/// Opt-out flag (issue #4930) for the durable **issue-cooldown ledger** that
/// de-duplicates the OODA-core auto-issue filers (`ooda-stuck`,
/// `recurring_goal_reblock`, `workstream_gap:issue`) across daemon exec-reload,
/// restart, and cross-client goal-board merges. ENABLED by default; only an
/// explicit falsey value (`0`/`false`/`no`/`off`) disables it, in which case
/// each filer falls back to its prior per-path dedup. Kept default-ON so the
/// storm-suppression safety mechanism is never lost by stealth.
pub const SIMARD_OVERSEER_ISSUE_COOLDOWN_ENV: &str = "SIMARD_OVERSEER_ISSUE_COOLDOWN";

/// Env var (issue #4930) setting the BASE cooldown window (seconds) of the
/// [`crate::overseer::issue_cooldown::IssueCooldownLedger`]. Clamped UP to at
/// least one full OODA cycle ([`overseer_interval_secs`]) so the same
/// `(goal, finding)` can never re-file every cycle — the storm's defining
/// symptom. Unset/empty/unparseable/zero/negative fall back to
/// [`DEFAULT_ISSUE_COOLDOWN_BASE_SECS`].
pub const SIMARD_OVERSEER_ISSUE_COOLDOWN_BASE_SECS_ENV: &str =
    "SIMARD_OVERSEER_ISSUE_COOLDOWN_BASE_SECS";

/// Env var (issue #4930) setting the hard CAP (seconds) on the issue-cooldown
/// window. Clamped UP to `>= base` so the window can never be negative. A
/// still-open finding therefore re-surfaces at least once per cap. Unset/empty/
/// unparseable/zero/negative fall back to [`DEFAULT_ISSUE_COOLDOWN_MAX_SECS`].
pub const SIMARD_OVERSEER_ISSUE_COOLDOWN_MAX_SECS_ENV: &str =
    "SIMARD_OVERSEER_ISSUE_COOLDOWN_MAX_SECS";

/// Env var (issue #4930) setting the rolling-hour emit budget shared with the
/// [`crate::overseer::guardrails::WhisperGate`] semantics. Unset/empty/
/// unparseable/zero fall back to [`DEFAULT_ISSUE_COOLDOWN_CAP_PER_HOUR`].
pub const SIMARD_OVERSEER_ISSUE_COOLDOWN_CAP_PER_HOUR_ENV: &str =
    "SIMARD_OVERSEER_ISSUE_COOLDOWN_CAP_PER_HOUR";

/// Default base issue-cooldown window: 6 h — already well above the one-OODA-cycle
/// hard floor, favouring quiet-by-default while still re-surfacing daily.
pub const DEFAULT_ISSUE_COOLDOWN_BASE_SECS: i64 = 21_600;

/// Default issue-cooldown window cap: 24 h, so a still-open finding is never
/// permanently silenced.
pub const DEFAULT_ISSUE_COOLDOWN_MAX_SECS: i64 = 86_400;

/// Default rolling-hour emit budget for the issue-cooldown ledger.
pub const DEFAULT_ISSUE_COOLDOWN_CAP_PER_HOUR: usize = 20;

/// Resolve the Overseer master flag from an env resolver. Fail-safe: only an
/// explicit truthy value (`1`/`true`/`yes`/`on`, case-insensitive) enables the
/// Overseer; everything else — including an unset var — leaves it OFF.
pub fn overseer_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    match lookup(OVERSEER_ENABLED_ENV) {
        Some(v) => is_truthy(&v),
        None => false,
    }
}

/// Production entry point: read the real process environment.
pub fn overseer_enabled() -> bool {
    overseer_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve the **acting** Overseer master flag with **default ON** semantics
/// (M2+ co-process). The daemon runs the acting Overseer UNLESS
/// `SIMARD_OVERSEER_ENABLED` is explicitly set to a falsey value
/// (`0`/`false`/`no`/`off`, case-insensitive). This is deliberately the inverse
/// default of [`overseer_enabled_from`] — which gates the M1 read-only sensor
/// default-**OFF** — because the acting co-process is opt-**out**: the operator
/// disables it explicitly, and an unset/empty var leaves it enabled.
pub fn overseer_acting_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    // Enabled unless an explicit falsey value is set. `matches!` returns true for
    // the falsey case; negate for the enabled result.
    !matches!(
        lookup(OVERSEER_ENABLED_ENV).as_deref().map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn overseer_acting_enabled() -> bool {
    overseer_acting_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve whether the **Simard Whisperer** is enabled, with **default ON**
/// opt-out semantics consistent with the acting Overseer. The whisperer is
/// enabled UNLESS [`SIMARD_OVERSEER_WHISPER_ENV`] is an explicit falsey value —
/// AND only while the acting Overseer itself is enabled (an explicitly-disabled
/// Overseer forces the whisperer off regardless of the whisper flag).
pub fn whisper_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    // No Overseer ⇒ no whisperer.
    if !overseer_acting_enabled_from(&lookup) {
        return false;
    }
    // Opt-out: enabled unless an explicit falsey value is set.
    !matches!(
        lookup(SIMARD_OVERSEER_WHISPER_ENV).as_deref().map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn whisper_enabled() -> bool {
    whisper_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve whether **goal-board health handling** (self-heal false-parked
/// perpetual goals + escalate genuine "needs human review" blocks) is enabled,
/// with **default ON** opt-out semantics consistent with the acting Overseer.
/// Enabled UNLESS [`SIMARD_OVERSEER_GOAL_HEALTH_ENV`] is an explicit falsey
/// value — AND only while the acting Overseer itself is enabled (an
/// explicitly-disabled Overseer forces goal-board health off).
pub fn goal_health_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    // No Overseer ⇒ no goal-board health handling.
    if !overseer_acting_enabled_from(&lookup) {
        return false;
    }
    // Opt-out: enabled unless an explicit falsey value is set.
    !matches!(
        lookup(SIMARD_OVERSEER_GOAL_HEALTH_ENV).as_deref().map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn goal_health_enabled() -> bool {
    goal_health_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve whether the Overseer's **cognitive-memory recall** (issue #2628) is
/// enabled, with **default ON** opt-out semantics consistent with the acting
/// Overseer. Enabled UNLESS [`SIMARD_OVERSEER_MEMORY_RECALL_ENV`] is an explicit
/// falsey value — AND only while the acting Overseer itself is enabled (an
/// explicitly-disabled Overseer forces recall off, since recall only makes
/// sense while the Overseer runs). Never panics on a malformed value.
pub fn memory_recall_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    // No Overseer ⇒ no memory recall.
    if !overseer_acting_enabled_from(&lookup) {
        return false;
    }
    // Opt-out: enabled unless an explicit falsey value is set.
    !matches!(
        lookup(SIMARD_OVERSEER_MEMORY_RECALL_ENV).as_deref().map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn memory_recall_enabled() -> bool {
    memory_recall_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve whether the recurring **backlog-coverage gap-scan** is enabled, with
/// **default ON** opt-out semantics consistent with the acting Overseer. Enabled
/// UNLESS [`SIMARD_OVERSEER_GAP_SCAN_ENV`] is an explicit falsey value — AND only
/// while the acting Overseer itself is enabled (an explicitly-disabled Overseer
/// forces the gap-scan off).
pub fn gap_scan_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    // No Overseer ⇒ no gap-scan.
    if !overseer_acting_enabled_from(&lookup) {
        return false;
    }
    // Opt-out: enabled unless an explicit falsey value is set.
    !matches!(
        lookup(SIMARD_OVERSEER_GAP_SCAN_ENV).as_deref().map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn gap_scan_enabled() -> bool {
    gap_scan_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve the gap-scan cadence divisor (run once every N ticks) from an env
/// resolver. Unset/empty/unparseable/zero/negative all clamp to the floor of `1`
/// (every tick) — the scan is never disabled by stealth via a bad divisor.
pub fn gap_scan_every_n_from(lookup: impl Fn(&str) -> Option<String>) -> u64 {
    match lookup(SIMARD_OVERSEER_GAP_SCAN_EVERY_N_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s.parse::<u64>().unwrap_or(1).max(1),
        _ => 1,
    }
}

/// Production entry point: read the real process environment.
pub fn gap_scan_every_n() -> u64 {
    gap_scan_every_n_from(|k| std::env::var(k).ok())
}

/// Resolve whether the agentic **Overseer health-review** rail is enabled, with
/// **default ON** opt-out semantics consistent with the acting Overseer. Enabled
/// UNLESS [`SIMARD_OVERSEER_HEALTH_REVIEW_ENV`] is an explicit falsey value — AND
/// only while the acting Overseer itself is enabled (an explicitly-disabled
/// Overseer forces health-review off).
pub fn health_review_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    // No Overseer ⇒ no health-review.
    if !overseer_acting_enabled_from(&lookup) {
        return false;
    }
    // Opt-out: enabled unless an explicit falsey value is set.
    !matches!(
        lookup(SIMARD_OVERSEER_HEALTH_REVIEW_ENV).as_deref().map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn health_review_enabled() -> bool {
    health_review_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve the systemd `--user` unit the health-review recipe reads the journal
/// from. Returns the [`SIMARD_OVERSEER_HEALTH_REVIEW_UNIT_ENV`] override when set
/// to a non-empty value, else the OODA daemon unit ([`crate::install::paths::OODA_UNIT`]).
/// A blank override never yields an empty unit.
pub fn health_review_service_unit_from(lookup: impl Fn(&str) -> Option<String>) -> String {
    match lookup(SIMARD_OVERSEER_HEALTH_REVIEW_UNIT_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => crate::install::paths::OODA_UNIT.to_string(),
    }
}

/// Production entry point: read the real process environment.
pub fn health_review_service_unit() -> String {
    health_review_service_unit_from(|k| std::env::var(k).ok())
}

/// Resolve the consecutive-transient self-heal ceiling `N` (#893) from an env
/// resolver. Fail-safe: unset/empty/whitespace/unparseable/zero/negative all
/// fall back to [`DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING`] — a garbage value
/// can never disable self-healing (the floor is always `>= 1`).
pub fn overseer_transient_backoff_ceiling_from(lookup: impl Fn(&str) -> Option<String>) -> u32 {
    match lookup(OVERSEER_TRANSIENT_BACKOFF_CEILING_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s
            .parse::<u32>()
            .ok()
            .filter(|n| *n >= 1)
            .unwrap_or(DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING),
        _ => DEFAULT_OVERSEER_TRANSIENT_BACKOFF_CEILING,
    }
}

/// Production entry point: read the real process environment.
pub fn overseer_transient_backoff_ceiling() -> u32 {
    overseer_transient_backoff_ceiling_from(|k| std::env::var(k).ok())
}

/// Resolve the gap-scan dedup backoff BASE window (seconds) from an env resolver
/// (Problem 1 / issue #4186). Fail-safe: unset/empty/whitespace/unparseable/
/// zero/negative all fall back to [`DEFAULT_OVERSEER_BACKOFF_BASE_SECS`] — a
/// garbage base can never collapse the suppression window to `0`.
pub fn overseer_backoff_base_secs_from(lookup: impl Fn(&str) -> Option<String>) -> i64 {
    match lookup(SIMARD_OVERSEER_BACKOFF_BASE_SECS_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_OVERSEER_BACKOFF_BASE_SECS),
        _ => DEFAULT_OVERSEER_BACKOFF_BASE_SECS,
    }
}

/// Production entry point: read the real process environment.
pub fn overseer_backoff_base_secs() -> i64 {
    overseer_backoff_base_secs_from(|k| std::env::var(k).ok())
}

/// Resolve the gap-scan dedup backoff GROWTH multiplier from an env resolver
/// (Problem 1 / issue #4186). Fail-safe: unset/empty/whitespace/unparseable or
/// any value `<= 1` (which would stop the window growing) all fall back to
/// [`DEFAULT_OVERSEER_BACKOFF_MULTIPLIER`].
pub fn overseer_backoff_multiplier_from(lookup: impl Fn(&str) -> Option<String>) -> i64 {
    match lookup(SIMARD_OVERSEER_BACKOFF_MULTIPLIER_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 1)
            .unwrap_or(DEFAULT_OVERSEER_BACKOFF_MULTIPLIER),
        _ => DEFAULT_OVERSEER_BACKOFF_MULTIPLIER,
    }
}

/// Production entry point: read the real process environment.
pub fn overseer_backoff_multiplier() -> i64 {
    overseer_backoff_multiplier_from(|k| std::env::var(k).ok())
}

/// Resolve the gap-scan dedup backoff hard CAP (seconds) from an env resolver
/// (Problem 1 / issue #4186). Fail-safe: unset/empty/whitespace/unparseable/
/// zero/negative all fall back to [`DEFAULT_OVERSEER_BACKOFF_MAX_SECS`].
pub fn overseer_backoff_max_secs_from(lookup: impl Fn(&str) -> Option<String>) -> i64 {
    match lookup(SIMARD_OVERSEER_BACKOFF_MAX_SECS_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_OVERSEER_BACKOFF_MAX_SECS),
        _ => DEFAULT_OVERSEER_BACKOFF_MAX_SECS,
    }
}

/// Production entry point: read the real process environment.
pub fn overseer_backoff_max_secs() -> i64 {
    overseer_backoff_max_secs_from(|k| std::env::var(k).ok())
}

/// Resolve whether the durable issue-cooldown ledger (issue #4930) is enabled
/// from an env resolver. Opt-out: ENABLED unless an explicit falsey value
/// (`0`/`false`/`no`/`off`) is set. Unset/empty/garbage all leave the ledger ON
/// so the storm-suppression mechanism is never lost by stealth.
pub fn issue_cooldown_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    !matches!(
        lookup(SIMARD_OVERSEER_ISSUE_COOLDOWN_ENV)
            .as_deref()
            .map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn issue_cooldown_enabled() -> bool {
    issue_cooldown_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve the issue-cooldown BASE window (seconds) from an env resolver,
/// clamped UP to at least one full OODA cycle ([`resolve_interval_secs`]) so the
/// same `(goal, finding)` can never re-file every cycle. Unset/empty/unparseable/
/// zero/negative fall back to [`DEFAULT_ISSUE_COOLDOWN_BASE_SECS`]; the cycle
/// floor is then applied regardless.
pub fn issue_cooldown_base_secs_from(lookup: impl Fn(&str) -> Option<String>) -> i64 {
    let requested = match lookup(SIMARD_OVERSEER_ISSUE_COOLDOWN_BASE_SECS_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_ISSUE_COOLDOWN_BASE_SECS),
        _ => DEFAULT_ISSUE_COOLDOWN_BASE_SECS,
    };
    // Never drop below one OODA cycle — the storm's defining symptom.
    requested.max(resolve_interval_secs(&lookup) as i64)
}

/// Production entry point: read the real process environment.
pub fn issue_cooldown_base_secs() -> i64 {
    issue_cooldown_base_secs_from(|k| std::env::var(k).ok())
}

/// Resolve the issue-cooldown window CAP (seconds) from an env resolver, clamped
/// UP to `>= base` so the window is never negative. Unset/empty/unparseable/
/// zero/negative fall back to [`DEFAULT_ISSUE_COOLDOWN_MAX_SECS`].
pub fn issue_cooldown_cap_secs_from(lookup: impl Fn(&str) -> Option<String>) -> i64 {
    let requested = match lookup(SIMARD_OVERSEER_ISSUE_COOLDOWN_MAX_SECS_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_ISSUE_COOLDOWN_MAX_SECS),
        _ => DEFAULT_ISSUE_COOLDOWN_MAX_SECS,
    };
    requested.max(issue_cooldown_base_secs_from(&lookup))
}

/// Production entry point: read the real process environment.
pub fn issue_cooldown_cap_secs() -> i64 {
    issue_cooldown_cap_secs_from(|k| std::env::var(k).ok())
}

/// Resolve the issue-cooldown rolling-hour emit budget from an env resolver.
/// Unset/empty/unparseable/zero fall back to
/// [`DEFAULT_ISSUE_COOLDOWN_CAP_PER_HOUR`] — a bad value never collapses the
/// budget to `0` and permanently silences a finding.
pub fn issue_cooldown_cap_per_hour_from(lookup: impl Fn(&str) -> Option<String>) -> usize {
    match lookup(SIMARD_OVERSEER_ISSUE_COOLDOWN_CAP_PER_HOUR_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_ISSUE_COOLDOWN_CAP_PER_HOUR),
        _ => DEFAULT_ISSUE_COOLDOWN_CAP_PER_HOUR,
    }
}

/// Production entry point: read the real process environment.
pub fn issue_cooldown_cap_per_hour() -> usize {
    issue_cooldown_cap_per_hour_from(|k| std::env::var(k).ok())
}

/// Resolve whether the stale-engineer-claim reaper (issue #4099) is enabled.
///
/// Enabled by default (the reaper closes the `engineer_claims` leak); DISABLED
/// only when [`SIMARD_CLAIM_REAP_ENABLED_ENV`] holds an explicit falsey value
/// (`0`/`false`/`no`/`off`). Unset/empty/garbage ⇒ enabled.
pub fn claim_reap_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    // Opt-out: enabled unless an explicit falsey value is set. Unset/empty/garbage
    // all leave the reaper ON so the leak-closing safety mechanism is never lost by
    // stealth.
    !matches!(
        lookup(SIMARD_CLAIM_REAP_ENABLED_ENV).as_deref().map(str::trim),
        Some(v) if is_falsey(v)
    )
}

/// Production entry point: read the real process environment.
pub fn claim_reap_enabled() -> bool {
    claim_reap_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve the reaper's idle-staleness threshold in seconds.
///
/// Unset/empty/unparseable ⇒ [`DEFAULT_CLAIM_REAP_STALE_SECS`] (fail-safe: a bad
/// value never collapses the threshold to a mass-reclaim `0`). An explicit
/// numeric value is honored.
pub fn claim_reap_stale_secs_from(lookup: impl Fn(&str) -> Option<String>) -> u64 {
    // Fail-safe: unset/empty/unparseable AND an explicit `0` (which would collapse
    // the window to a mass-reclaim) fall back to the generous default. Only a
    // positive integer is honored.
    match lookup(SIMARD_CLAIM_REAP_STALE_SECS_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => s
            .parse::<u64>()
            .ok()
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_CLAIM_REAP_STALE_SECS),
        _ => DEFAULT_CLAIM_REAP_STALE_SECS,
    }
}

/// Production entry point: read the real process environment.
pub fn claim_reap_stale_secs() -> u64 {
    claim_reap_stale_secs_from(|k| std::env::var(k).ok())
}

/// Resolve the Overseer's DISTINCT author login. Falls back to
/// [`DEFAULT_OVERSEER_AUTHOR_LOGIN`] when unset or empty.
pub fn overseer_author_login_from(lookup: impl Fn(&str) -> Option<String>) -> String {
    match lookup(OVERSEER_AUTHOR_LOGIN_ENV).as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => DEFAULT_OVERSEER_AUTHOR_LOGIN.to_string(),
    }
}

/// Production entry point: read the real process environment.
pub fn overseer_author_login() -> String {
    overseer_author_login_from(|k| std::env::var(k).ok())
}

// ─── autonomous-self-merge sensor gates (issue #4097) ──────────────────────
//
// Two operator hard-gates activate the `ready_prs` sensor rail. BOTH default
// to OFF and fail CLOSED so deploying the code does NOT immediately merge
// across every governed repo:
// - `SIMARD_AUTOMERGE_REPOS`: the explicit repo allowlist (comma-separated
//   `owner/name`). Default EMPTY => no repo eligible => autonomous merge OFF.
//   Enable ONE canary repo first.
// - `SIMARD_AUTOMERGE_AUTHOR`: the OODA/engineer `gh` login Simard authors her
//   PRs under. The sensor lists ONLY PRs whose `author.login` EXACTLY matches,
//   so it never acts on a human's PR. Default None => the sensor cannot tell
//   its own PRs from a human's => it yields NO candidates (fail-closed). This
//   is DISTINCT from [`OVERSEER_AUTHOR_LOGIN_ENV`] (the overseer-bot recursion
//   identity): they are different logins by design, so the downstream
//   recursion guard never collides with a valid self-merge candidate.

/// Explicit repo allowlist gating the autonomous self-merge sensor. See the
/// module note above. Comma-separated `owner/name`; default EMPTY = OFF.
pub const SIMARD_AUTOMERGE_REPOS_ENV: &str = "SIMARD_AUTOMERGE_REPOS";

/// The OODA/engineer `gh` login Simard authors her PRs under. See the module
/// note above. Default None = fail-closed (no candidates).
pub const SIMARD_AUTOMERGE_AUTHOR_ENV: &str = "SIMARD_AUTOMERGE_AUTHOR";

/// Resolve the autonomous-self-merge repo allowlist from an env resolver.
/// Comma-separated `owner/name`; entries are trimmed and empties dropped.
/// Unset/empty/comma-noise => EMPTY vec (autonomous merge OFF, fail-closed).
pub fn automerge_repos_from(lookup: impl Fn(&str) -> Option<String>) -> Vec<String> {
    lookup(SIMARD_AUTOMERGE_REPOS_ENV)
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Production entry point: read the real process environment.
pub fn automerge_repos() -> Vec<String> {
    automerge_repos_from(|k| std::env::var(k).ok())
}

/// Resolve the OODA/engineer author login the sensor filters candidates to.
/// Unset/empty/whitespace-only => None (fail-closed: the sensor yields no
/// candidates, so it can never merge a human's PR by mistake).
pub fn automerge_author_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    lookup(SIMARD_AUTOMERGE_AUTHOR_ENV)
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Production entry point: the OODA/engineer author login the self-merge sensor
/// filters candidates to (#4097). Read ONLY from the explicit
/// `SIMARD_AUTOMERGE_AUTHOR` env var; unset/empty/whitespace-only => `None` =>
/// the sensor yields no candidates (fail-closed), exactly as the module note
/// and the pure [`automerge_author_from`] resolver promise.
///
/// There is deliberately NO ambient `gh api user` fallback. An autonomous
/// self-merge must never adopt whatever identity the daemon's `gh` token
/// happens to resolve to: if that token were authenticated as a human operator
/// (a personal `gh auth login`, a PAT in CI), the sensor would treat that
/// human's own open PRs as self-merge candidates — and the recursion guard only
/// refuses the distinct `simard-overseer[bot]` login, so it would not catch
/// them. Both self-merge gates (this author and `SIMARD_AUTOMERGE_REPOS`)
/// therefore require explicit operator opt-in.
pub fn automerge_author() -> Option<String> {
    automerge_author_from(|k| std::env::var(k).ok())
}

/// The durable machine label Simard stamps on EVERY engineer / goal-advance PR
/// at `gh pr create` time (see `prompt_assets/simard/engineer_system.md`). It is
/// the PRIMARY self-identifying marker the autonomous-self-merge sensor (#4097)
/// uses to tell Simard's OWN merge-ready PRs from the operator's own review PRs
/// when BOTH are authored by the same `gh` login (e.g. `rysweet`). The author
/// filter alone cannot separate them; this label can.
///
/// Matched EXACTLY (whole-string, case-sensitive) by [`is_engineer_pr_label`]:
/// a substring or loose match would let a spoofed look-alike label
/// (`not-simard-autonomous`, `simard-autonomous-ish`) through the gate.
pub const SIMARD_ENGINEER_PR_LABEL: &str = "simard-autonomous";

/// The environment-variable name the amplihack publish step
/// (`workflow_publish_pr.sh`, amplihack-rs #979) reads for the comma-separated
/// list of best-effort labels to stamp on a PR at `gh pr create` time.
///
/// This is the SHELL CONTRACT wire name: `workflow_publish_pr.sh` greps for the
/// literal `WORKFLOW_PR_LABELS`, so this constant's VALUE is frozen — renaming
/// the Rust identifier is fine, but changing the string would silently break the
/// contract and make every engineer PR invisible to the self-merge queue again.
///
/// Every Simard-side PR-producing spawn/recipe site sets this env var to
/// [`SIMARD_ENGINEER_PR_LABEL`] so the published PR carries the durable
/// engineer marker. The variable is INERT until amplihack-rs #979 lands the
/// consumer (`workflow_publish_pr.sh`); until then it is a harmless,
/// backward-compatible no-op (unset => existing behavior).
pub const WORKFLOW_PR_LABELS_ENV: &str = "WORKFLOW_PR_LABELS";

/// The Rust-deterministic, engineer-EXCLUSIVE head-branch namespaces. These are
/// code-generated and NEVER hand-typed by a human operator, so a head branch
/// under one of them is proof-of-Simard-origin. They are the SECONDARY
/// (defense-in-depth) marker for the case where the best-effort label was not
/// applied to an engineer PR.
///
/// - `engineer/`       — engineer worktree branches (`engineer_worktree/mod.rs`
///   builds `engineer/<goal-id>-<suffix>`).
/// - `chore/advisory-` — supply-chain remediation branches
///   (`supply_chain_steward/execute.rs` builds `chore/advisory-<advisory-id>`).
///
/// Shared prefixes (`feat/`, `fix/`, bare `chore/`) are DELIBERATELY excluded:
/// the operator uses them too, so they are non-discriminating — on a shared
/// prefix the label is the only thing that qualifies a PR. Every entry is
/// non-empty so an empty head can never `starts_with`-match (fail-closed).
pub const ENGINEER_BRANCH_PREFIXES: &[&str] = &["engineer/", "chore/advisory-"];

/// True iff `label` is EXACTLY the durable engineer-PR marker
/// [`SIMARD_ENGINEER_PR_LABEL`]. Whole-string, case-sensitive.
pub fn is_engineer_pr_label(label: &str) -> bool {
    label == SIMARD_ENGINEER_PR_LABEL
}

/// True iff `head` rides a Rust-deterministic, engineer-only branch namespace
/// (see [`ENGINEER_BRANCH_PREFIXES`]). Anchored with `starts_with` so a
/// look-alike like `engineerish/…` never matches, and — because every prefix is
/// non-empty — an empty head ref can never qualify (fail-closed).
pub fn is_engineer_branch(head: &str) -> bool {
    ENGINEER_BRANCH_PREFIXES
        .iter()
        .any(|prefix| head.starts_with(prefix))
}

// ─── agentic merge-queue reasoning scope (issue #4097) ─────────────────────
//
// The reasoning-scope gate is DELIBERATELY DISTINCT from the automerge sensor
// gates above. Those gate merge ACTION and fail CLOSED (unset ⇒ OFF). This one
// gates merge REASONING and fails OPEN-TO-THE-ROSTER (unset ⇒ reason over the
// governed roster), because the ROOT-CAUSE bug (#4097) was that an unset env
// silently disabled ALL merge reasoning. Reasoning is broad and safe (it only
// proposes); merge AUTHORIZATION stays narrow (the re-narrowing projection +
// objective + agentic gates). So broadening the reasoning scope can NEVER widen
// what is authorized to merge.

/// Operator override for the agentic merge-queue REASONING scope (#4097).
/// Three-way: UNSET/blank ⇒ reason over the governed roster (default-ON);
/// an explicit comma-separated `owner/name` list ⇒ narrowed reasoning; an
/// explicit `off`/falsey value ⇒ reasoning DISABLED (surfaced LOUD upstream,
/// never a silent OFF). Distinct from `SIMARD_AUTOMERGE_REPOS` (which gates
/// merge ACTION), so reasoning stays on even when autonomous merge is off.
pub const SIMARD_MERGE_REASONING_SCOPE_ENV: &str = "SIMARD_MERGE_REASONING_SCOPE";

/// The resolved merge-queue reasoning scope (#4097). See
/// [`merge_reasoning_scope_from`]. `Roster` carries no list — the caller uses
/// the governed roster it already loaded; `Explicit` carries the narrowed,
/// roster-intersected list in operator order; `Disabled` is the ONLY value that
/// turns reasoning off, and only on an explicit operator opt-out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeReasoningScope {
    /// Unset / blank scope: reason over the full governed roster (default-ON).
    Roster,
    /// An operator-narrowed explicit list (already intersected with the roster so
    /// a broadened reasoning scope can never name an off-roster repo). Empty when
    /// the operator list is entirely off-roster (fail-closed: nothing to scan).
    Explicit(Vec<String>),
    /// The operator EXPLICITLY disabled reasoning. Surfaced LOUD upstream.
    Disabled,
}

/// True iff `v` is an explicit REASONING-DISABLE value. Case-insensitive,
/// trimmed. Distinct from the acting-Overseer [`is_falsey`] set: it additionally
/// honors the plain word `disabled`. Only these exact values disable reasoning;
/// unset/blank/garbage never do (that is the whole point of the #4097 fix).
fn is_reasoning_disabled_value(v: &str) -> bool {
    let norm = v.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "off" | "disabled" | "0" | "false" | "no")
}

/// Resolve the merge-queue reasoning scope from an env resolver and the governed
/// `roster` (#4097).
///
/// - UNSET / blank / whitespace-only ⇒ [`MergeReasoningScope::Roster`]
///   (DEFAULT-ON — the #4097 fix: an unset env must NOT silently disable
///   reasoning as the retired automerge-allowlist gate did).
/// - An explicit `off`/`disabled`/`0`/`false`/`no` ⇒ [`MergeReasoningScope::Disabled`]
///   (the ONLY disable path, surfaced LOUD upstream).
/// - Any other value is treated as an explicit comma-separated `owner/name`
///   allowlist ⇒ [`MergeReasoningScope::Explicit`], trimmed and parsed in order,
///   then INTERSECTED with `roster` so a broadened reasoning scope can never name
///   an off-roster repo (reasoning can only narrow, never widen the roster).
pub fn merge_reasoning_scope_from(
    lookup: impl Fn(&str) -> Option<String>,
    roster: &[String],
) -> MergeReasoningScope {
    let raw = lookup(SIMARD_MERGE_REASONING_SCOPE_ENV).unwrap_or_default();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return MergeReasoningScope::Roster;
    }
    if is_reasoning_disabled_value(trimmed) {
        return MergeReasoningScope::Disabled;
    }
    let explicit: Vec<String> = trimmed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| {
            let on_roster = roster.iter().any(|r| r == s);
            if !on_roster {
                tracing::warn!(
                    target: "overseer::merge",
                    repo = %s,
                    "SIMARD_MERGE_REASONING_SCOPE names an off-roster repo — dropping \
                     (reasoning can only NARROW the governed roster, never widen it)"
                );
            }
            on_roster
        })
        .map(str::to_string)
        .collect();
    MergeReasoningScope::Explicit(explicit)
}

/// Production entry point: read the real process environment (#4097).
pub fn merge_reasoning_scope(roster: &[String]) -> MergeReasoningScope {
    merge_reasoning_scope_from(|k| std::env::var(k).ok(), roster)
}

/// Resolve the daily budget from an env resolver, falling back to
/// [`DEFAULT_DAILY_BUDGET_USD`] when the value is unset, empty, unparseable, or
/// non-positive. This is the single source of truth the [`BudgetGate`] reads.
///
/// [`BudgetGate`]: crate::overseer::BudgetGate
pub fn resolve_daily_budget_usd(lookup: impl Fn(&str) -> Option<String>) -> f64 {
    match lookup(DAILY_BUDGET_ENV).as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => match s.parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v,
            _ => DEFAULT_DAILY_BUDGET_USD,
        },
        _ => DEFAULT_DAILY_BUDGET_USD,
    }
}

/// Production entry point: read the real process environment.
pub fn daily_budget_usd() -> f64 {
    resolve_daily_budget_usd(|k| std::env::var(k).ok())
}

/// Resolve the observer cadence from an env resolver, clamped to
/// [`MIN_OVERSEER_INTERVAL_SECS`]. Unset/empty/unparseable → the default.
pub fn resolve_interval_secs(lookup: impl Fn(&str) -> Option<String>) -> u64 {
    let requested = match lookup(OVERSEER_INTERVAL_ENV).as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s.parse::<u64>().unwrap_or(DEFAULT_OVERSEER_INTERVAL_SECS),
        _ => DEFAULT_OVERSEER_INTERVAL_SECS,
    };
    requested.max(MIN_OVERSEER_INTERVAL_SECS)
}

/// Production entry point: read the real process environment.
pub fn overseer_interval_secs() -> u64 {
    resolve_interval_secs(|k| std::env::var(k).ok())
}

/// Recognise an explicit truthy env value. Case-insensitive; trims surrounding
/// whitespace. Anything else (including `0`/`false`/empty) is falsey.
fn is_truthy(v: &str) -> bool {
    let norm = v.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "1" | "true" | "yes" | "on")
}

/// Recognise an explicit falsey env value used by the **acting** Overseer's
/// opt-out gate. Case-insensitive; trims surrounding whitespace. Only these
/// exact values disable the acting Overseer; everything else (including unset,
/// empty, or garbage) leaves it enabled.
fn is_falsey(v: &str) -> bool {
    let norm = v.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "0" | "false" | "no" | "off")
}

/// Resolve whether the native Overseer **Signal operator-liaison** (issue #4911,
/// Deliverable 1) is enabled. **Default OFF** — explicit truthy required — AND
/// only while the acting Overseer itself is enabled (an explicitly-disabled
/// Overseer forces it off regardless of the liaison flag).
pub fn signal_liaison_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    if !overseer_acting_enabled_from(&lookup) {
        return false;
    }
    matches!(
        lookup(SIMARD_OVERSEER_SIGNAL_LIAISON_ENV).as_deref().map(str::trim),
        Some(v) if is_truthy(v)
    )
}

/// Production entry point: read the real process environment.
pub fn signal_liaison_enabled() -> bool {
    signal_liaison_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve whether the autonomous **PR rework loop** (issue #4911, Deliverable 2)
/// is enabled. **Default OFF** — explicit truthy required — AND master-gated by
/// the acting Overseer flag. When OFF, a fixable hold stays held-with-reason.
pub fn rework_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    if !overseer_acting_enabled_from(&lookup) {
        return false;
    }
    matches!(
        lookup(SIMARD_OVERSEER_REWORK_ENV).as_deref().map(str::trim),
        Some(v) if is_truthy(v)
    )
}

/// Production entry point: read the real process environment.
pub fn rework_enabled() -> bool {
    rework_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve the per-PR rework attempt cap. Default [`DEFAULT_REWORK_MAX_ATTEMPTS`]
/// (3); a parsed value is clamped to `1..=10`; unset/empty/unparseable falls back
/// to the default. Never panics and never yields 0 (which would disable rework
/// by stealth).
pub fn rework_max_attempts_from(lookup: impl Fn(&str) -> Option<String>) -> u32 {
    match lookup(SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS_ENV)
        .as_deref()
        .map(str::trim)
    {
        Some(s) if !s.is_empty() => match s.parse::<u32>() {
            Ok(n) => n.clamp(1, 10),
            Err(_) => DEFAULT_REWORK_MAX_ATTEMPTS,
        },
        _ => DEFAULT_REWORK_MAX_ATTEMPTS,
    }
}

/// Production entry point: read the real process environment.
pub fn rework_max_attempts() -> u32 {
    rework_max_attempts_from(|k| std::env::var(k).ok())
}

/// Resolve the configured operator E.164 the liaison accepts messages from.
/// Trimmed; `None` when unset or empty.
pub fn signal_operator_number_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    non_empty_trimmed(lookup(SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER_ENV))
}

/// Production entry point: read the real process environment.
pub fn signal_operator_number() -> Option<String> {
    signal_operator_number_from(|k| std::env::var(k).ok())
}

/// Resolve the configured operator Signal group id the liaison acts on. Trimmed;
/// `None` when unset or empty.
pub fn signal_group_id_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    non_empty_trimmed(lookup(SIMARD_OVERSEER_SIGNAL_GROUP_ID_ENV))
}

/// Production entry point: read the real process environment.
pub fn signal_group_id() -> Option<String> {
    signal_group_id_from(|k| std::env::var(k).ok())
}

/// Trim a looked-up value and return `None` if it is absent or empty.
fn non_empty_trimmed(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an injectable env resolver from a fixed map — no `std::env` mutation.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn overseer_disabled_by_default_when_unset() {
        assert!(!overseer_enabled_from(env(&[])));
    }

    #[test]
    fn overseer_enabled_only_on_explicit_truthy_values() {
        for truthy in ["1", "true", "TRUE", "True", "yes", "YES", "on", "  on  "] {
            assert!(
                overseer_enabled_from(env(&[(OVERSEER_ENABLED_ENV, truthy)])),
                "{truthy:?} should enable the Overseer"
            );
        }
    }

    #[test]
    fn overseer_stays_off_for_falsey_or_garbage_values() {
        for falsey in ["0", "false", "no", "off", "", "  ", "maybe", "2"] {
            assert!(
                !overseer_enabled_from(env(&[(OVERSEER_ENABLED_ENV, falsey)])),
                "{falsey:?} must NOT enable the Overseer"
            );
        }
    }

    #[test]
    fn acting_overseer_enabled_by_default_when_unset() {
        // Opt-OUT semantics: an unset var leaves the acting co-process ENABLED.
        assert!(overseer_acting_enabled_from(env(&[])));
    }

    #[test]
    fn acting_overseer_disabled_only_on_explicit_falsey_values() {
        for falsey in ["0", "false", "FALSE", "False", "no", "off", "  off  "] {
            assert!(
                !overseer_acting_enabled_from(env(&[(OVERSEER_ENABLED_ENV, falsey)])),
                "{falsey:?} should DISABLE the acting Overseer"
            );
        }
    }

    #[test]
    fn acting_overseer_stays_on_for_truthy_empty_or_garbage_values() {
        // Anything that is not an explicit falsey value leaves it ON, including
        // empty/whitespace (treated as unset) and unrecognised strings.
        for on in ["1", "true", "yes", "on", "", "  ", "maybe", "2", "enabled"] {
            assert!(
                overseer_acting_enabled_from(env(&[(OVERSEER_ENABLED_ENV, on)])),
                "{on:?} must leave the acting Overseer ON (default)"
            );
        }
    }

    #[test]
    fn author_login_defaults_to_the_distinct_bot_identity() {
        assert_eq!(
            overseer_author_login_from(env(&[])),
            DEFAULT_OVERSEER_AUTHOR_LOGIN
        );
        assert_eq!(
            overseer_author_login_from(env(&[(OVERSEER_AUTHOR_LOGIN_ENV, "  ")])),
            DEFAULT_OVERSEER_AUTHOR_LOGIN
        );
    }

    #[test]
    fn author_login_reads_explicit_value() {
        assert_eq!(
            overseer_author_login_from(env(&[(OVERSEER_AUTHOR_LOGIN_ENV, " simard-bot ")])),
            "simard-bot"
        );
    }

    // ─── autonomous-self-merge allowlist (issue #4097) ─────────────────────
    //
    // The `ready_prs` sensor is gated behind an explicit repo allowlist so
    // deploying the code does NOT immediately merge across every governed
    // repo. Default = EMPTY = OFF; unknown/unset must fail CLOSED.

    #[test]
    fn automerge_repos_empty_by_default_is_off() {
        // Unset => empty allowlist => autonomous self-merge OFF.
        assert!(
            automerge_repos_from(env(&[])).is_empty(),
            "unset SIMARD_AUTOMERGE_REPOS must resolve to an EMPTY allowlist (OFF)"
        );
        // Empty / whitespace-only / comma-noise all collapse to empty.
        for off in ["", "   ", ",", " , , "] {
            assert!(
                automerge_repos_from(env(&[(SIMARD_AUTOMERGE_REPOS_ENV, off)])).is_empty(),
                "{off:?} must resolve to an EMPTY allowlist (fail-closed / OFF)"
            );
        }
    }

    #[test]
    fn automerge_repos_parses_trimmed_comma_separated_slugs() {
        assert_eq!(
            automerge_repos_from(env(&[(
                SIMARD_AUTOMERGE_REPOS_ENV,
                " rysweet/Simard , rysweet/other "
            )])),
            vec!["rysweet/Simard".to_string(), "rysweet/other".to_string()],
            "comma-separated owner/name slugs are trimmed; empty entries dropped"
        );
    }

    #[test]
    fn automerge_repos_single_canary_repo() {
        // The documented canary path: enable ONE repo first.
        assert_eq!(
            automerge_repos_from(env(&[(SIMARD_AUTOMERGE_REPOS_ENV, "rysweet/Simard")])),
            vec!["rysweet/Simard".to_string()]
        );
    }

    #[test]
    fn automerge_author_none_by_default() {
        // No configured author => the sensor cannot tell its OWN PRs from a
        // human's => it must fail closed (None => empty candidate list).
        assert!(
            automerge_author_from(env(&[])).is_none(),
            "unset SIMARD_AUTOMERGE_AUTHOR must resolve to None (fail-closed)"
        );
        assert!(
            automerge_author_from(env(&[(SIMARD_AUTOMERGE_AUTHOR_ENV, "   ")])).is_none(),
            "whitespace-only author must resolve to None (fail-closed)"
        );
    }

    #[test]
    fn automerge_author_reads_trimmed_explicit_value() {
        assert_eq!(
            automerge_author_from(env(&[(SIMARD_AUTOMERGE_AUTHOR_ENV, " simard-engineer ")])),
            Some("simard-engineer".to_string()),
            "the OODA/engineer gh identity is read and trimmed"
        );
    }

    #[test]
    fn budget_falls_back_to_default_when_unset() {
        assert!(approx(
            resolve_daily_budget_usd(env(&[])),
            DEFAULT_DAILY_BUDGET_USD
        ));
    }

    #[test]
    fn budget_reads_explicit_value() {
        assert!(approx(
            resolve_daily_budget_usd(env(&[(DAILY_BUDGET_ENV, "750")])),
            750.0
        ));
        assert!(approx(
            resolve_daily_budget_usd(env(&[(DAILY_BUDGET_ENV, "  1234.5 ")])),
            1234.5
        ));
    }

    #[test]
    fn budget_falls_back_on_unparseable_empty_or_nonpositive() {
        for bad in ["abc", "", "  ", "0", "-5", "-0.01", "nan", "inf"] {
            assert!(
                approx(
                    resolve_daily_budget_usd(env(&[(DAILY_BUDGET_ENV, bad)])),
                    DEFAULT_DAILY_BUDGET_USD
                ),
                "{bad:?} must fall back to the default budget"
            );
        }
    }

    #[test]
    fn interval_defaults_when_unset() {
        assert_eq!(
            resolve_interval_secs(env(&[])),
            DEFAULT_OVERSEER_INTERVAL_SECS
        );
    }

    #[test]
    fn interval_reads_explicit_value_above_floor() {
        assert_eq!(
            resolve_interval_secs(env(&[(OVERSEER_INTERVAL_ENV, "1800")])),
            1800
        );
    }

    #[test]
    fn interval_clamps_to_floor() {
        // Below-floor / zero / garbage values never produce a hot loop.
        for below in ["1", "0", "59", "abc", ""] {
            assert!(
                resolve_interval_secs(env(&[(OVERSEER_INTERVAL_ENV, below)]))
                    >= MIN_OVERSEER_INTERVAL_SECS,
                "{below:?} must clamp to the floor"
            );
        }
        assert_eq!(
            resolve_interval_secs(env(&[(OVERSEER_INTERVAL_ENV, "1")])),
            MIN_OVERSEER_INTERVAL_SECS
        );
    }

    // ----- Claim reaper config (issue #4099) — T5 --------------------------

    #[test]
    fn claim_reap_enabled_by_default_when_unset() {
        assert!(
            claim_reap_enabled_from(env(&[])),
            "the reaper is ENABLED by default (closes the engineer_claims leak)"
        );
    }

    #[test]
    fn claim_reap_disabled_by_explicit_falsey() {
        for falsey in ["0", "false", "no", "off", "OFF", " false "] {
            assert!(
                !claim_reap_enabled_from(env(&[(SIMARD_CLAIM_REAP_ENABLED_ENV, falsey)])),
                "{falsey:?} must disable the reaper"
            );
        }
    }

    #[test]
    fn claim_reap_stays_enabled_for_truthy_or_garbage() {
        for on in ["1", "true", "yes", "on", "garbage", ""] {
            assert!(
                claim_reap_enabled_from(env(&[(SIMARD_CLAIM_REAP_ENABLED_ENV, on)])),
                "{on:?} must leave the reaper enabled (only explicit falsey disables)"
            );
        }
    }

    #[test]
    fn claim_reap_stale_secs_defaults_when_unset() {
        assert_eq!(
            claim_reap_stale_secs_from(env(&[])),
            DEFAULT_CLAIM_REAP_STALE_SECS
        );
        assert_eq!(DEFAULT_CLAIM_REAP_STALE_SECS, 1800);
    }

    #[test]
    fn claim_reap_stale_secs_honors_explicit_value() {
        assert_eq!(
            claim_reap_stale_secs_from(env(&[(SIMARD_CLAIM_REAP_STALE_SECS_ENV, "3600")])),
            3600
        );
    }

    #[test]
    fn claim_reap_stale_secs_falls_back_on_bad_values() {
        // Garbage / empty never collapse the threshold to a mass-reclaim value;
        // they fall back to the safe default.
        for bad in ["abc", "", "-5", "1.5", "   "] {
            assert_eq!(
                claim_reap_stale_secs_from(env(&[(SIMARD_CLAIM_REAP_STALE_SECS_ENV, bad)])),
                DEFAULT_CLAIM_REAP_STALE_SECS,
                "{bad:?} must fall back to the default threshold"
            );
        }
    }

    // ── engineer-PR identity gate (issue #4097, G3) ──────────────────────────
    //
    // The autonomous-self-merge sensor scopes candidates by author (G2). But
    // Simard's engineer PRs AND the operator's own review PRs are BOTH authored
    // by the same login (`rysweet`), so the author filter alone cannot tell them
    // apart. G3 adds a NARROWING engineer-PR marker: a durable machine label
    // (primary — works even on shared `feat/`/`fix/` branches) OR a
    // Rust-deterministic engineer-only branch namespace (secondary,
    // defense-in-depth). These tests pin the primitives G3 is built from.

    /// The durable machine label Simard stamps on her own engineer PRs is an
    /// EXACT, case-sensitive, whole-string constant. A substring/loosely-matched
    /// value would let a spoofed look-alike label through the gate.
    #[test]
    fn engineer_pr_label_is_the_exact_expected_constant() {
        assert_eq!(
            SIMARD_ENGINEER_PR_LABEL, "simard-autonomous",
            "the engineer-PR label must be the exact durable machine marker \
             engineers stamp at `gh pr create` time"
        );
    }

    /// The env var name the amplihack publish step (`workflow_publish_pr.sh`,
    /// amplihack-rs #979) reads for best-effort PR labels is a FROZEN wire
    /// contract. The shell consumer greps for the literal `WORKFLOW_PR_LABELS`;
    /// renaming the constant's VALUE (even while keeping the Rust identifier)
    /// would silently break the contract and make every engineer PR invisible
    /// to the self-merge queue again. Pin the exact string so a rename can't
    /// pass CI unnoticed.
    #[test]
    fn workflow_pr_labels_env_is_the_frozen_wire_name() {
        assert_eq!(
            WORKFLOW_PR_LABELS_ENV, "WORKFLOW_PR_LABELS",
            "the env var name is a shell-contract shared with \
             workflow_publish_pr.sh (amplihack-rs #979); its value must not drift"
        );
    }

    /// Only the two Rust-deterministic, engineer-EXCLUSIVE branch namespaces are
    /// recognised: `engineer/` (engineer_worktree/mod.rs) and `chore/advisory-`
    /// (supply_chain_steward/execute.rs). Both are code-generated and never used
    /// by a human operator, so a match here is proof-of-Simard-origin.
    #[test]
    fn is_engineer_branch_accepts_only_engineer_exclusive_namespaces() {
        assert!(
            is_engineer_branch("engineer/4097-abcd1234"),
            "the deterministic engineer worktree branch prefix must be recognised"
        );
        assert!(
            is_engineer_branch("chore/advisory-rustsec-2024-0001"),
            "the deterministic supply-chain remediation branch prefix must be recognised"
        );
    }

    /// The gate must EXCLUDE every branch a human operator could author. The
    /// shared `feat/`/`fix/`/bare-`chore/` prefixes are used by operators AND
    /// engineers alike, so they are NON-discriminating and must NOT qualify on
    /// their own — the label is what distinguishes an engineer PR on a shared
    /// prefix. `cogthreads/…` models the operator's own review PRs (#3142) that
    /// must NEVER auto-merge.
    #[test]
    fn is_engineer_branch_rejects_operator_and_shared_branches() {
        for head in [
            "cogthreads/some-review",      // operator review PR (#3142) — never merge
            "feat/operator-manual-change", // shared prefix, operator-authored
            "feat/4097",                   // shared prefix — non-discriminating
            "fix/typo",                    // shared prefix — non-discriminating
            "chore/bump-deps",             // bare chore/ is NOT chore/advisory-
            "main",                        // a base branch, not an engineer head
            "engineerish/not-really",      // must anchor, not substring-match
            "release/9",
        ] {
            assert!(
                !is_engineer_branch(head),
                "{head:?} is operator-reachable / non-discriminating and must NOT \
                 qualify as an engineer branch"
            );
        }
    }

    /// Empty inputs must fail closed. An empty head ref (missing field) can never
    /// be an engineer branch, and no allow-list entry may be an empty prefix
    /// (which would `starts_with`-match EVERY branch and silently re-open the
    /// gate).
    #[test]
    fn is_engineer_branch_fails_closed_on_empty_head() {
        assert!(
            !is_engineer_branch(""),
            "an empty head ref must never qualify (fail-closed)"
        );
    }

    // ── Overseer gap-scan backoff params (Problem 1 / issue #4186) ───────────
    //
    // TDD (RED) contract for the `SIMARD_OVERSEER_BACKOFF_*` env accessors that
    // parameterise the new `guardrails::BackoffGate`. They mirror the existing
    // `*_from(lookup)` + clamp pattern (`gap_scan_every_n_from`,
    // `overseer_transient_backoff_ceiling_from`): a garbage / out-of-range value
    // must never DISABLE suppression or cause overflow, so every accessor
    // fails-safe to its documented default.

    #[test]
    fn backoff_base_secs_defaults_and_honours_valid_values() {
        assert_eq!(
            overseer_backoff_base_secs_from(env(&[])),
            DEFAULT_OVERSEER_BACKOFF_BASE_SECS,
            "unset ⇒ default base window"
        );
        assert_eq!(DEFAULT_OVERSEER_BACKOFF_BASE_SECS, 900);
        assert_eq!(
            overseer_backoff_base_secs_from(env(&[(
                SIMARD_OVERSEER_BACKOFF_BASE_SECS_ENV,
                "1800"
            )])),
            1800,
            "an explicit positive value is honoured"
        );
    }

    #[test]
    fn backoff_base_secs_fails_safe_on_bad_values() {
        // Empty / whitespace / garbage / zero / negative all clamp to the
        // default — a bad base window can never collapse the suppression window
        // to 0 (which would let every duplicate through).
        for bad in ["", "   ", "abc", "0", "-5", "9.5"] {
            assert_eq!(
                overseer_backoff_base_secs_from(env(&[(
                    SIMARD_OVERSEER_BACKOFF_BASE_SECS_ENV,
                    bad
                )])),
                DEFAULT_OVERSEER_BACKOFF_BASE_SECS,
                "{bad:?} must fall back to the default base window"
            );
        }
    }

    #[test]
    fn backoff_multiplier_defaults_and_rejects_le_one() {
        assert_eq!(
            overseer_backoff_multiplier_from(env(&[])),
            DEFAULT_OVERSEER_BACKOFF_MULTIPLIER,
            "unset ⇒ default multiplier"
        );
        assert_eq!(DEFAULT_OVERSEER_BACKOFF_MULTIPLIER, 2);
        assert_eq!(
            overseer_backoff_multiplier_from(env(&[(SIMARD_OVERSEER_BACKOFF_MULTIPLIER_ENV, "3")])),
            3,
            "an explicit multiplier > 1 is honoured"
        );
        // A multiplier <= 1 would make the backoff never grow (or shrink),
        // defeating the purpose — every such value must clamp to the default.
        for bad in ["1", "0", "-2", "", "abc"] {
            assert_eq!(
                overseer_backoff_multiplier_from(env(&[(
                    SIMARD_OVERSEER_BACKOFF_MULTIPLIER_ENV,
                    bad
                )])),
                DEFAULT_OVERSEER_BACKOFF_MULTIPLIER,
                "{bad:?} must clamp to the default multiplier (reject <= 1)"
            );
        }
    }

    #[test]
    fn backoff_max_secs_defaults_and_fails_safe() {
        assert_eq!(
            overseer_backoff_max_secs_from(env(&[])),
            DEFAULT_OVERSEER_BACKOFF_MAX_SECS,
            "unset ⇒ default 24h cap"
        );
        assert_eq!(DEFAULT_OVERSEER_BACKOFF_MAX_SECS, 86_400);
        assert_eq!(
            overseer_backoff_max_secs_from(env(&[(SIMARD_OVERSEER_BACKOFF_MAX_SECS_ENV, "3600")])),
            3600,
            "an explicit positive cap is honoured"
        );
        for bad in ["", "  ", "nope", "0", "-1"] {
            assert_eq!(
                overseer_backoff_max_secs_from(env(&[(SIMARD_OVERSEER_BACKOFF_MAX_SECS_ENV, bad)])),
                DEFAULT_OVERSEER_BACKOFF_MAX_SECS,
                "{bad:?} must fall back to the default cap"
            );
        }
    }

    #[test]
    fn backoff_defaults_form_a_coherent_growing_bounded_window() {
        // base < cap and multiplier > 1 — the invariant the BackoffGate needs so
        // the window genuinely grows yet stays bounded. Enforced at compile time
        // (these are `const`s) so a future edit that breaks the invariant fails
        // the build, not just this test.
        const _: () = assert!(
            DEFAULT_OVERSEER_BACKOFF_BASE_SECS < DEFAULT_OVERSEER_BACKOFF_MAX_SECS,
            "the base window must be below the cap"
        );
        const _: () = assert!(
            DEFAULT_OVERSEER_BACKOFF_MULTIPLIER > 1,
            "the multiplier must grow the window"
        );
    }

    // ─── [standing] agentic health-review opt-out + service unit ───────────

    #[test]
    fn health_review_enabled_by_default_with_the_acting_overseer() {
        // Opt-OUT semantics: unset (acting Overseer default-on) ⇒ enabled.
        assert!(health_review_enabled_from(env(&[])));
    }

    #[test]
    fn health_review_disabled_on_explicit_falsey_values() {
        for falsey in ["0", "false", "FALSE", "no", "off", "  off  "] {
            assert!(
                !health_review_enabled_from(env(&[(SIMARD_OVERSEER_HEALTH_REVIEW_ENV, falsey)])),
                "{falsey:?} should DISABLE the health-review rail"
            );
        }
    }

    #[test]
    fn health_review_stays_on_for_truthy_empty_or_garbage_values() {
        for on in ["1", "true", "yes", "on", "", "  ", "maybe", "2"] {
            assert!(
                health_review_enabled_from(env(&[(SIMARD_OVERSEER_HEALTH_REVIEW_ENV, on)])),
                "{on:?} must leave the health-review rail ON (default)"
            );
        }
    }

    #[test]
    fn health_review_forced_off_when_the_acting_overseer_is_disabled() {
        // A disabled acting Overseer forces the rail off regardless of the flag.
        assert!(!health_review_enabled_from(env(&[
            (OVERSEER_ENABLED_ENV, "false"),
            (SIMARD_OVERSEER_HEALTH_REVIEW_ENV, "true"),
        ])));
    }

    #[test]
    fn health_review_service_unit_defaults_to_the_ooda_unit() {
        assert_eq!(
            health_review_service_unit_from(env(&[])),
            crate::install::paths::OODA_UNIT
        );
        // A blank override never yields an empty unit.
        assert_eq!(
            health_review_service_unit_from(env(&[(SIMARD_OVERSEER_HEALTH_REVIEW_UNIT_ENV, "  ")])),
            crate::install::paths::OODA_UNIT
        );
    }

    #[test]
    fn health_review_service_unit_reads_explicit_override() {
        assert_eq!(
            health_review_service_unit_from(env(&[(
                SIMARD_OVERSEER_HEALTH_REVIEW_UNIT_ENV,
                " simard-custom.service "
            )])),
            "simard-custom.service"
        );
    }
}
