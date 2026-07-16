//! Gastronome — a pluggable Simard identity for culinary, menu, and event
//! design.
//!
//! Gastronome designs recipes and menus and turns a catering brief into a
//! **costed, scheduled menu plan** end-to-end. This module is the offline,
//! deterministic engine behind that capability plus the `simard-kitchen` CLI
//! (the small "run a kitchen" app):
//!
//! - [`types`]      — the domain model (units, ingredients, recipes, briefs).
//! - [`book`]       — the pantry + recipe repertoire, TOML I/O, and a demo book.
//! - [`scaling`]    — scale a recipe to a target number of servings.
//! - [`cost`]       — ingredient → recipe cost roll-ups.
//! - [`nutrition`]  — per-serving macro analysis.
//! - [`scheduling`] — backward prep schedule anchored to service time.
//! - [`planner`]    — [`planner::plan_event`], the end-to-end brief → plan core.
//!
//! The whole pipeline is pure (no LLM, no clock, no network) so it runs in unit
//! tests and CI. The identity's prompts (`prompt_assets/simard/gastronome_*`)
//! wrap this engine for conversational menu design. See
//! `docs/howto/design-a-menu-with-gastronome.md`.

pub mod book;
pub mod cost;
pub mod nutrition;
pub mod planner;
pub mod scaling;
pub mod scheduling;
pub mod types;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use book::KitchenBook;
use planner::{plan_event, render_plan_json, render_plan_text};
use scaling::scale_recipe;
use types::{EventBrief, GastronomeError, GastronomeResult};

/// CLI usage string for `simard-kitchen`.
#[must_use]
pub fn gastronome_usage() -> &'static str {
    "usage: simard-kitchen <command>\n\
     \n\
     commands:\n\
     \x20 demo [--json]\n\
     \x20     Plan the built-in demo garden-wedding menu end-to-end.\n\
     \x20 plan --file <book.toml> [--brief <brief.toml>] [--json]\n\
     \x20     Cost + schedule the menu in an event brief.\n\
     \x20 shopping-list --file <book.toml> [--brief <brief.toml>] [--json]\n\
     \x20     Print only the consolidated shopping list.\n\
     \x20 schedule --file <book.toml> [--brief <brief.toml>] [--json]\n\
     \x20     Print only the backward-planned prep schedule.\n\
     \x20 scale --file <book.toml> --recipe <id> --servings <n> [--json]\n\
     \x20     Scale one recipe to a target number of servings.\n\
     \n\
     A <book.toml> holds [[ingredient]] and [[recipe]] tables and may embed a\n\
     [brief]. Pass --brief to override it with a standalone brief file. --json\n\
     emits machine-readable output. See\n\
     docs/howto/design-a-menu-with-gastronome.md."
}

/// Dispatch the `simard-kitchen` CLI over an argument iterator (argv minus the
/// program name).
///
/// # Errors
/// Returns an error for unknown commands, missing/invalid arguments, or any
/// underlying I/O / parse / planning failure.
pub fn dispatch_gastronome_cli<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let out = run(args)?;
    print!("{out}");
    Ok(())
}

/// Run the CLI and return its stdout text (kept pure for testing).
///
/// # Errors
/// See [`dispatch_gastronome_cli`].
pub fn run<I>(args: I) -> GastronomeResult<String>
where
    I: IntoIterator<Item = String>,
{
    let argv: Vec<String> = args.into_iter().collect();
    let command = argv
        .first()
        .ok_or_else(|| GastronomeError::Usage(gastronome_usage().to_string()))?;
    let rest = &argv[1..];
    match command.as_str() {
        "demo" => cmd_demo(rest),
        "plan" => cmd_plan(rest),
        "shopping-list" => cmd_shopping_list(rest),
        "schedule" => cmd_schedule(rest),
        "scale" => cmd_scale(rest),
        "help" | "--help" | "-h" => Ok(format!("{}\n", gastronome_usage())),
        other => Err(GastronomeError::Usage(format!(
            "unknown command '{other}'\n{}",
            gastronome_usage()
        ))),
    }
}

// ── argument parsing ─────────────────────────────────────────────────────────

struct ParsedArgs {
    flags: BTreeMap<String, String>,
    switches: Vec<String>,
}

/// Parse `--flag value` pairs and boolean `--switch`es. `bool_flags` names the
/// switches that take no value (e.g. `json`).
fn parse_args(
    rest: &[String],
    value_flags: &[&str],
    bool_flags: &[&str],
) -> GastronomeResult<ParsedArgs> {
    let mut flags = BTreeMap::new();
    let mut switches = Vec::new();
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        let Some(name) = arg.strip_prefix("--") else {
            return Err(GastronomeError::Usage(format!(
                "unexpected positional argument '{arg}'"
            )));
        };
        if bool_flags.contains(&name) {
            switches.push(name.to_string());
        } else if value_flags.contains(&name) {
            let value = iter.next().ok_or_else(|| {
                GastronomeError::Usage(format!("flag '--{name}' expects a value"))
            })?;
            flags.insert(name.to_string(), value.clone());
        } else {
            return Err(GastronomeError::Usage(format!("unknown flag '--{name}'")));
        }
    }
    Ok(ParsedArgs { flags, switches })
}

fn read_file(path: &str) -> GastronomeResult<String> {
    fs::read_to_string(Path::new(path)).map_err(|e| GastronomeError::Io(format!("{path}: {e}")))
}

/// Load a book from `--file`, and resolve the brief: an explicit `--brief`
/// file wins, otherwise the book's embedded `[brief]` is used.
fn load_book_and_brief(parsed: &ParsedArgs) -> GastronomeResult<(KitchenBook, EventBrief)> {
    let file = parsed
        .flags
        .get("file")
        .ok_or_else(|| GastronomeError::Usage("missing required flag '--file'".to_string()))?;
    let book = KitchenBook::from_toml(&read_file(file)?)?;
    let brief = match parsed.flags.get("brief") {
        Some(brief_path) => {
            let text = read_file(brief_path)?;
            let brief: EventBrief =
                toml::from_str(&text).map_err(|e| GastronomeError::Parse(e.to_string()))?;
            book.validate_brief(&brief)?;
            brief
        }
        None => book.brief.clone().ok_or_else(|| {
            GastronomeError::Usage(
                "book has no embedded [brief]; pass --brief <brief.toml>".to_string(),
            )
        })?,
    };
    Ok((book, brief))
}

fn wants_json(parsed: &ParsedArgs) -> bool {
    parsed.switches.iter().any(|s| s == "json")
}

// ── commands ─────────────────────────────────────────────────────────────────

fn cmd_demo(rest: &[String]) -> GastronomeResult<String> {
    let parsed = parse_args(rest, &[], &["json"])?;
    let book = KitchenBook::demo();
    let brief = book.brief.clone().expect("demo book has a brief");
    let plan = plan_event(&book, &brief)?;
    if wants_json(&parsed) {
        Ok(format!("{}\n", render_plan_json(&plan)?))
    } else {
        Ok(render_plan_text(&plan))
    }
}

fn cmd_plan(rest: &[String]) -> GastronomeResult<String> {
    let parsed = parse_args(rest, &["file", "brief"], &["json"])?;
    let (book, brief) = load_book_and_brief(&parsed)?;
    let plan = plan_event(&book, &brief)?;
    if wants_json(&parsed) {
        Ok(format!("{}\n", render_plan_json(&plan)?))
    } else {
        Ok(render_plan_text(&plan))
    }
}

fn cmd_shopping_list(rest: &[String]) -> GastronomeResult<String> {
    let parsed = parse_args(rest, &["file", "brief"], &["json"])?;
    let (book, brief) = load_book_and_brief(&parsed)?;
    let plan = plan_event(&book, &brief)?;
    if wants_json(&parsed) {
        return serde_json::to_string_pretty(&plan.shopping_list)
            .map(|s| format!("{s}\n"))
            .map_err(|e| GastronomeError::Serialize(e.to_string()));
    }
    let mut out = String::from("Shopping list\n");
    use std::fmt::Write as _;
    for item in &plan.shopping_list {
        let _ = writeln!(
            out,
            "  {:<28} {:>10.1} {:<3} ${:>8.2}",
            item.name,
            item.base_quantity,
            item.base_unit.label(),
            cost::round_cents(item.cost)
        );
    }
    let _ = writeln!(
        out,
        "  {:<28} {:>14} ${:>8.2}",
        "TOTAL",
        "",
        cost::round_cents(plan.total_cost)
    );
    Ok(out)
}

fn cmd_schedule(rest: &[String]) -> GastronomeResult<String> {
    let parsed = parse_args(rest, &["file", "brief"], &["json"])?;
    let (book, brief) = load_book_and_brief(&parsed)?;
    let plan = plan_event(&book, &brief)?;
    if wants_json(&parsed) {
        return serde_json::to_string_pretty(&plan.schedule)
            .map(|s| format!("{s}\n"))
            .map_err(|e| GastronomeError::Serialize(e.to_string()));
    }
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Prep schedule (kitchen call {} → service {})",
        plan.schedule.kitchen_start, plan.schedule.service_time
    );
    for t in &plan.schedule.tasks {
        let _ = writeln!(
            out,
            "  {} – {}  {:<28} ({} min)",
            t.start, t.end, t.name, t.duration_minutes
        );
    }
    Ok(out)
}

fn cmd_scale(rest: &[String]) -> GastronomeResult<String> {
    let parsed = parse_args(rest, &["file", "recipe", "servings"], &["json"])?;
    let file = parsed
        .flags
        .get("file")
        .ok_or_else(|| GastronomeError::Usage("missing required flag '--file'".to_string()))?;
    let recipe_id = parsed
        .flags
        .get("recipe")
        .ok_or_else(|| GastronomeError::Usage("missing required flag '--recipe'".to_string()))?;
    let servings: f64 = parsed
        .flags
        .get("servings")
        .ok_or_else(|| GastronomeError::Usage("missing required flag '--servings'".to_string()))?
        .parse()
        .map_err(|_| GastronomeError::Usage("--servings must be a number".to_string()))?;
    let book = KitchenBook::from_toml(&read_file(file)?)?;
    let recipe = book.recipe(recipe_id)?;
    let scaled = scale_recipe(&book, recipe, servings)?;
    if wants_json(&parsed) {
        return serde_json::to_string_pretty(&scaled)
            .map(|s| format!("{s}\n"))
            .map_err(|e| GastronomeError::Serialize(e.to_string()));
    }
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} scaled to {} servings (×{:.3})",
        scaled.name, scaled.target_servings, scaled.scale_factor
    );
    for l in &scaled.lines {
        let _ = writeln!(
            out,
            "  {:<28} {:>10.1} {}",
            l.name,
            l.base_quantity,
            l.base_unit.label()
        );
    }
    Ok(out)
}

#[cfg(test)]
#[path = "tests_cli.rs"]
mod tests_cli;
