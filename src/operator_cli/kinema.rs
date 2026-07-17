//! `simard kinema` operator subcommand.
//!
//! Drives the Kinema animation pipeline from the command line:
//!
//! ```text
//! simard kinema build   --brief <shot.json> --out <dir> [--no-grease-pencil] [--no-composite] [--strict]
//! simard kinema inspect --out <dir>
//! ```

use std::path::PathBuf;

use crate::kinema::{self, BuildOptions, Manifest};

pub(super) const KINEMA_HELP: &str = "\
Simard kinema subcommand — 2D/3D animation & motion graphics

Usage:
  simard kinema build --brief <shot.json> --out <dir> [--no-grease-pencil] [--no-composite] [--strict]
  simard kinema inspect --out <dir>

build    Take a shot brief to a storyboard, a rig, a Synfig vector source, and a
         rendered PNG frame sequence, described by <out>/manifest.json. The
         pure-Rust rasterizer always renders the sequence; Blender (Grease
         Pencil), Synfig, and Natron are used when installed and skipped
         gracefully otherwise. --no-grease-pencil / --no-composite disable the
         optional Blender / Natron passes. --strict exits non-zero if the
         produced sequence fails verification.
inspect  Re-read and re-verify an existing package manifest under <dir>.
";

pub fn dispatch_kinema_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(subcommand) = args.next() else {
        print!("{KINEMA_HELP}");
        return Ok(());
    };
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{KINEMA_HELP}");
            Ok(())
        }
        "build" => run_build(args),
        "inspect" => run_inspect(args),
        other => Err(format!("unsupported command 'kinema {other}'").into()),
    }
}

struct BuildArgs {
    brief: PathBuf,
    out: PathBuf,
    grease_pencil: bool,
    composite: bool,
    strict: bool,
}

fn parse_build_args(
    mut args: impl Iterator<Item = String>,
) -> Result<BuildArgs, Box<dyn std::error::Error>> {
    let mut brief: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut grease_pencil = true;
    let mut composite = true;
    let mut strict = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--brief" => {
                brief = Some(PathBuf::from(args.next().ok_or("--brief requires a path")?));
            }
            "--out" => {
                out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?));
            }
            "--no-grease-pencil" => grease_pencil = false,
            "--no-composite" => composite = false,
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
        brief: brief.ok_or("missing required --brief <shot.json>")?,
        out: out.ok_or("missing required --out <dir>")?,
        grease_pencil,
        composite,
        strict,
    })
}

fn run_build(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = match parse_build_args(args) {
        Ok(p) => p,
        Err(e) if e.to_string() == "help" => {
            print!("{KINEMA_HELP}");
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let options = BuildOptions {
        grease_pencil: parsed.grease_pencil,
        composite: parsed.composite,
    };
    let manifest = kinema::build_package(&parsed.brief, &parsed.out, options)?;
    print_manifest_summary(&manifest, &parsed.out);

    if parsed.strict {
        // Surface a non-zero exit when the produced sequence fails verification.
        manifest.verified()?;
    }
    Ok(())
}

fn run_inspect(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut out: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?)),
            "--help" | "-h" => {
                print!("{KINEMA_HELP}");
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
    let out = out.ok_or("missing required --out <dir>")?;
    let manifest = kinema::inspect(&out)?;
    print_manifest_summary(&manifest, &out);
    // A failed inspection is an error so scripts/recipes can react.
    manifest.verified()?;
    Ok(())
}

fn print_manifest_summary(manifest: &Manifest, out: &std::path::Path) {
    println!(
        "kinema: {} ({}) — {:.2}s @ {} fps, {}x{}, {}/{} frames, {} objects, {} bones",
        manifest.shot_name,
        manifest.style,
        manifest.duration_s,
        manifest.fps,
        manifest.width,
        manifest.height,
        manifest.frames_rendered,
        manifest.frame_count,
        manifest.object_count,
        manifest.bone_count,
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
        "  verification: {} (external render: {})",
        if manifest.verification.ok {
            "PASS"
        } else {
            "FAIL"
        },
        if manifest.verification.external_render_ok {
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
            "s.json",
            "--out",
            "o",
            "--no-grease-pencil",
            "--no-composite",
            "--strict",
        ]))
        .unwrap();
        assert_eq!(a.brief, PathBuf::from("s.json"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert!(!a.grease_pencil);
        assert!(!a.composite);
        assert!(a.strict);
    }

    #[test]
    fn parse_build_args_defaults_enable_engines() {
        let a = parse_build_args(args(&["--brief=s.json", "--out=o"])).unwrap();
        assert_eq!(a.brief, PathBuf::from("s.json"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert!(a.grease_pencil);
        assert!(a.composite);
        assert!(!a.strict);
    }

    #[test]
    fn parse_build_args_requires_brief_and_out() {
        assert!(parse_build_args(args(&["--out", "o"])).is_err());
        assert!(parse_build_args(args(&["--brief", "s.json"])).is_err());
    }

    #[test]
    fn dispatch_help_is_ok() {
        assert!(dispatch_kinema_command(args(&["--help"])).is_ok());
        assert!(dispatch_kinema_command(args(&[])).is_ok());
    }

    #[test]
    fn dispatch_unknown_subcommand_errors() {
        assert!(dispatch_kinema_command(args(&["frobnicate"])).is_err());
    }
}
