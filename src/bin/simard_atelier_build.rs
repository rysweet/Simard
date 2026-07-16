//! `simard_atelier_build` — turn a product brief into fabrication artifacts.
//!
//! This is the repo-grounded surface the `simard-atelier` identity drives to
//! take a declarative product brief to an exported model + render end-to-end.
//!
//! Usage:
//!   simard-atelier-build --brief <brief.json> --out <dir> [--no-cad]
//!
//! On success: writes model.scad, model.stl, render.svg, cutlist.csv, bom.csv,
//! and manifest.json into <dir>, prints a one-line summary, exits 0.
//! On error: writes the error to stderr, exits 2.

use std::path::PathBuf;
use std::process::ExitCode;

use simard::atelier::{PipelineOptions, run_pipeline_from_file};

fn die(msg: impl AsRef<str>) -> ExitCode {
    eprintln!("simard-atelier-build: {}", msg.as_ref());
    ExitCode::from(2)
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let args = &argv[1..].to_vec();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "usage: simard-atelier-build --brief <brief.json> --out <dir> [--no-cad]\n\n\
             Generates model.scad, model.stl, render.svg, cutlist.csv, bom.csv,\n\
             and manifest.json from a declarative product brief. When the\n\
             `openscad` binary is available (and --no-cad is not passed) it also\n\
             emits a high-fidelity STL and PNG render."
        );
        return ExitCode::SUCCESS;
    }

    let brief = match arg(args, "--brief") {
        Some(p) => PathBuf::from(p),
        None => return die("missing required flag --brief <brief.json>"),
    };
    let out = match arg(args, "--out") {
        Some(p) => PathBuf::from(p),
        None => return die("missing required flag --out <dir>"),
    };
    let options = PipelineOptions {
        use_openscad: !args.iter().any(|a| a == "--no-cad"),
    };

    match run_pipeline_from_file(&brief, &out, &options) {
        Ok(outcome) => {
            println!("{}", outcome.summary());
            for artifact in &outcome.manifest.artifacts {
                println!(
                    "  [{}] {} ({})",
                    artifact.kind, artifact.file, artifact.producer
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => die(e.to_string()),
    }
}
