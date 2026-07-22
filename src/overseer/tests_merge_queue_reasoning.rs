//! TDD (Step 7) — FAILING tests that pin the AGENTIC observe/orient merge-queue
//! + issue reasoning contract (#4097).
//!
//! ROOT CAUSE these tests kill: the observe/orient stage populated
//! `ObservedState.ready_prs` from a single imperative allowlist sensor
//! (`survey_ready_prs(&automerge_repos())`). With `SIMARD_AUTOMERGE_REPOS` /
//! `SIMARD_AUTOMERGE_AUTHOR` unset in production, the allowlist is empty, the
//! sensor returns nothing, and the Overseer reasons about ZERO open PRs while a
//! CI-green merge queue piles up. Unset silently meant OFF.
//!
//! TARGET (made agentic behind a thin deterministic rail):
//!   * A recipe-driven reasoning pass (`MergeQueueReasoner`) surveys the open-PR
//!     queue + issue backlog across the governed roster and returns an OPAQUE
//!     brief that populates new `ObservedState` fields — non-empty even when the
//!     automerge env vars are unset (reasoning is DEFAULT-ON).
//!   * `merge_reasoning_scope_from` resolves scope three ways: unset ⇒ Roster
//!     (default-ON), explicit list ⇒ narrowed, `off`/falsey ⇒ Disabled (LOUD —
//!     never a silent OFF).
//!   * The brief is parsed FAIL-CLOSED (off-roster / malformed / missing entries
//!     dropped; whole-brief garbage ⇒ empty).
//!   * A re-narrowing projection re-applies the author guard + engineer-PR
//!     narrowing + objective gates so a broadened REASONING scope can NEVER widen
//!     merge AUTHORIZATION.
//!   * New `Stale`/`Duplicate`/`IssueNeedsWorkstream` signals drive gated
//!     `FlagStalePr` / `CloseDuplicatePr` interventions (RiskClass::MergeAuthority,
//!     positional argv, NEVER `--admin` / `--no-verify`).
//!
//! These tests reference the TARGET API and MUST fail to compile / fail against
//! the current tree (the new value types, `merge_reasoning_scope_from`, the
//! `merge_queue_observe` seam, the new signals/interventions, and the
//! `project_ready_prs` projection do not exist yet). They go GREEN only once the
//! feature lands. On GREEN they may be redistributed into their per-module
//! `#[cfg(test)]` blocks; the design keeps the whole contract co-located for the
//! RED phase.

use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::overseer::capabilities::{
    IssuePriority, IssueReadiness, MergeReasoningStatus, ObservedState, PrDisposition, PrRef,
    ReasonedPr, TriagedIssue,
};
use crate::overseer::config::{
    self, MergeReasoningScope, SIMARD_ENGINEER_PR_LABEL, SIMARD_MERGE_REASONING_SCOPE_ENV,
};
use crate::overseer::guardrails::{AutonomyGate, RiskClass, classify};
use crate::overseer::intervention::{Intervention, close_duplicate_pr_argv, flag_stale_pr_argv};
use crate::overseer::merge_queue_observe::{
    MergeQueueObserveRequest, MergeQueueReasoner, MergeQueueRecipeRunner, RecipeMergeQueueReasoner,
    parse_merge_queue_brief,
};
use crate::overseer::signal::{Signal, signals_from};
use crate::overseer::{ProjectionCandidate, project_ready_prs};
use crate::stewardship::PrSnapshot;
use crate::stewardship::merge_authority::CheckRollupEntry;

// ─────────────────────────── fakes / builders ──────────────────────────────

/// A `MergeQueueRecipeRunner` seam fake: records every request and returns a
/// scripted outcome, so the rail is exercised with NO subprocess, NO network,
/// NO `gh`. Mirrors the `ecosystem_observe` `FakeRunner`.
enum Scripted {
    Ok(String),
    Err(String),
}

struct FakeRunner {
    scripted: Scripted,
    calls: Mutex<Vec<MergeQueueObserveRequest>>,
}

impl FakeRunner {
    fn ok(output: &str) -> Self {
        Self {
            scripted: Scripted::Ok(output.to_string()),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn err(reason: &str) -> Self {
        Self {
            scripted: Scripted::Err(reason.to_string()),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl MergeQueueRecipeRunner for FakeRunner {
    fn run(&self, request: &MergeQueueObserveRequest) -> SimardResult<String> {
        self.calls.lock().unwrap().push(request.clone());
        match &self.scripted {
            Scripted::Ok(out) => Ok(out.clone()),
            Scripted::Err(reason) => Err(SimardError::AdapterInvocationFailed {
                base_type: "observe-merge-queue".to_string(),
                reason: reason.clone(),
            }),
        }
    }
}

/// The default reasoning scope used across parse tests — the roster trust
/// boundary. `parse_merge_queue_brief` MUST drop any entry whose `repo` is not
/// in this list.
fn scope() -> Vec<String> {
    vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()]
}

/// A PR snapshot that PASSES `evaluate_objective_gates(&snap, &["main"])`:
/// base `main`, `MERGEABLE`, all checks green, and carrying the engineer label
/// so the engineer-PR narrowing also passes.
fn green_engineer_snapshot() -> PrSnapshot {
    PrSnapshot {
        body: String::new(),
        mergeable: "MERGEABLE".to_string(),
        review_decision: "APPROVED".to_string(),
        checks: vec![CheckRollupEntry {
            name: "ci".to_string(),
            state: "SUCCESS".to_string(),
        }],
        base_ref_name: "main".to_string(),
        labels: vec![SIMARD_ENGINEER_PR_LABEL.to_string()],
    }
}

fn base_allowlist() -> Vec<String> {
    vec!["main".to_string()]
}

/// The default overseer-bot identity the anti-recursion author guard refuses.
fn overseer_login() -> String {
    config::DEFAULT_OVERSEER_AUTHOR_LOGIN.to_string()
}

// ════════════════════════════════════════════════════════════════════════════
// 1. config::merge_reasoning_scope — DEFAULT-ON / EXPLICIT / LOUD-DISABLE
//    (kills the "unset ⇒ silent OFF" invariant)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn scope_unset_defaults_on_to_roster() {
    // The whole point of the fix: unset is NOT disabled. It reasons over the
    // governed roster.
    let roster = scope();
    let resolved = config::merge_reasoning_scope_from(|_| None, &roster);
    assert!(
        matches!(resolved, MergeReasoningScope::Roster),
        "unset SIMARD_MERGE_REASONING_SCOPE must default-ON to the roster, not disable reasoning"
    );
}

#[test]
fn scope_blank_or_whitespace_defaults_on_to_roster() {
    for raw in ["", "   ", "\t\n"] {
        let resolved = config::merge_reasoning_scope_from(
            |k| lookup(k, SIMARD_MERGE_REASONING_SCOPE_ENV, raw),
            &scope(),
        );
        assert!(
            matches!(resolved, MergeReasoningScope::Roster),
            "blank scope {raw:?} must default-ON to the roster"
        );
    }
}

#[test]
fn scope_explicit_list_narrows_but_stays_on() {
    let resolved = config::merge_reasoning_scope_from(
        |k| {
            lookup(
                k,
                SIMARD_MERGE_REASONING_SCOPE_ENV,
                "rysweet/Simard, rysweet/azlin",
            )
        },
        &scope(),
    );
    match resolved {
        MergeReasoningScope::Explicit(list) => assert_eq!(
            list,
            vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()],
            "explicit scope is trimmed and parsed in order"
        ),
        other => panic!("explicit list must resolve to Explicit, got {other:?}"),
    }
}

#[test]
fn scope_off_and_falsey_values_disable_loudly() {
    // These are the ONLY values that disable — and they resolve to a distinct
    // Disabled variant (surfaced LOUD upstream), never a quiet Roster.
    for raw in ["off", "OFF", "disabled", "0", "false", "no"] {
        let resolved = config::merge_reasoning_scope_from(
            |k| lookup(k, SIMARD_MERGE_REASONING_SCOPE_ENV, raw),
            &scope(),
        );
        assert!(
            matches!(resolved, MergeReasoningScope::Disabled),
            "{raw:?} must resolve to Disabled (loud), not silently Roster"
        );
    }
}

#[test]
fn merge_reasoning_status_default_is_unknown_then_names_why_when_disabled() {
    // Additive default so existing ObservedState constructors compile unchanged.
    assert_eq!(
        MergeReasoningStatus::default(),
        MergeReasoningStatus::Unknown
    );

    // Disablement is loud: the status carries the raw reason so `simard status`
    // can name WHY reasoning is off.
    let disabled = MergeReasoningStatus::Disabled {
        reason: "SIMARD_MERGE_REASONING_SCOPE=off".to_string(),
    };
    match disabled {
        MergeReasoningStatus::Disabled { reason } => {
            assert!(reason.contains("off"), "the disabled status names WHY")
        }
        other => panic!("expected Disabled, got {other:?}"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 2. merge_queue_observe rail — routing + FAIL-CLOSED (mirrors ecosystem seam)
// ════════════════════════════════════════════════════════════════════════════

fn base_request(scope: Vec<String>) -> MergeQueueObserveRequest {
    MergeQueueObserveRequest {
        scope,
        inflight_refs: Vec::new(),
        escalation_note: String::new(),
    }
}

#[test]
fn rail_forwards_recipe_brief_verbatim() {
    let brief = r#"{"reasoned_prs":[],"triaged_issues":[]}"#;
    let reasoner = RecipeMergeQueueReasoner::new(FakeRunner::ok(brief));
    let out = reasoner
        .observe(base_request(scope()))
        .expect("a successful recipe run must not error");
    assert_eq!(
        out.as_deref(),
        Some(brief),
        "the opaque brief is forwarded verbatim; the rail never parses it"
    );
    assert_eq!(
        reasoner.runner().call_count(),
        1,
        "the recipe is invoked once"
    );
}

#[test]
fn rail_empty_scope_fails_closed_without_running_recipe() {
    let reasoner = RecipeMergeQueueReasoner::new(FakeRunner::ok("should never be used"));
    let out = reasoner
        .observe(base_request(Vec::new()))
        .expect("an empty scope is not an error");
    assert_eq!(out, None, "an empty scope fabricates no reasoning");
    assert_eq!(
        reasoner.runner().call_count(),
        0,
        "an empty scope must not invoke the recipe"
    );
}

#[test]
fn rail_runner_error_degrades_to_none() {
    let reasoner = RecipeMergeQueueReasoner::new(FakeRunner::err("recipe-runner-rs spawn failed"));
    let out = reasoner
        .observe(base_request(scope()))
        .expect("a recipe failure must degrade safely, not error out");
    assert_eq!(
        out, None,
        "a recipe failure fabricates no reasoning (fail-closed)"
    );
    assert_eq!(
        reasoner.runner().call_count(),
        1,
        "the recipe was attempted before degrading"
    );
}

#[test]
fn rail_blank_recipe_output_is_not_actionable() {
    let reasoner = RecipeMergeQueueReasoner::new(FakeRunner::ok("   \n \t "));
    let out = reasoner
        .observe(base_request(scope()))
        .expect("blank output is not an error");
    assert_eq!(out, None, "a blank recipe result is nothing actionable");
}

#[test]
fn rail_hands_scope_and_refs_to_recipe() {
    let reasoner = RecipeMergeQueueReasoner::new(FakeRunner::ok("ok"));
    let refs = vec!["pr:rysweet/Simard#4123".to_string()];
    let mut req = base_request(scope());
    req.inflight_refs = refs.clone();
    reasoner.observe(req).unwrap();

    let calls = reasoner.runner().calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].scope,
        scope(),
        "the resolved scope is handed to the recipe"
    );
    assert_eq!(
        calls[0].inflight_refs, refs,
        "in-flight refs are handed to the recipe for dedup"
    );
    assert!(
        calls[0].escalation_note.is_empty(),
        "the base pass carries no escalation note (rail-owned)"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 3. parse_merge_queue_brief — bounded schema + FAIL-CLOSED + roster boundary
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn parse_accepts_well_formed_brief() {
    let brief = r#"
    {
      "reasoned_prs": [
        {"repo":"rysweet/Simard","pr":4123,"disposition":"ready-for-merge","rationale":"CI green, MERGEABLE, approved","duplicate_of":null},
        {"repo":"rysweet/azlin","pr":88,"disposition":"stale","rationale":"no activity 40d","duplicate_of":null},
        {"repo":"rysweet/Simard","pr":4200,"disposition":"duplicate","rationale":"same fix as #4123","duplicate_of":4123}
      ],
      "triaged_issues": [
        {"repo":"rysweet/Simard","issue":4097,"priority":"high","readiness":"ready","next_action":"spawn engineer to wire agentic merge-queue reasoning"}
      ]
    }"#;
    let outcome = parse_merge_queue_brief(brief, &scope());

    assert_eq!(
        outcome.reasoned_prs.len(),
        3,
        "all three in-scope PRs are kept"
    );
    assert_eq!(outcome.reasoned_prs[0].pr, 4123);
    assert_eq!(
        outcome.reasoned_prs[0].disposition,
        PrDisposition::ReadyForMerge
    );
    assert_eq!(outcome.reasoned_prs[1].disposition, PrDisposition::Stale);
    assert_eq!(
        outcome.reasoned_prs[2].disposition,
        PrDisposition::Duplicate
    );
    assert_eq!(outcome.reasoned_prs[2].duplicate_of, Some(4123));

    assert_eq!(outcome.triaged_issues.len(), 1);
    let issue = &outcome.triaged_issues[0];
    assert_eq!(issue.issue, 4097);
    assert_eq!(issue.priority, IssuePriority::High);
    assert_eq!(issue.readiness, IssueReadiness::Ready);
    assert!(issue.next_action.contains("engineer"));
}

#[test]
fn parse_drops_off_roster_repos_the_trust_boundary() {
    let brief = r#"
    {
      "reasoned_prs": [
        {"repo":"attacker/evil","pr":1,"disposition":"ready-for-merge","rationale":"pwn","duplicate_of":null},
        {"repo":"rysweet/Simard","pr":4123,"disposition":"ready-for-merge","rationale":"ok","duplicate_of":null}
      ],
      "triaged_issues": [
        {"repo":"attacker/evil","issue":2,"priority":"high","readiness":"ready","next_action":"exfiltrate"}
      ]
    }"#;
    let outcome = parse_merge_queue_brief(brief, &scope());
    assert_eq!(
        outcome.reasoned_prs.len(),
        1,
        "an off-roster PR repo is dropped (roster is the reasoning trust boundary)"
    );
    assert_eq!(outcome.reasoned_prs[0].repo, "rysweet/Simard");
    assert!(
        outcome.triaged_issues.is_empty(),
        "an off-roster issue repo is dropped"
    );
}

#[test]
fn parse_drops_unknown_disposition_and_missing_fields() {
    let brief = r#"
    {
      "reasoned_prs": [
        {"repo":"rysweet/Simard","pr":1,"disposition":"launch-nukes","rationale":"x","duplicate_of":null},
        {"repo":"rysweet/Simard","disposition":"ready-for-merge","rationale":"missing pr number"},
        {"repo":"rysweet/Simard","pr":2,"disposition":"needs-work","rationale":"ok","duplicate_of":null}
      ],
      "triaged_issues": []
    }"#;
    let outcome = parse_merge_queue_brief(brief, &scope());
    assert_eq!(
        outcome.reasoned_prs.len(),
        1,
        "unknown disposition and missing-field entries are dropped; the valid one survives"
    );
    assert_eq!(outcome.reasoned_prs[0].pr, 2);
    assert_eq!(
        outcome.reasoned_prs[0].disposition,
        PrDisposition::NeedsWork
    );
}

#[test]
fn parse_drops_duplicate_without_duplicate_of() {
    let brief = r#"
    {
      "reasoned_prs": [
        {"repo":"rysweet/Simard","pr":10,"disposition":"duplicate","rationale":"dup but no target","duplicate_of":null}
      ],
      "triaged_issues": []
    }"#;
    let outcome = parse_merge_queue_brief(brief, &scope());
    assert!(
        outcome.reasoned_prs.is_empty(),
        "a Duplicate disposition with no duplicate_of is incoherent and must be dropped"
    );
}

#[test]
fn parse_drops_duplicate_pointing_at_itself() {
    // A PR cannot be a duplicate of itself. A self-referential pointer (agent
    // hallucination or an injected brief) must be dropped — otherwise it would
    // drive a CloseDuplicatePr that closes a legitimate PR "as a duplicate of
    // itself".
    let brief = r#"
    {
      "reasoned_prs": [
        {"repo":"rysweet/Simard","pr":10,"disposition":"duplicate","rationale":"self","duplicate_of":10}
      ],
      "triaged_issues": []
    }"#;
    let outcome = parse_merge_queue_brief(brief, &scope());
    assert!(
        outcome.reasoned_prs.is_empty(),
        "a Duplicate pointing at itself is incoherent and must be dropped"
    );
}

#[test]
fn parse_whole_brief_garbage_yields_empty_never_panics() {
    for garbage in ["", "   ", "not json at all", "{", r#"{"reasoned_prs": 42}"#] {
        let outcome = parse_merge_queue_brief(garbage, &scope());
        assert!(
            outcome.reasoned_prs.is_empty() && outcome.triaged_issues.is_empty(),
            "unparseable brief {garbage:?} must fail-closed to empty, not fabricate work or panic"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 4. Signals — StalePr / DuplicatePr / IssueNeedsWorkstream detection
//    AND the invariant: a ReadyForMerge REASONING never itself authorizes merge
// ════════════════════════════════════════════════════════════════════════════

fn state_with(reasoned: Vec<ReasonedPr>, triaged: Vec<TriagedIssue>) -> ObservedState {
    ObservedState {
        reasoned_prs: reasoned,
        triaged_issues: triaged,
        ..Default::default()
    }
}

#[test]
fn stale_disposition_produces_stale_signal() {
    let state = state_with(
        vec![ReasonedPr {
            repo: "rysweet/azlin".to_string(),
            pr: 88,
            disposition: PrDisposition::Stale,
            rationale: "no activity 40d".to_string(),
            duplicate_of: None,
        }],
        vec![],
    );
    let signals = signals_from(&state);
    assert!(
        signals.contains(&Signal::StalePrDetected {
            repo: "rysweet/azlin".to_string(),
            pr: 88,
        }),
        "a Stale reasoned PR must surface Signal::StalePrDetected; got {signals:?}"
    );
}

#[test]
fn duplicate_disposition_produces_duplicate_signal_with_original() {
    let state = state_with(
        vec![ReasonedPr {
            repo: "rysweet/Simard".to_string(),
            pr: 4200,
            disposition: PrDisposition::Duplicate,
            rationale: "same fix as #4123".to_string(),
            duplicate_of: Some(4123),
        }],
        vec![],
    );
    let signals = signals_from(&state);
    assert!(
        signals.contains(&Signal::DuplicatePrDetected {
            repo: "rysweet/Simard".to_string(),
            pr: 4200,
            duplicate_of: 4123,
        }),
        "a Duplicate reasoned PR must surface Signal::DuplicatePrDetected carrying the original; got {signals:?}"
    );
}

#[test]
fn ready_issue_produces_workstream_signal_blocked_issue_does_not() {
    let ready = TriagedIssue {
        repo: "rysweet/Simard".to_string(),
        issue: 4097,
        priority: IssuePriority::High,
        readiness: IssueReadiness::Ready,
        next_action: "spawn engineer to wire agentic reasoning".to_string(),
    };
    let blocked = TriagedIssue {
        repo: "rysweet/Simard".to_string(),
        issue: 9999,
        priority: IssuePriority::High,
        readiness: IssueReadiness::Blocked,
        next_action: "waiting on upstream".to_string(),
    };
    let signals = signals_from(&state_with(vec![], vec![ready, blocked]));

    assert!(
        signals.contains(&Signal::IssueNeedsWorkstream {
            repo: "rysweet/Simard".to_string(),
            issue: 4097,
            next_action: "spawn engineer to wire agentic reasoning".to_string(),
        }),
        "a Ready + High issue must surface Signal::IssueNeedsWorkstream; got {signals:?}"
    );
    assert!(
        !signals.iter().any(|s| matches!(
            s,
            Signal::IssueNeedsWorkstream { issue, .. } if *issue == 9999
        )),
        "a Blocked issue is NOT actionable-now and must not spawn a workstream"
    );
}

#[test]
fn ready_for_merge_reasoning_alone_does_not_authorize_merge() {
    // CRITICAL invariant: reasoning may PROPOSE, never AUTHORIZE. A ReadyForMerge
    // disposition in `reasoned_prs` — with an EMPTY `ready_prs` (i.e. it has NOT
    // been through the re-narrowing projection) — must NOT emit PrReadyToMerge.
    // Only the projected `ready_prs` view drives the merge chain.
    let state = state_with(
        vec![ReasonedPr {
            repo: "rysweet/Simard".to_string(),
            pr: 4123,
            disposition: PrDisposition::ReadyForMerge,
            rationale: "CI green".to_string(),
            duplicate_of: None,
        }],
        vec![],
    );
    assert!(
        state.ready_prs.is_empty(),
        "precondition: nothing projected yet"
    );
    let signals = signals_from(&state);
    assert!(
        !signals
            .iter()
            .any(|s| matches!(s, Signal::PrReadyToMerge { .. })),
        "a ReadyForMerge REASONING must not itself emit PrReadyToMerge — authorization comes only from the ready_prs projection; got {signals:?}"
    );
}

#[test]
fn pr_ready_signal_still_comes_from_the_projected_ready_prs_view() {
    // The existing merge chain is preserved: `ready_prs` (populated ONLY by the
    // re-narrowing projection) is what emits PrReadyToMerge.
    let state = ObservedState {
        ready_prs: vec![PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 4123,
        }],
        ..Default::default()
    };
    let signals = signals_from(&state);
    assert!(
        signals.contains(&Signal::PrReadyToMerge {
            repo: "rysweet/Simard".to_string(),
            pr: 4123,
        }),
        "the authorized ready_prs view still drives PrReadyToMerge; got {signals:?}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 5. The re-narrowing projection — reasoning is BROAD, authorization is NARROW
// ════════════════════════════════════════════════════════════════════════════

fn candidate(
    repo: &str,
    pr: u32,
    disposition: PrDisposition,
    author: &str,
    head: &str,
    snapshot: PrSnapshot,
) -> ProjectionCandidate {
    ProjectionCandidate {
        reasoned: ReasonedPr {
            repo: repo.to_string(),
            pr,
            disposition,
            rationale: "r".to_string(),
            duplicate_of: None,
        },
        author_login: author.to_string(),
        head_ref: head.to_string(),
        snapshot,
        // Default fixtures are non-draft so the pre-existing green projection
        // cases stay green; the #4339 draft tests below override this per-case.
        is_draft: Some(false),
    }
}

#[test]
fn projection_admits_a_ready_engineer_pr_that_passes_every_gate() {
    let cands = vec![candidate(
        "rysweet/Simard",
        4123,
        PrDisposition::ReadyForMerge,
        "rysweet",
        "engineer/4097-abcdef",
        green_engineer_snapshot(),
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
    assert_eq!(
        ready,
        vec![PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 4123,
        }],
        "a ReadyForMerge engineer PR passing author + engineer-PR + objective gates is authorized"
    );
}

#[test]
fn projection_excludes_non_ready_dispositions() {
    for disp in [
        PrDisposition::NeedsWork,
        PrDisposition::Stale,
        PrDisposition::Duplicate,
    ] {
        let cands = vec![candidate(
            "rysweet/Simard",
            4123,
            disp,
            "rysweet",
            "engineer/x",
            green_engineer_snapshot(),
        )];
        let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
        assert!(
            ready.is_empty(),
            "only ReadyForMerge is a merge candidate; {disp:?} must never be projected"
        );
    }
}

#[test]
fn projection_refuses_the_overseer_bots_own_pr_anti_recursion() {
    // Author guard: even a fully-green ReadyForMerge PR authored by the overseer
    // bot itself must never be projected (anti-recursion).
    let cands = vec![candidate(
        "rysweet/Simard",
        4123,
        PrDisposition::ReadyForMerge,
        &overseer_login(),
        "engineer/x",
        green_engineer_snapshot(),
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
    assert!(
        ready.is_empty(),
        "the anti-recursion author guard must exclude the overseer bot's own PR"
    );
}

#[test]
fn projection_refuses_a_pr_that_is_neither_labeled_nor_on_an_engineer_branch() {
    // Engineer-PR narrowing: an operator's own review PR (shared login, no
    // engineer label, ordinary branch) must never be projected, even green.
    let mut snap = green_engineer_snapshot();
    snap.labels = vec![]; // no simard-autonomous label
    let cands = vec![candidate(
        "rysweet/Simard",
        4123,
        PrDisposition::ReadyForMerge,
        "rysweet",
        "feature/human-typed-branch",
        snap,
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
    assert!(
        ready.is_empty(),
        "a PR that is neither labeled simard-autonomous nor on an engineer branch is an operator PR — never projected"
    );
}

#[test]
fn projection_refuses_a_pr_that_fails_the_objective_gates() {
    // Objective gate: a red / non-mergeable / off-base PR is excluded even with
    // a ReadyForMerge PROPOSAL and a valid engineer identity.
    for mutate in [
        |s: &mut PrSnapshot| s.mergeable = "CONFLICTING".to_string(),
        |s: &mut PrSnapshot| {
            s.checks = vec![CheckRollupEntry {
                name: "ci".to_string(),
                state: "FAILURE".to_string(),
            }]
        },
        |s: &mut PrSnapshot| s.base_ref_name = "some-stale-base".to_string(),
    ] {
        let mut snap = green_engineer_snapshot();
        mutate(&mut snap);
        let cands = vec![candidate(
            "rysweet/Simard",
            4123,
            PrDisposition::ReadyForMerge,
            "rysweet",
            "engineer/x",
            snap,
        )];
        let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
        assert!(
            ready.is_empty(),
            "a PR failing the objective gates must never be authorized, even with a ReadyForMerge proposal"
        );
    }
}

#[test]
fn projection_admits_via_engineer_branch_when_label_is_absent() {
    // Defense-in-depth: the engineer-EXCLUSIVE branch namespace is proof of
    // Simard origin even when the best-effort label was not applied.
    let mut snap = green_engineer_snapshot();
    snap.labels = vec![]; // rely on the branch namespace only
    let cands = vec![candidate(
        "rysweet/Simard",
        4123,
        PrDisposition::ReadyForMerge,
        "rysweet",
        "engineer/4097-fallback",
        snap,
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
    assert_eq!(
        ready.len(),
        1,
        "an engineer-exclusive branch namespace admits the PR even without the label"
    );
}

// ───────────────────── draft-PR exclusion (#4339) ────────────────────────────

/// #4339 draft guardrail (projection side): a DRAFT PR (`isDraft=true`) that
/// passes EVERY other gate — `ReadyForMerge` disposition, non-overseer author,
/// engineer branch/label, and all objective gates — must still be EXCLUDED. A
/// draft can never be merged, so the invariant must hold in BOTH producers, not
/// only in `survey_ready_prs`. Pure narrowing.
#[test]
fn projection_excludes_a_draft_pr_even_when_every_other_gate_passes() {
    let cands = vec![ProjectionCandidate {
        is_draft: Some(true),
        ..candidate(
            "rysweet/Simard",
            4336,
            PrDisposition::ReadyForMerge,
            "rysweet",
            "engineer/4336-draft",
            green_engineer_snapshot(),
        )
    }];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
    assert!(
        ready.is_empty(),
        "a draft PR must never be projected, even when disposition + author + \
         engineer-PR + objective gates all pass"
    );
}

/// #4339 counterpart: the SAME PR as non-draft (`isDraft=false`) IS projected —
/// the draft gate is a pure NARROWING that only removes drafts and must never
/// broaden merge authorization.
#[test]
fn projection_admits_identical_non_draft_pr() {
    let cands = vec![ProjectionCandidate {
        is_draft: Some(false),
        ..candidate(
            "rysweet/Simard",
            4336,
            PrDisposition::ReadyForMerge,
            "rysweet",
            "engineer/4336-draft",
            green_engineer_snapshot(),
        )
    }];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
    assert_eq!(
        ready,
        vec![PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 4336,
        }],
        "the same PR that is NOT a draft is authorized — the draft gate only \
         removes drafts, it never broadens authorization"
    );
}

/// #4339 fail-closed: when draft state is missing/unknown (`isDraft` ⇒ `None`),
/// the projection treats the PR as NOT ready and EXCLUDES it — mirroring the
/// projection's existing fail-closed posture. Admit ONLY `Some(false)`.
#[test]
fn projection_excludes_pr_with_unknown_draft_state_fail_closed() {
    let cands = vec![ProjectionCandidate {
        is_draft: None,
        ..candidate(
            "rysweet/Simard",
            4336,
            PrDisposition::ReadyForMerge,
            "rysweet",
            "engineer/4336-unknown",
            green_engineer_snapshot(),
        )
    }];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login());
    assert!(
        ready.is_empty(),
        "unknown draft state must fail closed to exclusion (admit only isDraft==Some(false))"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 6. New interventions — argv guard (NEVER --admin/--no-verify), gating, labels
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn flag_stale_pr_builds_a_positional_comment_argv() {
    let argv = flag_stale_pr_argv(
        "rysweet/azlin",
        88,
        "This PR looks stale (no activity 40d).",
    );
    assert_eq!(argv[0], "pr");
    assert_eq!(
        argv[1], "comment",
        "FlagStalePr is a comment — never a merge or close"
    );
    assert!(
        argv.iter().any(|a| a == "88"),
        "the PR number is passed positionally: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a == "rysweet/azlin"),
        "the repo slug is passed: {argv:?}"
    );
}

#[test]
fn close_duplicate_pr_builds_a_close_argv_referencing_the_original() {
    let argv = close_duplicate_pr_argv("rysweet/Simard", 4200, 4123);
    assert_eq!(argv[0], "pr");
    assert_eq!(
        argv[1], "close",
        "CloseDuplicatePr closes — via gh pr close"
    );
    assert!(
        argv.iter().any(|a| a == "4200"),
        "the duplicate PR number is passed: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a.contains("4123")),
        "the closing comment references the original (#4123): {argv:?}"
    );
}

#[test]
fn new_interventions_never_carry_admin_or_no_verify() {
    // The hard constraint, mirroring the conflict-path refusal test: no argv the
    // new interventions build may EVER contain --admin or --no-verify.
    let argvs = [
        flag_stale_pr_argv("rysweet/Simard", 1, "note"),
        close_duplicate_pr_argv("rysweet/Simard", 2, 3),
    ];
    for argv in argvs {
        assert!(
            !argv.iter().any(|a| a == "--admin"),
            "no intervention argv may contain --admin: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "--no-verify"),
            "no intervention argv may contain --no-verify: {argv:?}"
        );
    }
}

#[test]
fn new_interventions_have_stable_labels() {
    let flag = Intervention::FlagStalePr {
        repo: "rysweet/Simard".to_string(),
        pr: 1,
        note: "n".to_string(),
    };
    let close = Intervention::CloseDuplicatePr {
        repo: "rysweet/Simard".to_string(),
        pr: 2,
        duplicate_of: 3,
    };
    assert_eq!(flag.label(), "flag_stale_pr");
    assert_eq!(close.label(), "close_duplicate_pr");
}

#[test]
fn new_interventions_are_merge_authority_class() {
    let flag = Intervention::FlagStalePr {
        repo: "rysweet/Simard".to_string(),
        pr: 1,
        note: "n".to_string(),
    };
    let close = Intervention::CloseDuplicatePr {
        repo: "rysweet/Simard".to_string(),
        pr: 2,
        duplicate_of: 3,
    };
    assert_eq!(
        classify(&flag),
        RiskClass::MergeAuthority,
        "FlagStalePr acts on the merge queue — same opt-in class as VerifyAndMergePr"
    );
    assert_eq!(classify(&close), RiskClass::MergeAuthority);
}

#[test]
fn new_interventions_are_notify_only_until_opted_in() {
    let flag = Intervention::FlagStalePr {
        repo: "rysweet/Simard".to_string(),
        pr: 1,
        note: "n".to_string(),
    };
    // Default gate (allow_verify_merge = false): gated, not autonomously executed.
    let off = AutonomyGate::default();
    assert!(
        off.admit(&flag).is_err(),
        "with the MergeAuthority gate off, FlagStalePr must be notify-only (gated)"
    );
    // Opt-in flips the SAME switch as VerifyAndMergePr.
    let on = AutonomyGate {
        allow_verify_merge: true,
        ..Default::default()
    };
    assert!(
        on.admit(&flag).is_ok(),
        "opting into merge authority admits FlagStalePr for autonomous execution"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 7. Wiring / forward-compat — additive ObservedState fields, recipe resolution
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn observed_state_new_fields_are_additive_defaults() {
    // Existing constructors / the `observed_from_snapshot` projection must
    // compile unchanged: the three new fields default empty / Unknown.
    let s = ObservedState::default();
    assert!(s.reasoned_prs.is_empty());
    assert!(s.triaged_issues.is_empty());
    assert_eq!(s.merge_reasoning_status, MergeReasoningStatus::Unknown);
}

#[test]
fn merge_queue_recipe_resolves_to_the_in_tree_asset() {
    // The production runner resolves `observe-merge-queue.yaml` install-first,
    // then in-tree — mirroring the ecosystem-observe resolver. This pins that the
    // recipe asset must exist in the tree.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let resolved = crate::overseer::merge_queue_observe::resolve_merge_queue_recipe_path(
        repo_root,
        // Force in-tree resolution (ignore any ambient ~/.simard install).
        Some(std::path::Path::new("/nonexistent-home")),
    );
    assert_eq!(
        resolved,
        Some(repo_root.join("prompt_assets/simard/recipes/observe-merge-queue.yaml")),
        "the observe-merge-queue recipe must resolve to the committed in-tree asset"
    );
}

// ─────────────────────────── test-only helpers ─────────────────────────────

/// Minimal env-lookup shim: return `value` for `key == want`, else `None`.
fn lookup(key: &str, want: &str, value: &str) -> Option<String> {
    if key == want {
        Some(value.to_string())
    } else {
        None
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 8. PR reaper policy (issue #4) — pure, fail-closed, tighten-only validator
// ════════════════════════════════════════════════════════════════════════════
//
// RED-phase TDD spec for the `reaper_policy` layer that does not yet exist.
// ROOT CAUSE (issue #4): a large stale/CONFLICTING/near-duplicate open-PR
// backlog accumulates with no OODA reaper cleaning it up. The fix adds a PURE,
// deterministic, fail-closed post-parse layer that can only TIGHTEN an
// agent-proposed disposition before it reaches `AutonomyGate::admit`:
//
//   evaluate(proposed, facts, peers, thresholds, now, destructive_allowed)
//       -> ReaperDecision
//
// It performs no I/O and issues no `gh` command. Destructive `CloseDuplicate`
// stays behind `allow_verify_merge` (default dry-run/notify-only). Survivor
// selection is griefing-resistant: the lowest-numbered (earliest) PR survives.

use crate::overseer::config::{
    DEFAULT_REAPER_CONFLICTING_DAYS, DEFAULT_REAPER_SIMILARITY, DEFAULT_REAPER_STALE_DAYS,
    SIMARD_OVERSEER_REAPER_CONFLICTING_DAYS_ENV, SIMARD_OVERSEER_REAPER_SIMILARITY_ENV,
    SIMARD_OVERSEER_REAPER_STALE_DAYS_ENV, reaper_thresholds_from,
};
use crate::overseer::reaper_policy::{
    DuplicateReason, FlagStaleReason, MergeableState, PrFacts, ReaperDecision, ReaperThresholds,
    evaluate,
};
use std::collections::BTreeSet;

/// Default (shipped) thresholds: stale 14d, CONFLICTING 7d, similarity 0.85.
fn default_thresholds() -> ReaperThresholds {
    reaper_thresholds_from(|_| None)
}

fn files(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| (*p).to_string()).collect()
}

/// Build a `PrFacts` with an update time `days_ago` before `now`.
fn facts_updated_days_ago(
    number: u32,
    title: &str,
    changed: &[&str],
    days_ago: i64,
    mergeable: MergeableState,
    now: chrono::DateTime<chrono::Utc>,
) -> PrFacts {
    PrFacts {
        repo: "rysweet/Simard".to_string(),
        number,
        updated_at: Some(now - chrono::Duration::days(days_ago)),
        mergeable,
        normalized_title: title.to_string(),
        changed_files: files(changed),
        duplicate_of: None,
    }
}

// ── tighten-only contract ────────────────────────────────────────────────────

#[test]
fn evaluate_tightens_never_upgrades() {
    let now = chrono::Utc::now();
    let thresholds = default_thresholds();

    // A `Stale` proposal, even over duplicate-shaped facts with the destructive
    // gate OPEN, must NEVER escalate to a close — at most a flag (T1).
    let stale_like_dup = PrFacts {
        duplicate_of: Some(1),
        ..facts_updated_days_ago(
            9,
            "dropstopwords relevance",
            &["a.rs"],
            30,
            MergeableState::Mergeable,
            now,
        )
    };
    let survivor = facts_updated_days_ago(
        1,
        "dropstopwords relevance",
        &["a.rs"],
        40,
        MergeableState::Mergeable,
        now,
    );
    let decision = evaluate(
        PrDisposition::Stale,
        &stale_like_dup,
        &[survivor],
        &thresholds,
        now,
        true, // destructive allowed — still must not close a Stale proposal
    );
    assert!(
        !matches!(decision, ReaperDecision::CloseDuplicate { .. }),
        "a Stale proposal must never be upgraded to CloseDuplicate: {decision:?}"
    );

    // Non-reapable dispositions collapse to NoAction.
    for d in [PrDisposition::ReadyForMerge, PrDisposition::NeedsWork] {
        let f = facts_updated_days_ago(5, "t", &["a.rs"], 30, MergeableState::Mergeable, now);
        assert!(
            matches!(
                evaluate(d, &f, &[], &thresholds, now, true),
                ReaperDecision::NoAction
            ),
            "{d:?} is not a reaper disposition and must yield NoAction"
        );
    }
}

// ── stale flagging ───────────────────────────────────────────────────────────

#[test]
fn stale_flagged_only_past_threshold() {
    let now = chrono::Utc::now();
    let thresholds = default_thresholds(); // stale 14d

    // 30 days stale (> 14) ⇒ flag.
    let old = facts_updated_days_ago(10, "t", &["a.rs"], 30, MergeableState::Mergeable, now);
    assert!(
        matches!(
            evaluate(PrDisposition::Stale, &old, &[], &thresholds, now, false),
            ReaperDecision::Flag(FlagStaleReason::StaleNoUpdate)
        ),
        "a PR untouched for 30d (> 14d) must be flagged StaleNoUpdate"
    );

    // 3 days (< 14) ⇒ no action.
    let fresh = facts_updated_days_ago(11, "t", &["a.rs"], 3, MergeableState::Mergeable, now);
    assert!(
        matches!(
            evaluate(PrDisposition::Stale, &fresh, &[], &thresholds, now, false),
            ReaperDecision::NoAction
        ),
        "a PR updated 3d ago is within the stale window ⇒ NoAction"
    );
}

#[test]
fn long_conflicting_flagged() {
    let now = chrono::Utc::now();
    let thresholds = default_thresholds(); // stale 14d, conflicting 7d

    // CONFLICTING for 10 days: past the 7d conflicting bar but INSIDE the 14d
    // stale bar — so the flag reason must be LongConflicting, not StaleNoUpdate.
    let conflicting =
        facts_updated_days_ago(12, "t", &["a.rs"], 10, MergeableState::Conflicting, now);
    assert!(
        matches!(
            evaluate(
                PrDisposition::Stale,
                &conflicting,
                &[],
                &thresholds,
                now,
                false
            ),
            ReaperDecision::Flag(FlagStaleReason::LongConflicting)
        ),
        "a PR CONFLICTING for 10d (> 7d, < 14d) must be flagged LongConflicting"
    );
}

// ── duplicate handling ───────────────────────────────────────────────────────

#[test]
fn no_close_on_title_similarity_alone() {
    let now = chrono::Utc::now();
    let thresholds = default_thresholds();

    // Identical normalized titles (similarity 1.0 >= 0.85) but DISJOINT changed
    // files ⇒ never a close (R3 griefing: title alone is not evidence). T3.
    let candidate = PrFacts {
        duplicate_of: Some(20),
        ..facts_updated_days_ago(
            21,
            "same normalized title",
            &["b.rs"],
            1,
            MergeableState::Mergeable,
            now,
        )
    };
    let survivor = facts_updated_days_ago(
        20,
        "same normalized title",
        &["a.rs"],
        1,
        MergeableState::Mergeable,
        now,
    );
    assert!(
        matches!(
            evaluate(
                PrDisposition::Duplicate,
                &candidate,
                &[survivor],
                &thresholds,
                now,
                true
            ),
            ReaperDecision::NoAction
        ),
        "matching titles with no changed-file overlap must never close a PR"
    );
}

#[test]
fn duplicate_close_requires_overlap_and_optin() {
    let now = chrono::Utc::now();
    let thresholds = default_thresholds();

    let candidate = PrFacts {
        duplicate_of: Some(30),
        ..facts_updated_days_ago(
            31,
            "dup title",
            &["a.rs", "b.rs"],
            1,
            MergeableState::Mergeable,
            now,
        )
    };
    let survivor = facts_updated_days_ago(
        30,
        "dup title",
        &["a.rs", "b.rs"],
        2,
        MergeableState::Mergeable,
        now,
    );

    // Overlap + destructive gate OPEN ⇒ propose a close of the later PR (31)
    // as a duplicate of the survivor (30).
    match evaluate(
        PrDisposition::Duplicate,
        &candidate,
        std::slice::from_ref(&survivor),
        &thresholds,
        now,
        true,
    ) {
        ReaperDecision::CloseDuplicate {
            number,
            survivor: s,
            reason,
        } => {
            assert_eq!(
                number, 31,
                "the close candidate is the later (higher-numbered) PR"
            );
            assert_eq!(s, 30, "the survivor is the earlier (lower-numbered) PR");
            assert!(matches!(reason, DuplicateReason::TitleAndFileOverlap));
        }
        other => panic!("overlap + opt-in must propose a CloseDuplicate, got {other:?}"),
    }

    // Overlap but destructive gate CLOSED ⇒ downgrade to a non-destructive flag
    // that names the REAL reason (DuplicateNotClosable), never a stray close (T4).
    assert!(
        matches!(
            evaluate(
                PrDisposition::Duplicate,
                &candidate,
                &[survivor],
                &thresholds,
                now,
                false
            ),
            ReaperDecision::Flag(FlagStaleReason::DuplicateNotClosable)
        ),
        "with the destructive gate closed, a duplicate downgrades to a flag, not a close"
    );
}

#[test]
fn fail_closed_on_unknown_mergeable_or_timestamp() {
    let now = chrono::Utc::now();
    let thresholds = default_thresholds();

    // Unknown mergeable ⇒ never eligible for a duplicate close (T5).
    let unknown_mergeable = PrFacts {
        duplicate_of: Some(40),
        ..facts_updated_days_ago(41, "dup", &["a.rs"], 1, MergeableState::Unknown, now)
    };
    let survivor = facts_updated_days_ago(40, "dup", &["a.rs"], 1, MergeableState::Unknown, now);
    assert!(
        !matches!(
            evaluate(
                PrDisposition::Duplicate,
                &unknown_mergeable,
                &[survivor],
                &thresholds,
                now,
                true
            ),
            ReaperDecision::CloseDuplicate { .. }
        ),
        "an Unknown mergeable state must never produce a destructive close"
    );

    // Missing (unparseable) timestamp ⇒ a Stale proposal cannot be flagged.
    let no_ts = PrFacts {
        updated_at: None,
        ..facts_updated_days_ago(42, "t", &["a.rs"], 99, MergeableState::Mergeable, now)
    };
    assert!(
        matches!(
            evaluate(PrDisposition::Stale, &no_ts, &[], &thresholds, now, false),
            ReaperDecision::NoAction
        ),
        "a None timestamp is fail-closed: no stale flag without an age"
    );
}

#[test]
fn survivor_is_lowest_numbered_pr() {
    // Griefing resistance: an attacker who opens a near-duplicate AFTER a
    // legitimate PR must not be able to close the legitimate (earlier) one. The
    // survivor is deterministically the lowest-numbered PR; the later PR is the
    // close candidate.
    let now = chrono::Utc::now();
    let thresholds = default_thresholds();

    let legit = facts_updated_days_ago(
        100,
        "shared title",
        &["x.rs"],
        5,
        MergeableState::Mergeable,
        now,
    );
    let attacker = PrFacts {
        duplicate_of: Some(100),
        ..facts_updated_days_ago(
            205,
            "shared title",
            &["x.rs"],
            1,
            MergeableState::Mergeable,
            now,
        )
    };

    // Evaluating the LATER PR (205) ⇒ it is the close candidate, 100 survives.
    match evaluate(
        PrDisposition::Duplicate,
        &attacker,
        std::slice::from_ref(&legit),
        &thresholds,
        now,
        true,
    ) {
        ReaperDecision::CloseDuplicate {
            number, survivor, ..
        } => {
            assert_eq!(number, 205);
            assert_eq!(survivor, 100, "the earlier PR must survive");
        }
        other => panic!("expected the later PR to be the close candidate, got {other:?}"),
    }

    // Evaluating the EARLIER PR (100) with the later peer ⇒ 100 is the survivor,
    // so it is NEVER the close candidate (no self-close of the legitimate PR).
    let legit_as_candidate = PrFacts {
        duplicate_of: Some(205),
        ..legit.clone()
    };
    assert!(
        !matches!(
            evaluate(
                PrDisposition::Duplicate,
                &legit_as_candidate,
                &[attacker],
                &thresholds,
                now,
                true
            ),
            ReaperDecision::CloseDuplicate { .. }
        ),
        "the lowest-numbered (earliest) PR must never be closed as the duplicate"
    );
}

// ── dry-run gate + argv safety (end-to-end intent) ───────────────────────────

#[test]
fn dry_run_gate_default_is_notify_only() {
    let now = chrono::Utc::now();
    let thresholds = default_thresholds();

    let candidate = PrFacts {
        duplicate_of: Some(50),
        ..facts_updated_days_ago(51, "dup", &["a.rs"], 1, MergeableState::Mergeable, now)
    };
    let survivor = facts_updated_days_ago(50, "dup", &["a.rs"], 1, MergeableState::Mergeable, now);

    // With the default (dry-run) gate closed, evaluate() itself never yields a
    // destructive close ...
    let decision = evaluate(
        PrDisposition::Duplicate,
        &candidate,
        &[survivor],
        &thresholds,
        now,
        false,
    );
    assert!(
        !matches!(decision, ReaperDecision::CloseDuplicate { .. }),
        "default dry-run: reaper_policy must not emit a CloseDuplicate"
    );

    // ... and even a (hypothetical) CloseDuplicatePr intervention stays gated by
    // the SAME notify-only MergeAuthority switch, so nothing is closed unattended.
    let close = Intervention::CloseDuplicatePr {
        repo: "rysweet/Simard".to_string(),
        pr: 51,
        duplicate_of: 50,
    };
    assert!(
        AutonomyGate::default().admit(&close).is_err(),
        "CloseDuplicatePr stays MergeAuthority-gated under the default (dry-run) autonomy gate"
    );
}

#[test]
fn reaper_close_argv_never_contains_admin_or_no_verify() {
    let now = chrono::Utc::now();
    let thresholds = default_thresholds();

    let candidate = PrFacts {
        duplicate_of: Some(60),
        ..facts_updated_days_ago(61, "dup", &["a.rs"], 1, MergeableState::Mergeable, now)
    };
    let survivor = facts_updated_days_ago(60, "dup", &["a.rs"], 1, MergeableState::Mergeable, now);

    // Whatever close the reaper proposes must build a positional argv that can
    // NEVER carry a force-flag — no PR-controlled text reaches the command line.
    if let ReaperDecision::CloseDuplicate {
        number,
        survivor: s,
        ..
    } = evaluate(
        PrDisposition::Duplicate,
        &candidate,
        &[survivor],
        &thresholds,
        now,
        true,
    ) {
        let argv = close_duplicate_pr_argv("rysweet/Simard", number, s);
        assert!(
            !argv.iter().any(|a| a == "--admin"),
            "reaper close argv must not contain --admin: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "--no-verify"),
            "reaper close argv must not contain --no-verify: {argv:?}"
        );
    } else {
        panic!("expected a CloseDuplicate to build a close argv for the safety assertion");
    }
}

// ── config resolver: conservative defaults + clamps ──────────────────────────

#[test]
fn thresholds_resolver_defaults_and_clamps() {
    // Unset ⇒ shipped defaults.
    let d = reaper_thresholds_from(|_| None);
    assert_eq!(d.stale_days, DEFAULT_REAPER_STALE_DAYS);
    assert_eq!(d.stale_days, 14);
    assert_eq!(d.conflicting_days, DEFAULT_REAPER_CONFLICTING_DAYS);
    assert_eq!(d.conflicting_days, 7);
    assert!((d.similarity - DEFAULT_REAPER_SIMILARITY).abs() < f64::EPSILON);
    assert!((d.similarity - 0.85).abs() < f64::EPSILON);

    // Day thresholds clamp UP to a floor of 1 (a 0-day threshold is unsafe).
    let zero_days = reaper_thresholds_from(|k| match k {
        _ if k == SIMARD_OVERSEER_REAPER_STALE_DAYS_ENV => Some("0".to_string()),
        _ if k == SIMARD_OVERSEER_REAPER_CONFLICTING_DAYS_ENV => Some("0".to_string()),
        _ => None,
    });
    assert!(
        zero_days.stale_days >= 1,
        "stale_days must clamp to a floor of 1"
    );
    assert!(
        zero_days.conflicting_days >= 1,
        "conflicting_days must clamp to a floor of 1"
    );

    // Similarity clamps into [0.0, 1.0].
    let hi = reaper_thresholds_from(|k| {
        (k == SIMARD_OVERSEER_REAPER_SIMILARITY_ENV).then(|| "2.0".to_string())
    });
    assert!(
        hi.similarity <= 1.0,
        "similarity above 1.0 must clamp to 1.0"
    );
    let lo = reaper_thresholds_from(|k| {
        (k == SIMARD_OVERSEER_REAPER_SIMILARITY_ENV).then(|| "-1".to_string())
    });
    assert!(
        lo.similarity >= 0.0,
        "negative similarity must clamp to 0.0"
    );

    // Garbage ⇒ conservative defaults, never a permissive value.
    let garbage = reaper_thresholds_from(|k| match k {
        _ if k == SIMARD_OVERSEER_REAPER_STALE_DAYS_ENV => Some("abc".to_string()),
        _ if k == SIMARD_OVERSEER_REAPER_SIMILARITY_ENV => Some("not-a-float".to_string()),
        _ => None,
    });
    assert_eq!(garbage.stale_days, DEFAULT_REAPER_STALE_DAYS);
    assert!((garbage.similarity - DEFAULT_REAPER_SIMILARITY).abs() < f64::EPSILON);
}

#[test]
fn reaper_telemetry_names_follow_the_dotted_otel_house_style() {
    // Decision + downgrade counters and the decision attribute key follow the
    // house `simard.<subsystem>.<event>` / short-attr convention (names.rs).
    assert_eq!(
        crate::telemetry::names::OVERSEER_REAPER_DECISION,
        "simard.overseer.reaper_decision"
    );
    assert_eq!(
        crate::telemetry::names::OVERSEER_REAPER_DOWNGRADED,
        "simard.overseer.reaper_downgraded"
    );
    assert_eq!(crate::telemetry::names::ATTR_DECISION, "decision");
}
