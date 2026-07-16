//! `simard cartographer` operator subcommand.
//!
//! Drives the Cartographer data-storytelling pipeline from the command line:
//!
//! ```text
//! simard cartographer build   --brief <study.json> --out <dir> [--target html|streamlit|observable] [--strict]
//! simard cartographer inspect --out <dir>
//! simard cartographer serve   --out <dir> [--port <n>] [--self-check]
//! ```

use std::path::PathBuf;

use crate::cartographer::{self, AppTarget, BuildOptions, Manifest};

pub(super) const CARTOGRAPHER_HELP: &str = "\
Simard cartographer subcommand — data storytelling & interactive dashboards

Usage:
  simard cartographer build --brief <study.json> --out <dir> [--target html|streamlit|observable] [--strict]
  simard cartographer inspect --out <dir>
  simard cartographer serve --out <dir> [--port <n>] [--self-check]

build    Take a dataset + question to a served interactive dashboard
         (dashboard.html: Plotly + D3), a written narrative (narrative.md), and
         optional Streamlit / Observable delivery sources, all described by
         <out>/manifest.json. --target overrides the brief's delivery target.
         --strict exits non-zero if the produced package fails verification.
inspect  Re-read and re-verify an existing package manifest under <dir>.
serve    Serve the built package over HTTP from <dir>. --self-check performs a
         single self-request on an ephemeral port and exits (non-zero on
         failure); otherwise binds 127.0.0.1:<port> and serves until stopped.
";

pub fn dispatch_cartographer_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(subcommand) = args.next() else {
        print!("{CARTOGRAPHER_HELP}");
        return Ok(());
    };
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{CARTOGRAPHER_HELP}");
            Ok(())
        }
        "build" => run_build(args),
        "inspect" => run_inspect(args),
        "serve" => run_serve(args),
        other => Err(format!("unsupported command 'cartographer {other}'").into()),
    }
}

struct BuildArgs {
    brief: PathBuf,
    out: PathBuf,
    target: Option<AppTarget>,
    strict: bool,
}

fn parse_build_args(
    mut args: impl Iterator<Item = String>,
) -> Result<BuildArgs, Box<dyn std::error::Error>> {
    let mut brief: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut target: Option<AppTarget> = None;
    let mut strict = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--brief" => {
                brief = Some(PathBuf::from(args.next().ok_or("--brief requires a path")?));
            }
            "--out" => {
                out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?));
            }
            "--target" => {
                let v = args.next().ok_or("--target requires a value")?;
                target = Some(AppTarget::classify(&v));
            }
            "--strict" => strict = true,
            "--help" | "-h" => return Err("help".into()),
            other => {
                if let Some(v) = other.strip_prefix("--brief=") {
                    brief = Some(PathBuf::from(v));
                } else if let Some(v) = other.strip_prefix("--out=") {
                    out = Some(PathBuf::from(v));
                } else if let Some(v) = other.strip_prefix("--target=") {
                    target = Some(AppTarget::classify(v));
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
            }
        }
    }
    Ok(BuildArgs {
        brief: brief.ok_or("missing required --brief <study.json>")?,
        out: out.ok_or("missing required --out <dir>")?,
        target,
        strict,
    })
}

fn run_build(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = match parse_build_args(args) {
        Ok(p) => p,
        Err(e) if e.to_string() == "help" => {
            print!("{CARTOGRAPHER_HELP}");
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let options = BuildOptions {
        app_target: parsed.target,
    };
    let manifest = cartographer::build_package(&parsed.brief, &parsed.out, options)?;
    print_manifest_summary(&manifest, &parsed.out);

    if parsed.strict {
        // Surface a non-zero exit when the produced package fails verification.
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
                print!("{CARTOGRAPHER_HELP}");
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
    let manifest = cartographer::inspect(&out)?;
    print_manifest_summary(&manifest, &out);
    // A failed inspection is an error so scripts/recipes can react.
    manifest.verified()?;
    Ok(())
}

struct ServeArgs {
    out: PathBuf,
    port: u16,
    self_check: bool,
}

fn parse_serve_args(
    mut args: impl Iterator<Item = String>,
) -> Result<ServeArgs, Box<dyn std::error::Error>> {
    let mut out: Option<PathBuf> = None;
    let mut port: u16 = 0;
    let mut self_check = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?)),
            "--port" => {
                let v = args.next().ok_or("--port requires a value")?;
                port = v.parse().map_err(|_| format!("invalid --port: {v}"))?;
            }
            "--self-check" => self_check = true,
            "--help" | "-h" => return Err("help".into()),
            other => {
                if let Some(v) = other.strip_prefix("--out=") {
                    out = Some(PathBuf::from(v));
                } else if let Some(v) = other.strip_prefix("--port=") {
                    port = v.parse().map_err(|_| format!("invalid --port: {v}"))?;
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
            }
        }
    }
    Ok(ServeArgs {
        out: out.ok_or("missing required --out <dir>")?,
        port,
        self_check,
    })
}

fn run_serve(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = match parse_serve_args(args) {
        Ok(p) => p,
        Err(e) if e.to_string() == "help" => {
            print!("{CARTOGRAPHER_HELP}");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let report = cartographer::serve(&parsed.out, parsed.port, parsed.self_check)?;
    if parsed.self_check {
        println!(
            "cartographer: self-check {} — {} {} ({} bytes)",
            if report.served_ok { "PASS" } else { "FAIL" },
            report.addr,
            report.status,
            report.body_bytes,
        );
        if !report.served_ok {
            return Err(format!(
                "serve self-check failed: status {} ({} bytes)",
                report.status, report.body_bytes
            )
            .into());
        }
    }
    Ok(())
}

fn print_manifest_summary(manifest: &Manifest, out: &std::path::Path) {
    println!(
        "cartographer: {} [{}] — {} rows × {} cols, {} finding(s), {} chart(s)",
        manifest.title,
        manifest.app_target,
        manifest.row_count,
        manifest.column_count,
        manifest.finding_count,
        manifest.chart_count,
    );
    println!("  question: {}", manifest.question);
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
            "--target",
            "streamlit",
            "--strict",
        ]))
        .unwrap();
        assert_eq!(a.brief, PathBuf::from("b.json"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert_eq!(a.target, Some(AppTarget::Streamlit));
        assert!(a.strict);
    }

    #[test]
    fn parse_build_args_equals_form() {
        let a =
            parse_build_args(args(&["--brief=b.json", "--out=o", "--target=observable"])).unwrap();
        assert_eq!(a.brief, PathBuf::from("b.json"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert_eq!(a.target, Some(AppTarget::Observable));
    }

    #[test]
    fn parse_build_args_requires_brief_and_out() {
        assert!(parse_build_args(args(&["--out", "o"])).is_err());
        assert!(parse_build_args(args(&["--brief", "b.json"])).is_err());
    }

    #[test]
    fn parse_serve_args_flags() {
        let a = parse_serve_args(args(&["--out", "o", "--port", "8080", "--self-check"])).unwrap();
        assert_eq!(a.out, PathBuf::from("o"));
        assert_eq!(a.port, 8080);
        assert!(a.self_check);
    }

    #[test]
    fn parse_serve_args_defaults() {
        let a = parse_serve_args(args(&["--out=o"])).unwrap();
        assert_eq!(a.port, 0);
        assert!(!a.self_check);
    }

    #[test]
    fn parse_serve_args_rejects_bad_port() {
        assert!(parse_serve_args(args(&["--out", "o", "--port", "nope"])).is_err());
    }

    #[test]
    fn dispatch_help_is_ok() {
        assert!(dispatch_cartographer_command(args(&["--help"])).is_ok());
        assert!(dispatch_cartographer_command(args(&[])).is_ok());
    }

    #[test]
    fn dispatch_unknown_subcommand_errors() {
        assert!(dispatch_cartographer_command(args(&["frobnicate"])).is_err());
    }
}
