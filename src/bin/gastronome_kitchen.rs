//! `gastronome-kitchen` — the Gastronome identity's deterministic kitchen app.
//!
//! Turns an event/menu brief into a costed, scheduled menu plan. This is the
//! numeric engine the LLM-facing Gastronome persona delegates to so cost,
//! nutrition, scaling, and prep-schedule figures are reproducible.
//!
//! ## Usage
//!
//! ```text
//! gastronome-kitchen sample-brief                      # emit a sample brief JSON
//! gastronome-kitchen plan --brief <path> [--format json|text]
//! gastronome-kitchen plan --brief - [--format json|text]   # read brief from stdin
//! ```
//!
//! `plan` emits the plan to stdout (JSON by default, or a human-readable
//! `text` summary) and exits 0. On any error it writes a JSON envelope
//! `{ "error": "<msg>" }` to stderr and exits 2.

use std::io::Read;
use std::process::ExitCode;

use simard::gastronome::{BudgetStatus, EventBrief, MenuPlan, plan_event, sample_brief};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("sample-brief") => cmd_sample_brief(),
        Some("plan") => cmd_plan(&args[2..]),
        Some("--help" | "-h" | "help") => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("unknown subcommand '{other}'\n\n{}", usage())),
        None => Err(format!("missing subcommand\n\n{}", usage())),
    };

    match result {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(msg) => {
            let envelope = serde_json::json!({ "error": msg });
            eprintln!("{envelope}");
            ExitCode::from(2)
        }
    }
}

fn usage() -> String {
    concat!(
        "gastronome-kitchen — costed, scheduled menu planning\n\n",
        "USAGE:\n",
        "  gastronome-kitchen sample-brief\n",
        "  gastronome-kitchen plan --brief <path|-> [--format json|text]\n\n",
        "SUBCOMMANDS:\n",
        "  sample-brief   Emit a valid sample event brief as JSON.\n",
        "  plan           Plan an event from a brief JSON file (or '-' for stdin).\n\n",
        "OPTIONS (plan):\n",
        "  --brief <path>   Path to the brief JSON, or '-' to read stdin. Required.\n",
        "  --format <fmt>   Output format: json (default) or text.\n"
    )
    .to_string()
}

fn cmd_sample_brief() -> Result<String, String> {
    let brief = sample_brief();
    let json = serde_json::to_string_pretty(&brief).map_err(|e| e.to_string())?;
    Ok(format!("{json}\n"))
}

fn cmd_plan(args: &[String]) -> Result<String, String> {
    let mut brief_path: Option<String> = None;
    let mut format = "json".to_string();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--brief" => {
                brief_path = Some(
                    args.get(i + 1)
                        .cloned()
                        .ok_or_else(|| "--brief requires a path".to_string())?,
                );
                i += 2;
            }
            "--format" => {
                format = args
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| "--format requires a value".to_string())?;
                i += 2;
            }
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }

    let path = brief_path.ok_or_else(|| "--brief is required".to_string())?;
    let raw = read_source(&path)?;
    let brief: EventBrief =
        serde_json::from_str(&raw).map_err(|e| format!("invalid brief JSON: {e}"))?;
    let plan = plan_event(&brief).map_err(|e| e.to_string())?;

    match format.as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())?;
            Ok(format!("{json}\n"))
        }
        "text" => Ok(render_text(&plan)),
        other => Err(format!("unknown format '{other}' (expected json or text)")),
    }
}

fn read_source(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        Ok(buf)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("failed to read '{path}': {e}"))
    }
}

fn render_text(plan: &MenuPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Menu plan — {} ({} guests)\n",
        plan.event_name, plan.guest_count
    ));
    out.push_str(&format!(
        "Service: {}\n",
        plan.event_start.format("%Y-%m-%d %H:%M UTC")
    ));
    out.push('\n');

    out.push_str("Courses:\n");
    for recipe in &plan.scaled_recipes {
        out.push_str(&format!(
            "  - [{}] {}\n",
            recipe.course.label(),
            recipe.name
        ));
    }
    out.push('\n');

    out.push_str("Cost:\n");
    for rc in &plan.cost.per_recipe {
        out.push_str(&format!(
            "  {:<28} ${:>8.2} total  (${:.2}/guest)\n",
            rc.recipe, rc.total_usd, rc.per_serving_usd
        ));
    }
    out.push_str(&format!(
        "  {:<28} ${:>8.2} total  (${:.2}/guest)\n",
        "EVENT TOTAL", plan.cost.event_total_usd, plan.cost.per_guest_usd
    ));
    out.push_str(&format!("  Budget: {}\n", render_budget(&plan.budget)));
    out.push('\n');

    let n = &plan.nutrition_per_guest;
    out.push_str("Nutrition per guest:\n");
    out.push_str(&format!(
        "  {:.0} kcal | protein {:.1} g | carbs {:.1} g | fat {:.1} g\n\n",
        n.calories, n.protein_g, n.carbs_g, n.fat_g
    ));

    let s = &plan.schedule;
    out.push_str(&format!(
        "Prep schedule ({} cook(s), start {} → service {}):\n",
        s.cook_count,
        s.kitchen_start.format("%H:%M"),
        s.event_start.format("%H:%M")
    ));
    for task in &s.tasks {
        let tag = if task.make_ahead { "ahead " } else { "@svc  " };
        out.push_str(&format!(
            "  cook{} {}–{} [{}] {}: {} ({:.0}m)\n",
            task.cook,
            task.start.format("%H:%M"),
            task.end.format("%H:%M"),
            tag,
            task.recipe,
            task.step,
            task.minutes
        ));
    }
    out.push_str(&format!(
        "  Total active prep: {:.0} min across {} cook(s); makespan {:.0} min\n",
        s.total_active_minutes, s.cook_count, s.makespan_minutes
    ));

    out
}

fn render_budget(status: &BudgetStatus) -> String {
    match status {
        BudgetStatus::Unconstrained => "unconstrained".to_string(),
        BudgetStatus::WithinBudget {
            budget_per_guest_usd,
            per_guest_usd,
            headroom_usd,
        } => format!(
            "within ${budget_per_guest_usd:.2}/guest (spent ${per_guest_usd:.2}, ${headroom_usd:.2} headroom)"
        ),
        BudgetStatus::OverBudget {
            budget_per_guest_usd,
            per_guest_usd,
            overage_usd,
        } => format!(
            "OVER ${budget_per_guest_usd:.2}/guest (spent ${per_guest_usd:.2}, ${overage_usd:.2} over)"
        ),
    }
}
