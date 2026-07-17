//! Package orchestration, manifest, and verification.
//!
//! [`build_package`] is the end-to-end entry point: shot brief → storyboard →
//! rig → rendered frame sequence (pure-Rust) → optional Blender/Synfig/Natron
//! renders → verified `manifest.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::brief::ShotBrief;
use super::drivers;
use super::error::{KinemaError, KinemaResult};
use super::render::{self, Sequence};
use super::rig::{self, Rig};
use super::storyboard::{self, Storyboard};

/// Options controlling which optional engines are attempted.
#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    /// Attempt the Blender Grease Pencil render when Blender is installed.
    pub grease_pencil: bool,
    /// Attempt the Natron composite pass when Natron is installed.
    pub composite: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            grease_pencil: true,
            composite: true,
        }
    }
}

/// One produced (or skipped) artifact in the package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub file: String,
    pub kind: String,
    pub present: bool,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A single verification check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// Verification result for the whole package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub ok: bool,
    /// Whether an optional external engine produced a render.
    pub external_render_ok: bool,
    pub checks: Vec<Check>,
}

/// The package manifest, written as `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub shot_name: String,
    pub style: String,
    pub fps: u32,
    pub duration_s: f64,
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub frames_rendered: u32,
    pub object_count: u32,
    pub panel_count: u32,
    pub bone_count: u32,
    pub tools: Vec<drivers::ToolReport>,
    pub artifacts: Vec<Artifact>,
    pub verification: Verification,
}

impl Manifest {
    /// Consume the manifest, returning an error if verification did not pass.
    /// Advisory checks (external renders) never fail this; only the required
    /// minimum (storyboard, rig, rendered frames, sequence, vector source) does.
    pub fn verified(self) -> KinemaResult<Self> {
        if self.verification.ok {
            Ok(self)
        } else {
            let failed: Vec<String> = self
                .verification
                .checks
                .iter()
                .filter(|c| !c.ok)
                .map(|c| format!("{}: {}", c.name, c.detail))
                .collect();
            Err(KinemaError::verification(failed.join("; ")))
        }
    }
}

const STORYBOARD_JSON: &str = "storyboard.json";
const STORYBOARD_MD: &str = "storyboard.md";
const RIG_JSON: &str = "rig.json";
const SHOT_SIF: &str = "shot.sif";
const MANIFEST_JSON: &str = "manifest.json";
const BLENDER_DIR: &str = "blender";
const SYNFIG_DIR: &str = "synfig";
const COMPOSITE_DIR: &str = "composite";

fn write_file(path: &Path, contents: &str) -> KinemaResult<()> {
    std::fs::write(path, contents)
        .map_err(|e| KinemaError::io(format!("writing {}", path.display()), e))
}

fn bytes_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn dir_bytes(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Build the full animation package for `brief_path` into `out_dir`.
pub fn build_package(
    brief_path: &Path,
    out_dir: &Path,
    options: BuildOptions,
) -> KinemaResult<Manifest> {
    let brief = ShotBrief::from_path(brief_path)?;
    build_package_inner(&brief, Some(brief_path), out_dir, options)
}

/// Build a package from an already-parsed brief. The brief is re-serialised to a
/// temporary file so external engines that consume the brief JSON still work.
pub fn build_package_from_brief(
    brief: &ShotBrief,
    out_dir: &Path,
    options: BuildOptions,
) -> KinemaResult<Manifest> {
    build_package_inner(brief, None, out_dir, options)
}

fn build_package_inner(
    brief: &ShotBrief,
    brief_path: Option<&Path>,
    out_dir: &Path,
    options: BuildOptions,
) -> KinemaResult<Manifest> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| KinemaError::io(format!("creating {}", out_dir.display()), e))?;

    // 1. Storyboard (inspect → beats).
    let storyboard: Storyboard = storyboard::build_storyboard(brief);
    let sb_json = out_dir.join(STORYBOARD_JSON);
    write_file(&sb_json, &storyboard.to_json())?;
    let sb_md = out_dir.join(STORYBOARD_MD);
    write_file(&sb_md, &storyboard.to_markdown())?;

    // 2. Rig (armatures).
    let rig: Rig = rig::build_rig(brief);
    let rig_json = out_dir.join(RIG_JSON);
    write_file(&rig_json, &rig.to_json())?;

    // 3. Vector source (Synfig .sif) — always emitted, portable source.
    let sif_path = out_dir.join(SHOT_SIF);
    write_file(&sif_path, &drivers::synfig_document(brief))?;

    // 4. Rendered frame sequence (guaranteed pure-Rust engine).
    let sequence: Sequence = render::render_sequence(brief, out_dir)?;
    let frames_dir = out_dir.join(render::FRAMES_DIR);

    let mut artifacts = vec![
        artifact(&sb_json, STORYBOARD_JSON, "storyboard", None),
        artifact(&sb_md, STORYBOARD_MD, "storyboard", None),
        artifact(&rig_json, RIG_JSON, "rig", None),
        artifact(&sif_path, SHOT_SIF, "vector-source", None),
        Artifact {
            file: render::SEQUENCE_JSON.to_string(),
            kind: "sequence".to_string(),
            present: bytes_of(&out_dir.join(render::SEQUENCE_JSON)) > 0,
            bytes: bytes_of(&out_dir.join(render::SEQUENCE_JSON)),
            detail: None,
        },
        Artifact {
            file: format!("{}/", render::FRAMES_DIR),
            kind: "frames".to_string(),
            present: sequence.frame_count > 0,
            bytes: dir_bytes(&frames_dir),
            detail: Some(format!("{} PNG frames", sequence.frame_count)),
        },
    ];

    // 5. Optional Blender Grease Pencil render.
    let mut external_render_ok = false;
    if options.grease_pencil {
        let blender_dir = out_dir.join(BLENDER_DIR);
        let brief_json = ensure_brief_json(brief, brief_path, out_dir)?;
        let outcome = drivers::blender_grease_pencil(&brief_json, &blender_dir, out_dir);
        external_render_ok |= outcome.produced;
        artifacts.push(dir_artifact(
            &blender_dir,
            &format!("{BLENDER_DIR}/"),
            "blender-render",
            &outcome.detail,
            outcome.produced,
        ));
    }

    // 6. Optional Synfig render of the emitted .sif.
    {
        let synfig_dir = out_dir.join(SYNFIG_DIR);
        let outcome = drivers::synfig_render(&sif_path, &synfig_dir, brief.fps);
        external_render_ok |= outcome.produced;
        artifacts.push(dir_artifact(
            &synfig_dir,
            &format!("{SYNFIG_DIR}/"),
            "synfig-render",
            &outcome.detail,
            outcome.produced,
        ));
    }

    // 7. Optional Natron composite of the rendered frames.
    if options.composite {
        let outcome =
            drivers::natron_composite(&frames_dir, out_dir, out_dir, sequence.frame_count);
        external_render_ok |= outcome.produced;
        let composite_dir = out_dir.join(COMPOSITE_DIR);
        artifacts.push(dir_artifact(
            &composite_dir,
            &format!("{COMPOSITE_DIR}/"),
            "natron-composite",
            &outcome.detail,
            outcome.produced,
        ));
    }

    let tools = vec![
        drivers::probe("blender"),
        drivers::probe("synfig"),
        drivers::probe("NatronRenderer"),
        drivers::probe("natron"),
    ];

    let frames_rendered = render::count_present_frames(out_dir, sequence.frame_count);
    let verification = verify(
        brief,
        &storyboard,
        &rig,
        &sequence,
        out_dir,
        external_render_ok,
    );

    let manifest = Manifest {
        shot_name: brief.name.clone(),
        style: brief.normalized_style().label().to_string(),
        fps: brief.fps,
        duration_s: brief.duration_s,
        width: brief.resolution.width,
        height: brief.resolution.height,
        frame_count: sequence.frame_count,
        frames_rendered,
        object_count: brief.objects.len() as u32,
        panel_count: storyboard.panels.len() as u32,
        bone_count: rig.total_bones() as u32,
        tools,
        artifacts,
        verification,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| KinemaError::parse("manifest json", e.to_string()))?;
    write_file(&out_dir.join(MANIFEST_JSON), &manifest_json)?;

    Ok(manifest)
}

/// Ensure a brief JSON exists on disk for external engines that consume it,
/// returning its path. When the caller built from a file we reuse it; otherwise
/// we serialise the parsed brief into the out-dir.
fn ensure_brief_json(
    brief: &ShotBrief,
    brief_path: Option<&Path>,
    out_dir: &Path,
) -> KinemaResult<PathBuf> {
    if let Some(p) = brief_path {
        return Ok(p.to_path_buf());
    }
    let staged = out_dir.join("brief.json");
    let json = serde_json::to_string_pretty(brief)
        .map_err(|e| KinemaError::parse("brief json", e.to_string()))?;
    write_file(&staged, &json)?;
    Ok(staged)
}

fn artifact(path: &Path, file: &str, kind: &str, detail: Option<String>) -> Artifact {
    let bytes = bytes_of(path);
    Artifact {
        file: file.to_string(),
        kind: kind.to_string(),
        present: bytes > 0,
        bytes,
        detail,
    }
}

fn dir_artifact(dir: &Path, file: &str, kind: &str, detail: &str, produced: bool) -> Artifact {
    Artifact {
        file: file.to_string(),
        kind: kind.to_string(),
        present: produced,
        bytes: dir_bytes(dir),
        detail: Some(detail.to_string()),
    }
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        name: name.to_string(),
        ok,
        detail: detail.into(),
    }
}

/// Verify the produced package.
fn verify(
    brief: &ShotBrief,
    storyboard: &Storyboard,
    rig: &Rig,
    sequence: &Sequence,
    out_dir: &Path,
    external_render_ok: bool,
) -> Verification {
    let mut checks = Vec::new();

    // Storyboard present.
    let storyboard_ok = !storyboard.panels.is_empty();
    checks.push(check(
        "storyboard-present",
        storyboard_ok,
        format!("{} storyboard panels", storyboard.panels.len()),
    ));

    // Rig present: at least one bone per object.
    let rig_ok = rig.armatures.len() == brief.objects.len()
        && rig.armatures.iter().all(|a| a.bone_count() >= 1);
    checks.push(check(
        "rig-present",
        rig_ok,
        format!(
            "{} armatures, {} bones",
            rig.armatures.len(),
            rig.total_bones()
        ),
    ));

    // Frames rendered: every expected frame exists on disk and is non-empty.
    let present = render::count_present_frames(out_dir, sequence.frame_count);
    let frames_ok = sequence.frame_count >= 1 && present == sequence.frame_count;
    checks.push(check(
        "frames-rendered",
        frames_ok,
        format!("{present}/{} frames present", sequence.frame_count),
    ));

    // Sequence descriptor consistent with the rendered frames.
    let sequence_ok = sequence.frames.len() as u32 == sequence.frame_count;
    checks.push(check(
        "sequence-consistent",
        sequence_ok,
        format!(
            "sequence lists {} of {} frames",
            sequence.frames.len(),
            sequence.frame_count
        ),
    ));

    // Vector source present.
    let sif_ok = bytes_of(&out_dir.join(SHOT_SIF)) > 0;
    checks.push(check(
        "vector-source-present",
        sif_ok,
        if sif_ok {
            "shot.sif emitted".into()
        } else {
            "shot.sif missing or empty".to_string()
        },
    ));

    // External render (advisory — does not fail the package).
    checks.push(check(
        "external-render-present",
        external_render_ok,
        if external_render_ok {
            "an external engine produced a render".into()
        } else {
            "no external engine available (pure-Rust frames only)".to_string()
        },
    ));

    let ok = storyboard_ok && rig_ok && frames_ok && sequence_ok && sif_ok;

    Verification {
        ok,
        external_render_ok,
        checks,
    }
}

/// Read and re-check an existing package manifest in `out_dir`.
///
/// `inspect` re-verifies the package against what is actually on disk: it
/// recomputes how many frames survive and, if any required frame has gone
/// missing or empty since build time, flips the `frames-rendered` check and the
/// aggregate `verification.ok` to `false`. The storyboard/rig checks are trusted
/// from the persisted manifest (they cannot change on disk).
pub fn inspect(out_dir: &Path) -> KinemaResult<Manifest> {
    let manifest_path = out_dir.join(MANIFEST_JSON);
    let bytes = std::fs::read(&manifest_path)
        .map_err(|e| KinemaError::io(format!("reading manifest {}", manifest_path.display()), e))?;
    let mut manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| KinemaError::parse("manifest json", e.to_string()))?;

    // Recompute the on-disk state of required artifacts.
    let present = render::count_present_frames(out_dir, manifest.frame_count);
    manifest.frames_rendered = present;
    let sif_ok = bytes_of(&out_dir.join(SHOT_SIF)) > 0;

    for c in &mut manifest.verification.checks {
        match c.name.as_str() {
            "frames-rendered" => {
                c.ok = manifest.frame_count >= 1 && present == manifest.frame_count;
                c.detail = format!("{present}/{} frames present", manifest.frame_count);
            }
            "vector-source-present" => {
                c.ok = sif_ok;
                c.detail = if sif_ok {
                    "shot.sif emitted".into()
                } else {
                    "shot.sif missing or empty".to_string()
                };
            }
            _ => {}
        }
    }

    // Recompute the required-minimum aggregate from the (possibly updated) checks.
    let required = [
        "storyboard-present",
        "rig-present",
        "frames-rendered",
        "sequence-consistent",
        "vector-source-present",
    ];
    manifest.verification.ok = manifest
        .verification
        .checks
        .iter()
        .filter(|c| required.contains(&c.name.as_str()))
        .all(|c| c.ok);

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief() -> ShotBrief {
        let json = r#"{
            "name": "Hero enters", "style": "2d", "fps": 8, "duration_s": 0.5,
            "resolution": { "width": 64, "height": 48 },
            "objects": [
                { "name": "hero", "kind": "character", "size": 0.4,
                  "keyframes": [ {"t":0.0,"x":0.1,"y":0.5}, {"t":0.5,"x":0.6,"y":0.5} ] },
                { "name": "sun", "kind": "circle", "color": {"r":255,"g":220,"b":90}, "size": 0.12,
                  "keyframes": [ {"t":0.0,"x":0.8,"y":0.2} ] }
            ]
        }"#;
        ShotBrief::from_json_bytes(json.as_bytes()).unwrap()
    }

    #[test]
    fn build_produces_full_verified_package() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pkg");
        let m = build_package_from_brief(&brief(), &out, BuildOptions::default()).unwrap();

        assert_eq!(m.shot_name, "Hero enters");
        assert_eq!(m.frame_count, 4);
        assert_eq!(m.frames_rendered, 4);
        assert_eq!(m.object_count, 2);
        assert!(m.bone_count >= 8); // 7-bone character + 1-bone circle
        assert!(m.verification.ok, "verification should pass: {m:?}");

        for name in [
            STORYBOARD_JSON,
            STORYBOARD_MD,
            RIG_JSON,
            SHOT_SIF,
            "sequence.json",
            "manifest.json",
        ] {
            assert!(out.join(name).exists(), "{name} should exist");
        }
        assert!(
            out.join(render::FRAMES_DIR)
                .join("frame_00001.png")
                .exists()
        );
    }

    #[test]
    fn verified_ok_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pkg");
        let m = build_package_from_brief(&brief(), &out, BuildOptions::default()).unwrap();
        assert!(m.verified().is_ok());
    }

    #[test]
    fn inspect_matches_build() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pkg");
        build_package_from_brief(&brief(), &out, BuildOptions::default()).unwrap();
        let inspected = inspect(&out).unwrap();
        assert!(inspected.verification.ok);
        assert_eq!(inspected.frames_rendered, inspected.frame_count);
    }

    #[test]
    fn inspect_flips_when_a_frame_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pkg");
        let m = build_package_from_brief(&brief(), &out, BuildOptions::default()).unwrap();
        // Delete the last frame; inspect must now fail verification.
        let last = out
            .join(render::FRAMES_DIR)
            .join(render::frame_file_name(m.frame_count - 1));
        std::fs::remove_file(&last).unwrap();
        let inspected = inspect(&out).unwrap();
        assert!(!inspected.verification.ok);
        assert!(inspected.frames_rendered < inspected.frame_count);
    }

    #[test]
    fn default_options_enable_optional_engines() {
        let o = BuildOptions::default();
        assert!(o.grease_pencil);
        assert!(o.composite);
    }
}
