//! `simard-improve-step` — recipe-driven self-improvement helper binary.
//!
//! Single binary, multiple subcommands. Each subcommand corresponds to one
//! deterministic phase of the self-improvement cycle and is invoked from
//! `amplifier-bundle/recipes/simard-self-improve-cycle.yaml`. The agentic
//! phases (generate-patch, review-patch) live in the recipe itself, NOT
//! here — that is the architectural pivot.
//!
//! All subcommands take JSON args and emit JSON to stdout so the recipe
//! runner can wire them together via context variables. Errors go to
//! stderr with non-zero exit (Pillar 11 fail-fast).
//!
//! Subcommands
//! -----------
//!
//!   eval     --workspace P --suite-id S [--baseline-fixture-json J]
//!     Returns: GymSuiteScore as JSON.
//!     If --baseline-fixture-json is provided, returns it verbatim
//!     (used by parity tests; production wiring of the live transport
//!     is Phase 1.5).
//!
//!   analyze  --baseline-json J --weak-threshold T [--target-dimension D]
//!     Returns: Vec<WeakDimension> as JSON, sorted by deficit desc.
//!
//!   decide   --baseline-json B --post-json P --weak-dimensions-json W
//!            --proposal STR --research-decision STR
//!            --apply-result-json A [--target-dimension D]
//!     Returns: ImprovementCycle as JSON, byte-equivalent to what
//!     `run_improvement_cycle` returns for the same inputs.
//!
//!   apply-or-rollback --workspace P --review-json R --patch-stdin
//!     (Phase 1: stub that echoes review_json -> ApplyResult Applied
//!     when should_commit=true, ReviewBlocked otherwise.)

use std::io::{Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use simard::gym_scoring::{detect_regression, GymSuiteScore};
use simard::self_improve::{
    decide, find_weak_dimensions, ImprovementConfig, ImprovementCycle, ImprovementDecision,
    ImprovementPhase, ProposedChange, WeakDimension,
};

const DEFAULT_WEAK_THRESHOLD: f64 = 0.7;

#[derive(Debug, Deserialize, Serialize)]
struct ReviewOutput {
    findings: Vec<ReviewFinding>,
    should_commit: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct ReviewFinding {
    severity: String,
    message: String,
    #[serde(default)]
    file: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind")]
enum ApplyResultJson {
    Applied { findings: Vec<ReviewFinding> },
    ReviewBlocked { findings: Vec<ReviewFinding> },
    PatchFailed { reason: String },
}

fn die(msg: &str) -> ! {
    eprintln!("simard-improve-step: error: {msg}");
    std::process::exit(2);
}

fn arg_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == flag {
            return iter.next().map(String::as_str);
        }
    }
    None
}

fn require_arg<'a>(args: &'a [String], flag: &str) -> &'a str {
    arg_value(args, flag).unwrap_or_else(|| die(&format!("missing required arg: {flag}")))
}

fn parse_json<T: for<'de> Deserialize<'de>>(s: &str, what: &str) -> T {
    serde_json::from_str(s).unwrap_or_else(|e| die(&format!("invalid {what} JSON: {e}")))
}

fn emit<T: Serialize>(value: &T) {
    let s = serde_json::to_string(value).unwrap_or_else(|e| die(&format!("serialize: {e}")));
    let _ = writeln!(std::io::stdout(), "{s}");
}

fn cmd_eval(args: &[String]) {
    let _workspace = require_arg(args, "--workspace");
    let _suite_id = require_arg(args, "--suite-id");

    if let Some(fixture) = arg_value(args, "--baseline-fixture-json") {
        // Fixture path — parity-test shortcut. Validate JSON and emit verbatim.
        let score: GymSuiteScore = parse_json(fixture, "baseline-fixture");
        emit(&score);
        return;
    }

    // Production path: would build a SubprocessBridgeTransport and call
    // gym.run_suite. Phase 1.5 follow-up — see plan.md.
    die("live gym evaluation not yet wired in helper bin (Phase 1.5); pass --baseline-fixture-json for now");
}

fn cmd_analyze(args: &[String]) {
    let baseline_str = require_arg(args, "--baseline-json");
    let weak_threshold: f64 = arg_value(args, "--weak-threshold")
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| die("invalid --weak-threshold"))
        })
        .unwrap_or(DEFAULT_WEAK_THRESHOLD);
    let target_raw = arg_value(args, "--target-dimension").unwrap_or("");
    let target = if target_raw.is_empty() {
        None
    } else {
        Some(target_raw)
    };

    let baseline: GymSuiteScore = parse_json(baseline_str, "baseline");
    let weak = find_weak_dimensions(&baseline, weak_threshold, target);
    emit(&weak);
}

fn cmd_decide(args: &[String]) {
    let baseline_str = require_arg(args, "--baseline-json");
    let post_str = require_arg(args, "--post-json");
    let weak_str = require_arg(args, "--weak-dimensions-json");
    let proposal = require_arg(args, "--proposal");
    let research_decision = require_arg(args, "--research-decision");
    let apply_result_str = require_arg(args, "--apply-result-json");
    let target_raw = arg_value(args, "--target-dimension").unwrap_or("");
    let target = if target_raw.is_empty() {
        None
    } else {
        Some(target_raw.to_string())
    };

    let baseline: GymSuiteScore = parse_json(baseline_str, "baseline");
    let weak: Vec<WeakDimension> = parse_json(weak_str, "weak-dimensions");
    let weak_names: Vec<String> = weak.iter().map(|w| w.name.clone()).collect();

    // Build the same ImprovementConfig the Rust path uses, so `decide()` sees
    // identical thresholds. We only need the threshold-bearing fields.
    let config = ImprovementConfig {
        suite_id: "recipe".to_string(),
        weak_threshold: DEFAULT_WEAK_THRESHOLD,
        min_net_improvement: 0.01,
        max_single_regression: 0.05,
        target_dimension: target.clone(),
        auto_apply: false,
        proposed_changes: if proposal.is_empty() {
            Vec::new()
        } else {
            vec![ProposedChange {
                file_path: ".".to_string(),
                description: proposal.to_string(),
                expected_impact: format!("recipe-driven; research_decision={research_decision}"),
            }]
        },
    };

    // Short-circuit: no proposal -> Revert (mirrors run_improvement_cycle Phase 3)
    if research_decision == "REVERT_NO_PROPOSAL" || config.proposed_changes.is_empty() {
        let cycle = ImprovementCycle {
            baseline,
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Revert {
                reason: format!(
                    "no changes proposed; weak dimensions: {}",
                    if weak_names.is_empty() {
                        "none".to_string()
                    } else {
                        weak_names.join(", ")
                    }
                ),
            }),
            final_phase: ImprovementPhase::Analyze,
            weak_dimensions: weak_names,
            weak_dimension_details: weak,
            target_dimension: target,
        };
        emit(&cycle);
        return;
    }

    // Full path: post score must be present
    let post: GymSuiteScore = parse_json(post_str, "post-score");
    let regressions = detect_regression(&post, &baseline);
    let decision = decide(&config, &baseline, &post, &regressions);

    // If the apply step said ReviewBlocked or PatchFailed, override to Revert
    // with a reason mentioning the review block (preserves the engineer-loop
    // contract that critical findings stop commit).
    let final_decision = if !apply_result_str.is_empty() && apply_result_str != "null" {
        match serde_json::from_str::<serde_json::Value>(apply_result_str) {
            Ok(v) => {
                let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("");
                if kind == "ReviewBlocked" || kind == "PatchFailed" {
                    ImprovementDecision::Revert {
                        reason: format!("apply blocked: {kind}"),
                    }
                } else {
                    decision
                }
            }
            Err(_) => decision,
        }
    } else {
        decision
    };

    let cycle = ImprovementCycle {
        baseline,
        proposed_changes: config.proposed_changes,
        post_score: Some(post),
        regressions,
        decision: Some(final_decision),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: weak_names,
        weak_dimension_details: weak,
        target_dimension: target,
    };
    emit(&cycle);
}

fn cmd_apply_or_rollback(args: &[String]) {
    let _workspace: PathBuf = PathBuf::from(require_arg(args, "--workspace"));
    let review_str = require_arg(args, "--review-json");
    // Read patch from stdin (the recipe pipes it via heredoc)
    let mut patch = String::new();
    let _ = std::io::stdin().read_to_string(&mut patch);

    let review: ReviewOutput = parse_json(review_str, "review");
    let critical = review.findings.iter().any(|f| f.severity == "critical");
    let result = if critical || !review.should_commit {
        ApplyResultJson::ReviewBlocked {
            findings: review.findings,
        }
    } else if patch.trim().is_empty() {
        ApplyResultJson::PatchFailed {
            reason: "empty patch".to_string(),
        }
    } else {
        // Phase 1: just record applied — actual git apply + commit happens in
        // Phase 1.5 once the test harness is wired. The recipe's deterministic
        // contract holds: ReviewBlocked vs Applied is decided by review JSON.
        ApplyResultJson::Applied {
            findings: review.findings,
        }
    };
    emit(&json!(result));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => die("usage: simard-improve-step <eval|analyze|decide|apply-or-rollback> ..."),
    };
    match cmd {
        "eval" => cmd_eval(rest),
        "analyze" => cmd_analyze(rest),
        "decide" => cmd_decide(rest),
        "apply-or-rollback" => cmd_apply_or_rollback(rest),
        other => die(&format!("unknown subcommand: {other}")),
    }
}
