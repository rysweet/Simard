//! End-to-end integration test for the Cartographer data-storytelling pipeline
//! exposed via `simard cartographer build` / `serve` / `inspect`.
//!
//! The Cartographer identity (see `src/cartographer/`) takes a dataset + a
//! question and drives a pure-Rust pipeline to a written narrative
//! (`narrative.md`), designed charts (`charts.json`), and a served interactive
//! dashboard (`dashboard.html`, Plotly + D3) plus a `manifest.json` describing
//! the build and its verification result.
//!
//! Unlike Atelier, the happy path has **no external tool dependency** — the
//! dashboard is generated and served entirely in Rust — so this test always
//! runs. Streamlit / Observable are optional delivery targets whose runtime
//! availability is only recorded, never required.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &Path) -> serde_json::Value {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("manifest {} should be readable: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("manifest {} should be valid JSON: {e}", path.display()))
}

#[test]
fn cartographer_build_takes_a_dataset_and_question_to_a_served_dashboard() {
    let brief = repo_root().join("tests/fixtures/cartographer/regional-sales-brief.json");
    assert!(
        brief.exists(),
        "fixture brief should exist: {}",
        brief.display()
    );

    let out = tempfile::tempdir().expect("tempdir");
    let out_dir = out.path().join("pkg");

    // Build: dataset + question -> narrative + charts + interactive dashboard.
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("cartographer")
        .arg("build")
        .arg("--brief")
        .arg(&brief)
        .arg("--out")
        .arg(&out_dir)
        .arg("--strict")
        .output()
        .expect("simard cartographer build should spawn");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "cartographer build should succeed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Core deliverables must exist and be non-trivial.
    for (name, min_bytes) in [
        ("dataset.csv", 32usize),
        ("charts.json", 32),
        ("narrative.md", 64),
        ("dashboard.html", 512),
        ("manifest.json", 64),
    ] {
        let path = out_dir.join(name);
        let meta =
            std::fs::metadata(&path).unwrap_or_else(|e| panic!("{name} should be produced: {e}"));
        assert!(
            meta.len() as usize >= min_bytes,
            "{name} should be non-trivial (>= {min_bytes} bytes), got {}",
            meta.len()
        );
    }

    // The narrative must answer the question explicitly.
    let narrative = std::fs::read_to_string(out_dir.join("narrative.md")).expect("narrative");
    assert!(
        narrative.contains("Answer"),
        "narrative should contain an explicit Answer section: {narrative:.120}"
    );

    // The dashboard must embed data and reference interactive Plotly views.
    let dashboard = std::fs::read_to_string(out_dir.join("dashboard.html")).expect("dashboard");
    assert!(
        dashboard.contains("Plotly"),
        "dashboard should render Plotly views"
    );
    assert!(
        dashboard.contains("application/json"),
        "dashboard should embed the dataset/charts as JSON"
    );

    // Manifest + verification contract.
    let manifest = read_json(&out_dir.join("manifest.json"));
    assert!(
        manifest["row_count"].as_u64().unwrap_or(0) >= 1,
        "dataset should have rows; manifest: {manifest}"
    );
    assert!(
        manifest["chart_count"].as_u64().unwrap_or(0) >= 1,
        "at least one chart should be designed; manifest: {manifest}"
    );
    assert_eq!(
        manifest["verification"]["ok"], true,
        "verification.ok should be true; manifest: {manifest}"
    );

    // `inspect` should re-read the package and report the same verification.
    let inspect = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("cartographer")
        .arg("inspect")
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("simard cartographer inspect should spawn");
    assert!(
        inspect.status.success(),
        "cartographer inspect should succeed: {}",
        String::from_utf8_lossy(&inspect.stderr)
    );

    // `serve --self-check` proves the dashboard is actually served over HTTP.
    let serve = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("cartographer")
        .arg("serve")
        .arg("--out")
        .arg(&out_dir)
        .arg("--self-check")
        .output()
        .expect("simard cartographer serve should spawn");
    let serve_out = String::from_utf8_lossy(&serve.stdout);
    assert!(
        serve.status.success(),
        "cartographer serve self-check should succeed: {}\nstdout:\n{serve_out}",
        String::from_utf8_lossy(&serve.stderr)
    );
    assert!(
        serve_out.contains("PASS"),
        "serve self-check should report PASS: {serve_out}"
    );
}
