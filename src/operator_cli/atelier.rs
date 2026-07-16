//! Operator subcommand `simard atelier` (Simard Atelier identity).
//!
//! Drives the [`crate::atelier`] fabrication engine end-to-end: a product brief
//! becomes a parametric OpenSCAD model, a cut list, a bill of materials, and —
//! when OpenSCAD/FreeCAD are installed — an STL/STEP export and a PNG render.
//!
//! ```text
//! simard atelier fabricate --brief <path.json> [--out <dir>]
//! simard atelier demo [--out <dir>]
//! ```
//!
//! Missing CAD tools are reported as skipped, not failures, so the command
//! always succeeds at producing the deterministic shop artifacts.

use std::path::{Path, PathBuf};

use crate::atelier::{ArtifactStatus, DEMO_BRIEF_JSON, FabricationOutput, ProductBrief, fabricate};

const ATELIER_HELP: &str = "\
Simard atelier subcommand — industrial & furniture design

Usage:
  simard atelier fabricate --brief <path.json> [--out <dir>]
  simard atelier demo [--out <dir>]

Takes a product brief and produces a parametric OpenSCAD model, a cut list
(CSV), a bill of materials (JSON), and — when OpenSCAD/FreeCAD are installed —
an STL/STEP export and a PNG render. Output defaults to ./atelier-output.

Brief JSON fields:
  name, kind (table|shelf|box), width_mm, depth_mm, height_mm,
  panel_thickness_mm, material, shelves, quantity, finish
";

pub(crate) fn dispatch_atelier_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = match args.next() {
        Some(s) => s,
        None => {
            print!("{ATELIER_HELP}");
            return Ok(());
        }
    };

    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{ATELIER_HELP}");
            Ok(())
        }
        "fabricate" => run_fabricate(args),
        "demo" => run_demo(args),
        other => Err(format!(
            "unsupported command 'atelier {other}' (expected fabricate | demo | help)"
        )
        .into()),
    }
}

/// Parse `--brief <path>` / `--out <dir>` flags in any order.
struct FabricateArgs {
    brief_path: Option<PathBuf>,
    out_dir: PathBuf,
}

fn parse_fabricate_args(
    mut args: impl Iterator<Item = String>,
) -> Result<FabricateArgs, Box<dyn std::error::Error>> {
    let mut brief_path = None;
    let mut out_dir = PathBuf::from("atelier-output");
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--brief" => {
                brief_path = Some(PathBuf::from(
                    args.next().ok_or("expected a path after --brief")?,
                ));
            }
            "--out" => {
                out_dir = PathBuf::from(args.next().ok_or("expected a path after --out")?);
            }
            other => return Err(format!("unexpected argument '{other}'").into()),
        }
    }
    Ok(FabricateArgs {
        brief_path,
        out_dir,
    })
}

fn run_fabricate(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = parse_fabricate_args(args)?;
    let brief_path = parsed
        .brief_path
        .ok_or("missing required --brief <path.json>")?;
    let bytes = std::fs::read(&brief_path)
        .map_err(|e| format!("could not read brief '{}': {e}", brief_path.display()))?;
    let brief = ProductBrief::from_json(&bytes)?;
    let output = fabricate(&brief, &parsed.out_dir)?;
    report(&output);
    Ok(())
}

fn run_demo(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = parse_fabricate_args(args)?;
    if parsed.brief_path.is_some() {
        return Err("`atelier demo` does not take --brief (use `fabricate`)".into());
    }
    let out_dir = if parsed.out_dir == Path::new("atelier-output") {
        PathBuf::from("atelier-output/demo")
    } else {
        parsed.out_dir
    };
    let brief = ProductBrief::from_json(DEMO_BRIEF_JSON.as_bytes())?;
    let output = fabricate(&brief, &out_dir)?;
    report(&output);
    Ok(())
}

/// Print a human-readable summary of a fabrication run.
fn report(output: &FabricationOutput) {
    println!("Atelier: {}", output.summary);
    println!("Output:  {}", output.output_dir.display());
    println!("Artifacts:");
    for artifact in &output.artifacts {
        let status = match &artifact.status {
            ArtifactStatus::Written => "written".to_string(),
            ArtifactStatus::Produced { tool } => format!("produced via {tool}"),
            ArtifactStatus::SkippedToolMissing { tool } => {
                format!("skipped ({tool} not installed)")
            }
            ArtifactStatus::Failed { tool, reason } => format!("FAILED via {tool}: {reason}"),
        };
        println!("  - {:<24} {status}", artifact.name);
    }
    if output.produced_model_and_render() {
        println!("End-to-end: exported model + render produced.");
    } else {
        println!(
            "End-to-end: deterministic artifacts produced; install OpenSCAD (and FreeCAD for STEP) \
             to also generate the STL/STEP/render."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> impl Iterator<Item = String> {
        items
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn help_is_ok() {
        assert!(dispatch_atelier_command(args(&["--help"])).is_ok());
        assert!(dispatch_atelier_command(args(&[])).is_ok());
    }

    #[test]
    fn unknown_subcommand_errors() {
        let err = dispatch_atelier_command(args(&["frobnicate"])).unwrap_err();
        assert!(err.to_string().contains("unsupported command"));
    }

    #[test]
    fn fabricate_requires_brief() {
        let err = dispatch_atelier_command(args(&["fabricate"])).unwrap_err();
        assert!(err.to_string().contains("--brief"));
    }

    #[test]
    fn parse_fabricate_args_reads_flags() {
        let parsed = parse_fabricate_args(args(&["--brief", "b.json", "--out", "/tmp/o"])).unwrap();
        assert_eq!(parsed.brief_path.unwrap(), PathBuf::from("b.json"));
        assert_eq!(parsed.out_dir, PathBuf::from("/tmp/o"));
    }

    #[test]
    fn parse_fabricate_args_rejects_unknown_flag() {
        assert!(parse_fabricate_args(args(&["--nope"])).is_err());
    }

    #[test]
    fn demo_rejects_brief_flag() {
        let err = dispatch_atelier_command(args(&["demo", "--brief", "x.json"])).unwrap_err();
        assert!(err.to_string().contains("does not take --brief"));
    }

    #[test]
    fn demo_runs_end_to_end_into_tempdir() {
        let dir = std::env::temp_dir().join(format!("atelier-cli-demo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = dir.to_string_lossy().to_string();
        dispatch_atelier_command(args(&["demo", "--out", &out])).unwrap();
        // Deterministic artifacts must always exist regardless of installed tools.
        assert!(dir.join("manifest.json").exists());
        assert!(dir.join("cut_list.csv").exists());
        assert!(dir.join("bom.json").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
