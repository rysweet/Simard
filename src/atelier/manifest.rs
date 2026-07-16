//! Package orchestration, manifest, and verification.
//!
//! [`build_package`] is the end-to-end entry point: brief → parametric model →
//! STL + render → cut list + BOM → optional STEP → verified `manifest.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::brief::ProductBrief;
use super::drivers;
use super::error::{AtelierError, AtelierResult};
use super::fabrication::{self, Bom, CutList};
use super::geometry::{self, Assembly};
use super::scad;

/// Options controlling which exports are produced.
#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    /// Also produce the fabrication solid (STEP) when a solid kernel exists.
    pub fabrication: bool,
    pub render_width: u32,
    pub render_height: u32,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            fabrication: false,
            render_width: 1200,
            render_height: 900,
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
    pub render_ok: bool,
    pub checks: Vec<Check>,
}

/// The package manifest, written as `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub product_name: String,
    pub kind: String,
    pub dimensions_mm: [f64; 3],
    pub part_count: u32,
    pub instance_count: u32,
    pub sheets_required: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_material_cost: Option<f64>,
    pub over_budget: bool,
    pub tools: Vec<drivers::ToolReport>,
    pub artifacts: Vec<Artifact>,
    pub verification: Verification,
}

impl Manifest {
    /// Consume the manifest, returning an error if verification did not pass.
    /// Advisory checks (render, budget) never fail this; only the required
    /// minimum (geometry, STL, cut list, BOM, stock fit) does.
    pub fn verified(self) -> AtelierResult<Self> {
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
            Err(AtelierError::verification(failed.join("; ")))
        }
    }
}

const MODEL_SCAD: &str = "model.scad";
const MODEL_STL: &str = "model.stl";
const RENDER_PNG: &str = "render.png";
const RENDER_BLENDER_PNG: &str = "render_blender.png";
const MODEL_STEP: &str = "model.step";
const CUTLIST_CSV: &str = "cutlist.csv";
const BOM_CSV: &str = "bom.csv";
const MANIFEST_JSON: &str = "manifest.json";

fn write_file(path: &Path, contents: &str) -> AtelierResult<()> {
    std::fs::write(path, contents)
        .map_err(|e| AtelierError::io(format!("writing {}", path.display()), e))
}

fn bytes_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Build the full fabrication package for `brief_path` into `out_dir`.
pub fn build_package(
    brief_path: &Path,
    out_dir: &Path,
    options: BuildOptions,
) -> AtelierResult<Manifest> {
    let brief = ProductBrief::from_path(brief_path)?;
    build_package_from_brief(&brief, out_dir, options)
}

/// Build a package from an already-parsed brief.
pub fn build_package_from_brief(
    brief: &ProductBrief,
    out_dir: &Path,
    options: BuildOptions,
) -> AtelierResult<Manifest> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| AtelierError::io(format!("creating {}", out_dir.display()), e))?;

    let assembly = geometry::generate(brief);
    let cut_list = fabrication::build_cut_list(brief, &assembly);
    let bom = fabrication::build_bom(brief, &assembly, &cut_list);

    // 1. Parametric OpenSCAD source.
    let scad_path = out_dir.join(MODEL_SCAD);
    write_file(&scad_path, &scad::generate_scad(brief, &assembly))?;

    // 2. STL mesh (required).
    let stl_path = out_dir.join(MODEL_STL);
    drivers::openscad_stl(&scad_path, &stl_path)?;

    // 3. PNG render (best-effort).
    let png_path = out_dir.join(RENDER_PNG);
    let render = drivers::openscad_png(
        &scad_path,
        &png_path,
        options.render_width,
        options.render_height,
    );

    // 4. Cut list + BOM (always).
    let cutlist_path = out_dir.join(CUTLIST_CSV);
    write_file(&cutlist_path, &cut_list.to_csv())?;
    let bom_path = out_dir.join(BOM_CSV);
    write_file(&bom_path, &bom.to_csv())?;

    let mut artifacts = vec![
        artifact(&scad_path, MODEL_SCAD, "openscad-source", None),
        artifact(&stl_path, MODEL_STL, "mesh", None),
        artifact_maybe(
            &png_path,
            RENDER_PNG,
            "render",
            &render.detail,
            render.produced,
        ),
        artifact(&cutlist_path, CUTLIST_CSV, "cutlist", None),
        artifact(&bom_path, BOM_CSV, "bom", None),
    ];

    // 5. Optional Blender photoreal render.
    let blender_png = out_dir.join(RENDER_BLENDER_PNG);
    let blender = drivers::blender_render(&stl_path, &blender_png, out_dir);
    if blender.produced {
        artifacts.push(artifact_maybe(
            &blender_png,
            RENDER_BLENDER_PNG,
            "render",
            &blender.detail,
            true,
        ));
    }

    // 6. Optional STEP solid (fabrication mode).
    if options.fabrication {
        let step_path = out_dir.join(MODEL_STEP);
        let step = drivers::freecad_step(&assembly, &step_path, out_dir);
        artifacts.push(artifact_maybe(
            &step_path,
            MODEL_STEP,
            "solid",
            &step.detail,
            step.produced,
        ));
    }

    let tools = vec![
        drivers::probe("openscad"),
        drivers::probe("xvfb-run"),
        drivers::probe("freecadcmd"),
        drivers::probe("blender"),
    ];

    let verification = verify(
        brief,
        &assembly,
        &cut_list,
        &bom,
        &stl_path,
        render.produced,
    );

    let manifest = Manifest {
        product_name: brief.name.clone(),
        kind: assembly.kind.label().to_string(),
        dimensions_mm: [
            brief.dimensions_mm.width,
            brief.dimensions_mm.depth,
            brief.dimensions_mm.height,
        ],
        part_count: assembly.panels.len() as u32,
        instance_count: assembly.instance_count(),
        sheets_required: cut_list.sheets_required,
        estimated_material_cost: bom.total_cost,
        over_budget: bom.over_budget,
        tools,
        artifacts,
        verification,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AtelierError::parse("manifest json", e.to_string()))?;
    write_file(&out_dir.join(MANIFEST_JSON), &manifest_json)?;

    Ok(manifest)
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

fn artifact_maybe(path: &Path, file: &str, kind: &str, detail: &str, produced: bool) -> Artifact {
    let bytes = bytes_of(path);
    Artifact {
        file: file.to_string(),
        kind: kind.to_string(),
        present: produced && bytes > 0,
        bytes,
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

/// Verify the produced package against the geometry and stock constraints.
fn verify(
    brief: &ProductBrief,
    assembly: &Assembly,
    cut_list: &CutList,
    bom: &Bom,
    stl_path: &Path,
    render_ok: bool,
) -> Verification {
    let mut checks = Vec::new();

    // Geometry sanity: every instance has positive size.
    let geometry_ok = assembly
        .panels
        .iter()
        .flat_map(|p| p.instances.iter())
        .all(|i| i.size.iter().all(|&s| s > 0.0));
    checks.push(check(
        "geometry-valid",
        geometry_ok,
        if geometry_ok {
            "all panel instances have positive size".into()
        } else {
            "one or more panel instances have a non-positive dimension".to_string()
        },
    ));

    // STL present + non-empty.
    let stl_ok = std::fs::metadata(stl_path)
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    checks.push(check(
        "stl-present",
        stl_ok,
        if stl_ok {
            "model.stl exported".into()
        } else {
            "model.stl missing or empty".to_string()
        },
    ));

    // Cut list + BOM non-empty.
    let cutlist_ok = !cut_list.rows.is_empty();
    checks.push(check(
        "cutlist-present",
        cutlist_ok,
        format!("{} cut-list rows", cut_list.rows.len()),
    ));
    let bom_ok = !bom.rows.is_empty();
    checks.push(check(
        "bom-present",
        bom_ok,
        format!("{} BOM rows", bom.rows.len()),
    ));

    // Stock fit: every part must fit the sheet in some orientation.
    let sheet = brief.material.sheet();
    let sheet_long = sheet.length.max(sheet.width);
    let sheet_short = sheet.length.min(sheet.width);
    let mut offending = Vec::new();
    for panel in &assembly.panels {
        let long = panel.length_mm.max(panel.width_mm);
        let short = panel.length_mm.min(panel.width_mm);
        if long > sheet_long + 1e-6 || short > sheet_short + 1e-6 {
            offending.push(panel.label.clone());
        }
    }
    let stock_ok = offending.is_empty();
    checks.push(check(
        "stock-fit",
        stock_ok,
        if stock_ok {
            format!(
                "all parts fit {}×{}mm stock",
                trim(sheet.length),
                trim(sheet.width)
            )
        } else {
            format!("parts exceed stock: {}", offending.join(", "))
        },
    ));

    // Budget (advisory only — does not fail the package).
    checks.push(check(
        "within-budget",
        !bom.over_budget,
        if bom.over_budget {
            "estimated material cost exceeds the brief budget".into()
        } else {
            "within budget (or no budget set)".to_string()
        },
    ));

    // Render (recommended — advisory, does not fail the package).
    checks.push(check(
        "render-present",
        render_ok,
        if render_ok {
            "render.png produced".into()
        } else {
            "render.png not produced (no display/GL context)".to_string()
        },
    ));

    // Required minimum for `ok`. Render + budget are advisory.
    let ok = geometry_ok && stl_ok && cutlist_ok && bom_ok && stock_ok;

    Verification {
        ok,
        render_ok,
        checks,
    }
}

/// Read and re-check an existing package manifest in `out_dir`.
///
/// `inspect` re-verifies the package against what is actually on disk: it
/// re-scans every artifact and, when a **required** artifact (STL, cut list,
/// BOM) has gone missing or empty since build time, it flips the corresponding
/// verification check and the aggregate `verification.ok` to `false`. The
/// brief/assembly are not available at inspect time, so the geometry-valid and
/// stock-fit checks are trusted from the persisted manifest (they cannot change
/// on disk); only the artifact-presence checks are recomputed.
pub fn inspect(out_dir: &Path) -> AtelierResult<Manifest> {
    let manifest_path = out_dir.join(MANIFEST_JSON);
    let bytes = std::fs::read(&manifest_path)
        .map_err(|e| AtelierError::io(format!("reading {}", manifest_path.display()), e))?;
    let mut manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| AtelierError::parse("manifest json", e.to_string()))?;

    // Re-confirm artifacts still exist on disk.
    for artifact in &mut manifest.artifacts {
        let path: PathBuf = out_dir.join(&artifact.file);
        let bytes = bytes_of(&path);
        if artifact.present && bytes == 0 {
            artifact.present = false;
            artifact.detail = Some("artifact missing on disk at inspect time".into());
        }
    }

    // Re-derive the artifact-presence checks from current on-disk state so a
    // corrupted package cannot report a stale PASS.
    let present = |file: &str| -> bool {
        manifest
            .artifacts
            .iter()
            .any(|a| a.file == file && a.present)
    };
    let stl_ok = present(MODEL_STL);
    let cutlist_ok = present(CUTLIST_CSV);
    let bom_ok = present(BOM_CSV);
    let render_ok = present(RENDER_PNG) || present(RENDER_BLENDER_PNG);

    for check in &mut manifest.verification.checks {
        let (ok, detail): (bool, Option<String>) = match check.name.as_str() {
            "stl-present" => (
                stl_ok,
                Some(if stl_ok {
                    "model.stl present".into()
                } else {
                    "model.stl missing or empty at inspect time".into()
                }),
            ),
            "cutlist-present" if !cutlist_ok => (
                false,
                Some("cutlist.csv missing or empty at inspect time".into()),
            ),
            "bom-present" if !bom_ok => (
                false,
                Some("bom.csv missing or empty at inspect time".into()),
            ),
            "render-present" => (
                render_ok,
                Some(if render_ok {
                    "render present".into()
                } else {
                    "render missing at inspect time".into()
                }),
            ),
            _ => (check.ok, None),
        };
        check.ok = ok;
        if let Some(detail) = detail {
            check.detail = detail;
        }
    }

    // Aggregate `ok` = every required check currently true. Geometry-valid and
    // stock-fit are trusted from the persisted checks.
    let required_ok = manifest
        .verification
        .checks
        .iter()
        .filter(|c| {
            matches!(
                c.name.as_str(),
                "geometry-valid" | "stl-present" | "cutlist-present" | "bom-present" | "stock-fit"
            )
        })
        .all(|c| c.ok);
    manifest.verification.ok = required_ok;
    manifest.verification.render_ok = render_ok;

    Ok(manifest)
}

fn trim(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::brief::ProductBrief;

    fn brief(json: &str) -> ProductBrief {
        ProductBrief::from_json_bytes(json.as_bytes()).unwrap()
    }

    const BOOKCASE: &str = r#"{"name":"Bookcase","kind":"bookcase",
        "dimensions_mm":{"width":800,"depth":300,"height":1000},
        "material":{"name":"Birch ply","thickness_mm":18,"grain":true,"cost_per_sheet":55},
        "parameters":{"shelves":2,"back_panel":true}}"#;

    fn openscad_available() -> bool {
        drivers::binary_available("openscad")
    }

    /// Build a valid package directory using the pure-Rust builders (no
    /// OpenSCAD), writing real artifact files + `manifest.json`. Lets us test
    /// verification/inspect logic on hosts without OpenSCAD (e.g. CI).
    fn stage_package(dir: &Path, b: &ProductBrief) -> Manifest {
        let assembly = geometry::generate(b);
        let cut_list = fabrication::build_cut_list(b, &assembly);
        let bom = fabrication::build_bom(b, &assembly, &cut_list);
        write_file(&dir.join(MODEL_SCAD), &scad::generate_scad(b, &assembly)).unwrap();
        write_file(&dir.join(MODEL_STL), "solid x\nendsolid x\n").unwrap();
        write_file(&dir.join(CUTLIST_CSV), &cut_list.to_csv()).unwrap();
        write_file(&dir.join(BOM_CSV), &bom.to_csv()).unwrap();
        let artifacts = vec![
            artifact(&dir.join(MODEL_SCAD), MODEL_SCAD, "openscad-source", None),
            artifact(&dir.join(MODEL_STL), MODEL_STL, "mesh", None),
            artifact(&dir.join(CUTLIST_CSV), CUTLIST_CSV, "cutlist", None),
            artifact(&dir.join(BOM_CSV), BOM_CSV, "bom", None),
        ];
        let verification = verify(b, &assembly, &cut_list, &bom, &dir.join(MODEL_STL), false);
        let manifest = Manifest {
            product_name: b.name.clone(),
            kind: assembly.kind.label().to_string(),
            dimensions_mm: [
                b.dimensions_mm.width,
                b.dimensions_mm.depth,
                b.dimensions_mm.height,
            ],
            part_count: assembly.panels.len() as u32,
            instance_count: assembly.instance_count(),
            sheets_required: cut_list.sheets_required,
            estimated_material_cost: bom.total_cost,
            over_budget: bom.over_budget,
            tools: Vec::new(),
            artifacts,
            verification,
        };
        write_file(
            &dir.join(MANIFEST_JSON),
            &serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        manifest
    }

    const GIANT: &str = r#"{"name":"Giant","kind":"table",
        "dimensions_mm":{"width":3000,"depth":1000,"height":740},
        "material":{"name":"ply","thickness_mm":18}}"#;

    #[test]
    fn build_package_writes_core_artifacts() {
        if !openscad_available() {
            eprintln!("skipping: openscad not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let manifest =
            build_package_from_brief(&brief(BOOKCASE), dir.path(), BuildOptions::default())
                .unwrap();
        // Core artifacts always exist.
        for f in [MODEL_SCAD, MODEL_STL, CUTLIST_CSV, BOM_CSV, MANIFEST_JSON] {
            assert!(dir.path().join(f).exists(), "missing {f}");
        }
        assert!(bytes_of(&dir.path().join(MODEL_STL)) > 0);
        assert!(
            manifest.verification.ok,
            "verification should pass: {:?}",
            manifest.verification.checks
        );
        assert_eq!(manifest.kind, "bookcase");
        assert!(manifest.instance_count >= 4);
    }

    #[test]
    fn fabrication_mode_records_step_artifact() {
        if !openscad_available() {
            eprintln!("skipping: openscad not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let opts = BuildOptions {
            fabrication: true,
            ..BuildOptions::default()
        };
        let manifest = build_package_from_brief(&brief(BOOKCASE), dir.path(), opts).unwrap();
        // STEP artifact is recorded even when freecad is absent (present=false).
        assert!(manifest.artifacts.iter().any(|a| a.file == MODEL_STEP));
    }

    #[test]
    fn tools_are_probed() {
        if !openscad_available() {
            eprintln!("skipping: openscad not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let manifest =
            build_package_from_brief(&brief(BOOKCASE), dir.path(), BuildOptions::default())
                .unwrap();
        assert!(manifest.tools.iter().any(|t| t.name == "openscad"));
    }

    // ── Verification + inspect logic (pure, no OpenSCAD required) ────────────

    #[test]
    fn verify_passes_for_valid_package_even_without_render() {
        let dir = tempfile::tempdir().unwrap();
        let b = brief(BOOKCASE);
        let assembly = geometry::generate(&b);
        let cut_list = fabrication::build_cut_list(&b, &assembly);
        let bom = fabrication::build_bom(&b, &assembly, &cut_list);
        let stl = dir.path().join(MODEL_STL);
        write_file(&stl, "solid x\nendsolid x\n").unwrap();
        // render_ok = false: the render is advisory and must not fail `ok`.
        let v = verify(&b, &assembly, &cut_list, &bom, &stl, false);
        assert!(v.ok, "required checks should pass: {:?}", v.checks);
        assert!(!v.render_ok);
    }

    #[test]
    fn oversized_part_fails_stock_fit() {
        let dir = tempfile::tempdir().unwrap();
        let b = brief(GIANT);
        let assembly = geometry::generate(&b);
        let cut_list = fabrication::build_cut_list(&b, &assembly);
        let bom = fabrication::build_bom(&b, &assembly, &cut_list);
        let stl = dir.path().join(MODEL_STL);
        write_file(&stl, "solid x\nendsolid x\n").unwrap();
        let v = verify(&b, &assembly, &cut_list, &bom, &stl, true);
        assert!(!v.ok);
        let stock = v.checks.iter().find(|c| c.name == "stock-fit").unwrap();
        assert!(!stock.ok);
    }

    #[test]
    fn verified_ok_returns_manifest_else_error() {
        let dir = tempfile::tempdir().unwrap();
        let ok = stage_package(dir.path(), &brief(BOOKCASE));
        assert!(ok.verified().is_ok());

        let dir2 = tempfile::tempdir().unwrap();
        let bad = stage_package(dir2.path(), &brief(GIANT));
        assert!(bad.verified().is_err());
    }

    #[test]
    fn manifest_roundtrips_via_inspect() {
        let dir = tempfile::tempdir().unwrap();
        stage_package(dir.path(), &brief(BOOKCASE));
        let inspected = inspect(dir.path()).unwrap();
        assert_eq!(inspected.product_name, "Bookcase");
        assert!(inspected.verification.ok);
    }

    #[test]
    fn inspect_flips_ok_when_required_artifact_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let staged = stage_package(dir.path(), &brief(BOOKCASE));
        assert!(staged.verification.ok);

        // Delete the required STL after "build": inspect must NOT report a
        // stale PASS.
        std::fs::remove_file(dir.path().join(MODEL_STL)).unwrap();
        let inspected = inspect(dir.path()).unwrap();
        assert!(
            !inspected.verification.ok,
            "inspect must fail when a required artifact is gone"
        );
        let stl = inspected
            .verification
            .checks
            .iter()
            .find(|c| c.name == "stl-present")
            .unwrap();
        assert!(!stl.ok);
        assert!(inspected.verified().is_err());
    }

    #[test]
    fn inspect_missing_manifest_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(inspect(dir.path()).is_err());
    }
}
