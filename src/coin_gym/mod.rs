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
//! `coin-gym run|matchup|score|compare|improve|profiles` CLI. The whole pipeline runs
//! offline against a mock oracle so it is exercised without a VM. Live grading
//! (Phase 3 VM) remains a follow-up on issue #2823; the live self-improvement
//! loop with verify/rollback (Phase 5, issue #2825) is implemented offline in
//! [`improve_loop`] behind `coin-gym improve --holdout fresh`.

pub mod agent_runner;
pub mod executor;
pub mod improve;
pub mod improve_loop;
pub mod leaderboard;
pub mod matchup;
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
mod tests_matchup;
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
use matchup::{StrategyMatchup, decide_matchup};
use profiles::{PersistedRun, default_home, ensure_profile, list_profiles, load_run, save_run};
use scorer::{Score, score_run};
use target_loader::DemoScenario;
use types::{CoinGymError, CoinGymResult, RunReport, Strategy};

/// CLI usage string.
#[must_use]
pub fn coin_gym_usage() -> &'static str {
    "usage: coin-gym <command>\n\
     \n\
     commands:\n\
     \x20 run <model> [--strategy baseline|team] [--profile <name>] [--targets <path>]\n\
     \x20 matchup <model> [--profile <name>] [--targets <path>]\n\
     \x20 score <run-id> [--profile <name>]\n\
     \x20 compare <run-id> [--profile <name>]\n\
     \x20 improve <run-id> [--profile <name>] [--holdout fresh]\n\
     \x20 contract [--dataset <repo>] [--revision <tag>] [--split a,b] [--project x,y] [--source rebuild|image]\n\
     \x20 profiles\n\
     \n\
     Offline scaffold (Phase 4): runs grade against a mock oracle. Live grading\n\
     needs `coin evaluate` on a Docker host (Phase 3, issue #2823). `improve\n\
     --holdout fresh` runs the Phase-5 self-improvement loop (failure-analyst →\n\
     overfitting gate → verify on held-out fresh → keep/rollback + durable tactic\n\
     memory) offline. `contract` prints the real coin evaluate/verify wiring\n\
     without running anything (LOCAL-ONLY). See\n\
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
        "matchup" => cmd_matchup(home, rest),
        "score" => cmd_score(home, rest),
        "compare" => cmd_compare(home, rest),
        "improve" => cmd_improve(home, rest),
        "contract" => cmd_contract(rest),
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

// ── matchup ────────────────────────────────────────────────────────────────

/// Run the single-model **baseline** and the multi-agent **team** over the same
/// pinned targets, persist both runs, and print the head-to-head verdict.
///
/// This is the COIN Gym's core measurement: does the multi-agent pattern beat
/// single-model execution on the LOCAL leaderboard? LOCAL-ONLY — nothing is
/// submitted externally.
fn cmd_matchup(home: &Path, rest: &[String]) -> CoinGymResult<()> {
    let parsed = parse_args(rest, &["profile", "targets"])?;
    let model = parsed
        .positionals
        .first()
        .ok_or_else(|| CoinGymError::Usage("matchup: expected <model>".to_string()))?
        .clone();
    let scenario = match parsed.flags.get("targets") {
        Some(path) => DemoScenario::from_path(Path::new(path))?,
        None => DemoScenario::sample()?,
    };
    validate_offline_scenario(&scenario)?;
    let profile_name = parsed.flags.get("profile").map_or_else(
        || profiles::sanitize_name(&model),
        |p| profiles::sanitize_name(p),
    );
    ensure_profile(home, &profile_name, &model)?;

    // Both strategies grade against the SAME targets/oracle so the comparison is
    // apples-to-apples; only the reasoning scaffold differs.
    let baseline_report = execute_run(&model, Strategy::Baseline, &scenario)?;
    let team_report = execute_run(&model, Strategy::Team, &scenario)?;

    let baseline_path = save_run(
        home,
        &profile_name,
        &PersistedRun {
            report: baseline_report.clone(),
            targets: scenario.targets.clone(),
            offline: scenario.offline_scaffold(),
        },
    )?;
    let team_path = save_run(
        home,
        &profile_name,
        &PersistedRun {
            report: team_report.clone(),
            targets: scenario.targets.clone(),
            offline: scenario.offline_scaffold(),
        },
    )?;

    let baseline_score = score_run(&baseline_report);
    let team_score = score_run(&team_report);
    let result = decide_matchup(&baseline_score, &team_score);

    println!("model:    {}", result.model);
    println!("snapshot: {}", baseline_report.snapshot);
    println!("profile:  {profile_name}");
    println!("targets:  {}", result.targets);
    print_offline_note(result.offline_scaffold);
    print_matchup(&result);
    println!("baseline: {}", baseline_path.display());
    println!("team:     {}", team_path.display());
    Ok(())
}

/// Render a baseline-vs-team [`StrategyMatchup`].
fn print_matchup(m: &StrategyMatchup) {
    println!(
        "reach:     baseline {:.1}%  vs team {:.1}%  (Δ {:+.1} pp)",
        m.baseline_reach_pct, m.team_reach_pct, m.reach_delta_pp
    );
    println!(
        "precision: baseline {:.1}%  vs team {:.1}%  (Δ {:+.1} pp)",
        m.baseline_precision_pct, m.team_precision_pct, m.precision_delta_pp
    );
    println!(
        "verdict:  {} (multiagent vs single-model)",
        m.verdict.label()
    );
    println!("note:     LOCAL comparison only — nothing submitted externally.");
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
