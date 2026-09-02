//! M1 — the Overseer's **read-only observer** surface.
//!
//! M1 is the "observe → orient → report → file deduped issues" milestone. It
//! deliberately takes **no write action beyond issue-filing**: it never launches
//! recipes, verifies/merges PRs, resolves conflicts, deploys, or transfers goals.
//! This module supplies the two M1-specific pieces on top of the shared
//! Observe/Orient vocabulary in [`signal`](crate::overseer::signal):
//!
//! - [`StewardshipIssueFiler`] — a real [`IssueFiler`] backed by the shipped
//!   stewardship loop (`crate::stewardship::process_orchestrator_run`), so
//!   recurring failures produce a single **deduplicated** GitHub issue and
//!   re-observing the same failure is idempotent.
//! - [`decide_read_only`] + [`is_m1_permitted`] — the M1 restriction of `decide`
//!   that maps every `Problem` to a write-free intervention, with a
//!   machine-checkable predicate proving M1's exit criterion.
//!
//! Reuse map (see `docs/design/overseer.md` §capability table):
//! `stewardship::process_orchestrator_run` (`src/stewardship/mod.rs:51`) and
//! dedup via `stewardship::failure_signature` (`src/stewardship/dedup.rs`).
//! Filed issues are not fed back into the goal board.

use std::sync::Arc;

use crate::overseer::capabilities::{
    IssueFiler, IssueOutcome, OrchestratorRunBrief, OverseerError,
};
use crate::overseer::intervention::Intervention;
use crate::overseer::signal::{Problem, ProblemKind, Signal};
use crate::stewardship::{
    GhClient, OrchestratorRunSummary, StewardshipOutcome, failure_signature,
    process_orchestrator_run,
};

/// A real [`IssueFiler`] backed by Simard's stewardship loop. Wraps
/// `stewardship::process_orchestrator_run`, which searches the routed repo for
/// an OPEN issue carrying the same `failure_signature` and files a new one only
/// when none exists — so repeated Observe cycles over the same failure are
/// idempotent (`FiledNew` once, `MatchedExisting` thereafter).
///
/// The `gh` handle is the only network surface (a `RealGhClient` in the daemon,
/// a fake in tests).
pub struct StewardshipIssueFiler {
    gh: Arc<dyn GhClient + Send + Sync>,
}

impl StewardshipIssueFiler {
    /// Construct an issue filer over the supplied GitHub client.
    pub fn new(gh: Arc<dyn GhClient + Send + Sync>) -> Self {
        Self { gh }
    }
}

impl IssueFiler for StewardshipIssueFiler {
    fn file(&self, run: &OrchestratorRunBrief) -> Result<IssueOutcome, OverseerError> {
        let summary = brief_to_summary(run);
        let outcome = process_orchestrator_run(&summary, self.gh.as_ref()).map_err(|e| {
            OverseerError::Capability {
                what: "file_issue",
                detail: e.to_string(),
            }
        })?;
        Ok(match outcome {
            StewardshipOutcome::FiledNew { url, .. } => IssueOutcome::FiledNew { url },
            StewardshipOutcome::MatchedExisting { url, .. } => {
                IssueOutcome::MatchedExisting { url }
            }
        })
    }
}

/// Convert an Overseer [`OrchestratorRunBrief`] into a stewardship
/// [`OrchestratorRunSummary`]. The brief carries no `run_id`; we synthesise a
/// STABLE one from the failure signature so re-observing the same failure keeps
/// the same run identity and dedups to the same issue (the signature itself
/// depends only on `failure_kind` + `error_text`).
pub fn brief_to_summary(run: &OrchestratorRunBrief) -> OrchestratorRunSummary {
    let signature = failure_signature(&run.failure_kind, &run.error_text);
    OrchestratorRunSummary {
        run_id: format!("overseer-{signature}"),
        recipe_name: run.recipe_name.clone(),
        failed_step: run.failed_step.clone(),
        source_module: run.source_module.clone(),
        failure_kind: run.failure_kind.clone(),
        error_text: run.error_text.clone(),
    }
}

/// The M1 restriction of `decide`: every `Problem` maps to a **write-free**
/// intervention. Recurring *defects* (process-health, quality regressions, goal
/// hygiene, cross-cutting) become a single DEDUPLICATED
/// [`Intervention::FileIssue`] — the read-only observer's durable finding
/// (stewardship rule: "durable findings are GitHub issues or code"). Transient
/// operational pressure and positive delivery states are surfaced in the
/// periodic [`Intervention::Report`] instead of filing an issue. Unlike the
/// full `decide`, M1 NEVER emits `LaunchRecipe`, `VerifyAndMergePr`,
/// `ResolveConflict`, `Deploy`, or `TransferGoal` — autonomy is earned in later
/// milestones, not defaulted.
pub fn decide_read_only(problem: &Problem) -> Intervention {
    match problem.kind {
        // Recurring defects the operator should track → one deduplicated issue.
        ProblemKind::ProcessHealth
        | ProblemKind::QualityRegression
        | ProblemKind::GoalHygiene
        | ProblemKind::StepFailure
        | ProblemKind::CrossCutting => Intervention::FileIssue {
            run: problem_to_run_brief(problem),
        },
        // Transient resource pressure / positive delivery signals — report, do
        // not file. Filing per-cycle issues for e.g. momentary budget pressure
        // would be noise; the value is in the rolled-up Report. Loop/drift
        // conditions are advisory: the read-only sensor surfaces them in the
        // Report (the acting Overseer whispers; M1 does not).
        ProblemKind::ResourcePressure
        | ProblemKind::DeliveryReady
        | ProblemKind::LoopDetected
        | ProblemKind::DriftCorrection
        // Backlog-coverage gaps are acted on by the acting Overseer (notify +
        // deduped file). The read-only M1 sensor never surveys gaps, so this is
        // unreachable in M1 — surface it in the Report if it ever appears.
        | ProblemKind::WorkstreamCoverage
        // Deploy drift is a HIGH-RISK acting-Overseer concern (guarded
        // self-deploy). The read-only M1 sensor never deploys — surface it in
        // the Report if it ever appears here.
        | ProblemKind::DeployDrift => Intervention::Report,
    }
}

/// Build the deduplicated stewardship brief for a filed problem. The brief is
/// consumed by [`StewardshipIssueFiler`] → `stewardship::process_orchestrator_run`,
/// which routes on `source_module` and dedups on
/// `failure_signature(failure_kind, error_text)`.
///
/// The `dedup_key` is folded through [`fold_volatile_goal_ids`] before it flows
/// into BOTH `failure_kind` and the error text (process_health): a re-block
/// finding embeds a volatile goal identifier (`simard-identity-<slug>` /
/// positional `goal-<n>`), so without folding every re-observation of the SAME
/// underlying cause produced a fresh `failure_signature` and filed a duplicate
/// `recurring_goal_reblock in simard::overseer` issue — the storm this ends.
fn problem_to_run_brief(problem: &Problem) -> OrchestratorRunBrief {
    OrchestratorRunBrief {
        recipe_name: "overseer-observer".to_string(),
        failed_step: kind_step_label(problem.kind).to_string(),
        source_module: routable_source_module(problem),
        failure_kind: fold_volatile_goal_ids(&problem.dedup_key),
        error_text: stable_error_text(problem),
    }
}

/// A `source_module` string that `stewardship::route_failure` always resolves
/// (never `StewardshipRoutingAmbiguous`). CI-failure clusters route to the
/// affected repo; every other Overseer finding is about Simard's OWN process and
/// routes to Simard. The token is a FIXED routable prefix, so free-form dedup
/// keys (e.g. anomaly text) can never accidentally re-route the issue.
fn routable_source_module(problem: &Problem) -> String {
    for s in &problem.evidence {
        if let Signal::CiFailureCluster { repo, .. } = s
            && repo.to_ascii_lowercase().contains("amplihack")
        {
            return "amplihack::overseer".to_string();
        }
    }
    "simard::overseer".to_string()
}

/// A short, stable phase label per problem kind (the stewardship `failed_step`).
fn kind_step_label(kind: ProblemKind) -> &'static str {
    match kind {
        ProblemKind::ProcessHealth => "process_health",
        ProblemKind::ResourcePressure => "resource_pressure",
        ProblemKind::DeliveryReady => "delivery_ready",
        ProblemKind::QualityRegression => "quality_regression",
        ProblemKind::GoalHygiene => "goal_hygiene",
        ProblemKind::CrossCutting => "cross_cutting",
        ProblemKind::LoopDetected => "loop_detected",
        ProblemKind::DriftCorrection => "drift_correction",
        ProblemKind::WorkstreamCoverage => "workstream_coverage",
        ProblemKind::StepFailure => "step_failure",
        ProblemKind::DeployDrift => "deploy_drift",
    }
}

/// STABLE error text (no fluctuating metric values) so `failure_signature` folds
/// every recurrence of the same problem into ONE deduplicated issue. Live metric
/// values live in the periodic Report / `simard status` telemetry, not the issue
/// body. Keyed on the (already stable) `dedup_key` — with volatile goal
/// identifiers folded via [`fold_volatile_goal_ids`] so re-block recurrences of
/// the same cause collapse to one signature — plus the evidence signal kinds
/// (invariant across observation cycles for a given problem).
fn stable_error_text(problem: &Problem) -> String {
    format!(
        "Overseer read-only observer detected a recurring {kind:?} problem \
         (dedup key `{key}`; evidence: {kinds}). Filed once per recurring \
         signature — see `simard status` telemetry for current values.",
        kind = problem.kind,
        key = fold_volatile_goal_ids(&problem.dedup_key),
        kinds = evidence_kind_labels(&problem.evidence),
    )
}

/// Fold **volatile goal identifiers** in a stewardship `dedup_key` to stable
/// placeholders so recurrences of the SAME re-block cause collapse to ONE
/// `failure_signature` (process_health). Two shapes are folded:
///
/// * `simard-identity-<slug>` → `simard-identity-*` (the codename identity goals,
///   whose slug is a volatile lowercase-and-hyphen codename), and
/// * positional `goal-<n>` (a run of ASCII digits) → `goal-*`.
///
/// Everything else is returned **byte-for-byte** — the fold is deliberately
/// conservative so two *genuinely different* causes never over-collapse into one
/// issue. `goal-` NOT followed by a digit (e.g. `coverage-goal-parity`) and a
/// bare `identity` are left untouched. Pure and total; no `regex` dependency (a
/// single forward scan) so it is always compiled in.
pub fn fold_volatile_goal_ids(dedup_key: &str) -> String {
    const IDENTITY_PREFIX: &str = "simard-identity-";
    const GOAL_PREFIX: &str = "goal-";
    // A slug byte: the lowercase-and-hyphen (plus defensive alphanumeric) run
    // that makes up a codename identity slug. Terminated by a space, `:` etc.
    fn is_slug_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'-'
    }

    let bytes = dedup_key.as_bytes();
    let mut out = String::with_capacity(dedup_key.len());
    let mut i = 0;
    while i < bytes.len() {
        let rest = &dedup_key[i..];
        if let Some(slug) = rest.strip_prefix(IDENTITY_PREFIX) {
            let slug_len = slug.bytes().take_while(|&b| is_slug_byte(b)).count();
            if slug_len > 0 {
                out.push_str("simard-identity-*");
                i += IDENTITY_PREFIX.len() + slug_len;
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix(GOAL_PREFIX) {
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 {
                out.push_str("goal-*");
                i += GOAL_PREFIX.len() + digits;
                continue;
            }
        }
        // Default: copy exactly one UTF-8 scalar, preserving char boundaries.
        let ch = rest.chars().next().expect("non-empty remainder");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// De-duplicated, order-stable list of the signal *variant* names backing a
/// problem (values omitted — only kinds, so the string is invariant per cycle).
fn evidence_kind_labels(evidence: &[Signal]) -> String {
    let mut labels: Vec<&'static str> = evidence.iter().map(signal_kind_label).collect();
    labels.sort_unstable();
    labels.dedup();
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join(", ")
    }
}

/// Stable variant name for a signal (no embedded values).
fn signal_kind_label(s: &Signal) -> &'static str {
    match s {
        Signal::DistillFailureRate { .. } => "DistillFailureRate",
        Signal::RestartChurn { .. } => "RestartChurn",
        Signal::LadderExhausted { .. } => "LadderExhausted",
        Signal::BudgetPressure { .. } => "BudgetPressure",
        Signal::EngineerSpawnRate { .. } => "EngineerSpawnRate",
        Signal::MemoryGrowth { .. } => "MemoryGrowth",
        Signal::GymSkipped => "GymSkipped",
        Signal::CiFailureCluster { .. } => "CiFailureCluster",
        Signal::PrReadyToMerge { .. } => "PrReadyToMerge",
        Signal::StaleGoal { .. } => "StaleGoal",
        Signal::Anomaly { .. } => "Anomaly",
        Signal::LoopDetected { .. } => "LoopDetected",
        Signal::DriftCorrection { .. } => "DriftCorrection",
        Signal::GoalBlocked { .. } => "GoalBlocked",
        Signal::RecurringSignature { .. } => "RecurringSignature",
        Signal::WorkstreamGap { .. } => "WorkstreamGap",
        Signal::StepFailureDiagnosed { .. } => "StepFailureDiagnosed",
        Signal::StalePrDetected { .. } => "StalePrDetected",
        Signal::DuplicatePrDetected { .. } => "DuplicatePrDetected",
        Signal::IssueNeedsWorkstream { .. } => "IssueNeedsWorkstream",
        Signal::DeployDriftDetected { .. } => "DeployDriftDetected",
    }
}

/// True iff `iv` performs **no write action beyond deduplicated issue-filing** —
/// the machine-checkable invariant behind M1's exit criterion ("provably takes
/// no write action beyond issue-filing"). Only [`Intervention::Report`],
/// [`Intervention::FileIssue`], and [`Intervention::Escalate`] (notify-only) are
/// permitted; every other intervention launches, merges, resolves, deploys, or
/// transfers and is therefore an M2+ action.
pub fn is_m1_permitted(iv: &Intervention) -> bool {
    matches!(
        iv,
        Intervention::Report | Intervention::FileIssue { .. } | Intervention::Escalate { .. }
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::error::{SimardError, SimardResult};
    use crate::overseer::decide;
    use crate::overseer::signal::Priority;
    use crate::stewardship::GhIssue;

    /// Stateful, **no-network** fake: `create_issue` registers the issue so a
    /// later `search_issues` for the same signature returns it — exactly how the
    /// real `gh` would behave across two Observe cycles. No re-seeding needed,
    /// so idempotency is exercised end-to-end through the adapter.
    #[derive(Default)]
    struct StatefulFakeGh {
        issues: Mutex<Vec<GhIssue>>,
        next_number: Mutex<u64>,
        search_calls: Mutex<usize>,
        create_calls: Mutex<usize>,
    }

    impl StatefulFakeGh {
        fn new() -> Self {
            Self {
                next_number: Mutex::new(1),
                ..Default::default()
            }
        }
        fn search_calls(&self) -> usize {
            *self.search_calls.lock().unwrap()
        }
        fn create_calls(&self) -> usize {
            *self.create_calls.lock().unwrap()
        }
    }

    impl GhClient for StatefulFakeGh {
        fn search_issues(&self, _repo: &str, signature: &str) -> SimardResult<Vec<GhIssue>> {
            *self.search_calls.lock().unwrap() += 1;
            let needle = format!("stewardship-signature: {signature}");
            Ok(self
                .issues
                .lock()
                .unwrap()
                .iter()
                .filter(|i| i.body.contains(&needle))
                .cloned()
                .collect())
        }
        fn create_issue(&self, repo: &str, title: &str, body: &str) -> SimardResult<GhIssue> {
            *self.create_calls.lock().unwrap() += 1;
            let number = {
                let mut n = self.next_number.lock().unwrap();
                let cur = *n;
                *n += 1;
                cur
            };
            let issue = GhIssue {
                number,
                url: format!("https://github.com/{repo}/issues/{number}"),
                title: title.to_string(),
                body: body.to_string(),
            };
            self.issues.lock().unwrap().push(issue.clone());
            Ok(issue)
        }
    }

    /// A fake whose `search_issues` always errors — used to prove capability
    /// failures surface as `OverseerError::Capability`, not panics.
    struct FailingGh;
    impl GhClient for FailingGh {
        fn search_issues(&self, _repo: &str, _signature: &str) -> SimardResult<Vec<GhIssue>> {
            Err(SimardError::StewardshipGhCommandFailed {
                reason: "boom".to_string(),
            })
        }
        fn create_issue(&self, _repo: &str, _title: &str, _body: &str) -> SimardResult<GhIssue> {
            unreachable!("create must not be called after a search failure")
        }
    }

    fn sample_brief() -> OrchestratorRunBrief {
        OrchestratorRunBrief {
            recipe_name: "smart-orchestrator".to_string(),
            failed_step: "distill".to_string(),
            source_module: "simard::engineer_loop".to_string(),
            failure_kind: "PanicInStep".to_string(),
            error_text: "panic at /home/user/src/foo.rs:42:7\nbacktrace deadbeef".to_string(),
        }
    }

    fn url_of(outcome: &IssueOutcome) -> &str {
        match outcome {
            IssueOutcome::FiledNew { url } | IssueOutcome::MatchedExisting { url } => url,
        }
    }

    #[test]
    fn issue_filer_is_idempotent_across_cycles_no_network() {
        let gh = Arc::new(StatefulFakeGh::new());
        let filer = StewardshipIssueFiler::new(gh.clone());
        let brief = sample_brief();

        let first = filer.file(&brief).expect("first file");
        assert!(
            matches!(first, IssueOutcome::FiledNew { .. }),
            "first cycle files a NEW issue, got {first:?}"
        );

        let second = filer.file(&brief).expect("second file");
        assert!(
            matches!(second, IssueOutcome::MatchedExisting { .. }),
            "second cycle MATCHES the existing issue, got {second:?}"
        );

        // Same issue both times; exactly one create despite two cycles.
        assert_eq!(
            url_of(&first),
            url_of(&second),
            "same issue URL both cycles"
        );
        assert_eq!(gh.create_calls(), 1, "no duplicate issue created");
        assert_eq!(gh.search_calls(), 2, "each cycle searches exactly once");
    }

    #[test]
    fn issue_filer_surfaces_capability_failure_without_panic() {
        let filer = StewardshipIssueFiler::new(Arc::new(FailingGh));
        let err = filer
            .file(&sample_brief())
            .expect_err("search failure surfaces");
        assert!(
            matches!(
                err,
                OverseerError::Capability {
                    what: "file_issue",
                    ..
                }
            ),
            "gh failure must surface as a capability error, got {err:?}"
        );
    }

    #[test]
    fn brief_to_summary_synthesises_stable_run_id_from_signature() {
        let brief = sample_brief();
        let summary = brief_to_summary(&brief);
        let sig = failure_signature(&brief.failure_kind, &brief.error_text);
        assert_eq!(summary.run_id, format!("overseer-{sig}"));
        assert_eq!(summary.recipe_name, brief.recipe_name);
        assert_eq!(summary.failed_step, brief.failed_step);
        assert_eq!(summary.source_module, brief.source_module);
        assert_eq!(summary.failure_kind, brief.failure_kind);
        assert_eq!(summary.error_text, brief.error_text);
    }

    #[test]
    fn dedup_signature_ignores_recipe_and_step_differences() {
        // Two briefs describing the SAME underlying failure (same kind + text)
        // but different recipe/step must share a signature → same issue.
        let a = sample_brief();
        let mut b = sample_brief();
        b.recipe_name = "default-workflow".to_string();
        b.failed_step = "orient".to_string();
        assert_eq!(
            failure_signature(&a.failure_kind, &a.error_text),
            failure_signature(&b.failure_kind, &b.error_text)
        );

        let gh = Arc::new(StatefulFakeGh::new());
        let filer = StewardshipIssueFiler::new(gh.clone());
        assert!(matches!(
            filer.file(&a).unwrap(),
            IssueOutcome::FiledNew { .. }
        ));
        assert!(
            matches!(
                filer.file(&b).unwrap(),
                IssueOutcome::MatchedExisting { .. }
            ),
            "a differently-labelled run of the same failure must match, not re-file"
        );
        assert_eq!(gh.create_calls(), 1);
    }

    // ── M1 read-only guarantee ───────────────────────────────────────────────

    fn problem(kind: ProblemKind, evidence: Vec<Signal>) -> Problem {
        Problem {
            kind,
            priority: Priority::Normal,
            dedup_key: "k".to_string(),
            summary: "s".to_string(),
            evidence,
            why: None,
        }
    }

    #[test]
    fn m1_decide_is_write_free_for_every_problem_kind() {
        for kind in [
            ProblemKind::ProcessHealth,
            ProblemKind::ResourcePressure,
            ProblemKind::DeliveryReady,
            ProblemKind::QualityRegression,
            ProblemKind::GoalHygiene,
            ProblemKind::CrossCutting,
        ] {
            let iv = decide_read_only(&problem(kind, vec![]));
            assert!(
                is_m1_permitted(&iv),
                "{kind:?} → {iv:?} must be M1-permitted (no launch/merge/deploy/transfer)"
            );
        }
    }

    #[test]
    fn m1_files_deduped_issue_for_ci_cluster() {
        let iv = decide_read_only(&problem(
            ProblemKind::QualityRegression,
            vec![Signal::CiFailureCluster {
                repo: "rysweet/Simard".to_string(),
                failing: 4,
            }],
        ));
        assert!(matches!(iv, Intervention::FileIssue { .. }));
        assert!(is_m1_permitted(&iv));
    }

    #[test]
    fn m1_files_not_launches_even_when_full_decide_would() {
        // ProcessHealth: the full `decide` LAUNCHES a recipe; M1 must instead
        // file a deduplicated issue and never launch.
        let p = problem(
            ProblemKind::ProcessHealth,
            vec![Signal::DistillFailureRate { pct: 62.0 }],
        );
        assert!(
            matches!(decide_read_only(&p), Intervention::FileIssue { .. }),
            "M1 files an issue for a process-health defect, never launches"
        );
        assert!(is_m1_permitted(&decide_read_only(&p)));
        assert!(
            matches!(decide(&p), Intervention::LaunchRecipe { .. }),
            "sanity: the full (M2+) decide DOES launch for ProcessHealth"
        );
        assert!(
            !is_m1_permitted(&decide(&p)),
            "the launch the full decide plans is NOT M1-permitted"
        );
    }

    #[test]
    fn resource_and_delivery_problems_report_not_file() {
        // Transient pressure / positive delivery states are surfaced in the
        // Report, never filed as recurring-defect issues.
        for kind in [ProblemKind::ResourcePressure, ProblemKind::DeliveryReady] {
            assert_eq!(
                decide_read_only(&problem(kind, vec![])),
                Intervention::Report,
                "{kind:?} must report, not file"
            );
        }
    }

    #[test]
    fn every_filed_problem_routes_without_ambiguity() {
        use crate::stewardship::route_failure;
        // A representative problem per defect kind (those that FILE issues). Each
        // filed brief's source_module MUST resolve — never StewardshipRoutingAmbiguous.
        let cases = [
            problem(
                ProblemKind::ProcessHealth,
                vec![Signal::DistillFailureRate { pct: 62.0 }],
            ),
            problem(
                ProblemKind::QualityRegression,
                vec![Signal::CiFailureCluster {
                    repo: "rysweet/Simard".to_string(),
                    failing: 3,
                }],
            ),
            problem(
                ProblemKind::QualityRegression,
                vec![Signal::CiFailureCluster {
                    repo: "rysweet/amplihack".to_string(),
                    failing: 1,
                }],
            ),
            problem(
                ProblemKind::GoalHygiene,
                vec![Signal::StaleGoal {
                    goal_id: "g1".to_string(),
                }],
            ),
            problem(
                ProblemKind::CrossCutting,
                vec![Signal::Anomaly {
                    detail: "rename sweep".to_string(),
                }],
            ),
        ];
        for p in &cases {
            match decide_read_only(p) {
                Intervention::FileIssue { run } => assert!(
                    route_failure(&run.source_module).is_ok(),
                    "filed source_module {:?} must route",
                    run.source_module
                ),
                other => panic!("{:?} must file an issue, got {other:?}", p.kind),
            }
        }
    }

    #[test]
    fn amplihack_ci_cluster_routes_to_amplihack() {
        use crate::stewardship::{TargetRepo, route_failure};
        let p = problem(
            ProblemKind::QualityRegression,
            vec![Signal::CiFailureCluster {
                repo: "rysweet/amplihack".to_string(),
                failing: 2,
            }],
        );
        let Intervention::FileIssue { run } = decide_read_only(&p) else {
            panic!("amplihack CI cluster must file an issue");
        };
        assert_eq!(
            route_failure(&run.source_module).unwrap(),
            TargetRepo::Amplihack,
            "an amplihack CI cluster routes its issue to amplihack"
        );
    }

    #[test]
    fn same_process_problem_dedups_to_one_issue_across_cycles() {
        // Two observation cycles of the SAME recurring process problem must file
        // exactly one issue (stable error_text → stable failure signature).
        let gh = Arc::new(StatefulFakeGh::new());
        let filer = StewardshipIssueFiler::new(gh.clone());
        let mut p = problem(
            ProblemKind::ProcessHealth,
            vec![Signal::DistillFailureRate { pct: 62.0 }],
        );
        p.dedup_key = "process:distill_fail".to_string();

        let Intervention::FileIssue { run: run1 } = decide_read_only(&p) else {
            panic!("cycle 1 must file");
        };
        // A LATER cycle observes a different live pct — the brief text is stable,
        // so it must still dedup to the same issue.
        let mut p2 = p.clone();
        p2.evidence = vec![Signal::DistillFailureRate { pct: 71.0 }];
        let Intervention::FileIssue { run: run2 } = decide_read_only(&p2) else {
            panic!("cycle 2 must file");
        };

        assert!(matches!(
            filer.file(&run1).unwrap(),
            IssueOutcome::FiledNew { .. }
        ));
        assert!(
            matches!(
                filer.file(&run2).unwrap(),
                IssueOutcome::MatchedExisting { .. }
            ),
            "a later cycle of the same problem must match, not re-file"
        );
        assert_eq!(
            gh.create_calls(),
            1,
            "a recurring process problem files exactly one issue"
        );
    }

    #[test]
    fn write_actions_are_not_m1_permitted() {
        use crate::overseer::capabilities::{AuditScope, GoalBrief, RecipeBrief};
        let writes = [
            Intervention::LaunchRecipe {
                brief: RecipeBrief {
                    task_description: "x".to_string(),
                    target_repo: "rysweet/Simard".to_string(),
                    sequence_group: None,
                },
            },
            Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 1,
            },
            Intervention::ResolveConflict {
                repo: "rysweet/Simard".to_string(),
                pr: 1,
            },
            Intervention::Deploy {
                commit: "abc123".to_string(),
            },
            Intervention::TransferGoal {
                goal: GoalBrief {
                    title: "t".to_string(),
                    rationale: "r".to_string(),
                    priority: 3,
                    target_repo: "rysweet/Simard".to_string(),
                },
            },
            Intervention::RunAudit {
                scope: AuditScope::SelfHealth,
            },
        ];
        for iv in &writes {
            assert!(
                !is_m1_permitted(iv),
                "{} must NOT be M1-permitted",
                iv.label()
            );
        }
    }
}
