//! `simard cartographer` operator subcommand.
//!
//! Drives the Cartographer data-storytelling pipeline from the command line:
//!
//! ```text
//! simard cartographer build   (--brief <brief.json> | --dataset <data> --question <q>)
//!                             --out <dir> [--no-streamlit] [--serve] [--port N] [--strict]
//! simard cartographer inspect --out <dir>
//! simard cartographer serve   --out <dir> [--host <addr>] [--port <n>]
//! ```

use std::path::PathBuf;

use crate::cartographer::{self, BuildOptions, DashboardServer, Manifest};

pub(super) const CARTOGRAPHER_HELP: &str = "\
Simard cartographer subcommand — data storytelling & dashboards

Usage:
  simard cartographer build (--brief <brief.json> | --dataset <data> --question <q>) \\
      --out <dir> [--no-streamlit] [--serve] [--host <addr>] [--port <n>] [--strict]
  simard cartographer inspect --out <dir>
  simard cartographer serve --out <dir> [--host <addr>] [--port <n>]

build    Take a dataset + question to an interactive Plotly dashboard
         (dashboard.html), a written narrative (narrative.md), a machine-readable
         analysis (analysis.json), and a Streamlit app (app.py), described by
         <out>/manifest.json. --no-streamlit skips app.py. --serve serves the
         package after building. --strict exits non-zero if verification fails.
inspect  Re-read and re-verify an existing package manifest under <dir>.
serve    Serve a built package over HTTP (\"/\" -> dashboard.html).
";

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8787;

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
    brief: Option<PathBuf>,
    dataset: Option<PathBuf>,
    question: Option<String>,
    out: PathBuf,
    streamlit: bool,
    serve: bool,
    host: String,
    port: u16,
    strict: bool,
}

fn parse_build_args(
    mut args: impl Iterator<Item = String>,
) -> Result<BuildArgs, Box<dyn std::error::Error>> {
    let mut brief: Option<PathBuf> = None;
    let mut dataset: Option<PathBuf> = None;
    let mut question: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut streamlit = true;
    let mut serve = false;
    let mut host = DEFAULT_HOST.to_string();
    let mut port = DEFAULT_PORT;
    let mut strict = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--brief" => brief = Some(PathBuf::from(args.next().ok_or("--brief requires a path")?)),
            "--dataset" => {
                dataset = Some(PathBuf::from(
                    args.next().ok_or("--dataset requires a path")?,
                ))
            }
            "--question" => question = Some(args.next().ok_or("--question requires text")?),
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?)),
            "--no-streamlit" => streamlit = false,
            "--serve" => serve = true,
            "--host" => host = args.next().ok_or("--host requires an address")?,
            "--port" => {
                port = args
                    .next()
                    .ok_or("--port requires a number")?
                    .parse()
                    .map_err(|e| format!("invalid --port: {e}"))?
            }
            "--strict" => strict = true,
            "--help" | "-h" => return Err("help".into()),
            other => {
                if let Some(v) = other.strip_prefix("--brief=") {
                    brief = Some(PathBuf::from(v));
                } else if let Some(v) = other.strip_prefix("--dataset=") {
                    dataset = Some(PathBuf::from(v));
                } else if let Some(v) = other.strip_prefix("--question=") {
                    question = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--out=") {
                    out = Some(PathBuf::from(v));
                } else if let Some(v) = other.strip_prefix("--host=") {
                    host = v.to_string();
                } else if let Some(v) = other.strip_prefix("--port=") {
                    port = v.parse().map_err(|e| format!("invalid --port: {e}"))?;
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
            }
        }
    }

    let out = out.ok_or("missing required --out <dir>")?;
    if brief.is_none() && (dataset.is_none() || question.is_none()) {
        return Err(
            "provide either --brief <brief.json> or both --dataset <data> and --question <q>"
                .into(),
        );
    }
    Ok(BuildArgs {
        brief,
        dataset,
        question,
        out,
        streamlit,
        serve,
        host,
        port,
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
        streamlit: parsed.streamlit,
    };

    let manifest = match &parsed.brief {
        Some(brief_path) => cartographer::build_package(brief_path, &parsed.out, options)?,
        None => cartographer::build_package_ad_hoc(
            parsed.dataset.as_ref().expect("validated present"),
            parsed.question.as_deref().expect("validated present"),
            &parsed.out,
            options,
        )?,
    };
    print_manifest_summary(&manifest, &parsed.out);

    if parsed.strict {
        manifest.verified()?;
    }

    if parsed.serve {
        serve_dir(&parsed.out, &parsed.host, parsed.port)?;
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
    manifest.verified()?;
    Ok(())
}

fn run_serve(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut out: Option<PathBuf> = None;
    let mut host = DEFAULT_HOST.to_string();
    let mut port = DEFAULT_PORT;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?)),
            "--host" => host = args.next().ok_or("--host requires an address")?,
            "--port" => {
                port = args
                    .next()
                    .ok_or("--port requires a number")?
                    .parse()
                    .map_err(|e| format!("invalid --port: {e}"))?
            }
            "--help" | "-h" => {
                print!("{CARTOGRAPHER_HELP}");
                return Ok(());
            }
            other => {
                if let Some(v) = other.strip_prefix("--out=") {
                    out = Some(PathBuf::from(v));
                } else if let Some(v) = other.strip_prefix("--host=") {
                    host = v.to_string();
                } else if let Some(v) = other.strip_prefix("--port=") {
                    port = v.parse().map_err(|e| format!("invalid --port: {e}"))?;
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
            }
        }
    }
    let out = out.ok_or("missing required --out <dir>")?;
    serve_dir(&out, &host, port)
}

fn serve_dir(
    out: &std::path::Path,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let server = DashboardServer::bind(out, host, port)?;
    let addr = server.local_addr()?;
    println!("cartographer: serving {} at http://{addr}/", out.display());
    println!("  open http://{addr}/ (Ctrl-C to stop)");
    server.serve_forever()?;
    Ok(())
}

fn print_manifest_summary(manifest: &Manifest, out: &std::path::Path) {
    println!(
        "cartographer: {} — {} rows / {} columns, {} chart(s) [{}]",
        manifest.title,
        manifest.row_count,
        manifest.column_count,
        manifest.chart_count,
        manifest.chart_kinds.join(", "),
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
        }
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
    fn parse_build_args_brief_form() {
        let a = parse_build_args(args(&["--brief", "b.json", "--out", "o"])).unwrap();
        assert_eq!(a.brief, Some(PathBuf::from("b.json")));
        assert_eq!(a.out, PathBuf::from("o"));
        assert!(a.streamlit);
    }

    #[test]
    fn parse_build_args_dataset_question_form() {
        let a = parse_build_args(args(&[
            "--dataset",
            "d.csv",
            "--question",
            "why?",
            "--out",
            "o",
            "--no-streamlit",
        ]))
        .unwrap();
        assert_eq!(a.dataset, Some(PathBuf::from("d.csv")));
        assert_eq!(a.question.as_deref(), Some("why?"));
        assert!(!a.streamlit);
    }

    #[test]
    fn parse_build_args_equals_form_and_port() {
        let a = parse_build_args(args(&[
            "--brief=b.json",
            "--out=o",
            "--serve",
            "--port=9000",
        ]))
        .unwrap();
        assert_eq!(a.brief, Some(PathBuf::from("b.json")));
        assert!(a.serve);
        assert_eq!(a.port, 9000);
    }

    #[test]
    fn parse_build_args_requires_out() {
        assert!(parse_build_args(args(&["--brief", "b.json"])).is_err());
    }

    #[test]
    fn parse_build_args_requires_brief_or_dataset_question() {
        assert!(parse_build_args(args(&["--out", "o"])).is_err());
        assert!(parse_build_args(args(&["--dataset", "d.csv", "--out", "o"])).is_err());
    }

    #[test]
    fn parse_build_args_rejects_bad_port() {
        assert!(
            parse_build_args(args(&["--brief", "b.json", "--out", "o", "--port", "abc"])).is_err()
        );
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

    #[test]
    fn build_then_inspect_via_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let dataset = dir.path().join("d.csv");
        std::fs::write(&dataset, "region,revenue\nNorth,10\nSouth,20\nNorth,30\n").unwrap();
        let out = dir.path().join("pkg");
        dispatch_cartographer_command(args(&[
            "build",
            "--dataset",
            dataset.to_str().unwrap(),
            "--question",
            "which region?",
            "--out",
            out.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(out.join("dashboard.html").exists());
        dispatch_cartographer_command(args(&["inspect", "--out", out.to_str().unwrap()])).unwrap();
    }
}
