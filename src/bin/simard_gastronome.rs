//! `simard-gastronome` — the small "kitchen app" for the Gastronome identity.
//!
//! Turns a kitchen brief bundle (pantry + recipes + menu + event brief) into a
//! costed, nutrition-analysed, dietary-screened and prep-scheduled menu plan.
//! This is the end-to-end "brief → plan" deliverable, runnable with zero
//! external files via `--demo`.
//!
//! ## Usage
//! ```text
//! simard-gastronome --demo [--format text|json] [--strict]
//! simard-gastronome plan <bundle.json> [--format text|json] [--strict]
//! simard-gastronome --help
//! ```
//!
//! Exit codes: `0` on a successful plan, `1` on a usage/parse/planning error,
//! `3` when `--strict` is set and the plan is over budget or not dietary
//! compliant.

use std::process::ExitCode;

use simard::gastronome::BudgetReport;
use simard::gastronome::{KitchenBrief, MenuPlan, demo_bundle, plan_from_bundle, render_plan_text};

const HELP: &str = "\
simard-gastronome — costed, scheduled menu planning for the Gastronome identity

USAGE:
    simard-gastronome --demo [--format text|json] [--strict]
    simard-gastronome plan <bundle.json> [--format text|json] [--strict]
    simard-gastronome --help

ARGS:
    plan <bundle.json>   Plan the menu described by a kitchen brief bundle JSON.
    --demo               Plan a built-in sample brief (no input file needed).

OPTIONS:
    --format text|json   Output format (default: text).
    --strict             Exit non-zero (3) if the plan is over budget or not
                         dietary compliant.
    --help, -h           Show this help.

A kitchen brief bundle is a JSON object: { ingredients, recipes, menu, brief }.
See docs/tutorials/design-a-menu-with-the-gastronome.md for the schema.
";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Json,
}

struct Options {
    source: Source,
    format: Format,
    strict: bool,
}

enum Source {
    Demo,
    File(String),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(None) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Some(opts)) => run(&opts),
        Err(message) => {
            eprintln!("error: {message}\n");
            eprint!("{HELP}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    if args.is_empty() {
        return Ok(None);
    }

    let mut source: Option<Source> = None;
    let mut format = Format::Text;
    let mut strict = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => return Ok(None),
            "--demo" => set_source(&mut source, Source::Demo)?,
            "--strict" => strict = true,
            "--format" => {
                i += 1;
                let value = args.get(i).ok_or("--format requires a value")?;
                format = parse_format(value)?;
            }
            "plan" => {
                i += 1;
                let path = args.get(i).ok_or("plan requires a <bundle.json> path")?;
                set_source(&mut source, Source::File(path.clone()))?;
            }
            other => return Err(format!("unexpected argument '{other}'")),
        }
        i += 1;
    }

    let source = source.ok_or("nothing to plan: pass --demo or 'plan <bundle.json>'")?;
    Ok(Some(Options {
        source,
        format,
        strict,
    }))
}

fn set_source(slot: &mut Option<Source>, value: Source) -> Result<(), String> {
    if slot.is_some() {
        return Err("choose exactly one of --demo or 'plan <bundle.json>'".to_string());
    }
    *slot = Some(value);
    Ok(())
}

fn parse_format(value: &str) -> Result<Format, String> {
    match value {
        "text" => Ok(Format::Text),
        "json" => Ok(Format::Json),
        other => Err(format!("unknown --format '{other}' (expected text|json)")),
    }
}

fn run(opts: &Options) -> ExitCode {
    let bundle = match load_bundle(&opts.source) {
        Ok(bundle) => bundle,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

    let plan = match plan_from_bundle(&bundle) {
        Ok(plan) => plan,
        Err(err) => {
            eprintln!("error: could not plan menu: {err}");
            return ExitCode::FAILURE;
        }
    };

    match opts.format {
        Format::Text => print!("{}", render_plan_text(&plan)),
        Format::Json => match serde_json::to_string_pretty(&plan) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("error: could not serialise plan: {err}");
                return ExitCode::FAILURE;
            }
        },
    }

    if opts.strict && !plan_is_clean(&plan) {
        eprintln!("strict: plan is over budget or not dietary compliant");
        return ExitCode::from(3);
    }

    ExitCode::SUCCESS
}

fn plan_is_clean(plan: &MenuPlan) -> bool {
    plan.is_dietary_compliant() && !matches!(plan.budget, BudgetReport::OverBudget { .. })
}

fn load_bundle(source: &Source) -> Result<KitchenBrief, String> {
    match source {
        Source::Demo => Ok(demo_bundle()),
        Source::File(path) => {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| format!("could not read '{path}': {e}"))?;
            serde_json::from_str(&raw).map_err(|e| format!("could not parse '{path}': {e}"))
        }
    }
}
