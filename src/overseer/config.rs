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

/// GitHub login the acting Overseer authors its own workstreams under. Sourced
/// here so the daemon and the merge/recursion path agree on ONE stable, DISTINCT
/// identity (never the human operator's login). Defaults to
/// [`DEFAULT_OVERSEER_AUTHOR_LOGIN`] when unset.
pub const OVERSEER_AUTHOR_LOGIN_ENV: &str = "SIMARD_OVERSEER_AUTHOR_LOGIN";

/// The Overseer's well-known bot login, distinct from the engineer/OODA
/// identity. Used by the anti-recursion guard so the Overseer never
/// verifies/merges/deploys its OWN PRs and never re-opens its own goals.
pub const DEFAULT_OVERSEER_AUTHOR_LOGIN: &str = "simard-overseer[bot]";

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
}
