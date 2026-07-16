//! Pipeline: product brief -> parametric model -> fabrication artifacts.
//!
//! The pipeline always produces a deterministic, dependency-free set of
//! outputs (OpenSCAD source, STL mesh, SVG render, cut list, BOM, manifest).
//! When the `openscad` binary is available it *additionally* renders a
//! high-fidelity STL and PNG from the generated `.scad`, but those are
//! best-effort: their absence never fails the pipeline. This keeps
//! "brief -> exported model + render" working end-to-end in any environment
//! while still lighting up the full CAD toolchain where present.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use super::brief::ProductBrief;
use super::error::{AtelierError, AtelierResult};
use super::export;
use super::model::Model;

/// One emitted artifact.
#[derive(Clone, Debug, Serialize)]
pub struct Artifact {
    /// Logical role, e.g. "stl", "render", "cutlist".
    pub kind: String,
    /// File name relative to the output directory.
    pub file: String,
    /// Which producer created it: "builtin" or "openscad".
    pub producer: String,
}

/// The machine-readable manifest written as `manifest.json`.
#[derive(Clone, Debug, Serialize)]
pub struct AtelierManifest {
    pub name: String,
    pub product_type: String,
    pub material: String,
    pub quantity: u32,
    pub bounds_min_mm: [f64; 3],
    pub bounds_max_mm: [f64; 3],
    pub part_count: usize,
    pub material_volume_mm3: f64,
    pub openscad_used: bool,
    pub artifacts: Vec<Artifact>,
}

/// The result of a pipeline run.
#[derive(Clone, Debug)]
pub struct PipelineOutcome {
    pub output_dir: PathBuf,
    pub manifest: AtelierManifest,
}

impl PipelineOutcome {
    /// A short human summary suitable for CLI output and evidence.
    pub fn summary(&self) -> String {
        format!(
            "Atelier built '{}' ({}): {} parts, {} artifacts, openscad={} -> {}",
            self.manifest.name,
            self.manifest.product_type,
            self.manifest.part_count,
            self.manifest.artifacts.len(),
            self.manifest.openscad_used,
            self.output_dir.display(),
        )
    }
}

/// Options controlling a pipeline run.
#[derive(Clone, Debug)]
pub struct PipelineOptions {
    /// If false, never shell out to `openscad` (fully hermetic run).
    pub use_openscad: bool,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self { use_openscad: true }
    }
}

/// Run the full pipeline: read a brief file, generate a model, and write every
/// fabrication artifact into `output_dir`.
pub fn run_pipeline_from_file(
    brief_path: &Path,
    output_dir: &Path,
    options: &PipelineOptions,
) -> AtelierResult<PipelineOutcome> {
    let bytes = fs::read(brief_path).map_err(|e| AtelierError::io(brief_path, &e))?;
    let brief = ProductBrief::from_json_slice(&bytes)?;
    run_pipeline(&brief, output_dir, options)
}

/// Run the full pipeline for an already-parsed brief.
pub fn run_pipeline(
    brief: &ProductBrief,
    output_dir: &Path,
    options: &PipelineOptions,
) -> AtelierResult<PipelineOutcome> {
    fs::create_dir_all(output_dir).map_err(|e| AtelierError::io(output_dir, &e))?;
    let model = Model::from_brief(brief);

    let mut artifacts = Vec::new();

    // --- Deterministic, dependency-free artifacts (always produced) ---
    let scad = model.to_openscad();
    write(output_dir, "model.scad", &scad)?;
    artifacts.push(builtin("openscad-source", "model.scad"));

    write(output_dir, "model.stl", &export::to_ascii_stl(&model))?;
    artifacts.push(builtin("stl", "model.stl"));

    write(output_dir, "render.svg", &export::to_svg_render(&model))?;
    artifacts.push(builtin("render", "render.svg"));

    write(output_dir, "cutlist.csv", &export::cut_list_csv(&model))?;
    artifacts.push(builtin("cutlist", "cutlist.csv"));

    write(output_dir, "bom.csv", &export::bom_csv(&model))?;
    artifacts.push(builtin("bom", "bom.csv"));

    // --- Optional high-fidelity CAD outputs (best-effort) ---
    let mut openscad_used = false;
    if options.use_openscad {
        let scad_path = output_dir.join("model.scad");
        if let Some(file) = try_openscad_export(&scad_path, output_dir, "model.cad.stl") {
            artifacts.push(openscad_artifact("stl", &file));
            openscad_used = true;
        }
        if let Some(file) = try_openscad_export(&scad_path, output_dir, "render.png") {
            artifacts.push(openscad_artifact("render", &file));
            openscad_used = true;
        }
    }

    let (bounds_min, bounds_max) = model.bounds();
    let manifest = AtelierManifest {
        name: brief.name.clone(),
        product_type: brief.product_type.label().to_string(),
        material: brief.material.clone(),
        quantity: brief.quantity,
        bounds_min_mm: bounds_min,
        bounds_max_mm: bounds_max,
        part_count: model.solids.len(),
        material_volume_mm3: model.material_volume_mm3(),
        openscad_used,
        artifacts: artifacts.clone(),
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|e| AtelierError::BriefParse {
            reason: e.to_string(),
        })?;
    write(output_dir, "manifest.json", &manifest_json)?;

    Ok(PipelineOutcome {
        output_dir: output_dir.to_path_buf(),
        manifest,
    })
}

fn builtin(kind: &str, file: &str) -> Artifact {
    Artifact {
        kind: kind.to_string(),
        file: file.to_string(),
        producer: "builtin".to_string(),
    }
}

fn openscad_artifact(kind: &str, file: &str) -> Artifact {
    Artifact {
        kind: kind.to_string(),
        file: file.to_string(),
        producer: "openscad".to_string(),
    }
}

fn write(dir: &Path, name: &str, contents: &str) -> AtelierResult<()> {
    let path = dir.join(name);
    fs::write(&path, contents).map_err(|e| AtelierError::io(&path, &e))
}

/// Attempt to run `openscad -o <out> <scad>`. Returns the output file name on
/// success, or `None` if openscad is missing or the render failed (best-effort).
/// Any empty/partial output left behind by a failed run is removed so the
/// output directory only ever contains complete artifacts.
fn try_openscad_export(scad_path: &Path, output_dir: &Path, out_name: &str) -> Option<String> {
    let out_path = output_dir.join(out_name);
    let status = Command::new("openscad")
        .arg("-o")
        .arg(&out_path)
        .arg(scad_path)
        .status();
    let ok = matches!(&status, Ok(s) if s.success())
        && fs::metadata(&out_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
    if ok {
        Some(out_name.to_string())
    } else {
        // Remove any zero-byte / partial file a failed headless render created.
        let _ = fs::remove_file(&out_path);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::brief::{Dimensions, ProductType};

    fn brief() -> ProductBrief {
        ProductBrief {
            name: "Pipeline Desk".into(),
            product_type: ProductType::Table,
            dimensions: Dimensions {
                length_mm: 1400.0,
                width_mm: 700.0,
                height_mm: 740.0,
                thickness_mm: 18.0,
            },
            material: "18mm oak plywood".into(),
            quantity: 1,
            shelf_count: 3,
            leg_section_mm: 60.0,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "atelier-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        d
    }

    #[test]
    fn hermetic_run_produces_all_builtin_artifacts() {
        let dir = temp_dir("hermetic");
        let out = run_pipeline(
            &brief(),
            &dir,
            &PipelineOptions {
                use_openscad: false,
            },
        )
        .unwrap();
        assert!(!out.manifest.openscad_used);
        for f in [
            "model.scad",
            "model.stl",
            "render.svg",
            "cutlist.csv",
            "bom.csv",
            "manifest.json",
        ] {
            assert!(dir.join(f).exists(), "missing artifact {f}");
        }
        // STL must look like a valid ASCII mesh.
        let stl = fs::read_to_string(dir.join("model.stl")).unwrap();
        assert!(stl.contains("facet normal"));
        // manifest round-trips and lists the builtin artifacts.
        let mj = fs::read_to_string(dir.join("manifest.json")).unwrap();
        assert!(mj.contains("\"product_type\": \"table\""));
        assert!(out.manifest.artifacts.iter().any(|a| a.kind == "render"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_from_file_parses_and_builds() {
        let dir = temp_dir("fromfile");
        fs::create_dir_all(&dir).unwrap();
        let brief_path = dir.join("brief.json");
        fs::write(&brief_path, serde_json::to_string(&brief()).unwrap()).unwrap();
        let out_dir = dir.join("out");
        let outcome = run_pipeline_from_file(
            &brief_path,
            &out_dir,
            &PipelineOptions {
                use_openscad: false,
            },
        )
        .unwrap();
        assert_eq!(outcome.manifest.part_count, 5);
        assert!(outcome.summary().contains("Pipeline Desk"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_brief_file_errors() {
        let dir = temp_dir("bad");
        fs::create_dir_all(&dir).unwrap();
        let brief_path = dir.join("brief.json");
        fs::write(&brief_path, b"{ nope").unwrap();
        let err = run_pipeline_from_file(
            &brief_path,
            &dir.join("out"),
            &PipelineOptions {
                use_openscad: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, AtelierError::BriefParse { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_brief_file_is_io_error() {
        let err = run_pipeline_from_file(
            Path::new("/nonexistent/atelier/brief.json"),
            Path::new("/tmp/atelier-none"),
            &PipelineOptions {
                use_openscad: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, AtelierError::Io { .. }));
    }
}
