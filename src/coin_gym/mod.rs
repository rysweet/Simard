//! LOCAL COIN Gym harness (Phase 4 of issue #2713).
//!
//! A local harness that runs the COIN benchmark shape, scores locally against
//! the published leaderboard, and measures a single-model **baseline** vs. a
//! multi-agent **team**, mirroring skwaq's failure-analysis +
//! overfitting-reviewer gating. See
//! `docs/research/coin-benchmark-and-skwaq-study.md` (Part 3) for the design and
//! `docs/howto/run-the-coin-gym-harness.md` for usage.
//!
//! ## Scope (Phase 4)
//! This module is the **local scaffold**: a target loader, an agent runner
//! (baseline and team strategies behind one interface), a **mockable** harness
//! executor that delegates to `coin evaluate` (real Docker wiring is Phase 3), a
//! scorer, a leaderboard comparator, an offline failure-analyst plus
//! overfitting-reviewer gate, profiles, and the
//! `coin-gym run|score|compare|improve|contract|verify|profiles` CLI. The whole
//! pipeline runs
//! offline against a mock oracle so it is exercised without a VM. Live grading
//! (Phase 3 VM) remains a follow-up on issue #2823; the live self-improvement
//! loop with verify/rollback (Phase 5, issue #2825) is implemented offline in
//! [`improve_loop`] behind `coin-gym improve --holdout fresh`.

pub mod agent_runner;
pub mod executor;
pub mod improve;
pub mod improve_loop;
pub mod leaderboard;
pub mod profiles;
pub mod scorer;
pub mod target_loader;
pub mod types;

#[cfg(test)]
mod tests_agent_runner;
#[cfg(test)]
mod tests_cli;
#[cfg(test)]
mod tests_executor;
#[cfg(test)]
mod tests_improve;
#[cfg(test)]
mod tests_improve_loop;
#[cfg(test)]
mod tests_leaderboard;
#[cfg(test)]
mod tests_profiles;
#[cfg(test)]
mod tests_scorer;
#[cfg(test)]
mod tests_target_loader;
#[cfg(test)]
mod tests_types;

use std::collections::BTreeMap;
use std::path::Path;

use agent_runner::{AgentRunner, BaselineStrategy, FixtureReasoner, TeamStrategy};
use executor::{
    ANSWER_BLOB_BIN, ANSWER_BLOB_HARNESS, ANSWER_UNREACHABLE_MD, CoinEvaluateConfig,
    CoinEvaluateExecutor, EvaluateSource, LOCAL_ONLY, MockHarnessExecutor,
};
use improve::analyze_and_review;
use improve_loop::{SelfImproveReport, run_self_improvement};
use leaderboard::compare_to_leaderboard;
use profiles::{PersistedRun, default_home, ensure_profile, list_profiles, load_run, save_run};
use scorer::{Score, score_run};
use target_loader::DemoScenario;
use types::{CoinGymError, CoinGymResult, RunReport, Strategy, TargetFamily};

/// CLI usage string.
#[must_use]
pub fn coin_gym_usage() -> &'static str {
    "usage: coin-gym <command>\n\
     \n\
     commands:\n\
     \x20 run <model> [--strategy baseline|team] [--profile <name>] [--targets <path>]\n\
     \x20 score <run-id> [--profile <name>]\n\
     \x20 compare <run-id> [--profile <name>]\n\
     \x20 improve <run-id> [--profile <name>] [--holdout fresh]\n\
     \x20 contract [--dataset <repo>] [--revision <tag>] [--split a,b] [--project x,y] [--source rebuild|image]\n\
     \x20 verify\n\
     \x20 profiles\n\
     \n\
     Offline scaffold (Phase 4): runs grade against a mock oracle. Live grading\n\
     needs `coin evaluate` on a Docker host (Phase 3, issue #2823). `improve\n\
     --holdout fresh` runs the Phase-5 self-improvement loop (failure-analyst →\n\
     overfitting gate → verify on held-out fresh → keep/rollback + durable tactic\n\
     memory) offline. `contract` prints the real coin evaluate/verify wiring\n\
     without running anything (LOCAL-ONLY). `verify` runs the LOCAL harness\n\
     acceptance self-check (measurable done-criteria) offline and exits non-zero\n\
     if any criterion fails. See\n\
     docs/howto/run-the-coin-gym-harness.md."
}

/// Dispatch the `coin-gym` CLI over an argument iterator (argv minus the
/// program name).
///
/// # Errors
/// Returns an error for unknown commands, missing/invalid arguments, or any
/// underlying I/O / parse failure.
pub fn dispatch_coin_gym_cli<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let home = default_home();
    dispatch_with_home(&home, args)?;
    Ok(())
}

/// Dispatch against an explicit home directory (used by tests).
pub(crate) fn dispatch_with_home<I>(home: &Path, args: I) -> CoinGymResult<()>
where
    I: IntoIterator<Item = String>,
{
    let argv: Vec<String> = args.into_iter().collect();
    let command = argv
        .first()
        .ok_or_else(|| CoinGymError::Usage(coin_gym_usage().to_string()))?;
    let rest = &argv[1..];
    match command.as_str() {
        "run" => cmd_run(home, rest),
        "score" => cmd_score(home, rest),
        "compare" => cmd_compare(home, rest),
        "improve" => cmd_improve(home, rest),
        "contract" => cmd_contract(rest),
        "verify" => cmd_verify(rest),
        "profiles" => cmd_profiles(home, rest),
        other => Err(CoinGymError::Usage(format!(
            "unknown command '{other}'\n{}",
            coin_gym_usage()
        ))),
    }
}

// ── Argument parsing ─────────────────────────────────────────────────────────

struct ParsedArgs {
    positionals: Vec<String>,
    flags: BTreeMap<String, String>,
}

fn parse_args(rest: &[String], allowed_flags: &[&str]) -> CoinGymResult<ParsedArgs> {
    let mut positionals = Vec::new();
    let mut flags = BTreeMap::new();
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        if let Some(name) = arg.strip_prefix("--") {
            if !allowed_flags.contains(&name) {
                return Err(CoinGymError::Usage(format!("unknown flag '--{name}'")));
            }
            let value = iter
                .next()
                .ok_or_else(|| CoinGymError::Usage(format!("flag '--{name}' expects a value")))?;
            flags.insert(name.to_string(), value.clone());
        } else {
            positionals.push(arg.clone());
        }
    }
    Ok(ParsedArgs { positionals, flags })
}

// ── run ──────────────────────────────────────────────────────────────────────

fn cmd_run(home: &Path, rest: &[String]) -> CoinGymResult<()> {
    let parsed = parse_args(rest, &["strategy", "profile", "targets"])?;
    let model = parsed
        .positionals
        .first()
        .ok_or_else(|| CoinGymError::Usage("run: expected <model>".to_string()))?
        .clone();
    let strategy = match parsed.flags.get("strategy") {
        Some(v) => Strategy::parse(v).map_err(CoinGymError::Usage)?,
        None => Strategy::Baseline,
    };
    let scenario = match parsed.flags.get("targets") {
        Some(path) => DemoScenario::from_path(Path::new(path))?,
        None => DemoScenario::sample()?,
    };
    validate_offline_scenario(&scenario)?;
    let profile_name = parsed.flags.get("profile").map_or_else(
        || profiles::sanitize_name(&model),
        |p| profiles::sanitize_name(p),
    );

    let report = execute_run(&model, strategy, &scenario)?;
    ensure_profile(home, &profile_name, &model)?;
    let persisted = PersistedRun {
        report: report.clone(),
        targets: scenario.targets.clone(),
        offline: scenario.offline_scaffold(),
    };
    let path = save_run(home, &profile_name, &persisted)?;
    let score = score_run(&report);

    println!("run-id:  {}", report.run_id);
    println!("model:   {}", report.model);
    println!("strategy:{}", report.strategy);
    println!("snapshot:{}", report.snapshot);
    println!("profile: {profile_name}");
    println!("saved:   {}", path.display());
    print_offline_note(report.offline_scaffold);
    print_score(&score);
    Ok(())
}

/// Guard against **hollow** offline runs: an offline scaffold run grades against
/// the manifest's mock `oracle` and draws candidates from its `script`. Without
/// pinned targets, oracle coverage, and at least one candidate, a run would
/// "succeed" while producing only meaningless `N`/`W` outcomes. Refuse instead.
fn validate_offline_scenario(scenario: &DemoScenario) -> CoinGymResult<()> {
    if scenario.targets.pinned.is_empty() {
        return Err(CoinGymError::Usage(
            "snapshot has no pinned targets to evaluate".to_string(),
        ));
    }
    let missing_oracle: Vec<&str> = scenario
        .targets
        .pinned
        .iter()
        .filter(|t| !scenario.oracle.contains_key(&t.id))
        .map(|t| t.id.as_str())
        .collect();
    if !missing_oracle.is_empty() {
        return Err(CoinGymError::Usage(format!(
            "offline run requires an `oracle` (mock reaching input) for every pinned target; \
             missing: {}",
            missing_oracle.join(", ")
        )));
    }
    if scenario.script.is_empty() {
        return Err(CoinGymError::Usage(
            "offline run requires a `script` of agent candidates; the manifest provided none"
                .to_string(),
        ));
    }
    if !scenario
        .targets
        .pinned
        .iter()
        .any(|t| scenario.script.contains_key(&t.id))
    {
        return Err(CoinGymError::Usage(
            "offline run requires at least one `script` candidate keyed to a pinned target; \
             the manifest's script keys do not match any pinned target id"
                .to_string(),
        ));
    }
    Ok(())
}

/// Build the reasoner/executor/strategy and run. Split out so tests can drive it
/// directly without touching disk.
pub(crate) fn execute_run(
    model: &str,
    strategy: Strategy,
    scenario: &DemoScenario,
) -> CoinGymResult<RunReport> {
    let reasoner = FixtureReasoner::new(scenario.script.clone());
    let executor = MockHarnessExecutor::from_oracle(scenario.oracle.clone());
    let snapshot = scenario.targets.snapshot.clone();
    match strategy {
        Strategy::Baseline => {
            let s = BaselineStrategy::new(reasoner);
            AgentRunner::new(&s, &executor, model, snapshot).run(&scenario.targets.pinned)
        }
        Strategy::Team => {
            let s = TeamStrategy::new(reasoner);
            AgentRunner::new(&s, &executor, model, snapshot).run(&scenario.targets.pinned)
        }
    }
}

// ── score ────────────────────────────────────────────────────────────────────

fn cmd_score(home: &Path, rest: &[String]) -> CoinGymResult<()> {
    let parsed = parse_args(rest, &["profile"])?;
    let run_id = require_run_id(&parsed, "score")?;
    let profile = sanitized_profile(&parsed);
    let persisted = load_run(home, profile.as_deref(), &run_id)?;
    let score = score_run(&persisted.report);
    println!("run-id: {run_id}");
    print_offline_note(persisted.report.offline_scaffold);
    print_score(&score);
    Ok(())
}

// ── compare ──────────────────────────────────────────────────────────────────

fn cmd_compare(home: &Path, rest: &[String]) -> CoinGymResult<()> {
    let parsed = parse_args(rest, &["profile"])?;
    let run_id = require_run_id(&parsed, "compare")?;
    let profile = sanitized_profile(&parsed);
    let persisted = load_run(home, profile.as_deref(), &run_id)?;
    let score = score_run(&persisted.report);
    println!("run-id: {run_id}");
    println!("model:  {}", score.model);
    match compare_to_leaderboard(&score) {
        Some(cmp) => {
            println!(
                "reach:     local {:.1}%  vs published {:.1}%  (Δ {:+.1} pts)",
                cmp.local_reach_pct, cmp.published_reach_pct, cmp.reach_delta_pct
            );
            println!(
                "precision: local {:.1}%  vs published {:.1}%  (Δ {:+.1} pts)",
                cmp.local_precision_pct, cmp.published_precision_pct, cmp.precision_delta_pct
            );
            println!("published: {}", cmp.published_model);
            println!(
                "material-deviation: {}",
                if cmp.material_deviation { "YES" } else { "no" }
            );
            println!("note: {}", cmp.note);
        }
        None => {
            print_offline_note(persisted.report.offline_scaffold);
            println!(
                "note: model '{}' is not on the published COIN leaderboard; nothing to compare",
                score.model
            );
        }
    }
    Ok(())
}

// ── improve ──────────────────────────────────────────────────────────────────

fn cmd_improve(home: &Path, rest: &[String]) -> CoinGymResult<()> {
    let parsed = parse_args(rest, &["profile", "holdout"])?;
    let run_id = require_run_id(&parsed, "improve")?;
    let profile = sanitized_profile(&parsed);
    let persisted = load_run(home, profile.as_deref(), &run_id)?;

    if let Some(holdout) = parsed.flags.get("holdout") {
        if holdout != "fresh" {
            return Err(CoinGymError::Usage(format!(
                "improve --holdout only supports 'fresh' (got '{holdout}')"
            )));
        }
        // Tactic memory is banked under a concrete profile: the explicit
        // `--profile`, else the run's model-derived default (matching `run`).
        let mem_profile =
            profile.unwrap_or_else(|| profiles::sanitize_name(&persisted.report.model));
        let report = run_self_improvement(home, &mem_profile, &persisted)?;
        print_self_improve(&mem_profile, &report);
        return Ok(());
    }

    let report = analyze_and_review(&persisted.report, &persisted.targets);
    println!("run-id:   {run_id}");
    println!("analyzed: {} unreached target(s)", report.analyzed);
    println!(
        "accepted: {}  rejected: {}",
        report.accepted, report.rejected
    );
    for reviewed in &report.proposals {
        println!(
            "  [{}] {} — {}",
            reviewed.verdict_label(),
            reviewed.proposal.target_id,
            reviewed.reason
        );
        println!("        tactic: {}", reviewed.proposal.tactic);
    }
    println!("note: {}", report.note);
    Ok(())
}

/// Render a live self-improvement (`improve --holdout fresh`) report.
fn print_self_improve(profile: &str, report: &SelfImproveReport) {
    println!("run-id:   {}", report.run_id);
    println!("model:    {}", report.model);
    println!("profile:  {profile}");
    println!(
        "gate:     {} accepted  {} rejected",
        report.gate_accepted, report.gate_rejected
    );
    println!(
        "holdout:  reach {:.1}% → {:.1}%   (kept {}, rolled back {}, train/held-out-gap warnings {})",
        report.holdout_reach_before_pct,
        report.holdout_reach_after_pct,
        report.kept,
        report.rolled_back,
        report.overfitting_warnings
    );
    println!(
        "memory:   {} → {} durable tactic(s)",
        report.memory_before, report.memory_after
    );
    for v in &report.verified {
        println!(
            "  [{}] {} ({}) — {}",
            v.decision.label(),
            v.source_target_id,
            v.category,
            v.reason
        );
        if let Some(w) = &v.overfitting_warning {
            println!("        ⚠ train/held-out gap: {w}");
        }
    }
    println!("note: {}", report.note);
}

// ── contract ─────────────────────────────────────────────────────────────────

/// Print the **real** `coin evaluate` / `coin verify` wiring the harness would
/// drive for a snapshot — without running anything. Makes the Phase-4 executor
/// wiring (issue #3001) observable and copy-pasteable, and states the LOCAL-ONLY
/// guardrail explicitly. Defaults to the published `COIN-Bench/coin@v2026-07`.
fn cmd_contract(rest: &[String]) -> CoinGymResult<()> {
    let parsed = parse_args(
        rest,
        &[
            "dataset",
            "revision",
            "split",
            "project",
            "source",
            "experiment",
        ],
    )?;
    let dataset = parsed
        .flags
        .get("dataset")
        .cloned()
        .unwrap_or_else(|| "COIN-Bench/coin".to_string());
    let revision = parsed
        .flags
        .get("revision")
        .cloned()
        .unwrap_or_else(|| "v2026-07".to_string());
    let mut config = CoinEvaluateConfig::new(dataset, revision);
    if let Some(splits) = parsed.flags.get("split") {
        for s in splits.split(',').filter(|s| !s.is_empty()) {
            config = config.with_split(s);
        }
    }
    if let Some(projects) = parsed.flags.get("project") {
        for p in projects.split(',').filter(|s| !s.is_empty()) {
            config = config.with_project(p);
        }
    }
    if let Some(src) = parsed.flags.get("source") {
        let source = match src.as_str() {
            "rebuild" => EvaluateSource::Rebuild,
            "image" => EvaluateSource::Image,
            other => {
                return Err(CoinGymError::Usage(format!(
                    "unknown --source '{other}' (expected 'rebuild' or 'image')"
                )));
            }
        };
        config = config.with_source(source);
    }
    let experiment = parsed
        .flags
        .get("experiment")
        .cloned()
        .unwrap_or_else(|| "<experiment-id>".to_string());
    let exec = CoinEvaluateExecutor::new(config);

    println!(
        "LOCAL-ONLY: {LOCAL_ONLY} (no external submission, no leaderboard entry, no VM provisioning)"
    );
    println!("evaluate: {}", exec.build_evaluate_argv().join(" "));
    println!(
        "verify:   {}",
        exec.build_verify_argv(&experiment, None).join(" ")
    );
    println!("submission-contract:");
    println!("  attempt:  /answer/{ANSWER_BLOB_BIN} + /answer/{ANSWER_BLOB_HARNESS}");
    println!("  abstain:  /answer/{ANSWER_UNREACHABLE_MD}  (and NO {ANSWER_BLOB_BIN})");
    println!("verdict:  read `reached` from each result.json (never re-checked locally)");
    Ok(())
}

// ── verify (measurable done-criteria self-check) ─────────────────────────────

/// The published leaderboard model used to exercise the comparator during
/// `verify`. It must be a real row in [`leaderboard::published_leaderboard`].
const VERIFY_PUBLISHED_MODEL: &str = "GPT-5.4";

/// One acceptance criterion result in the LOCAL COIN Gym done-gate self-check.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AcceptanceCheck {
    /// Short, stable name of the criterion.
    pub criterion: &'static str,
    /// Whether the criterion held.
    pub passed: bool,
    /// Human-readable measured detail (counts, percentages, or the failure).
    pub detail: String,
}

impl AcceptanceCheck {
    fn pass(criterion: &'static str, detail: impl Into<String>) -> Self {
        Self {
            criterion,
            passed: true,
            detail: detail.into(),
        }
    }

    fn fail(criterion: &'static str, detail: impl Into<String>) -> Self {
        Self {
            criterion,
            passed: false,
            detail: detail.into(),
        }
    }
}

/// The full acceptance report for the LOCAL COIN Gym harness.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AcceptanceReport {
    /// One row per criterion, in a stable order.
    pub checks: Vec<AcceptanceCheck>,
}

impl AcceptanceReport {
    /// Number of criteria that passed.
    pub(crate) fn passed_count(&self) -> usize {
        self.checks.iter().filter(|c| c.passed).count()
    }

    /// Total number of criteria evaluated.
    pub(crate) fn total(&self) -> usize {
        self.checks.len()
    }

    /// `true` only when every criterion passed.
    pub(crate) fn all_passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.passed)
    }
}

/// The `contract` wiring criterion: the executor can build a non-empty
/// `coin evaluate`/`coin verify` argv (independent of the sample snapshot).
fn contract_wiring_check() -> AcceptanceCheck {
    let exec = CoinEvaluateExecutor::new(CoinEvaluateConfig::new("COIN-Bench/coin", "v2026-07"));
    let evaluate = exec.build_evaluate_argv();
    let verify = exec.build_verify_argv("<experiment-id>", None);
    if !evaluate.is_empty() && !verify.is_empty() {
        AcceptanceCheck::pass(
            "contract-wiring",
            format!(
                "evaluate ({} args) + verify ({} args) argv present; LOCAL-ONLY={LOCAL_ONLY}",
                evaluate.len(),
                verify.len()
            ),
        )
    } else {
        AcceptanceCheck::fail(
            "contract-wiring",
            format!(
                "empty argv (evaluate={} verify={})",
                evaluate.len(),
                verify.len()
            ),
        )
    }
}

/// Run the LOCAL COIN Gym acceptance self-check: exercise every harness
/// component (issue #2713 design summary) offline against the built-in sample
/// snapshot and assert a concrete, measurable postcondition for each. This is
/// the machine-checkable **done-criteria** for the LOCAL harness goal. Live VM
/// grading (Phase 3, issue #2823) is externally gated and intentionally out of
/// this gate's scope.
///
/// `mem_home` isolates the self-improvement tactic memory so the check never
/// touches the user's real profiles; callers should pass a throwaway directory.
pub(crate) fn run_acceptance_checks(mem_home: &Path) -> AcceptanceReport {
    let mut checks = Vec::new();

    // 1. Target loader: pinned + held-out fresh slices, both families present.
    let scenario = match DemoScenario::sample() {
        Ok(s) => {
            let pinned = s.targets.pinned.len();
            let held = s.targets.held_out_fresh.len();
            let has_frontier = s
                .targets
                .pinned
                .iter()
                .any(|t| t.family == TargetFamily::Frontier);
            let has_ntr = s
                .targets
                .pinned
                .iter()
                .any(|t| t.family == TargetFamily::NonTrivialReachable);
            if pinned > 0 && held > 0 && has_frontier && has_ntr {
                checks.push(AcceptanceCheck::pass(
                    "target-loader",
                    format!(
                        "{pinned} pinned + {held} held-out-fresh target(s); both families present"
                    ),
                ));
                Some(s)
            } else {
                checks.push(AcceptanceCheck::fail(
                    "target-loader",
                    format!(
                        "pinned={pinned} held_out_fresh={held} \
                         frontier={has_frontier} non_trivial_reachable={has_ntr}"
                    ),
                ));
                None
            }
        }
        Err(e) => {
            checks.push(AcceptanceCheck::fail(
                "target-loader",
                format!("sample snapshot failed to load: {e}"),
            ));
            None
        }
    };

    let Some(scenario) = scenario else {
        for criterion in [
            "baseline-runner",
            "team-runner",
            "scorer",
            "leaderboard-comparator",
            "self-improvement-loop",
        ] {
            checks.push(AcceptanceCheck::fail(
                criterion,
                "skipped: sample snapshot unavailable",
            ));
        }
        checks.push(contract_wiring_check());
        return AcceptanceReport { checks };
    };

    let expected = scenario.targets.pinned.len();
    let baseline = execute_run(VERIFY_PUBLISHED_MODEL, Strategy::Baseline, &scenario);

    // 2. Baseline runner: exactly one graded outcome per pinned target.
    match &baseline {
        Ok(report) if report.outcomes.len() == expected && expected > 0 => {
            checks.push(AcceptanceCheck::pass(
                "baseline-runner",
                format!(
                    "{} outcome(s) for {expected} pinned target(s)",
                    report.outcomes.len()
                ),
            ));
        }
        Ok(report) => checks.push(AcceptanceCheck::fail(
            "baseline-runner",
            format!(
                "got {} outcome(s) for {expected} pinned target(s)",
                report.outcomes.len()
            ),
        )),
        Err(e) => checks.push(AcceptanceCheck::fail(
            "baseline-runner",
            format!("run failed: {e}"),
        )),
    }

    // 3. Team runner: exactly one graded outcome per pinned target.
    match execute_run(VERIFY_PUBLISHED_MODEL, Strategy::Team, &scenario) {
        Ok(report) if report.outcomes.len() == expected && expected > 0 => {
            checks.push(AcceptanceCheck::pass(
                "team-runner",
                format!(
                    "{} outcome(s) for {expected} pinned target(s)",
                    report.outcomes.len()
                ),
            ));
        }
        Ok(report) => checks.push(AcceptanceCheck::fail(
            "team-runner",
            format!(
                "got {} outcome(s) for {expected} pinned target(s)",
                report.outcomes.len()
            ),
        )),
        Err(e) => checks.push(AcceptanceCheck::fail(
            "team-runner",
            format!("run failed: {e}"),
        )),
    }

    // 4. Scorer: bounded reach/precision, per-family split, histogram accounts
    //    for every outcome.
    match &baseline {
        Ok(report) => {
            let score = score_run(report);
            let reach = score.overall.reach_pct();
            let precision = score.overall.precision_pct();
            let bounded = (0.0..=100.0).contains(&reach) && (0.0..=100.0).contains(&precision);
            let hist_total = score.histogram.total();
            let families = score.by_family.len();
            if bounded && families >= 2 && hist_total == report.outcomes.len() {
                checks.push(AcceptanceCheck::pass(
                    "scorer",
                    format!(
                        "reach {reach:.1}% / precision {precision:.1}%; \
                         {families} family split; histogram covers {hist_total}/{} outcome(s)",
                        report.outcomes.len()
                    ),
                ));
            } else {
                checks.push(AcceptanceCheck::fail(
                    "scorer",
                    format!(
                        "reach={reach:.1} precision={precision:.1} families={families} \
                         histogram_total={hist_total} outcomes={}",
                        report.outcomes.len()
                    ),
                ));
            }
        }
        Err(_) => checks.push(AcceptanceCheck::fail(
            "scorer",
            "skipped: baseline run unavailable",
        )),
    }

    // 5. Leaderboard comparator: the published model diffs against its row.
    match &baseline {
        Ok(report) => {
            let score = score_run(report);
            match compare_to_leaderboard(&score) {
                Some(cmp) => checks.push(AcceptanceCheck::pass(
                    "leaderboard-comparator",
                    format!(
                        "compared vs published '{}' (reach Δ {:+.1} pts, material-deviation={})",
                        cmp.published_model, cmp.reach_delta_pct, cmp.material_deviation
                    ),
                )),
                None => checks.push(AcceptanceCheck::fail(
                    "leaderboard-comparator",
                    format!("model '{VERIFY_PUBLISHED_MODEL}' unexpectedly absent from the published leaderboard"),
                )),
            }
        }
        Err(_) => checks.push(AcceptanceCheck::fail(
            "leaderboard-comparator",
            "skipped: baseline run unavailable",
        )),
    }

    // 6. Self-improvement loop: held-out reach must not regress (keep-iff-improves
    //    else roll back) and durable tactic memory must never shrink.
    match &baseline {
        Ok(report) => {
            let persisted = PersistedRun {
                report: report.clone(),
                targets: scenario.targets.clone(),
                offline: scenario.offline_scaffold(),
            };
            match run_self_improvement(mem_home, "coin-gym-verify", &persisted) {
                Ok(rep) => {
                    let non_regress =
                        rep.holdout_reach_after_pct >= rep.holdout_reach_before_pct - f64::EPSILON;
                    let durable = rep.memory_after >= rep.memory_before;
                    if non_regress && durable {
                        checks.push(AcceptanceCheck::pass(
                            "self-improvement-loop",
                            format!(
                                "held-out reach {:.1}% → {:.1}% (kept {}, rolled back {}); \
                                 memory {} → {} tactic(s)",
                                rep.holdout_reach_before_pct,
                                rep.holdout_reach_after_pct,
                                rep.kept,
                                rep.rolled_back,
                                rep.memory_before,
                                rep.memory_after
                            ),
                        ));
                    } else {
                        checks.push(AcceptanceCheck::fail(
                            "self-improvement-loop",
                            format!(
                                "non_regress={non_regress} durable={durable} \
                                 (reach {:.1}%→{:.1}%, memory {}→{})",
                                rep.holdout_reach_before_pct,
                                rep.holdout_reach_after_pct,
                                rep.memory_before,
                                rep.memory_after
                            ),
                        ));
                    }
                }
                Err(e) => checks.push(AcceptanceCheck::fail(
                    "self-improvement-loop",
                    format!("loop failed: {e}"),
                )),
            }
        }
        Err(_) => checks.push(AcceptanceCheck::fail(
            "self-improvement-loop",
            "skipped: baseline run unavailable",
        )),
    }

    // 7. Contract wiring.
    checks.push(contract_wiring_check());

    AcceptanceReport { checks }
}

/// Run the LOCAL harness acceptance self-check and print a PASS/FAIL matrix.
/// Exits non-zero (via `Err`) when any criterion fails, so `coin-gym verify`
/// is a measurable, CI-friendly done-gate for the LOCAL COIN Gym goal.
fn cmd_verify(rest: &[String]) -> CoinGymResult<()> {
    let _parsed = parse_args(rest, &[])?;
    let tmp = tempfile::tempdir()
        .map_err(|e| CoinGymError::Io(format!("verify: cannot create temp home: {e}")))?;
    let report = run_acceptance_checks(tmp.path());

    println!("coin-gym verify — LOCAL harness acceptance self-check");
    println!("snapshot: built-in sample (offline mock oracle)");
    for c in &report.checks {
        println!(
            "  [{}] {:<24} {}",
            if c.passed { "PASS" } else { "FAIL" },
            c.criterion,
            c.detail
        );
    }
    println!(
        "result: {}/{} criteria passed",
        report.passed_count(),
        report.total()
    );
    println!(
        "scope: LOCAL offline harness only. Live VM grading (`coin evaluate`/`coin verify`) is \
         Phase 3 — externally gated on a provisioned Docker host (issue #2823) and intentionally \
         out of this gate."
    );

    if report.all_passed() {
        Ok(())
    } else {
        Err(CoinGymError::Usage(format!(
            "{} of {} LOCAL acceptance criteria failed; see the FAIL rows above",
            report.total() - report.passed_count(),
            report.total()
        )))
    }
}

// ── profiles ─────────────────────────────────────────────────────────────────

fn cmd_profiles(home: &Path, rest: &[String]) -> CoinGymResult<()> {
    let _parsed = parse_args(rest, &[])?;
    let profiles = list_profiles(home)?;
    if profiles.is_empty() {
        println!("no profiles under {}", home.display());
        return Ok(());
    }
    println!("profiles under {}:", home.display());
    for p in profiles {
        println!("- {}  (model={})", p.name, p.model);
    }
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn require_run_id(parsed: &ParsedArgs, cmd: &str) -> CoinGymResult<String> {
    let run_id = parsed
        .positionals
        .first()
        .cloned()
        .ok_or_else(|| CoinGymError::Usage(format!("{cmd}: expected <run-id>")))?;
    // A run-id becomes a filename (`<run-id>.json`); reject path separators and
    // parent traversal so it can never escape the profile's runs directory.
    if run_id.contains('/') || run_id.contains('\\') || run_id.contains("..") {
        return Err(CoinGymError::Usage(format!(
            "invalid run-id '{run_id}': must not contain path separators or '..'"
        )));
    }
    Ok(run_id)
}

/// Sanitised `--profile` value (directory-safe), or `None` when the flag is
/// absent (⇒ search all profiles).
fn sanitized_profile(parsed: &ParsedArgs) -> Option<String> {
    parsed
        .flags
        .get("profile")
        .map(|p| profiles::sanitize_name(p))
}

fn print_offline_note(offline_scaffold: bool) {
    if offline_scaffold {
        println!(
            "note:   OFFLINE SCAFFOLD (mock oracle) — not a real coin evaluate grade (Phase 3)"
        );
    }
}

fn print_score(score: &Score) {
    println!(
        "reach: {:.1}%  ({}/{})   precision: {:.1}%  ({}/{})",
        score.overall.reach_pct(),
        score.overall.reached,
        score.overall.total,
        score.overall.precision_pct(),
        score.overall.reached,
        score.overall.submitted,
    );
    for fs in &score.by_family {
        println!(
            "  {:<22} reach {:.1}%  precision {:.1}%  (n={})",
            fs.family.label(),
            fs.score.reach_pct(),
            fs.score.precision_pct(),
            fs.score.total,
        );
    }
    println!("  histogram: {}", score.histogram.render());
}
