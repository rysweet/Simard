//! `simard gastronome` operator subcommands — the "kitchen app" for the
//! Gastronome culinary / menu & event-design identity.
//!
//! Subcommands:
//!   - `gastronome plan <brief-file> [--json]` — read an event/menu brief
//!     (JSON or TOML) and emit a costed, scheduled [`MenuPlan`].
//!   - `gastronome demo [--json]` — plan a built-in example brief end-to-end.
//!   - `gastronome recipes [--json]` — list the built-in recipe library.
//!   - `gastronome menus [--json]` — list the built-in menus.
//!   - `gastronome scale <recipe-id> <servings> [--json]` — scale one library
//!     recipe to a target serving count.
//!
//! All commands are pure and deterministic: they resolve against the built-in
//! [`crate::gastronome::Pantry`], so no network, clock, or external data is
//! required. Human output goes to stdout; `--json` emits a machine-readable
//! document instead (nothing else is written to stdout so it stays pipe-safe).

use std::error::Error;
use std::path::PathBuf;

use crate::gastronome::scaling::scale_recipe;
use crate::gastronome::{builtin_pantry, parse_brief, plan_event, render_plan};

pub(super) const GASTRONOME_HELP: &str = "\
Simard gastronome subcommand — culinary / menu & event design

Usage:
  simard gastronome plan <brief-file> [--json]
                          — plan an event/menu brief (JSON or TOML) into a
                            costed, scheduled menu plan
  simard gastronome demo [--json]
                          — plan a built-in example brief end-to-end
  simard gastronome recipes [--json]
                          — list the built-in recipe library
  simard gastronome menus [--json]
                          — list the built-in menus
  simard gastronome scale <recipe-id> <servings> [--json]
                          — scale one library recipe to a serving count

A brief is a JSON or TOML document, e.g.:
  {\"event_name\":\"Gala\",\"guest_count\":24,\"menu_id\":\"italian-dinner\",
   \"dietary_restrictions\":[],\"budget_per_guest\":12.0,\"service_time_min\":1080}
where service_time_min is minutes-from-midnight (1080 = 18:00).
";

/// Split trailing args into an optional positional list plus a `--json` flag.
fn take_json_flag(args: &mut Vec<String>) -> bool {
    if let Some(pos) = args.iter().position(|a| a == "--json") {
        args.remove(pos);
        true
    } else {
        false
    }
}

pub fn dispatch_gastronome_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    let mut args: Vec<String> = args.collect();
    let Some(subcommand) = args.first().cloned() else {
        print!("{GASTRONOME_HELP}");
        return Ok(());
    };
    args.remove(0);

    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{GASTRONOME_HELP}");
            Ok(())
        }
        "plan" => cmd_plan(args),
        "demo" => cmd_demo(args),
        "recipes" => cmd_recipes(args),
        "menus" => cmd_menus(args),
        "scale" => cmd_scale(args),
        other => Err(format!("unsupported command 'gastronome {other}'").into()),
    }
}

fn cmd_plan(mut args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let json = take_json_flag(&mut args);
    let brief_file = match args.as_slice() {
        [path] => PathBuf::from(path),
        [] => return Err("expected <brief-file>".into()),
        _ => return Err(format!("unexpected trailing arguments: {}", args.join(" ")).into()),
    };
    let text = std::fs::read_to_string(&brief_file)
        .map_err(|e| format!("cannot read brief '{}': {e}", brief_file.display()))?;
    let brief = parse_brief(&text)?;
    let pantry = builtin_pantry();
    let plan = plan_event(&pantry, &brief)?;
    emit_plan(&plan, json)
}

fn cmd_demo(mut args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let json = take_json_flag(&mut args);
    if !args.is_empty() {
        return Err(format!("unexpected trailing arguments: {}", args.join(" ")).into());
    }
    let pantry = builtin_pantry();
    let plan = plan_event(&pantry, &crate::gastronome::demo_brief())?;
    emit_plan(&plan, json)
}

fn emit_plan(plan: &crate::gastronome::MenuPlan, json: bool) -> Result<(), Box<dyn Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(plan)?);
    } else {
        print!("{}", render_plan(plan));
    }
    Ok(())
}

fn cmd_recipes(mut args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let json = take_json_flag(&mut args);
    if !args.is_empty() {
        return Err(format!("unexpected trailing arguments: {}", args.join(" ")).into());
    }
    let pantry = builtin_pantry();
    let recipes: Vec<_> = pantry.recipes().cloned().collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&recipes)?);
    } else {
        println!("Built-in recipes:");
        for r in &recipes {
            println!(
                "  {:<16} {:<10} base {} servings — {}",
                r.id, r.course, r.servings, r.name
            );
        }
    }
    Ok(())
}

fn cmd_menus(mut args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let json = take_json_flag(&mut args);
    if !args.is_empty() {
        return Err(format!("unexpected trailing arguments: {}", args.join(" ")).into());
    }
    let pantry = builtin_pantry();
    let menus: Vec<_> = pantry.menus().cloned().collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&menus)?);
    } else {
        println!("Built-in menus:");
        for m in &menus {
            println!("  {:<16} {} — {}", m.id, m.name, m.recipe_ids.join(", "));
        }
    }
    Ok(())
}

fn cmd_scale(mut args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let json = take_json_flag(&mut args);
    let (recipe_id, servings) = match args.as_slice() {
        [recipe_id, servings] => (recipe_id.clone(), servings.clone()),
        _ => return Err("expected <recipe-id> <servings>".into()),
    };
    let servings: u32 = servings
        .parse()
        .map_err(|_| format!("invalid servings '{servings}': must be a positive integer"))?;
    let pantry = builtin_pantry();
    let recipe = pantry.recipe(&recipe_id)?;
    let scaled = scale_recipe(&pantry, recipe, servings)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&scaled)?);
    } else {
        println!(
            "{} — {} servings (x{:.2}), cost {:.2}",
            scaled.name, scaled.target_servings, scaled.scale_factor, scaled.cost_total
        );
        for line in &scaled.ingredients {
            println!(
                "  {:>10.1} {:<6} {:<20} ({:.2})",
                line.quantity, line.unit, line.name, line.line_cost
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<(), Box<dyn Error>> {
        dispatch_gastronome_command(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn help_is_ok() {
        assert!(run(&["--help"]).is_ok());
        assert!(run(&[]).is_ok());
    }

    #[test]
    fn demo_runs() {
        assert!(run(&["demo"]).is_ok());
        assert!(run(&["demo", "--json"]).is_ok());
    }

    #[test]
    fn recipes_and_menus_list() {
        assert!(run(&["recipes"]).is_ok());
        assert!(run(&["recipes", "--json"]).is_ok());
        assert!(run(&["menus"]).is_ok());
        assert!(run(&["menus", "--json"]).is_ok());
    }

    #[test]
    fn scale_valid_and_invalid() {
        assert!(run(&["scale", "caprese", "12"]).is_ok());
        assert!(run(&["scale", "caprese", "12", "--json"]).is_ok());
        assert!(run(&["scale", "caprese", "0"]).is_err());
        assert!(run(&["scale", "nope", "4"]).is_err());
        assert!(run(&["scale", "caprese", "abc"]).is_err());
        assert!(run(&["scale", "caprese"]).is_err());
    }

    #[test]
    fn plan_from_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("gastronome-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("brief.json");
        std::fs::write(
            &path,
            r#"{"event_name":"T","guest_count":8,"menu_id":"italian-dinner","service_time_min":1080}"#,
        )
        .unwrap();
        assert!(run(&["plan", path.to_str().unwrap()]).is_ok());
        assert!(run(&["plan", path.to_str().unwrap(), "--json"]).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_missing_file_errors() {
        assert!(run(&["plan", "/nonexistent/brief.json"]).is_err());
    }

    #[test]
    fn plan_requires_arg() {
        assert!(run(&["plan"]).is_err());
    }

    #[test]
    fn unknown_subcommand_errors() {
        assert!(run(&["frobnicate"]).is_err());
    }
}
