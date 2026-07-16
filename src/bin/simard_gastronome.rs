//! `simard-gastronome` — the Gastronome identity's kitchen app.
//!
//! Turns an event/menu brief into a costed, nutritionally analysed, and
//! prep-scheduled menu plan. This is the runnable, end-to-end surface of the
//! Gastronome identity: a brief in, a plan out.
//!
//! ## Usage
//!
//! ```text
//! simard-gastronome plan [--brief <path>] [--recipes <path>] [--json]
//! simard-gastronome demo [--json]     # plan a built-in demo brief
//! simard-gastronome recipes [--json]  # list the built-in recipe book
//! simard-gastronome --help
//! ```
//!
//! `--brief <path>`   JSON `EventBrief`. Omitted → the built-in demo brief.
//! `--recipes <path>` JSON recipe book (array or `{ "recipes": [...] }`).
//!                    Omitted → the built-in sample book.
//! `--json`           Emit machine-readable JSON instead of the text report.
//!
//! On success the plan is written to stdout (exit 0). On failure a JSON error
//! envelope `{ "error": "<msg>" }` is written to stderr (exit 2).

use std::process::ExitCode;

use simard::gastronome::{CourseRequest, DietaryTag, EventBrief, RecipeBook, plan_event};

const HELP: &str = "\
simard-gastronome — turn an event/menu brief into a costed, scheduled plan

USAGE:
    simard-gastronome plan [--brief <path>] [--recipes <path>] [--json]
    simard-gastronome demo [--json]
    simard-gastronome recipes [--json]
    simard-gastronome --help

COMMANDS:
    plan       Plan an event from a brief (default command).
    demo       Plan a built-in three-course demo brief.
    recipes    List the recipe book that would be used.

OPTIONS:
    --brief <path>     JSON EventBrief. Omitted uses the demo brief.
    --recipes <path>   JSON recipe book. Omitted uses the built-in sample.
    --json             Emit JSON instead of the human-readable report.
    -h, --help         Show this help.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(output) => {
            print!("{output}");
            if !output.ends_with('\n') {
                println!();
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            let envelope = serde_json::json!({ "error": message });
            eprintln!("{envelope}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        return Ok(HELP.to_string());
    }

    let (command, rest) = match args.split_first() {
        Some((first, rest)) if !first.starts_with('-') => (first.as_str(), rest),
        _ => ("plan", args),
    };

    let flags = Flags::parse(rest)?;

    match command {
        "recipes" => {
            let book = load_book(flags.recipes.as_deref())?;
            if flags.json {
                serde_json::to_string_pretty(&book).map_err(|e| format!("serialize failed: {e}"))
            } else {
                Ok(render_recipe_list(&book))
            }
        }
        "plan" | "demo" => {
            let brief = if command == "demo" || flags.brief.is_none() {
                demo_brief()
            } else {
                load_brief(flags.brief.as_deref().unwrap())?
            };
            let book = load_book(flags.recipes.as_deref())?;
            let plan = plan_event(&brief, &book).map_err(|e| e.to_string())?;
            if flags.json {
                serde_json::to_string_pretty(&plan).map_err(|e| format!("serialize failed: {e}"))
            } else {
                Ok(plan.render())
            }
        }
        other => Err(format!(
            "unknown command '{other}' (expected plan, demo, or recipes; --help for usage)"
        )),
    }
}

struct Flags {
    brief: Option<String>,
    recipes: Option<String>,
    json: bool,
}

impl Flags {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut brief = None;
        let mut recipes = None;
        let mut json = false;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--json" => json = true,
                "--brief" => {
                    i += 1;
                    brief = Some(
                        args.get(i)
                            .ok_or("--brief requires a path argument")?
                            .clone(),
                    );
                }
                "--recipes" => {
                    i += 1;
                    recipes = Some(
                        args.get(i)
                            .ok_or("--recipes requires a path argument")?
                            .clone(),
                    );
                }
                other => return Err(format!("unexpected argument '{other}' (see --help)")),
            }
            i += 1;
        }
        Ok(Self {
            brief,
            recipes,
            json,
        })
    }
}

fn load_book(path: Option<&str>) -> Result<RecipeBook, String> {
    match path {
        None => Ok(RecipeBook::builtin()),
        Some(p) => {
            let json = std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read recipes file '{p}': {e}"))?;
            RecipeBook::from_json(&json).map_err(|e| e.to_string())
        }
    }
}

fn load_brief(path: &str) -> Result<EventBrief, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read brief file '{path}': {e}"))?;
    serde_json::from_str(&json).map_err(|e| format!("invalid brief JSON in '{path}': {e}"))
}

fn render_recipe_list(book: &RecipeBook) -> String {
    let mut out = String::from("Recipe book\n===========\n");
    let mut recipes: Vec<_> = book.recipes.iter().collect();
    recipes.sort_by(|a, b| a.course.cmp(&b.course).then_with(|| a.id.cmp(&b.id)));
    for r in recipes {
        let tags: Vec<String> = r.dietary_tags.iter().map(ToString::to_string).collect();
        out.push_str(&format!(
            "  [{:<8}] {:<26} ({} base servings, {:.2}/serving) {}\n",
            r.course,
            r.name,
            r.base_servings,
            r.cost_per_serving(),
            if tags.is_empty() {
                String::new()
            } else {
                format!("— {}", tags.join(", "))
            },
        ));
    }
    out
}

/// A built-in three-course brief so `demo` (and a bare invocation) always plans
/// something real.
fn demo_brief() -> EventBrief {
    EventBrief {
        name: "Gastronome Demo Dinner".into(),
        guests: 12,
        serve_time: "19:30".into(),
        courses: vec![
            CourseRequest::new("starter"),
            CourseRequest::new("main"),
            CourseRequest::new("side"),
            CourseRequest::new("dessert"),
        ],
        dietary_constraints: vec![DietaryTag::NutFree],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_flag_shows_usage() {
        let out = run(&["--help".to_string()]).unwrap();
        assert!(out.contains("USAGE"));
    }

    #[test]
    fn bare_invocation_plans_demo() {
        let out = run(&[]).unwrap();
        assert!(out.contains("MENU PLAN"));
        assert!(out.contains("Gastronome Demo Dinner"));
    }

    #[test]
    fn demo_json_is_valid_json() {
        let out = run(&["demo".to_string(), "--json".to_string()]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(value.get("cost").is_some());
        assert!(value.get("schedule").is_some());
    }

    #[test]
    fn recipes_command_lists_book() {
        let out = run(&["recipes".to_string()]).unwrap();
        assert!(out.contains("Recipe book"));
        assert!(out.contains("Chickpea"));
    }

    #[test]
    fn unknown_command_errors() {
        assert!(run(&["frobnicate".to_string()]).is_err());
    }

    #[test]
    fn missing_flag_value_errors() {
        assert!(run(&["plan".to_string(), "--brief".to_string()]).is_err());
    }

    #[test]
    fn missing_brief_file_errors() {
        let err = run(&[
            "plan".to_string(),
            "--brief".to_string(),
            "/nonexistent/brief.json".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("cannot read brief file"));
    }
}
