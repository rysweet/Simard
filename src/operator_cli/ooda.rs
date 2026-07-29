use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::operator_commands_ooda::{DaemonDashboardConfig, run_ooda_daemon};

use super::args::next_required;

pub(super) const OODA_HELP: &str = "\
Simard OODA daemon subcommand

Usage: simard ooda <command> [args]

Commands:
  run [--cycles=N] [--no-auto-reload] [--no-dashboard] [--dashboard-port=PORT] [state-root]
                              Run the OODA loop daemon.
  outcomes get --state-root <PATH> --request-id <ID>
                              Read one authoritative typed terminal.
  outcomes list --state-root <PATH> [--limit <N>]
                              List authoritative typed terminals.
  terminal <spawn-engineer|no-action|blocked|completed> [SCOPED OPTIONS]
                              Record exactly one authenticated typed terminal.
  record-decision --choice <continue|spawn|reorient|investigate|wait|complete>
                  (--reason <TEXT> | --reason-path <FILE>)
                  --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                  [--task-hint <TEXT> | --task-hint-path <FILE>]
                              Record exactly one typed, validated per-goal-cycle
                              decision (the reasoner's tool; zero privilege).
  record-outcome  --choice <mark_achieved|reopen|replan|keep_open_and_report>
                  (--reason <TEXT> | --reason-path <FILE>)
                  --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                  [--replan-hint <TEXT> | --replan-hint-path <FILE>]
                              Record exactly one typed, validated goal-outcome-
                              verification decision (the reasoner's tool; zero
                              privilege). --replan-hint is OWNED by replan
                              (optional even there), REJECTED on every other
                              choice.
  record-orient   --adjusted-urgency <F> --confidence <F> --demotion-applied <F>
                  --base-urgency <F> (--reason <TEXT> | --reason-path <FILE>)
                  --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                              Record exactly one typed, validated OODA Orient
                              judgment (the reasoner's tool; zero privilege).
  record-decide   --choice <poll_developer_activity|consolidate_memory|run_improvement|
                            extract_ideas|safe_update|research_query|run_gym_eval|
                            build_skill|launch_session|advance_goal>
                  (--reason <TEXT> | --reason-path <FILE>)
                  --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                              Record exactly one typed, validated OODA Decide
                              action-routing (the reasoner's tool; zero privilege).
  record-lifecycle-decision --decision <continue_skipping|reclaim_and_redispatch|
                            deprioritize|open_tracking_issue|mark_goal_blocked|
                            consider_self_update>
                  [--rationale <TEXT> | --rationale-path <FILE>]
                  --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                              Record exactly one typed, validated engineer-
                              lifecycle Act decision (the reasoner's tool; zero
                              privilege). The extra-field variants derive their
                              body/reason/redispatch text from --rationale.
  record-admission --choice <admit|defer|serialize_after>
                   (--rationale <TEXT> | --rationale-path <FILE>)
                   --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                   [--blocked-by <csv> --retry-after-secs <u64>
                    --after-goal-id <ID> --overlap-files <csv>]
                              Record exactly one typed, validated engineer-
                              admission verdict (the reasoner's tool; zero
                              privilege). Variant-owned fields: --blocked-by /
                              --retry-after-secs (defer), --after-goal-id /
                              --overlap-files (serialize_after).
  record-resource-admission --choice <admit|defer|reclaim_first>
                   (--rationale <TEXT> | --rationale-path <FILE>)
                   --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                              Record exactly one typed, validated resource-
                              admission verdict (the reasoner's tool; zero privilege).
  record-idea-dedup --choice <create_new|skip|enhance_existing>
                   (--reason <TEXT> | --reason-path <FILE>) [--target-node-id <ID>]
                   --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                              Record exactly one typed, validated creative-idea
                              semantic-dedup verdict (the reasoner's tool; zero
                              privilege). --target-node-id is REQUIRED on
                              enhance_existing, REJECTED on create_new/skip.
  record-idea-consolidation --clusters-path <ABSOLUTE_PATH>
                   --record-path <ABSOLUTE_PATH> --goal-id <ID> --cycle-number <N>
                              Record exactly one typed, validated creative-ideas
                              consolidation cluster list read from the JSON-array
                              file at --clusters-path (the reasoner's tool; zero
                              privilege). An empty array is a valid \"nothing to
                              consolidate\" record.
  approvals issue --state-root <PATH> --effect-id <ID> --request-id <ID>
                              Issue a privileged merge/deploy approval from
                              the configured server principal and signing key.
  fixture run --state-root <PATH> --scenario <spawn-engineer|no-action|agent-spawn-engineer|agent-no-action> --request-id <ID>
                              Run a deterministic typed acceptance cycle
                              (requires SIMARD_TYPED_OODA_FIXTURE=1).
  help, -h, --help            Show this help message and exit.
";

pub(super) fn dispatch_ooda_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "ooda command")?;
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{OODA_HELP}");
            Ok(())
        }
        "run" => {
            let mut max_cycles: u32 = 0; // 0 = infinite
            let mut state_root: Option<PathBuf> = None;
            let mut auto_reload = true;
            let mut dashboard = DaemonDashboardConfig::default();

            for arg in args {
                if let Some(n) = arg.strip_prefix("--cycles=") {
                    max_cycles = n
                        .parse()
                        .map_err(|_| format!("invalid --cycles value: {n}"))?;
                } else if arg == "--no-auto-reload" {
                    auto_reload = false;
                } else if arg == "--no-dashboard" {
                    dashboard.enabled = false;
                } else if let Some(p) = arg.strip_prefix("--dashboard-port=") {
                    dashboard.port = p
                        .parse()
                        .map_err(|_| format!("invalid --dashboard-port value: {p}"))?;
                } else if state_root.is_none() {
                    state_root = Some(PathBuf::from(arg));
                } else {
                    return Err(format!("unexpected argument: {arg}").into());
                }
            }

            run_ooda_daemon(max_cycles, state_root, auto_reload, dashboard)
        }
        "outcomes" => dispatch_outcomes(args),
        "fixture" => dispatch_fixture(args),
        "terminal" => dispatch_terminal(args),
        "record-decision" => dispatch_record_decision(args),
        "record-outcome" => dispatch_record_outcome(args),
        "record-orient" => dispatch_record_orient(args),
        "record-decide" => dispatch_record_decide(args),
        "record-lifecycle-decision" => dispatch_record_lifecycle_decision(args),
        "record-admission" => dispatch_record_admission(args),
        "record-resource-admission" => dispatch_record_resource_admission(args),
        "record-idea-dedup" => dispatch_record_idea_dedup(args),
        "record-idea-consolidation" => dispatch_record_idea_consolidation(args),
        "approvals" => dispatch_approvals(args),
        other => Err(format!("unsupported command 'ooda {other}'").into()),
    }
}

/// `simard ooda record-decision` — the zero-privilege tool the OODA per-goal-
/// cycle reasoner calls to record EXACTLY ONE typed, validated decision.
///
/// It validates the closed `--choice` enum, requires a non-empty `--reason`,
/// hardens `--record-path` (absolute, no `..`), then writes exactly one atomic
/// `0o600` [`PerGoalDecisionRecord`]. Any validation failure ⇒ a non-zero exit
/// AND **no file on disk** (validate-all-then-write-once). The tool holds no
/// privilege: its only side effect is that one write. `RecipeBrain` reads the
/// record back with `read_verified` — it never scrapes the agent's stdout.
///
/// See `docs/reference/ooda-record-decision-cli.md` for the full contract.
fn dispatch_record_decision(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &[
        "choice",
        "reason",
        "reason-path",
        "task-hint",
        "task-hint-path",
        "record-path",
        "goal-id",
        "cycle-number",
    ];

    // `parse_named_args` already rejects duplicate flags and value-less flags.
    let parsed = parse_named_args(args)?;

    // Reject any unknown flag — never silently ignore an argument.
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    // --- Required scalar fields (validated before any write) ---
    let choice = required_named(&parsed, "choice")?;
    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);

    // SR-VAL-8 — the record path must be ABSOLUTE and free of `..` traversal.
    // The daemon supplies a fresh per-cycle temp dir; a relative or `..`-bearing
    // path is a misuse we reject before touching the filesystem.
    harden_path(record_path, "record-path")?;

    // --- Free text: inline XOR file, per field (large payloads via file) ---
    let reason = resolve_field(&parsed, "reason", "reason-path")?
        .ok_or("a decision requires --reason or --reason-path")?;
    let task_hint = resolve_field(&parsed, "task-hint", "task-hint-path")?.unwrap_or_default();

    // Validate the closed enum + non-empty reason through the SINGLE shared
    // chokepoint (case-insensitive choice, sanitized+bounded free text). An
    // unknown choice or an empty reason ⇒ None ⇒ rejected here, before any write.
    let action = crate::ooda_brain::PerGoalAction::from_choice_fields(choice, &reason, &task_hint)
        .ok_or_else(|| {
            format!(
                "invalid decision: unknown --choice {choice:?} or empty --reason \
                 (choice must be one of continue|spawn|reorient|investigate|wait|complete)"
            )
        })?;

    let record = crate::ooda_brain::PerGoalDecisionRecord {
        schema: crate::ooda_brain::EXPECTED_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        action,
    };

    // Validate-all-then-write-once: this atomic, owner-only (0o600) write is the
    // tool's ONLY side effect. It runs only after EVERY check above passed.
    crate::persistence::persist_json("ooda-per-goal-decision", record_path, &record)?;
    Ok(())
}

/// `simard ooda record-outcome` — the zero-privilege tool the OODA closed-loop
/// OUTCOME-VERIFICATION reasoner calls to record EXACTLY ONE typed, validated
/// decision (Group D of epic #4719).
///
/// It validates the closed 4-variant `--choice` enum + non-empty `--reason`
/// through the SINGLE shared
/// [`GoalOutcomeDecision::from_choice_fields`](crate::ooda_brain::GoalOutcomeDecision::from_choice_fields)
/// chokepoint, hardens `--record-path` (absolute, no `..`), then writes exactly
/// one atomic `0o600`
/// [`OutcomeDecisionRecord`](crate::ooda_brain::OutcomeDecisionRecord). Any
/// validation failure ⇒ a non-zero exit AND **no file on disk**
/// (validate-all-then-write-once). `--replan-hint` is OWNED by `replan`
/// (optional even there) and REJECTED on every other choice by the chokepoint.
/// `RecipeBrain` reads the record back with `read_verified_outcome` — it never
/// scrapes the agent's stdout.
fn dispatch_record_outcome(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &[
        "choice",
        "reason",
        "reason-path",
        "replan-hint",
        "replan-hint-path",
        "record-path",
        "goal-id",
        "cycle-number",
    ];

    let parsed = parse_named_args(args)?;
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    let choice = required_named(&parsed, "choice")?;
    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);
    harden_path(record_path, "record-path")?;

    let reason = resolve_field(&parsed, "reason", "reason-path")?
        .ok_or("an outcome decision requires --reason or --reason-path")?;
    // Optional variant-owned free text (single field). The chokepoint enforces
    // that only `replan` may carry a non-empty hint — a hint supplied on any
    // other choice is rejected there, before any write.
    let replan_hint =
        resolve_field(&parsed, "replan-hint", "replan-hint-path")?.unwrap_or_default();

    let decision =
        crate::ooda_brain::GoalOutcomeDecision::from_choice_fields(choice, &reason, &replan_hint)
            .ok_or_else(|| {
            format!(
                "invalid outcome decision: unknown --choice {choice:?}, empty --reason, or a \
                     --replan-hint on a non-replan choice (choice must be one of \
                     mark_achieved|reopen|replan|keep_open_and_report; --replan-hint is owned by \
                     replan)"
            )
        })?;

    let record = crate::ooda_brain::OutcomeDecisionRecord {
        schema: crate::ooda_brain::OUTCOME_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        decision,
    };

    crate::persistence::persist_json("ooda-goal-outcome-decision", record_path, &record)?;
    Ok(())
}

/// `simard ooda record-orient` — the zero-privilege tool the OODA **Orient**
/// reasoner calls to record EXACTLY ONE typed, validated urgency judgment.
///
/// It validates the numeric fields + reason through the SINGLE shared
/// [`OrientFields::from_fields`](crate::ooda_brain::OrientFields::from_fields)
/// chokepoint (finite, `[0,1]`, no escalation `adjusted ≤ base`, non-empty
/// sanitized reason), hardens `--record-path` (absolute, no `..`), then writes
/// exactly one atomic `0o600` [`OrientDecisionRecord`](crate::ooda_brain::OrientDecisionRecord).
/// Any validation failure ⇒ a non-zero exit AND **no file on disk**
/// (validate-all-then-write-once). `--confidence`, `--demotion-applied`, and
/// `--base-urgency` are REQUIRED — the typed CLI deliberately tightens the
/// legacy wire's `#[serde(default)]` behaviour so writer and reader agree.
///
/// See `docs/reference/ooda-record-orient-decide-cli.md` for the full contract.
fn dispatch_record_orient(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &[
        "adjusted-urgency",
        "confidence",
        "demotion-applied",
        "base-urgency",
        "reason",
        "reason-path",
        "record-path",
        "goal-id",
        "cycle-number",
    ];

    let parsed = parse_named_args(args)?;
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);
    harden_path(record_path, "record-path")?;

    let parse_f64 = |flag: &str| -> Result<f64, Box<dyn std::error::Error>> {
        required_named(&parsed, flag)?
            .parse::<f64>()
            .map_err(|_| format!("invalid --{flag} (expected a float)").into())
    };
    let adjusted_urgency = parse_f64("adjusted-urgency")?;
    let confidence = parse_f64("confidence")?;
    let demotion_applied = parse_f64("demotion-applied")?;
    let base_urgency = parse_f64("base-urgency")?;

    let reason = resolve_field(&parsed, "reason", "reason-path")?
        .ok_or("an orient judgment requires --reason or --reason-path")?;

    // Validate the numerics + reason through the SINGLE shared chokepoint. Any
    // non-finite / out-of-range / escalating value or an empty reason ⇒ Err
    // here, before any write.
    let fields = crate::ooda_brain::OrientFields::from_fields(
        adjusted_urgency,
        confidence,
        demotion_applied,
        &reason,
        base_urgency,
    )
    .map_err(|e| format!("invalid orient judgment: {e}"))?;

    let record = crate::ooda_brain::OrientDecisionRecord {
        schema: crate::ooda_brain::ORIENT_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        base_urgency,
        fields,
    };

    crate::persistence::persist_json("ooda-orient-decision", record_path, &record)?;
    Ok(())
}

/// `simard ooda record-decide` — the zero-privilege tool the OODA **Decide**
/// reasoner calls to record EXACTLY ONE typed, validated action routing.
///
/// It validates the closed 10-variant `--choice` enum + non-empty `--reason`
/// through the SINGLE shared
/// [`DecideChoice::from_choice_fields`](crate::ooda_brain::DecideChoice::from_choice_fields)
/// chokepoint, hardens `--record-path` (absolute, no `..`), then writes exactly
/// one atomic `0o600` [`DecideDecisionRecord`](crate::ooda_brain::DecideDecisionRecord).
/// Any validation failure ⇒ a non-zero exit AND **no file on disk**
/// (validate-all-then-write-once).
///
/// See `docs/reference/ooda-record-orient-decide-cli.md` for the full contract.
fn dispatch_record_decide(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &[
        "choice",
        "reason",
        "reason-path",
        "record-path",
        "goal-id",
        "cycle-number",
    ];

    let parsed = parse_named_args(args)?;
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    let choice = required_named(&parsed, "choice")?;
    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);
    harden_path(record_path, "record-path")?;

    let reason = resolve_field(&parsed, "reason", "reason-path")?
        .ok_or("a decision requires --reason or --reason-path")?;

    // Validate the closed enum + non-empty reason through the SINGLE shared
    // chokepoint. An unknown choice or an empty reason ⇒ None ⇒ rejected here,
    // before any write.
    let choice =
        crate::ooda_brain::DecideChoice::from_choice_fields(choice, &reason).ok_or_else(|| {
            format!(
                "invalid decision: unknown --choice {choice:?} or empty --reason \
                 (choice must be one of poll_developer_activity|consolidate_memory|\
                 run_improvement|extract_ideas|safe_update|research_query|run_gym_eval|\
                 build_skill|launch_session|advance_goal)"
            )
        })?;

    let record = crate::ooda_brain::DecideDecisionRecord {
        schema: crate::ooda_brain::DECIDE_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        choice,
    };

    crate::persistence::persist_json("ooda-decide-decision", record_path, &record)?;
    Ok(())
}

/// `simard ooda record-lifecycle-decision` — the zero-privilege tool the OODA
/// engineer-lifecycle reasoner calls to record EXACTLY ONE typed, validated Act
/// decision (Group E, #4967; retires the last reasoner-decision stdout scrape).
///
/// It validates the closed `--decision` variant + bounds/sanitizes the optional
/// `--rationale` through the SINGLE shared
/// [`sanitize_lifecycle_fields`](crate::ooda_brain::sanitize_lifecycle_fields)
/// chokepoint (the same one the reader applies, so writer and reader can never
/// drift), hardens `--record-path` (absolute, no `..`), stamps `written_at_epoch`
/// = now, then writes exactly one atomic `0o600`
/// [`EngineerLifecycleRecord`](crate::ooda_brain::EngineerLifecycleRecord). Any
/// validation failure ⇒ a non-zero exit AND **no file on disk**
/// (validate-all-then-write-once). The tool holds no privilege: its only side
/// effect is that one write. `RecipeBrain` reads the record back with
/// [`read_verified_engineer_lifecycle_decision`](crate::ooda_brain::read_verified_engineer_lifecycle_decision)
/// — it never scrapes the agent's stdout.
///
/// Unlike the sibling record verbs, `--rationale` is OPTIONAL (an empty
/// rationale is a valid record); the extra-field variants
/// (`reclaim_and_redispatch` / `open_tracking_issue` / `mark_goal_blocked`)
/// derive their body/reason/redispatch text from it, exactly as the retired
/// scrape path did.
fn dispatch_record_lifecycle_decision(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &[
        "decision",
        "rationale",
        "rationale-path",
        "record-path",
        "goal-id",
        "cycle-number",
    ];

    let parsed = parse_named_args(args)?;
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    let decision = required_named(&parsed, "decision")?;
    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);
    harden_path(record_path, "record-path")?;

    // --rationale is OPTIONAL (empty is valid); the extra-field variants reuse it.
    let rationale = resolve_field(&parsed, "rationale", "rationale-path")?.unwrap_or_default();

    // Validate the closed variant + bound/sanitize the rationale through the
    // SINGLE shared chokepoint. An out-of-set decision or an oversize rationale
    // ⇒ None ⇒ rejected here, before any write. Returns the canonical token +
    // sanitized rationale, so the persisted record is already normalized.
    let (canonical, clean_rationale) =
        crate::ooda_brain::sanitize_lifecycle_fields(decision, &rationale).ok_or_else(|| {
            format!(
                "invalid lifecycle decision: unknown --decision {decision:?} or a rationale that \
                 is too long after sanitize (decision must be one of {})",
                crate::ooda_brain::LIFECYCLE_VARIANT_LIST
            )
        })?;

    let written_at_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let record = crate::ooda_brain::EngineerLifecycleRecord {
        schema: crate::ooda_brain::ENGINEER_LIFECYCLE_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        decision: canonical.to_string(),
        rationale: clean_rationale,
        written_at_epoch,
    };

    crate::persistence::persist_json("ooda-engineer-lifecycle-decision", record_path, &record)?;
    Ok(())
}

/// `simard ooda record-admission` — the zero-privilege tool the OODA
/// engineer-admission reasoner calls to record EXACTLY ONE typed, validated
/// verdict.
///
/// It validates the closed 3-variant `--choice` enum + non-empty `--rationale`
/// AND the per-variant field ownership through the SINGLE shared
/// [`EngineerAdmissionDecision::from_choice_fields`](crate::ooda_brain::EngineerAdmissionDecision::from_choice_fields)
/// chokepoint, hardens `--record-path` (absolute, no `..`), then writes exactly
/// one atomic `0o600` [`AdmissionDecisionRecord`](crate::ooda_brain::AdmissionDecisionRecord).
/// Any validation failure ⇒ a non-zero exit AND **no file on disk**
/// (validate-all-then-write-once). `--blocked-by` / `--overlap-files` are
/// single-value CSV (a repeated flag is rejected by `parse_named_args`).
///
/// See `docs/reference/ooda-record-admission-cli.md` for the full contract.
fn dispatch_record_admission(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &[
        "choice",
        "rationale",
        "rationale-path",
        "blocked-by",
        "retry-after-secs",
        "after-goal-id",
        "overlap-files",
        "record-path",
        "goal-id",
        "cycle-number",
    ];

    let parsed = parse_named_args(args)?;
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    let choice = required_named(&parsed, "choice")?;
    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);
    harden_path(record_path, "record-path")?;

    let rationale = resolve_field(&parsed, "rationale", "rationale-path")?
        .ok_or("an admission verdict requires --rationale or --rationale-path")?;

    // Optional variant-owned fields (single-value CSV for the two list flags).
    // The chokepoint enforces which variant may carry which — a field supplied
    // on the wrong variant is rejected there, before any write.
    let blocked_by = parsed
        .get("blocked-by")
        .map(|v| split_csv(v))
        .unwrap_or_default();
    let overlap_files = parsed
        .get("overlap-files")
        .map(|v| split_csv(v))
        .unwrap_or_default();
    let after_goal_id = parsed
        .get("after-goal-id")
        .map(String::as_str)
        .unwrap_or("");
    let retry_after_secs = match parsed.get("retry-after-secs") {
        Some(v) => Some(
            v.parse::<u64>()
                .map_err(|_| "invalid --retry-after-secs (expected a u64)")?,
        ),
        None => None,
    };

    let decision = crate::ooda_brain::EngineerAdmissionDecision::from_choice_fields(
        choice,
        &rationale,
        &blocked_by,
        after_goal_id,
        &overlap_files,
        retry_after_secs,
    )
    .ok_or_else(|| {
        format!(
            "invalid admission verdict: unknown --choice {choice:?}, empty --rationale, or a \
             variant-owned field on the wrong variant (choice must be one of \
             admit|defer|serialize_after; --blocked-by/--retry-after-secs are owned by defer, \
             --after-goal-id/--overlap-files by serialize_after)"
        )
    })?;

    let record = crate::ooda_brain::AdmissionDecisionRecord {
        schema: crate::ooda_brain::ADMISSION_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        decision,
    };

    crate::persistence::persist_json("ooda-engineer-admission-decision", record_path, &record)?;
    Ok(())
}

/// `simard ooda record-resource-admission` — the zero-privilege tool the OODA
/// resource-admission reasoner calls to record EXACTLY ONE typed, validated
/// verdict.
///
/// It validates the closed 3-variant `--choice` enum + non-empty `--rationale`
/// through the SINGLE shared
/// [`ResourceAdmissionDecision::from_choice_fields`](crate::ooda_brain::ResourceAdmissionDecision::from_choice_fields)
/// chokepoint, hardens `--record-path` (absolute, no `..`), then writes exactly
/// one atomic `0o600`
/// [`ResourceAdmissionDecisionRecord`](crate::ooda_brain::ResourceAdmissionDecisionRecord).
/// All variants carry only `rationale`, so any engineer-admission-owned flag is
/// unknown here and rejected against `KNOWN_FLAGS`. Any validation failure ⇒ a
/// non-zero exit AND **no file on disk** (validate-all-then-write-once).
///
/// See `docs/reference/ooda-record-admission-cli.md` for the full contract.
fn dispatch_record_resource_admission(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &[
        "choice",
        "rationale",
        "rationale-path",
        "record-path",
        "goal-id",
        "cycle-number",
    ];

    let parsed = parse_named_args(args)?;
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    let choice = required_named(&parsed, "choice")?;
    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);
    harden_path(record_path, "record-path")?;

    let rationale = resolve_field(&parsed, "rationale", "rationale-path")?
        .ok_or("a resource-admission verdict requires --rationale or --rationale-path")?;

    let decision =
        crate::ooda_brain::ResourceAdmissionDecision::from_choice_fields(choice, &rationale)
            .ok_or_else(|| {
                format!(
                    "invalid resource-admission verdict: unknown --choice {choice:?} or empty \
                     --rationale (choice must be one of admit|defer|reclaim_first)"
                )
            })?;

    let record = crate::ooda_brain::ResourceAdmissionDecisionRecord {
        schema: crate::ooda_brain::RESOURCE_ADMISSION_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        decision,
    };

    crate::persistence::persist_json("ooda-resource-admission-decision", record_path, &record)?;
    Ok(())
}

/// `simard ooda record-idea-dedup` — the zero-privilege tool the Creative Ideas
/// SEMANTIC-dedup reasoner calls to record EXACTLY ONE typed, validated verdict
/// (issue #2925; Group C of epic #4719).
///
/// It validates the closed 3-variant `--choice` enum (`create_new|skip|
/// enhance_existing`, case-insensitive) + a non-empty `--reason` AND the
/// per-variant field ownership (`--target-node-id` REQUIRED on `enhance_existing`,
/// REJECTED on `create_new`/`skip`) through the SINGLE shared
/// [`IdeaDedupDecision::from_choice_fields`](crate::ooda_brain::IdeaDedupDecision::from_choice_fields)
/// chokepoint, hardens `--record-path` (absolute, no `..`), then writes exactly
/// one atomic `0o600`
/// [`IdeaDedupDecisionRecord`](crate::ooda_brain::IdeaDedupDecisionRecord). Any
/// validation failure ⇒ a non-zero exit AND **no file on disk**
/// (validate-all-then-write-once).
///
/// See `docs/reference/ooda-record-idea-dedup-consolidation-cli.md`.
fn dispatch_record_idea_dedup(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &[
        "choice",
        "reason",
        "reason-path",
        "target-node-id",
        "record-path",
        "goal-id",
        "cycle-number",
    ];

    let parsed = parse_named_args(args)?;
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    let choice = required_named(&parsed, "choice")?;
    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);
    harden_path(record_path, "record-path")?;

    let reason = resolve_field(&parsed, "reason", "reason-path")?
        .ok_or("a dedup verdict requires --reason or --reason-path")?;

    // Optional variant-owned field. The chokepoint enforces which variant may
    // carry it — a target on create_new/skip, or a missing target on
    // enhance_existing, is rejected there, before any write.
    let target_node_id = parsed
        .get("target-node-id")
        .map(String::as_str)
        .unwrap_or("");

    let decision =
        crate::ooda_brain::IdeaDedupDecision::from_choice_fields(choice, &reason, target_node_id)
            .ok_or_else(|| {
            format!(
                "invalid dedup verdict: unknown --choice {choice:?}, empty --reason, or a \
                     misplaced --target-node-id (choice must be one of \
                     create_new|skip|enhance_existing; --target-node-id is required on \
                     enhance_existing and rejected on create_new/skip)"
            )
        })?;

    let record = crate::ooda_brain::IdeaDedupDecisionRecord {
        schema: crate::ooda_brain::IDEA_DEDUP_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        decision,
    };

    crate::persistence::persist_json("ooda-idea-dedup-decision", record_path, &record)?;
    Ok(())
}

/// `simard ooda record-idea-consolidation` — the zero-privilege tool the
/// Creative Ideas CONSOLIDATION reasoner calls to record EXACTLY ONE typed,
/// validated cluster list (issue #2925; Group C of epic #4719).
///
/// The clusters are a LIST, not an enum, so they are read from the JSON-array
/// FILE at `--clusters-path` (inline argv would hit E2BIG for large lists). Each
/// cluster passes the shared
/// [`IdeaCluster::sanitized`](crate::ooda_brain::IdeaCluster::sanitized)
/// chokepoint (headless clusters dropped, fields sanitized + bounded); the list
/// is capped at 64. An empty array `[]` is a VALID "nothing to consolidate"
/// record. Both paths are hardened (absolute, no `..`); the tool writes exactly
/// one atomic `0o600`
/// [`IdeaConsolidationRecord`](crate::ooda_brain::IdeaConsolidationRecord). Any
/// validation failure ⇒ a non-zero exit AND **no file on disk**.
///
/// See `docs/reference/ooda-record-idea-dedup-consolidation-cli.md`.
fn dispatch_record_idea_consolidation(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    const KNOWN_FLAGS: &[&str] = &["clusters-path", "record-path", "goal-id", "cycle-number"];

    let parsed = parse_named_args(args)?;
    for flag in parsed.keys() {
        if !KNOWN_FLAGS.contains(&flag.as_str()) {
            return Err(format!("unknown option --{flag}").into());
        }
    }

    let goal_id = required_named(&parsed, "goal-id")?;
    let cycle_number: u32 = required_named(&parsed, "cycle-number")?
        .parse()
        .map_err(|_| "invalid --cycle-number (expected a u32)")?;
    let record_path = Path::new(required_named(&parsed, "record-path")?);
    harden_path(record_path, "record-path")?;

    let clusters_path = Path::new(required_named(&parsed, "clusters-path")?);
    harden_path(clusters_path, "clusters-path")?;
    let raw = read_bounded_clusters_file(clusters_path)?;

    // Validate-all-then-write-once: the whole array must parse before any write.
    let parsed_clusters: Vec<crate::ooda_brain::IdeaCluster> = serde_json::from_str(&raw)
        .map_err(|e| format!("--clusters-path must be a JSON array of clusters: {e}"))?;

    // Sanitize each cluster through the SAME chokepoint the reader re-runs;
    // headless clusters are dropped, and the list is capped at 64.
    let clusters: Vec<crate::ooda_brain::IdeaCluster> = parsed_clusters
        .iter()
        .filter_map(crate::ooda_brain::IdeaCluster::sanitized)
        .take(64)
        .collect();

    let record = crate::ooda_brain::IdeaConsolidationRecord {
        schema: crate::ooda_brain::IDEA_CONSOLIDATION_SCHEMA.to_string(),
        goal_id: goal_id.to_string(),
        cycle_number,
        clusters,
    };

    crate::persistence::persist_json("ooda-idea-consolidation-decision", record_path, &record)?;
    Ok(())
}

/// Split a single comma-separated list-flag value into its trimmed, non-empty
/// elements. The admission list flags (`--blocked-by`, `--overlap-files`) accept
/// ONE comma-separated value because `parse_named_args` rejects a repeated flag;
/// this keeps them consistent with that single-value contract. Empty elements
/// (e.g. a trailing comma) are dropped; the chokepoint sanitizes each survivor.
fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Maximum bytes read from a `--<field>-path` input file before failing closed.
/// The free-text fields are bounded to 500 chars downstream, so a 64 KiB cap is
/// ample headroom for legitimate (multi-byte) input while preventing a transient
/// OOM from reading a hostile or accidental huge file into memory first.
const MAX_FIELD_FILE_BYTES: u64 = 64 * 1024;

/// Harden a caller-supplied path (SR-VAL-8): it must be ABSOLUTE and free of
/// `..` traversal. Shared by the record output path and the free-text input
/// file paths so every path the tool touches passes the identical gate.
fn harden_path(path: &Path, flag: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !path.is_absolute() {
        return Err(format!("--{flag} must be absolute, got {}", path.display()).into());
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("--{flag} must not contain '..', got {}", path.display()).into());
    }
    Ok(())
}

/// Read at most [`MAX_FIELD_FILE_BYTES`] from a field-input file, failing closed
/// if the file is larger. The daemon owns these paths, so an oversized file is
/// misuse we surface as a hard error rather than silently truncating.
fn read_bounded_field_file(path: &Path, flag: &str) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut reader = std::fs::File::open(path)?.take(MAX_FIELD_FILE_BYTES + 1);
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    if buf.len() as u64 > MAX_FIELD_FILE_BYTES {
        return Err(format!(
            "--{flag} file {} exceeds the {MAX_FIELD_FILE_BYTES}-byte cap",
            path.display()
        )
        .into());
    }
    Ok(buf)
}

/// Maximum bytes read from a `--clusters-path` JSON-array file before failing
/// closed. Larger than [`MAX_FIELD_FILE_BYTES`] because a consolidation list can
/// carry up to 64 clusters each with several bounded free-text fields — 1 MiB is
/// ample headroom while still bounding a hostile/accidental huge file.
const MAX_CLUSTERS_FILE_BYTES: u64 = 1024 * 1024;

/// Read at most [`MAX_CLUSTERS_FILE_BYTES`] from the `--clusters-path` file,
/// failing closed if the file is larger. The list is re-capped + re-sanitized
/// after parsing; this only bounds the raw read to prevent a transient OOM.
fn read_bounded_clusters_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut reader = std::fs::File::open(path)?.take(MAX_CLUSTERS_FILE_BYTES + 1);
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    if buf.len() as u64 > MAX_CLUSTERS_FILE_BYTES {
        return Err(format!(
            "--clusters-path file {} exceeds the {MAX_CLUSTERS_FILE_BYTES}-byte cap",
            path.display()
        )
        .into());
    }
    Ok(buf)
}

/// Resolve one free-text field that may be supplied inline (`--<inline>`) OR
/// from a file (`--<path>`), but never both. Returns `Ok(None)` when neither is
/// present (the caller decides whether that field is required). A file source is
/// hardened (absolute, no `..`) and read under a byte cap before use.
fn resolve_field(
    parsed: &std::collections::BTreeMap<String, String>,
    inline: &str,
    path: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match (parsed.get(inline), parsed.get(path)) {
        (Some(_), Some(_)) => Err(format!("--{inline} and --{path} are mutually exclusive").into()),
        (Some(value), None) => Ok(Some(value.clone())),
        (None, Some(file)) => {
            let file_path = Path::new(file);
            harden_path(file_path, path)?;
            Ok(Some(read_bounded_field_file(file_path, path)?))
        }
        (None, None) => Ok(None),
    }
}

fn dispatch_terminal(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let terminal = next_required(&mut args, "terminal command")?;
    let parsed = parse_named_args(args)?;
    let ledger_path = Path::new(required_named(&parsed, "ledger-path")?);
    let policy_path = Path::new(required_named(&parsed, "policy-path")?);
    let session_id = required_named(&parsed, "session-id")?;
    let cycle_id = required_named(&parsed, "cycle-id")?;
    let policy = crate::typed_ooda::CapabilityPolicy::from_toml_file(policy_path)?;
    let handler = crate::typed_ooda::CapabilityHandler::open(ledger_path, policy)?;
    if terminal == "status" {
        let status = if handler.terminal_for_cycle(session_id, cycle_id)?.is_some() {
            "present"
        } else {
            "missing"
        };
        println!("{status}");
        return Ok(());
    }
    let goal_id = required_named(&parsed, "goal-id")?;
    let token = std::fs::read_to_string(required_named(&parsed, "auth-token-path")?)?;
    let actor = handler.authenticate_actor_session(token.trim(), session_id, cycle_id, goal_id)?;
    let admission: crate::typed_ooda::AdmissionSnapshot =
        serde_json::from_slice(&std::fs::read(required_named(&parsed, "admission-path")?)?)?;
    let request_id = required_named(&parsed, "request-id")?;
    let identity =
        crate::typed_ooda::TerminalRequestIdentity::new(request_id, session_id, cycle_id, goal_id);
    let raw_semantic = read_opaque(&parsed, "raw-semantic-path")?;
    let outcome = match terminal.as_str() {
        "spawn-engineer" => {
            let repository = actor
                .bound_repository()
                .cloned()
                .ok_or("authenticated actor has no repository scope")?;
            let claim_key = format!("{}/{}:{goal_id}", repository.owner, repository.name);
            handler.record_action(
                &actor,
                crate::typed_ooda::RecordActionRequest {
                    identity,
                    action: crate::typed_ooda::Action::SpawnEngineer(
                        crate::typed_ooda::SpawnEngineerAction {
                            task: read_opaque(&parsed, "task-path")?,
                            repository,
                            base_type: crate::typed_ooda::BaseType::Copilot,
                            requested_permissions: actor.engineer_permissions().clone(),
                            claim_key,
                        },
                    ),
                    raw_semantic,
                    evidence: Vec::new(),
                },
                &admission,
            )?
        }
        "no-action" => handler.record_no_action(
            &actor,
            crate::typed_ooda::RecordNoActionRequest {
                identity,
                reason: read_opaque(&parsed, "reason-path")?,
                raw_semantic,
                evidence: Vec::new(),
            },
        )?,
        "blocked" => handler.record_blocked(
            &actor,
            crate::typed_ooda::RecordBlockedRequest {
                identity,
                reason: read_opaque(&parsed, "reason-path")?,
                blocker: crate::typed_ooda::BlockerRef::External {
                    provider: "goal-session".to_string(),
                    reference: required_named(&parsed, "blocker")?.to_string(),
                },
                retry: crate::typed_ooda::RetryPolicy::Never,
                raw_semantic,
                evidence: Vec::new(),
            },
        )?,
        "completed" => {
            let verification_evidence = read_typed_evidence(&parsed, "evidence-path")?;
            handler.record_completed(
                &actor,
                crate::typed_ooda::RecordCompletedRequest {
                    identity,
                    summary: read_opaque(&parsed, "summary-path")?,
                    completion: crate::typed_ooda::CompletionRef {
                        criterion_id: required_named(&parsed, "criterion-id")?.to_string(),
                        verification_evidence: verification_evidence.clone(),
                    },
                    raw_semantic,
                    evidence: verification_evidence,
                },
            )?
        }
        other => return Err(format!("unsupported command 'ooda terminal {other}'").into()),
    };
    println!("{}", outcome.outcome_id);
    Ok(())
}

fn read_opaque(
    values: &std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<crate::typed_ooda::OpaqueBytes, Box<dyn std::error::Error>> {
    Ok(crate::typed_ooda::OpaqueBytes::from(std::fs::read(
        required_named(values, key)?,
    )?))
}

/// Read a typed evidence list from a JSON file the actor wrote via its file
/// tool. Evidence is a machine-owned tool-protocol argument (a `Vec` of typed
/// `EvidenceRef`), not agent prose, so deserializing it here does not violate
/// the zero-parser invariant. A completion must carry at least one entry.
fn read_typed_evidence(
    values: &std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<Vec<crate::typed_ooda::EvidenceRef>, Box<dyn std::error::Error>> {
    let path = required_named(values, key)?;
    let bytes = std::fs::read(path)?;
    let evidence: Vec<crate::typed_ooda::EvidenceRef> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("--{key} must be a JSON array of typed evidence: {error}"))?;
    Ok(evidence)
}

fn dispatch_approvals(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let command = next_required(&mut args, "approvals command")?;
    if command != "issue" {
        return Err(format!("unsupported command 'ooda approvals {command}'").into());
    }
    let parsed = parse_named_args(args)?;
    let state_root = Path::new(required_named(&parsed, "state-root")?);
    let effect_id = required_named(&parsed, "effect-id")?;
    let request_id = required_named(&parsed, "request-id")?;
    let handler = open_ledger(state_root)?;
    let authority = crate::typed_ooda::ApprovalAuthority::from_environment()?;
    let approval = handler.issue_privileged_approval(&authority, request_id, effect_id)?;
    println!("{}", serde_json::to_string(&approval)?);
    Ok(())
}

fn dispatch_outcomes(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let command = next_required(&mut args, "outcomes command")?;
    let parsed = parse_named_args(args)?;
    let state_root = required_named(&parsed, "state-root")?;
    let handler = open_ledger(Path::new(state_root))?;
    match command.as_str() {
        "get" => {
            let request_id = required_named(&parsed, "request-id")?;
            let outcome = handler
                .terminal_for_request(request_id)?
                .ok_or_else(|| format!("typed outcome not found for request {request_id:?}"))?;
            let effect = handler.effect_for_outcome(&outcome.outcome_id)?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "outcome": outcome,
                    "effect": effect,
                }))?
            );
            Ok(())
        }
        "list" => {
            let limit = parsed
                .get("limit")
                .map(|value| value.parse::<usize>())
                .transpose()
                .map_err(|_| "--limit must be a positive integer")?
                .unwrap_or(100);
            let outcomes = handler.list_terminals(limit)?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({ "outcomes": outcomes }))?
            );
            Ok(())
        }
        other => Err(format!("unsupported command 'ooda outcomes {other}'").into()),
    }
}

fn dispatch_fixture(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("SIMARD_TYPED_OODA_FIXTURE").ok().as_deref() != Some("1") {
        return Err("typed OODA fixture is disabled; set SIMARD_TYPED_OODA_FIXTURE=1 only in an isolated acceptance environment".into());
    }
    let command = next_required(&mut args, "fixture command")?;
    if command != "run" {
        return Err(format!("unsupported command 'ooda fixture {command}'").into());
    }
    let parsed = parse_named_args(args)?;
    let state_root = Path::new(required_named(&parsed, "state-root")?);
    let scenario = required_named(&parsed, "scenario")?;
    let request_id = required_named(&parsed, "request-id")?;
    if matches!(scenario, "agent-spawn-engineer" | "agent-no-action") {
        return dispatch_agent_fixture(state_root, scenario, request_id);
    }
    let handler = open_ledger(state_root)?;
    let session_id = "typed-ooda-fixture";
    let cycle_id = format!("cycle-{request_id}");
    let actor = crate::typed_ooda::AuthenticatedToolContext::new(
        "typed-ooda-fixture",
        session_id,
        [
            crate::typed_ooda::CapabilityGrant::RecordAction(
                crate::typed_ooda::ActionKind::SpawnEngineer,
            ),
            crate::typed_ooda::CapabilityGrant::RecordNoAction,
        ],
    )
    .scoped_to_repository(crate::typed_ooda::RepositoryRef::new("rysweet", "Simard"))
    .with_engineer_permissions(["repo_read"]);
    let executor = crate::typed_ooda::GoalSessionExecutor::new(
        handler,
        actor,
        crate::typed_ooda::AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 0,
            active_claims: BTreeSet::new(),
            policy_revision: "goal-session-policy-v1".to_string(),
        },
        Box::new(FixtureEffects),
    );
    let invocation = crate::typed_ooda::GoalSessionInvocation {
        session_id: session_id.to_string(),
        cycle_id,
        goal_id: "fixture-goal".to_string(),
        task: crate::typed_ooda::OpaqueBytes::from(
            b"\nfixture task\0\x1b[31m marker-looking text\n".to_vec(),
        ),
        reason: crate::typed_ooda::OpaqueBytes::from(b"fixture reason\n".to_vec()),
        observe_output: crate::typed_ooda::OpaqueBytes::from(b"fixture observe\n".to_vec()),
        orient_output: crate::typed_ooda::OpaqueBytes::from(b"fixture orient\n".to_vec()),
        decide_output: crate::typed_ooda::OpaqueBytes::from(b"fixture decide\n".to_vec()),
    };
    let execution = executor.execute(&invocation, |received, tools| {
        match scenario {
            "spawn-engineer" => {
                tools.record_action(
                    request_id,
                    crate::typed_ooda::Action::SpawnEngineer(
                        crate::typed_ooda::SpawnEngineerAction {
                            task: received.task.clone(),
                            repository: crate::typed_ooda::RepositoryRef::new("rysweet", "Simard"),
                            base_type: crate::typed_ooda::BaseType::Copilot,
                            requested_permissions: BTreeSet::from(["repo_read".to_string()]),
                            claim_key: "rysweet/Simard:fixture-goal".to_string(),
                        },
                    ),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
            }
            "no-action" => {
                tools.record_no_action(
                    request_id,
                    received.reason.clone(),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
            }
            other => {
                return Err(crate::typed_ooda::RecipeProcessError::failed(format!(
                    "unknown fixture scenario {other:?}"
                )));
            }
        }
        Ok(())
    })?;
    let effect = executor
        .handler()
        .effect_for_outcome(&execution.outcome.outcome_id)?;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "outcome": execution.outcome,
            "effect": effect,
        }))?
    );
    Ok(())
}

fn dispatch_agent_fixture(
    state_root: &Path,
    scenario: &str,
    request_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = std::env::current_dir()?;
    let route = crate::typed_ooda::TypedGoalSessionRoute::production(&repo_root)?;
    let policy = route.load_policy()?;
    let ledger_path = crate::typed_ooda::ledger_path(state_root);
    std::fs::create_dir_all(
        ledger_path
            .parent()
            .ok_or_else(|| std::io::Error::other("typed-OODA ledger path has no parent"))?,
    )?;
    let handler = crate::typed_ooda::CapabilityHandler::open(&ledger_path, policy)?;
    let session_id = format!("typed-ooda-agent-fixture-{request_id}");
    let cycle_id = format!("agent-cycle-{request_id}");
    let goal_id = format!("agent-goal-{request_id}");
    let actor = crate::typed_ooda::AuthenticatedToolContext::new(
        "goal-session-actor",
        &session_id,
        [
            crate::typed_ooda::CapabilityGrant::RecordAction(
                crate::typed_ooda::ActionKind::SpawnEngineer,
            ),
            crate::typed_ooda::CapabilityGrant::RecordNoAction,
            crate::typed_ooda::CapabilityGrant::RecordBlocked,
            crate::typed_ooda::CapabilityGrant::RecordCompleted,
        ],
    )
    .scoped_to_repository(crate::typed_ooda::RepositoryRef::new("rysweet", "Simard"))
    .scoped_to_working_directory(&repo_root)
    .with_engineer_permissions(["repo_read", "repo_write"]);
    let (task, reason) = match scenario {
        "agent-spawn-engineer" => (
            "No engineer, branch, or pull request exists for this bounded goal. Start one engineer to implement it.",
            "The goal is actionable now and needs a single engineer.",
        ),
        "agent-no-action" => (
            "An engineer is already active for this goal and reported progress moments ago.",
            "Avoid duplicate work while the active engineer continues.",
        ),
        _ => unreachable!("caller restricts agent fixture scenarios"),
    };
    let invocation = crate::typed_ooda::GoalSessionInvocation {
        session_id: session_id.clone(),
        cycle_id,
        goal_id,
        task: crate::typed_ooda::OpaqueBytes::from(task.as_bytes().to_vec()),
        reason: crate::typed_ooda::OpaqueBytes::from(reason.as_bytes().to_vec()),
        observe_output: crate::typed_ooda::OpaqueBytes::from(
            b"Observe found the stated engineer lifecycle facts.".to_vec(),
        ),
        orient_output: crate::typed_ooda::OpaqueBytes::from(
            b"Orient found no conflicting higher-priority constraint.".to_vec(),
        ),
        decide_output: crate::typed_ooda::OpaqueBytes::from(
            b"Decide delegated the semantic terminal choice to this actor.".to_vec(),
        ),
    };
    let execution = route.execute(
        &repo_root,
        &ledger_path,
        &handler,
        &actor,
        &crate::typed_ooda::AdmissionSnapshot {
            concurrent_engineers: usize::from(scenario == "agent-no-action"),
            disk_used_percent: 0,
            active_claims: if scenario == "agent-no-action" {
                BTreeSet::from([format!("rysweet/Simard:{}", invocation.goal_id)])
            } else {
                BTreeSet::new()
            },
            policy_revision: "goal-session-policy-v1".to_string(),
        },
        &invocation,
    )?;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({ "outcome": execution.outcome }))?
    );
    Ok(())
}

struct FixtureEffects;

impl crate::typed_ooda::EffectExecutor for FixtureEffects {
    fn execute(
        &self,
        _job: &crate::typed_ooda::EffectJob,
    ) -> Result<crate::typed_ooda::EffectResult, crate::typed_ooda::EffectExecutionError> {
        Ok(crate::typed_ooda::EffectResult::Succeeded {
            evidence: Vec::new(),
        })
    }
}

fn open_ledger(
    state_root: &Path,
) -> Result<crate::typed_ooda::CapabilityHandler, Box<dyn std::error::Error>> {
    let ledger_path = crate::typed_ooda::ledger_path(state_root);
    std::fs::create_dir_all(
        ledger_path
            .parent()
            .ok_or_else(|| std::io::Error::other("typed-OODA ledger path has no parent"))?,
    )?;
    Ok(crate::typed_ooda::CapabilityHandler::open(
        ledger_path,
        crate::typed_ooda::CapabilityPolicy::goal_session_default("goal-session-policy-v1"),
    )?)
}

fn parse_named_args(
    args: impl Iterator<Item = String>,
) -> Result<std::collections::BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let values: Vec<_> = args.collect();
    let mut parsed = std::collections::BTreeMap::new();
    let mut index = 0;
    while index < values.len() {
        let flag = values[index]
            .strip_prefix("--")
            .ok_or_else(|| format!("expected named option, got {:?}", values[index]))?;
        let value = values
            .get(index + 1)
            .ok_or_else(|| format!("--{flag} requires a value"))?;
        if parsed.insert(flag.to_string(), value.clone()).is_some() {
            return Err(format!("duplicate option --{flag}").into());
        }
        index += 2;
    }
    Ok(parsed)
}

fn required_named<'a>(
    values: &'a std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    values
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("missing required option --{key}").into())
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    #[test]
    fn test_ooda_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["ooda".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected ooda command")
        );
    }

    #[test]
    fn test_ooda_unknown_subcommand() {
        let result = dispatch_operator_cli(vec!["ooda".to_string(), "xyz".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'ooda xyz'")
        );
    }

    #[test]
    fn test_ooda_help_exits_ok() {
        let result = dispatch_operator_cli(vec!["ooda".to_string(), "--help".to_string()]);
        assert!(result.is_ok(), "ooda --help must exit Ok, got: {result:?}");
    }

    #[test]
    fn test_ooda_short_help_exits_ok() {
        let result = dispatch_operator_cli(vec!["ooda".to_string(), "-h".to_string()]);
        assert!(result.is_ok(), "ooda -h must exit Ok, got: {result:?}");
    }

    #[test]
    fn test_ooda_run_invalid_cycles() {
        let result = dispatch_operator_cli(vec![
            "ooda".to_string(),
            "run".to_string(),
            "--cycles=abc".to_string(),
        ]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid --cycles"));
    }

    #[test]
    fn test_ooda_run_extra_positional_after_state_root() {
        let result = dispatch_operator_cli(vec![
            "ooda".to_string(),
            "run".to_string(),
            "/state".to_string(),
            "extra".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected argument")
        );
    }
}
