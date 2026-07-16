//! `simard atelier` operator subcommand.
//!
//! Drives the Atelier design pipeline from the command line:
//!
//! ```text
//! simard atelier build   --brief <brief.json> --out <dir> [--fabrication] [--strict]
//! simard atelier inspect --out <dir> [--fabrication]
//! ```

use std::path::PathBuf;

use crate::atelier::{self, BuildOptions, Manifest};

pub(super) const ATELIER_HELP: &str = "\
Simard atelier subcommand — industrial & furniture design

Usage:
  simard atelier build --brief <brief.json> --out <dir> [--fabrication] [--strict]
  simard atelier inspect --out <dir> [--fabrication]

build    Take a product brief to a parametric model (OpenSCAD), an STL mesh, a
         PNG render, a cut list, and a bill of materials, described by
         <out>/manifest.json. --fabrication also emits a STEP solid when a solid
         kernel (FreeCAD) is available. --strict exits non-zero if the produced
         package fails verification.
inspect  Re-read and re-verify an existing package manifest under <dir>.
";

pub fn dispatch_atelier_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(subcommand) = args.next() else {
        print!("{ATELIER_HELP}");
        return Ok(());
    };
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{ATELIER_HELP}");
            Ok(())
        }
        "build" => run_build(args),
        "inspect" => run_inspect(args),
        other => Err(format!("unsupported command 'atelier {other}'").into()),
    }
}

struct BuildArgs {
    brief: PathBuf,
    out: PathBuf,
    fabrication: bool,
    strict: bool,
}

fn parse_build_args(
    mut args: impl Iterator<Item = String>,
) -> Result<BuildArgs, Box<dyn std::error::Error>> {
    let mut brief: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut fabrication = false;
    let mut strict = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--brief" => {
                brief = Some(PathBuf::from(args.next().ok_or("--brief requires a path")?));
            }
            "--out" => {
                out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?));
            }
            "--fabrication" => fabrication = true,
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
        fabrication,
        strict,
    })
}

fn run_build(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = match parse_build_args(args) {
        Ok(p) => p,
        Err(e) if e.to_string() == "help" => {
            print!("{ATELIER_HELP}");
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let options = BuildOptions {
        fabrication: parsed.fabrication,
        ..BuildOptions::default()
    };
    let manifest = atelier::build_package(&parsed.brief, &parsed.out, options)?;
    print_manifest_summary(&manifest, &parsed.out);

    if parsed.strict {
        // Surface a non-zero exit when the produced package fails verification.
        manifest.verified()?;
    }
    Ok(())
}

struct InspectArgs {
    out: PathBuf,
    #[allow(dead_code)]
    fabrication: bool,
}

fn run_inspect(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut out: Option<PathBuf> = None;
    let mut fabrication = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?)),
            "--fabrication" => fabrication = true,
            "--help" | "-h" => {
                print!("{ATELIER_HELP}");
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
        fabrication,
    };
    let manifest = atelier::inspect(&args.out)?;
    print_manifest_summary(&manifest, &args.out);
    // A failed inspection is an error so scripts/recipes can react.
    manifest.verified()?;
    Ok(())
}

fn print_manifest_summary(manifest: &Manifest, out: &std::path::Path) {
    println!(
        "atelier: {} ({}) — {} parts / {} instances, {} sheet(s)",
        manifest.product_name,
        manifest.kind,
        manifest.part_count,
        manifest.instance_count,
        manifest.sheets_required,
    );
    if let Some(cost) = manifest.estimated_material_cost {
        println!(
            "  estimated material cost: {cost:.2}{}",
            if manifest.over_budget {
                " (OVER BUDGET)"
            } else {
                ""
            }
        );
    }
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
        "  verification: {} (render: {})",
        if manifest.verification.ok {
            "PASS"
        } else {
            "FAIL"
        },
        if manifest.verification.render_ok {
            "yes"
        } else {
            "no"
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
            "--fabrication",
            "--strict",
        ]))
        .unwrap();
        assert_eq!(a.brief, PathBuf::from("b.json"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert!(a.fabrication);
        assert!(a.strict);
    }

    #[test]
    fn parse_build_args_equals_form() {
        let a = parse_build_args(args(&["--brief=b.json", "--out=o"])).unwrap();
        assert_eq!(a.brief, PathBuf::from("b.json"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert!(!a.fabrication);
    }

    #[test]
    fn parse_build_args_requires_brief_and_out() {
        assert!(parse_build_args(args(&["--out", "o"])).is_err());
        assert!(parse_build_args(args(&["--brief", "b.json"])).is_err());
    }

    #[test]
    fn dispatch_help_is_ok() {
        assert!(dispatch_atelier_command(args(&["--help"])).is_ok());
        assert!(dispatch_atelier_command(args(&[])).is_ok());
    }

    #[test]
    fn dispatch_unknown_subcommand_errors() {
        assert!(dispatch_atelier_command(args(&["frobnicate"])).is_err());
    }
}
