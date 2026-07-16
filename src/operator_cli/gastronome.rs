//! `simard gastronome` operator subcommand.
//!
//! Drives the Gastronome menu/event-design pipeline from the command line:
//!
//! ```text
//! simard gastronome build   --brief <brief.json> --out <dir> [--prep-app] [--strict]
//! simard gastronome inspect --out <dir> [--prep-app]
//! ```

use std::path::PathBuf;

use crate::gastronome::{self, BuildOptions, Manifest};

pub(super) const GASTRONOME_HELP: &str = "\
Simard gastronome subcommand — culinary, menu & event design

Usage:
  simard gastronome build --brief <brief.json> --out <dir> [--prep-app] [--strict]
  simard gastronome inspect --out <dir> [--prep-app]

build    Take an event/menu brief to a costed, scheduled menu plan: a menu card
         (menu.md), a consolidated shopping list (shopping_list.csv), a nutrition
         breakdown (nutrition.csv), and a back-timed prep schedule
         (prep_schedule.csv), described by <out>/manifest.json. --prep-app also
         emits a self-contained prep_app.html kitchen app. --strict exits
         non-zero if the produced plan fails verification.
inspect  Re-read and re-verify an existing menu plan under <dir>.
";

pub fn dispatch_gastronome_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(subcommand) = args.next() else {
        print!("{GASTRONOME_HELP}");
        return Ok(());
    };
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{GASTRONOME_HELP}");
            Ok(())
        }
        "build" => run_build(args),
        "inspect" => run_inspect(args),
        other => Err(format!("unsupported command 'gastronome {other}'").into()),
    }
}

struct BuildArgs {
    brief: PathBuf,
    out: PathBuf,
    prep_app: bool,
    strict: bool,
}

fn parse_build_args(
    mut args: impl Iterator<Item = String>,
) -> Result<BuildArgs, Box<dyn std::error::Error>> {
    let mut brief: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut prep_app = false;
    let mut strict = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--brief" => {
                brief = Some(PathBuf::from(args.next().ok_or("--brief requires a path")?));
            }
            "--out" => {
                out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?));
            }
            "--prep-app" => prep_app = true,
            "--strict" => strict = true,
            "--help" | "-h" => return Err("help".into()),
            other => {
                if let Some(v) = other.strip_prefix("--brief=") {
                    brief = Some(PathBuf::from(v));
                } else if let Some(v) = other.strip_prefix("--out=") {
                    out = Some(PathBuf::from(v));
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
            }
        }
    }
    Ok(BuildArgs {
        brief: brief.ok_or("missing required --brief <brief.json>")?,
        out: out.ok_or("missing required --out <dir>")?,
        prep_app,
        strict,
    })
}

fn run_build(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = match parse_build_args(args) {
        Ok(p) => p,
        Err(e) if e.to_string() == "help" => {
            print!("{GASTRONOME_HELP}");
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let options = BuildOptions {
        prep_app: parsed.prep_app,
    };
    let manifest = gastronome::build_package(&parsed.brief, &parsed.out, options)?;
    print_manifest_summary(&manifest, &parsed.out);

    if parsed.strict {
        // Surface a non-zero exit when the produced plan fails verification.
        manifest.verified()?;
    }
    Ok(())
}

struct InspectArgs {
    out: PathBuf,
    #[allow(dead_code)]
    prep_app: bool,
}

fn run_inspect(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut out: Option<PathBuf> = None;
    let mut prep_app = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?)),
            "--prep-app" => prep_app = true,
            "--help" | "-h" => {
                print!("{GASTRONOME_HELP}");
                return Ok(());
            }
            other => {
                if let Some(v) = other.strip_prefix("--out=") {
                    out = Some(PathBuf::from(v));
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
            }
        }
    }
    let args = InspectArgs {
        out: out.ok_or("missing required --out <dir>")?,
        prep_app,
    };
    let manifest = gastronome::inspect(&args.out)?;
    print_manifest_summary(&manifest, &args.out);
    // A failed inspection is an error so scripts/recipes can react.
    manifest.verified()?;
    Ok(())
}

fn print_manifest_summary(manifest: &Manifest, out: &std::path::Path) {
    println!(
        "gastronome: {} — {} guests, {} dish(es) across {} course(s), {} serving(s)",
        manifest.event,
        manifest.guests,
        manifest.dish_count,
        manifest.course_count,
        manifest.total_servings,
    );
    if let Some(cost) = manifest.estimated_total_cost {
        let per = manifest
            .cost_per_guest
            .map(|c| format!(", {c:.2}/guest"))
            .unwrap_or_default();
        println!(
            "  estimated cost: {cost:.2} {}{per}{}",
            manifest.currency,
            if manifest.over_budget {
                " (OVER BUDGET)"
            } else {
                ""
            }
        );
    }
    println!(
        "  per-guest nutrition: {} kcal, {} g protein, {} g carbs, {} g fat",
        manifest.per_guest_nutrition.kcal,
        manifest.per_guest_nutrition.protein_g,
        manifest.per_guest_nutrition.carbs_g,
        manifest.per_guest_nutrition.fat_g,
    );
    println!(
        "  prep: {} min critical path{}",
        manifest.total_prep_minutes,
        manifest
            .service_time
            .as_deref()
            .map(|t| format!(" (service {t})"))
            .unwrap_or_default(),
    );
    for artifact in &manifest.artifacts {
        let status = if artifact.present { "ok" } else { "skipped" };
        let detail = artifact
            .detail
            .as_deref()
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        println!(
            "  [{status:>7}] {} ({} bytes){detail}",
            artifact.file, artifact.bytes
        );
    }
    println!(
        "  verification: {}",
        if manifest.verification.ok {
            "PASS"
        } else {
            "FAIL"
        },
    );
    for c in &manifest.verification.checks {
        println!(
            "    {} {}: {}",
            if c.ok { "✓" } else { "✗" },
            c.name,
            c.detail
        );
    }
    println!("  manifest: {}", out.join("manifest.json").display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> impl Iterator<Item = String> {
        v.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parse_build_args_flags() {
        let a = parse_build_args(args(&[
            "--brief",
            "b.json",
            "--out",
            "o",
            "--prep-app",
            "--strict",
        ]))
        .unwrap();
        assert_eq!(a.brief, PathBuf::from("b.json"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert!(a.prep_app);
        assert!(a.strict);
    }

    #[test]
    fn parse_build_args_equals_form() {
        let a = parse_build_args(args(&["--brief=b.json", "--out=o"])).unwrap();
        assert_eq!(a.brief, PathBuf::from("b.json"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert!(!a.prep_app);
    }

    #[test]
    fn parse_build_args_requires_brief_and_out() {
        assert!(parse_build_args(args(&["--out", "o"])).is_err());
        assert!(parse_build_args(args(&["--brief", "b.json"])).is_err());
    }

    #[test]
    fn dispatch_help_is_ok() {
        assert!(dispatch_gastronome_command(args(&["--help"])).is_ok());
        assert!(dispatch_gastronome_command(args(&[])).is_ok());
    }

    #[test]
    fn dispatch_unknown_subcommand_errors() {
        assert!(dispatch_gastronome_command(args(&["frobnicate"])).is_err());
    }
}
