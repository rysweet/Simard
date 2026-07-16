//! End-to-end integration test for the Atelier furniture/product-design
//! pipeline exposed via `simard atelier build`.
//!
//! The Atelier identity (see `src/atelier/`) takes a product brief and drives
//! OpenSCAD to produce a 3D model (STL), a render (PNG), and a fabrication
//! package (cut list + BOM CSV) plus a `manifest.json` describing the build
//! and its verification result.
//!
//! OpenSCAD is the only hard dependency of the happy path. When it is not
//! installed (some CI images), the STL export cannot run, so the test skips
//! itself rather than failing — matching the pipeline's graceful-degradation
//! contract for the optional FreeCAD/Blender drivers.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn openscad_available() -> bool {
    Command::new("openscad")
        .arg("--version")
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty() || !o.stdout.is_empty())
        .unwrap_or(false)
}

fn read_json(path: &Path) -> serde_json::Value {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("manifest {} should be readable: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("manifest {} should be valid JSON: {e}", path.display()))
}

#[test]
fn atelier_build_takes_a_brief_to_model_render_and_fabrication_package() {
    if !openscad_available() {
        eprintln!("skipping: openscad not installed on this host");
        return;
    }

    let brief = repo_root().join("tests/fixtures/atelier/bookcase-brief.json");
    assert!(
        brief.exists(),
        "fixture brief should exist: {}",
        brief.display()
    );

    let out = tempfile::tempdir().expect("tempdir");
    let out_dir = out.path().join("pkg");

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("atelier")
        .arg("build")
        .arg("--brief")
        .arg(&brief)
        .arg("--out")
        .arg(&out_dir)
        .arg("--fabrication")
        .output()
        .expect("simard atelier build should spawn");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "atelier build should succeed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Core deliverables: a 3D model and a fabrication package must exist and be
    // non-empty.
    for (name, min_bytes) in [
        ("model.scad", 1usize),
        ("model.stl", 128),
        ("cutlist.csv", 1),
        ("bom.csv", 1),
        ("manifest.json", 1),
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

    // The STL must be a real OpenSCAD-exported solid.
    let stl = std::fs::read_to_string(out_dir.join("model.stl")).expect("stl readable");
    assert!(
        stl.contains("solid"),
        "STL should contain a solid: {stl:.64}"
    );
    assert!(stl.contains("facet"), "STL should contain facets");

    // Manifest + verification contract.
    let manifest = read_json(&out_dir.join("manifest.json"));
    assert_eq!(manifest["kind"], "bookcase");
    assert_eq!(manifest["product_name"], "Two-shelf bookcase");
    assert!(
        manifest["part_count"].as_u64().unwrap_or(0) >= 3,
        "bookcase should have several parts"
    );
    assert_eq!(
        manifest["verification"]["ok"], true,
        "verification.ok should be true; manifest: {manifest}"
    );

    // Cut list header + at least one data row.
    let cutlist = std::fs::read_to_string(out_dir.join("cutlist.csv")).expect("cutlist readable");
    assert!(
        cutlist.starts_with("part,qty,length_mm,width_mm,thickness_mm,material,grain"),
        "cutlist header unexpected: {cutlist:.80}"
    );
    assert!(
        cutlist.lines().count() >= 2,
        "cutlist should have data rows: {cutlist}"
    );

    // BOM header + material row.
    let bom = std::fs::read_to_string(out_dir.join("bom.csv")).expect("bom readable");
    assert!(
        bom.starts_with("item,category,qty,unit,unit_cost,total_cost"),
        "bom header unexpected: {bom:.80}"
    );
    assert!(
        bom.contains("material"),
        "bom should include a material line: {bom}"
    );

    // `inspect` should re-read the package and report the same verification.
    let inspect = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("atelier")
        .arg("inspect")
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("simard atelier inspect should spawn");
    assert!(
        inspect.status.success(),
        "atelier inspect should succeed: {}",
        String::from_utf8_lossy(&inspect.stderr)
    );
}
