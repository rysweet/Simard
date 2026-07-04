//! `simard-rust-gym` — run the Rust competency gym and emit scorecards.
//!
//! Roadmap #2491 first experiment: measure the baseline Rust pass-rate (no
//! pack), the pack-lifted pass-rate, and the deliberately-degraded calibration
//! run, then write an inspectable JSON artifact and print the "before/after"
//! numbers. The calibration guard (issue #1241) is enforced: a non-zero exit
//! means the grader failed to distinguish real competence from a degraded state.
//!
//! Usage:
//!   simard-rust-gym [OUTPUT_DIR]
//!
//! OUTPUT_DIR defaults to `target/simard-rust-gym`.

use std::path::PathBuf;

use serde::Serialize;
use simard::rust_expertise::ingest::IngestReport;
use simard::rust_expertise::measurement::{
    CALIBRATION_DEGRADED_MAX, CALIBRATION_HEALTHY_MIN, CALIBRATION_MIN_GAP, calibration_gap,
};
use simard::rust_expertise::{RustScorecard, run_baseline, run_degraded, run_with_pack};

#[derive(Serialize)]
struct GymArtifact {
    domain: String,
    baseline: RustScorecard,
    with_pack: RustScorecard,
    degraded: RustScorecard,
    pack_ingest: IngestReport,
    calibration: Calibration,
}

#[derive(Serialize)]
struct Calibration {
    healthy_pass_rate: f64,
    degraded_pass_rate: f64,
    gap: f64,
    healthy_min: f64,
    degraded_max: f64,
    min_gap: f64,
    passed: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/simard-rust-gym"));

    let baseline = run_baseline()?;
    let (pack_ingest, with_pack) = run_with_pack()?;
    let (_degraded_ingest, degraded) = run_degraded()?;

    let gap = calibration_gap(&with_pack, &degraded);
    let calibration = Calibration {
        healthy_pass_rate: with_pack.pass_rate,
        degraded_pass_rate: degraded.pass_rate,
        gap,
        healthy_min: CALIBRATION_HEALTHY_MIN,
        degraded_max: CALIBRATION_DEGRADED_MAX,
        min_gap: CALIBRATION_MIN_GAP,
        passed: with_pack.pass_rate > CALIBRATION_HEALTHY_MIN
            && degraded.pass_rate < CALIBRATION_DEGRADED_MAX
            && gap >= CALIBRATION_MIN_GAP,
    };

    println!("=== Simard Rust competency gym (roadmap #2491) ===");
    println!("baseline  {}", baseline.headline());
    println!("with-pack {}", with_pack.headline());
    println!("degraded  {}", degraded.headline());
    println!(
        "pack yield: {} facts + {} procedures ingested into cognitive memory",
        pack_ingest.facts_ingested, pack_ingest.procedures_ingested
    );
    println!("per-sub-skill (with pack):");
    for s in &with_pack.per_subskill {
        println!(
            "  {:<16} {:.2} ({}/{})",
            s.subskill, s.pass_rate, s.passed, s.total
        );
    }
    println!(
        "calibration guard (#1241): healthy {:.2} > {:.2}, degraded {:.2} < {:.2}, gap {:.2} >= {:.2} => {}",
        calibration.healthy_pass_rate,
        calibration.healthy_min,
        calibration.degraded_pass_rate,
        calibration.degraded_max,
        calibration.gap,
        calibration.min_gap,
        if calibration.passed { "PASS" } else { "FAIL" },
    );

    let calibration_passed = calibration.passed;
    let artifact = GymArtifact {
        domain: "rust".to_string(),
        baseline,
        with_pack,
        degraded,
        pack_ingest,
        calibration,
    };

    std::fs::create_dir_all(&output_dir)?;
    let artifact_path = output_dir.join("scorecard.json");
    std::fs::write(&artifact_path, serde_json::to_vec_pretty(&artifact)?)?;
    println!("wrote scorecard artifact: {}", artifact_path.display());

    if !calibration_passed {
        eprintln!("calibration guard FAILED: grader does not distinguish real competence");
        std::process::exit(1);
    }
    Ok(())
}
