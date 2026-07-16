//! End-to-end integration test for the Cartographer data-storytelling pipeline
//! exposed via `simard cartographer build` / `inspect` / the built-in server.
//!
//! The Cartographer identity (see `src/cartographer/`) takes a dataset + a
//! question and drives a pure-Rust pipeline to produce a profiled analysis, an
//! interactive Plotly dashboard, a written narrative, and a Streamlit app, all
//! described by a verified `manifest.json`. Unlike a CAD pipeline it has no hard
//! external dependency, so this test always runs.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;

use simard::cartographer::DashboardServer;

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
    let brief = repo_root().join("tests/fixtures/cartographer/sales-brief.json");
    assert!(
        brief.exists(),
        "fixture brief should exist: {}",
        brief.display()
    );

    let out = tempfile::tempdir().expect("tempdir");
    let out_dir = out.path().join("pkg");

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("cartographer")
        .arg("build")
        .arg("--brief")
        .arg(&brief)
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("simard cartographer build should spawn");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "cartographer build should succeed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Core deliverables must exist and be non-empty.
    for (name, min_bytes) in [
        ("analysis.json", 64usize),
        ("dashboard.html", 512),
        ("narrative.md", 128),
        ("app.py", 128),
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

    // The dashboard must be a real interactive Plotly page.
    let html = std::fs::read_to_string(out_dir.join("dashboard.html")).expect("html readable");
    assert!(
        html.starts_with("<!DOCTYPE html>"),
        "dashboard should be HTML"
    );
    assert!(
        html.contains("cdn.plot.ly"),
        "dashboard should load Plotly.js"
    );
    assert!(
        html.contains("Plotly.newPlot"),
        "dashboard should render charts client-side"
    );

    // The narrative must restate the question and have findings.
    let narrative = std::fs::read_to_string(out_dir.join("narrative.md")).expect("md readable");
    assert!(
        narrative.contains("Which region and product drive the most revenue"),
        "narrative should restate the question"
    );
    assert!(
        narrative.contains("## Key findings"),
        "narrative should have a findings section"
    );

    // Manifest + verification contract.
    let manifest = read_json(&out_dir.join("manifest.json"));
    assert_eq!(manifest["title"], "Regional Sales Dashboard");
    assert!(
        manifest["chart_count"].as_u64().unwrap_or(0) >= 1,
        "at least one chart should be designed"
    );
    assert_eq!(
        manifest["verification"]["ok"], true,
        "verification.ok should be true; manifest: {manifest}"
    );

    // analysis.json must carry the column profile and chart specs.
    let analysis = read_json(&out_dir.join("analysis.json"));
    assert!(
        analysis["profile"]["columns"]
            .as_array()
            .is_some_and(|c| !c.is_empty()),
        "analysis should profile the columns"
    );
    assert!(
        analysis["charts"].as_array().is_some_and(|c| !c.is_empty()),
        "analysis should include chart specs"
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

    // End-to-end: the built dashboard must be *serveable* and reachable over
    // HTTP. Serve the package in-process on an ephemeral port and GET `/`.
    let server = DashboardServer::bind(&out_dir, "127.0.0.1", 0).expect("bind server");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || {
        server.accept_and_handle().expect("serve one request");
    });

    let mut client = TcpStream::connect(addr).expect("connect to dashboard server");
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .expect("send request");
    let mut response = String::new();
    client.read_to_string(&mut response).expect("read response");
    handle.join().expect("server thread");

    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "served dashboard should return 200: {response:.80}"
    );
    assert!(
        response.contains("text/html"),
        "served dashboard should be HTML"
    );
    assert!(
        response.contains("Plotly.newPlot"),
        "served dashboard should be the interactive Plotly page"
    );
}
