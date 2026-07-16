//! End-to-end integration test for the Gastronome culinary/menu/event-design
//! pipeline exposed via `simard gastronome build`.
//!
//! The Gastronome identity (see `src/gastronome/`) takes an event/menu brief and
//! produces a menu card (`menu.md`), a consolidated shopping list, a nutrition
//! breakdown, a back-timed prep schedule, and — with `--prep-app` — a
//! self-contained kitchen prep app, plus a `manifest.json` describing the plan
//! and its verification result.
//!
//! Gastronome has no external tool dependency, so — unlike Atelier — this test
//! always runs; there is nothing to skip.

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
fn gastronome_build_takes_a_brief_to_a_costed_scheduled_menu_plan() {
    let brief = repo_root().join("tests/fixtures/gastronome/dinner-brief.json");
    assert!(
        brief.exists(),
        "fixture brief should exist: {}",
        brief.display()
    );

    let out = tempfile::tempdir().expect("tempdir");
    let out_dir = out.path().join("plan");

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("gastronome")
        .arg("build")
        .arg("--brief")
        .arg(&brief)
        .arg("--out")
        .arg(&out_dir)
        .arg("--prep-app")
        .arg("--strict")
        .output()
        .expect("simard gastronome build should spawn");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "gastronome build should succeed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Core deliverables must exist and be non-empty.
    for (name, min_bytes) in [
        ("menu.md", 1usize),
        ("shopping_list.csv", 1),
        ("nutrition.csv", 1),
        ("prep_schedule.csv", 1),
        ("prep_app.html", 128),
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

    // Manifest + verification contract.
    let manifest = read_json(&out_dir.join("manifest.json"));
    assert_eq!(manifest["event"], "Autumn tasting dinner");
    assert_eq!(manifest["guests"], 24);
    assert_eq!(manifest["dish_count"], 3);
    assert_eq!(
        manifest["verification"]["ok"], true,
        "verification.ok should be true; manifest: {manifest}"
    );
    assert!(
        manifest["estimated_total_cost"].as_f64().unwrap_or(0.0) > 0.0,
        "cost should be rolled up"
    );
    assert!(
        manifest["total_prep_minutes"].as_f64().unwrap_or(0.0) > 0.0,
        "prep schedule should have a critical path"
    );

    // Shopping list header + at least one aggregated data row.
    let shopping =
        std::fs::read_to_string(out_dir.join("shopping_list.csv")).expect("shopping readable");
    assert!(
        shopping.starts_with("ingredient,unit,qty,unit_cost,total_cost"),
        "shopping list header unexpected: {shopping:.80}"
    );
    assert!(
        shopping.lines().count() >= 2,
        "shopping list should have data rows: {shopping}"
    );

    // Nutrition header + per-guest row.
    let nutrition =
        std::fs::read_to_string(out_dir.join("nutrition.csv")).expect("nutrition readable");
    assert!(
        nutrition.starts_with("scope,detail,servings,kcal,protein_g,carbs_g,fat_g"),
        "nutrition header unexpected: {nutrition:.80}"
    );
    assert!(
        nutrition.contains("per_guest"),
        "nutrition should include a per-guest line: {nutrition}"
    );

    // Prep schedule header + back-timed clock time (service is 19:00).
    let schedule =
        std::fs::read_to_string(out_dir.join("prep_schedule.csv")).expect("schedule readable");
    assert!(
        schedule.starts_with("order,dish,task,station,minutes,start_offset_min,start_clock"),
        "schedule header unexpected: {schedule:.80}"
    );
    assert!(
        schedule.lines().count() >= 2,
        "schedule should have data rows: {schedule}"
    );

    // Prep app is a self-contained HTML file with no external resources.
    let app = std::fs::read_to_string(out_dir.join("prep_app.html")).expect("app readable");
    assert!(app.contains("<!doctype html>"), "prep app should be HTML");
    assert!(
        !app.contains("http://") && !app.contains("https://"),
        "prep app must be fully self-contained (no network resources)"
    );

    // `inspect` should re-read the plan and report the same verification.
    let inspect = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("gastronome")
        .arg("inspect")
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("simard gastronome inspect should spawn");
    assert!(
        inspect.status.success(),
        "gastronome inspect should succeed: {}",
        String::from_utf8_lossy(&inspect.stderr)
    );
}

#[test]
fn gastronome_build_rejects_an_invalid_brief() {
    let out = tempfile::tempdir().expect("tempdir");
    let brief_path = out.path().join("bad-brief.json");
    // Zero guests is semantically invalid.
    std::fs::write(
        &brief_path,
        r#"{"event":"x","guests":0,"dishes":[{"name":"d","course":"main",
            "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}]}]}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("gastronome")
        .arg("build")
        .arg("--brief")
        .arg(&brief_path)
        .arg("--out")
        .arg(out.path().join("plan"))
        .output()
        .expect("simard gastronome build should spawn");

    assert!(
        !output.status.success(),
        "an invalid brief should make the build fail"
    );
}
