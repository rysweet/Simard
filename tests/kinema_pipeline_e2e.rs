//! End-to-end integration test for the Kinema animation pipeline exposed via
//! `simard kinema build`.
//!
//! The Kinema identity (see `src/kinema/`) takes a shot brief and drives a
//! guaranteed pure-Rust rasterizer to a rendered animated PNG frame sequence,
//! alongside a storyboard, a rig (one armature per object), a Synfig vector
//! source, and a `manifest.json` describing the build and its verification
//! result. Blender (Grease Pencil), Synfig, and Natron are additionally driven
//! when installed and degrade gracefully when not.
//!
//! Unlike Atelier (which hard-depends on OpenSCAD), Kinema's happy path has no
//! external dependency, so this test never skips: the animated sequence must
//! render end-to-end on any host.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &Path) -> serde_json::Value {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("file {} should be readable: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("file {} should be valid JSON: {e}", path.display()))
}

/// Assert the file begins with the 8-byte PNG signature and is non-trivial.
fn assert_is_png(path: &Path) {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("frame {} should be readable: {e}", path.display()));
    assert!(
        bytes.len() > 64,
        "frame {} should be a non-trivial PNG (got {} bytes)",
        path.display(),
        bytes.len()
    );
    assert_eq!(
        &bytes[..8],
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        "frame {} should carry the PNG signature",
        path.display()
    );
}

#[test]
fn kinema_build_takes_a_brief_to_a_rendered_animated_sequence() {
    let brief = repo_root().join("tests/fixtures/kinema/hero-crossing.json");
    assert!(
        brief.exists(),
        "fixture brief should exist: {}",
        brief.display()
    );

    let out = tempfile::tempdir().expect("tempdir");
    let out_dir = out.path().join("pkg");

    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("kinema")
        .arg("build")
        .arg("--brief")
        .arg(&brief)
        .arg("--out")
        .arg(&out_dir)
        .arg("--strict")
        .output()
        .expect("simard kinema build should spawn");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "kinema build should succeed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Core deliverables: storyboard, rig, vector source, sequence descriptor,
    // and manifest must all exist and be non-empty.
    for (name, min_bytes) in [
        ("storyboard.json", 1usize),
        ("storyboard.md", 1),
        ("rig.json", 1),
        ("shot.sif", 64),
        ("sequence.json", 1),
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

    // The Synfig vector source must be a real .sif document.
    let sif = std::fs::read_to_string(out_dir.join("shot.sif")).expect("sif readable");
    assert!(
        sif.contains("<canvas"),
        "shot.sif should be a Synfig canvas document: {sif:.80}"
    );

    // Manifest + verification contract.
    let manifest = read_json(&out_dir.join("manifest.json"));
    assert_eq!(manifest["shot_name"], "hero-crossing");
    assert_eq!(manifest["style"], "grease-pencil-2d");
    assert_eq!(
        manifest["object_count"].as_u64().unwrap_or(0),
        3,
        "the brief has three objects"
    );
    let frame_count = manifest["frame_count"].as_u64().unwrap_or(0);
    assert_eq!(frame_count, 24, "2.0s @ 12fps should render 24 frames");
    assert_eq!(
        manifest["frames_rendered"].as_u64().unwrap_or(0),
        frame_count,
        "every expected frame should be rendered"
    );
    assert!(
        manifest["bone_count"].as_u64().unwrap_or(0) >= 9,
        "a 7-bone character plus two single-bone shapes = 9 bones"
    );
    assert_eq!(
        manifest["verification"]["ok"], true,
        "verification.ok should be true; manifest: {manifest}"
    );

    // Every frame on disk must be a real PNG, and the count must match.
    let frames_dir = out_dir.join("frames");
    let mut png_frames: Vec<PathBuf> = std::fs::read_dir(&frames_dir)
        .expect("frames dir should exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "png").unwrap_or(false))
        .collect();
    png_frames.sort();
    assert_eq!(
        png_frames.len() as u64,
        frame_count,
        "frames/ should hold exactly {frame_count} PNG frames"
    );
    assert_is_png(&png_frames[0]);
    assert_is_png(&png_frames[png_frames.len() - 1]);

    // The sequence descriptor must agree with the rendered frames.
    let sequence = read_json(&out_dir.join("sequence.json"));
    assert_eq!(
        sequence["frame_count"].as_u64().unwrap_or(0),
        frame_count,
        "sequence.json frame_count should match the manifest"
    );

    // `inspect` should re-read the package and report the same passing
    // verification.
    let inspect = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("kinema")
        .arg("inspect")
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("simard kinema inspect should spawn");
    assert!(
        inspect.status.success(),
        "kinema inspect should succeed: {}",
        String::from_utf8_lossy(&inspect.stderr)
    );

    // Removing a frame must make inspect fail: the durable rendered sequence is
    // the verified business outcome, not a checkbox.
    std::fs::remove_file(&png_frames[png_frames.len() - 1]).expect("remove last frame");
    let inspect_broken = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("kinema")
        .arg("inspect")
        .arg("--out")
        .arg(&out_dir)
        .output()
        .expect("simard kinema inspect should spawn");
    assert!(
        !inspect_broken.status.success(),
        "kinema inspect should fail when a rendered frame is missing"
    );
}
