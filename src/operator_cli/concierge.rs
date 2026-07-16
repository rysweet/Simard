//! `simard concierge` — the Concierge identity's operator surface.
//!
//! Concierge designs hotels and scaffolds the software to run them. This
//! subcommand exposes the deterministic backbone in `crate::concierge`:
//!
//! - `concept`  — design a hotel concept from a brief (or `--demo`).
//! - `scaffold` — write the concept + a runnable reservations/PMS prototype seed.
//! - `run`      — execute a scaffolded prototype end-to-end.
//! - `demo`     — one-shot design → scaffold → run, proving the whole path.

use std::path::PathBuf;

use crate::concierge::concept::{HotelBrief, HotelConcept, Positioning};
use crate::concierge::scaffold::{self, PrototypeSeed};
use crate::concierge::{run_end_to_end, run_prototype};

pub(super) const CONCIERGE_HELP: &str = "\
Simard concierge subcommand — hospitality design + operations software

Usage: simard concierge <command> [args]

Commands:
  concept  [brief] [--out <dir>] [--json]   Design a hotel concept.
  scaffold [brief] --out <dir>              Write concept.md + a runnable
                                            reservations/PMS prototype seed.
  run <dir> [--json]                        Execute a scaffolded prototype
                                            end-to-end (booking → check-in →
                                            housekeeping → check-out → channel).
  demo [--out <dir>] [--json]               One-shot design → scaffold → run.
  help, -h, --help                          Show this help message and exit.

Brief flags (omit all for the built-in demo brief, or pass --demo explicitly):
  --demo                        Use the built-in demo brief.
  --name <name>                 Hotel name.
  --location <location>         Location / setting.
  --rooms <n>                   Room count (4..=2000).
  --theme <theme>               Design theme, e.g. \"coastal forest modern\".
  --positioning <tier>          select | upscale | luxury.
";

/// Parsed brief-selection flags shared by `concept`, `scaffold`, and `demo`.
struct BriefFlags {
    demo: bool,
    name: Option<String>,
    location: Option<String>,
    rooms: Option<u32>,
    theme: Option<String>,
    positioning: Option<Positioning>,
    out: Option<PathBuf>,
    json: bool,
}

impl BriefFlags {
    fn any_brief_field(&self) -> bool {
        self.name.is_some()
            || self.location.is_some()
            || self.rooms.is_some()
            || self.theme.is_some()
            || self.positioning.is_some()
    }

    /// Resolve the flags into a concrete [`HotelBrief`]. With no brief fields
    /// (or `--demo`) this returns the demo brief; otherwise every required
    /// field must be supplied.
    fn into_brief(self) -> Result<HotelBrief, Box<dyn std::error::Error>> {
        if self.demo || !self.any_brief_field() {
            return Ok(HotelBrief::demo());
        }
        let missing = |field: &str| -> Box<dyn std::error::Error> {
            format!("missing --{field} (required when building a custom brief)").into()
        };
        Ok(HotelBrief {
            name: self.name.ok_or_else(|| missing("name"))?,
            location: self.location.ok_or_else(|| missing("location"))?,
            rooms: self.rooms.ok_or_else(|| missing("rooms"))?,
            theme: self.theme.ok_or_else(|| missing("theme"))?,
            positioning: self.positioning.ok_or_else(|| missing("positioning"))?,
        })
    }
}

/// Parse the shared brief/output flags from an argument iterator.
fn parse_brief_flags(
    mut args: impl Iterator<Item = String>,
) -> Result<BriefFlags, Box<dyn std::error::Error>> {
    let mut flags = BriefFlags {
        demo: false,
        name: None,
        location: None,
        rooms: None,
        theme: None,
        positioning: None,
        out: None,
        json: false,
    };

    while let Some(arg) = args.next() {
        let mut take_value = |label: &str| -> Result<String, Box<dyn std::error::Error>> {
            args.next()
                .ok_or_else(|| format!("flag {label} expects a value").into())
        };
        match arg.as_str() {
            "--demo" => flags.demo = true,
            "--json" => flags.json = true,
            "--name" => flags.name = Some(take_value("--name")?),
            "--location" => flags.location = Some(take_value("--location")?),
            "--theme" => flags.theme = Some(take_value("--theme")?),
            "--rooms" => {
                let raw = take_value("--rooms")?;
                let n: u32 = raw
                    .parse()
                    .map_err(|_| format!("invalid --rooms value '{raw}' (expected an integer)"))?;
                flags.rooms = Some(n);
            }
            "--positioning" => {
                let raw = take_value("--positioning")?;
                flags.positioning = Some(Positioning::parse(&raw)?);
            }
            "--out" => flags.out = Some(PathBuf::from(take_value("--out")?)),
            other => return Err(format!("unexpected argument '{other}'").into()),
        }
    }

    Ok(flags)
}

pub(super) fn dispatch_concierge_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = super::args::next_required(&mut args, "concierge command")?;
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{CONCIERGE_HELP}");
            Ok(())
        }
        "concept" => run_concept(parse_brief_flags(args)?),
        "scaffold" => run_scaffold(parse_brief_flags(args)?),
        "run" => run_run(args),
        "demo" => run_demo(parse_brief_flags(args)?),
        other => Err(format!("unsupported command 'concierge {other}'").into()),
    }
}

fn run_concept(flags: BriefFlags) -> Result<(), Box<dyn std::error::Error>> {
    let out = flags.out.clone();
    let json = flags.json;
    let brief = flags.into_brief()?;
    let concept = HotelConcept::design(brief)?;

    if let Some(dir) = &out {
        let written = scaffold::scaffold(&concept, dir)?;
        println!("Wrote hotel concept to {}", written.concept_md.display());
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&concept)?);
    } else {
        print!("{}", concept.to_markdown());
    }
    Ok(())
}

fn run_scaffold(flags: BriefFlags) -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = flags.out.clone() else {
        return Err("concierge scaffold requires --out <dir>".into());
    };
    let brief = flags.into_brief()?;
    let concept = HotelConcept::design(brief)?;
    let out = scaffold::scaffold(&concept, &dir)?;
    println!(
        "Scaffolded reservations/PMS prototype for {}",
        concept.brief.name
    );
    println!("  concept:   {}", out.concept_md.display());
    println!("  prototype: {}", out.prototype_json.display());
    println!("  readme:    {}", out.readme_md.display());
    println!("\nRun it with: simard concierge run {}", dir.display());
    Ok(())
}

fn run_run(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let dir = super::args::next_required(&mut args, "prototype directory")?;
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other => return Err(format!("unexpected argument '{other}'").into()),
        }
    }
    let seed: PrototypeSeed = scaffold::load(&PathBuf::from(&dir))?;
    let report = run_prototype(seed);
    if json {
        emit_report_json(&report)?;
    } else {
        print!("{}", report.to_text());
    }
    Ok(())
}

fn run_demo(flags: BriefFlags) -> Result<(), Box<dyn std::error::Error>> {
    let out = flags.out.clone();
    let json = flags.json;
    let brief = flags.into_brief()?;

    // If an output dir is given, prove the on-disk scaffold → load → run path.
    // Otherwise run entirely in-memory.
    let (concept, report) = if let Some(dir) = &out {
        let concept = HotelConcept::design(brief)?;
        scaffold::scaffold(&concept, dir)?;
        let seed = scaffold::load(dir)?;
        (concept, run_prototype(seed))
    } else {
        run_end_to_end(brief)?
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&concept)?);
        emit_report_json(&report)?;
    } else {
        print!("{}", concept.to_markdown());
        println!("\n---\n");
        print!("{}", report.to_text());
    }

    if let Some(dir) = &out {
        println!("\nScaffold written to {}", dir.display());
    }
    Ok(())
}

/// Emit the operations report as a JSON object (the report type is not itself
/// Serialize; project only the operator-relevant fields).
fn emit_report_json(
    report: &crate::concierge::OperationsReport,
) -> Result<(), Box<dyn std::error::Error>> {
    let availability: Vec<serde_json::Value> = report
        .availability
        .iter()
        .map(|(category, available, total)| {
            serde_json::json!({
                "category": category,
                "available": available,
                "total": total,
            })
        })
        .collect();
    let value = serde_json::json!({
        "property": report.property,
        "bookings_made": report.bookings_made,
        "check_ins": report.check_ins,
        "check_outs": report.check_outs,
        "housekeeping_rooms_advanced": report.housekeeping_rooms_advanced,
        "occupied_after": report.occupied_after,
        "availability": availability,
        "trace": report.trace,
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    #[test]
    fn concierge_missing_subcommand_errors() {
        let result = dispatch_operator_cli(vec!["concierge".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected concierge command")
        );
    }

    #[test]
    fn concierge_unknown_subcommand_errors() {
        let result = dispatch_operator_cli(vec!["concierge".to_string(), "nope".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'concierge nope'")
        );
    }

    #[test]
    fn concierge_help_exits_ok() {
        for flag in ["--help", "-h", "help"] {
            let result = dispatch_operator_cli(vec!["concierge".to_string(), flag.to_string()]);
            assert!(result.is_ok(), "concierge {flag} must exit Ok: {result:?}");
        }
    }

    #[test]
    fn concierge_concept_demo_runs() {
        let result = dispatch_operator_cli(vec!["concierge".to_string(), "concept".to_string()]);
        assert!(result.is_ok(), "concept demo must succeed: {result:?}");
    }

    #[test]
    fn concierge_demo_runs() {
        let result = dispatch_operator_cli(vec!["concierge".to_string(), "demo".to_string()]);
        assert!(result.is_ok(), "demo must succeed: {result:?}");
    }

    #[test]
    fn concierge_demo_json_runs() {
        let result = dispatch_operator_cli(vec![
            "concierge".to_string(),
            "demo".to_string(),
            "--json".to_string(),
        ]);
        assert!(result.is_ok(), "demo --json must succeed: {result:?}");
    }

    #[test]
    fn concierge_scaffold_requires_out() {
        let result = dispatch_operator_cli(vec!["concierge".to_string(), "scaffold".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--out"));
    }

    #[test]
    fn concierge_run_missing_dir_errors() {
        let result = dispatch_operator_cli(vec!["concierge".to_string(), "run".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected prototype directory")
        );
    }

    #[test]
    fn concierge_custom_brief_missing_field_errors() {
        let result = dispatch_operator_cli(vec![
            "concierge".to_string(),
            "concept".to_string(),
            "--name".to_string(),
            "Lonely Inn".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing --location")
        );
    }

    #[test]
    fn concierge_invalid_positioning_errors() {
        let result = dispatch_operator_cli(vec![
            "concierge".to_string(),
            "concept".to_string(),
            "--positioning".to_string(),
            "spaceship".to_string(),
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn concierge_scaffold_then_run_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("simard-concierge-cli-{}", uuid::Uuid::now_v7()));
        let scaffold = dispatch_operator_cli(vec![
            "concierge".to_string(),
            "scaffold".to_string(),
            "--demo".to_string(),
            "--out".to_string(),
            dir.to_string_lossy().to_string(),
        ]);
        assert!(scaffold.is_ok(), "scaffold must succeed: {scaffold:?}");

        let run = dispatch_operator_cli(vec![
            "concierge".to_string(),
            "run".to_string(),
            dir.to_string_lossy().to_string(),
        ]);
        assert!(run.is_ok(), "run must succeed: {run:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
