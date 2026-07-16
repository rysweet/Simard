//! End-to-end fabrication pipeline: **product brief → exported model + render**.
//!
//! [`run_pipeline`] always writes the deterministic artifacts (parametric
//! `.scad`, `cut_list.csv`, `bom.json`, `brief.json`, a FreeCAD STEP macro, and
//! a `manifest.json`). It then *attempts* the tool-backed artifacts — an STL and
//! a PNG render via OpenSCAD, and a STEP export via FreeCAD — recording each as
//! produced, skipped (tool missing), or failed. Missing tools are a graceful
//! skip, never a hard error, so the pipeline is usable on any machine and fully
//! testable without the heavy CAD binaries installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use super::brief::ProductBrief;
use super::error::{AtelierError, AtelierResult};
use super::fabrication::{bill_of_materials, cut_list};
use super::model::{generate_openscad, geometry_summary};

/// Abstraction over the external CAD toolchain so the pipeline can be exercised
/// deterministically in tests without OpenSCAD/FreeCAD installed.
pub trait ToolRunner: Send + Sync {
    /// Whether `program` is invocable on this host.
    fn available(&self, program: &str) -> bool;
    /// Run `program args...`, returning an error if it ran and failed. The tool
    /// is responsible for writing any `-o` output file.
    fn run(&self, program: &str, args: &[&str]) -> AtelierResult<()>;
}

/// The real system toolchain, shelling out to `openscad` / `freecadcmd`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemTools;

impl ToolRunner for SystemTools {
    fn available(&self, program: &str) -> bool {
        // A tool is available if we can spawn it at all. `--version` is cheap and
        // supported by both openscad and freecadcmd; a NotFound spawn error means
        // the binary is absent.
        match Command::new(program).arg("--version").output() {
            Ok(_) => true,
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> AtelierResult<()> {
        let output = Command::new(program).args(args).output().map_err(|error| {
            AtelierError::ToolFailed {
                tool: program.to_string(),
                reason: error.to_string(),
            }
        })?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(AtelierError::ToolFailed {
                tool: program.to_string(),
                reason: format!(
                    "exit {}: {}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim()
                ),
            })
        }
    }
}

/// Disposition of a single output artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum ArtifactStatus {
    /// Written deterministically by the engine (no external tool).
    Written,
    /// Produced by an external tool.
    Produced { tool: String },
    /// Skipped because the required tool is not installed.
    SkippedToolMissing { tool: String },
    /// The tool ran but failed.
    Failed { tool: String, reason: String },
}

impl ArtifactStatus {
    /// Whether the artifact file actually exists on disk.
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Written | Self::Produced { .. })
    }
}

/// One output artifact and where it lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Artifact {
    pub name: String,
    pub path: PathBuf,
    #[serde(flatten)]
    pub status: ArtifactStatus,
}

/// The result of a fabrication run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FabricationOutput {
    pub product: String,
    pub kind: String,
    pub summary: String,
    pub output_dir: PathBuf,
    pub artifacts: Vec<Artifact>,
}

impl FabricationOutput {
    /// Whether the two headline deliverables — an exported model (STL or STEP)
    /// and a render (PNG) — were produced. This is the objective's definition of
    /// a complete end-to-end run.
    pub fn produced_model_and_render(&self) -> bool {
        let model = self.artifacts.iter().any(|a| {
            matches!(a.status, ArtifactStatus::Produced { .. })
                && (a.name.ends_with(".stl") || a.name.ends_with(".step"))
        });
        let render = self.artifacts.iter().any(|a| {
            matches!(a.status, ArtifactStatus::Produced { .. }) && a.name.ends_with(".png")
        });
        model && render
    }

    /// Look up an artifact by file name.
    pub fn artifact(&self, name: &str) -> Option<&Artifact> {
        self.artifacts.iter().find(|a| a.name == name)
    }
}

/// Convenience wrapper over [`run_pipeline`] using the real system toolchain.
pub fn fabricate(brief: &ProductBrief, output_dir: &Path) -> AtelierResult<FabricationOutput> {
    run_pipeline(brief, output_dir, &SystemTools)
}

/// Run the full pipeline with an injectable toolchain.
pub fn run_pipeline(
    brief: &ProductBrief,
    output_dir: &Path,
    tools: &dyn ToolRunner,
) -> AtelierResult<FabricationOutput> {
    brief.validate()?;
    create_dir(output_dir)?;

    let slug = brief.slug();
    let scad_name = format!("{slug}.scad");
    let stl_name = format!("{slug}.stl");
    let png_name = format!("{slug}.png");
    let step_name = format!("{slug}.step");
    let macro_name = "export_step.py".to_string();

    let scad_path = output_dir.join(&scad_name);
    let stl_path = output_dir.join(&stl_name);
    let png_path = output_dir.join(&png_name);
    let step_path = output_dir.join(&step_name);
    let macro_path = output_dir.join(&macro_name);

    let mut artifacts = Vec::new();

    // --- deterministic artifacts -------------------------------------------
    write_file(&scad_path, generate_openscad(brief))?;
    artifacts.push(written(&scad_name, &scad_path));

    write_file(output_dir.join("brief.json"), pretty_json(brief)?)?;
    artifacts.push(written("brief.json", &output_dir.join("brief.json")));

    let list = cut_list(brief);
    write_file(output_dir.join("cut_list.csv"), list.to_csv())?;
    artifacts.push(written("cut_list.csv", &output_dir.join("cut_list.csv")));

    let bom = bill_of_materials(brief);
    write_file(output_dir.join("bom.json"), pretty_json(&bom)?)?;
    artifacts.push(written("bom.json", &output_dir.join("bom.json")));

    write_file(&macro_path, freecad_step_macro(&stl_path, &step_path))?;
    artifacts.push(written(&macro_name, &macro_path));

    // --- tool-backed artifacts ---------------------------------------------
    let scad_str = scad_path.to_string_lossy().to_string();
    let stl_str = stl_path.to_string_lossy().to_string();
    let png_str = png_path.to_string_lossy().to_string();

    // STL export via OpenSCAD.
    let stl_status = try_tool(tools, "openscad", &["-o", &stl_str, &scad_str]);
    artifacts.push(Artifact {
        name: stl_name.clone(),
        path: stl_path.clone(),
        status: stl_status.clone(),
    });

    // PNG render via OpenSCAD. OpenSCAD needs an OpenGL context; on a headless
    // host (no DISPLAY) we transparently wrap it in `xvfb-run` when available so
    // the render still succeeds.
    let png_status = render_png(tools, &scad_str, &png_str);
    artifacts.push(Artifact {
        name: png_name.clone(),
        path: png_path.clone(),
        status: png_status,
    });

    // STEP export via FreeCAD, converting the OpenSCAD-exported STL. Only viable
    // once the STL exists.
    let macro_str = macro_path.to_string_lossy().to_string();
    let step_status = if matches!(stl_status, ArtifactStatus::Produced { .. }) {
        try_tool(tools, "freecadcmd", &[&macro_str])
    } else {
        ArtifactStatus::SkippedToolMissing {
            tool: "freecadcmd".into(),
        }
    };
    artifacts.push(Artifact {
        name: step_name,
        path: step_path,
        status: step_status,
    });

    let output = FabricationOutput {
        product: brief.name.clone(),
        kind: brief.kind.label().to_string(),
        summary: geometry_summary(brief),
        output_dir: output_dir.to_path_buf(),
        artifacts,
    };

    // Manifest is written last so it can describe every other artifact.
    write_file(output_dir.join("manifest.json"), pretty_json(&output)?)?;

    Ok(output)
}

/// Run a tool if available, mapping the outcome to an [`ArtifactStatus`].
fn try_tool(tools: &dyn ToolRunner, program: &str, args: &[&str]) -> ArtifactStatus {
    if !tools.available(program) {
        return ArtifactStatus::SkippedToolMissing {
            tool: program.to_string(),
        };
    }
    match tools.run(program, args) {
        Ok(()) => ArtifactStatus::Produced {
            tool: program.to_string(),
        },
        Err(AtelierError::ToolFailed { tool, reason }) => ArtifactStatus::Failed { tool, reason },
        Err(other) => ArtifactStatus::Failed {
            tool: program.to_string(),
            reason: other.to_string(),
        },
    }
}

/// Render a PNG preview of the model with OpenSCAD, transparently using
/// `xvfb-run` on a headless host so the required OpenGL context can be created.
/// The artifact tool is always reported as `openscad` (the logical renderer).
fn render_png(tools: &dyn ToolRunner, scad: &str, out: &str) -> ArtifactStatus {
    render_png_inner(tools, scad, out, is_headless())
}

fn render_png_inner(
    tools: &dyn ToolRunner,
    scad: &str,
    out: &str,
    headless: bool,
) -> ArtifactStatus {
    if !tools.available("openscad") {
        return ArtifactStatus::SkippedToolMissing {
            tool: "openscad".to_string(),
        };
    }
    let openscad_args = [
        "-o",
        out,
        "--autocenter",
        "--viewall",
        "--imgsize=1024,768",
        "--colorscheme=Tomorrow",
        scad,
    ];

    let result = if headless && tools.available("xvfb-run") {
        let mut wrapped = vec!["-a", "openscad"];
        wrapped.extend_from_slice(&openscad_args);
        tools.run("xvfb-run", &wrapped)
    } else {
        tools.run("openscad", &openscad_args)
    };

    match result {
        Ok(()) => ArtifactStatus::Produced {
            tool: "openscad".to_string(),
        },
        Err(AtelierError::ToolFailed { reason, .. }) => ArtifactStatus::Failed {
            tool: "openscad".to_string(),
            reason,
        },
        Err(other) => ArtifactStatus::Failed {
            tool: "openscad".to_string(),
            reason: other.to_string(),
        },
    }
}

/// Whether we appear to be running without a usable X display.
fn is_headless() -> bool {
    std::env::var("DISPLAY")
        .map(|d| d.trim().is_empty())
        .unwrap_or(true)
}

fn written(name: &str, path: &Path) -> Artifact {
    Artifact {
        name: name.to_string(),
        path: path.to_path_buf(),
        status: ArtifactStatus::Written,
    }
}

/// A FreeCAD macro that converts the OpenSCAD-exported STL mesh into a solid and
/// exports it as a STEP (BREP) file — the fabrication-shop interchange format.
fn freecad_step_macro(stl_path: &Path, step_path: &Path) -> String {
    format!(
        "# Simard Atelier — FreeCAD STL->STEP conversion macro\n\
         # Run with: freecadcmd export_step.py\n\
         import Mesh, Part\n\
         stl_path = {stl:?}\n\
         step_path = {step:?}\n\
         mesh = Mesh.Mesh(stl_path)\n\
         shape = Part.Shape()\n\
         shape.makeShapeFromMesh(mesh.Topology, 0.100)\n\
         solid = Part.makeSolid(shape)\n\
         Part.export([solid], step_path)\n\
         print('wrote', step_path)\n",
        stl = stl_path.to_string_lossy(),
        step = step_path.to_string_lossy(),
    )
}

fn create_dir(dir: &Path) -> AtelierResult<()> {
    std::fs::create_dir_all(dir).map_err(|error| AtelierError::Io {
        path: dir.to_string_lossy().to_string(),
        reason: error.to_string(),
    })
}

fn write_file(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> AtelierResult<()> {
    let path = path.as_ref();
    std::fs::write(path, contents).map_err(|error| AtelierError::Io {
        path: path.to_string_lossy().to_string(),
        reason: error.to_string(),
    })
}

fn pretty_json<T: Serialize>(value: &T) -> AtelierResult<String> {
    serde_json::to_string_pretty(value).map_err(|error| AtelierError::Io {
        path: "<json>".to_string(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::brief::ProductKind;
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn brief() -> ProductBrief {
        ProductBrief {
            name: "Test Desk".into(),
            kind: ProductKind::Table,
            width_mm: 1200.0,
            depth_mm: 600.0,
            height_mm: 740.0,
            panel_thickness_mm: 18.0,
            material: "oak".into(),
            shelves: 0,
            quantity: 1,
            finish: "oil".into(),
        }
    }

    /// A fake toolchain whose availability is configurable and whose `run`
    /// writes the `-o` target (or the macro's step output) so downstream checks
    /// see real files.
    struct FakeTools {
        available: HashSet<String>,
        fail: HashSet<String>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeTools {
        fn new(available: &[&str]) -> Self {
            Self {
                available: available.iter().map(|s| s.to_string()).collect(),
                fail: HashSet::new(),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn failing(mut self, program: &str) -> Self {
            self.fail.insert(program.to_string());
            self
        }
    }

    impl ToolRunner for FakeTools {
        fn available(&self, program: &str) -> bool {
            self.available.contains(program)
        }
        fn run(&self, program: &str, args: &[&str]) -> AtelierResult<()> {
            self.calls.lock().unwrap().push(program.to_string());
            if self.fail.contains(program) {
                return Err(AtelierError::ToolFailed {
                    tool: program.to_string(),
                    reason: "forced".into(),
                });
            }
            // Emulate `-o <path>` writers.
            if let Some(idx) = args.iter().position(|a| *a == "-o")
                && let Some(out) = args.get(idx + 1)
            {
                let _ = std::fs::write(out, b"fake");
            }
            Ok(())
        }
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "atelier-test-{tag}-{}-{}",
            std::process::id(),
            fastcounter()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn fastcounter() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        C.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn writes_all_deterministic_artifacts_without_tools() {
        let dir = tmpdir("deterministic");
        let out = run_pipeline(&brief(), &dir, &FakeTools::new(&[])).unwrap();
        for name in [
            "test-desk.scad",
            "brief.json",
            "cut_list.csv",
            "bom.json",
            "export_step.py",
        ] {
            let a = out
                .artifact(name)
                .unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(a.status, ArtifactStatus::Written, "for {name}");
            assert!(a.path.exists(), "file {name} should exist");
        }
        assert!(dir.join("manifest.json").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skips_tool_artifacts_when_tools_missing() {
        let dir = tmpdir("skip");
        let out = run_pipeline(&brief(), &dir, &FakeTools::new(&[])).unwrap();
        let stl = out.artifact("test-desk.stl").unwrap();
        assert!(matches!(
            stl.status,
            ArtifactStatus::SkippedToolMissing { .. }
        ));
        assert!(!out.produced_model_and_render());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn produces_model_and_render_with_full_toolchain() {
        let dir = tmpdir("full");
        let tools = FakeTools::new(&["openscad", "freecadcmd"]);
        let out = run_pipeline(&brief(), &dir, &tools).unwrap();
        let stl = out.artifact("test-desk.stl").unwrap();
        let png = out.artifact("test-desk.png").unwrap();
        let step = out.artifact("test-desk.step").unwrap();
        assert!(matches!(stl.status, ArtifactStatus::Produced { .. }));
        assert!(matches!(png.status, ArtifactStatus::Produced { .. }));
        assert!(matches!(step.status, ArtifactStatus::Produced { .. }));
        assert!(out.produced_model_and_render());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_uses_xvfb_wrapper_on_headless_host() {
        // On a headless host (no display) with xvfb-run available, the render
        // must be routed through `xvfb-run` yet still report the logical
        // `openscad` tool.
        let dir = tmpdir("xvfb");
        std::fs::create_dir_all(&dir).unwrap();
        let scad = dir.join("m.scad");
        std::fs::write(&scad, "cube(1);").unwrap();
        let out = dir.join("m.png");
        let tools = FakeTools::new(&["openscad", "xvfb-run"]);
        let status = render_png_inner(&tools, scad.to_str().unwrap(), out.to_str().unwrap(), true);
        assert!(matches!(
            status,
            ArtifactStatus::Produced { ref tool } if tool == "openscad"
        ));
        assert!(
            tools.calls.lock().unwrap().iter().any(|c| c == "xvfb-run"),
            "expected xvfb-run to be invoked for the render on a headless host"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_calls_openscad_directly_when_display_present() {
        let dir = tmpdir("nodisplay");
        std::fs::create_dir_all(&dir).unwrap();
        let scad = dir.join("m.scad");
        std::fs::write(&scad, "cube(1);").unwrap();
        let out = dir.join("m.png");
        let tools = FakeTools::new(&["openscad", "xvfb-run"]);
        let status = render_png_inner(&tools, scad.to_str().unwrap(), out.to_str().unwrap(), false);
        assert!(matches!(status, ArtifactStatus::Produced { .. }));
        let calls = tools.calls.lock().unwrap();
        assert!(calls.iter().any(|c| c == "openscad"));
        assert!(!calls.iter().any(|c| c == "xvfb-run"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn step_skipped_when_stl_not_produced() {
        // freecad present but openscad missing => no STL => STEP skipped.
        let dir = tmpdir("nostep");
        let out = run_pipeline(&brief(), &dir, &FakeTools::new(&["freecadcmd"])).unwrap();
        let step = out.artifact("test-desk.step").unwrap();
        assert!(matches!(
            step.status,
            ArtifactStatus::SkippedToolMissing { .. }
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn records_tool_failure() {
        let dir = tmpdir("fail");
        let tools = FakeTools::new(&["openscad"]).failing("openscad");
        let out = run_pipeline(&brief(), &dir, &tools).unwrap();
        let stl = out.artifact("test-desk.stl").unwrap();
        assert!(matches!(stl.status, ArtifactStatus::Failed { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_invalid_brief_before_writing() {
        let dir = tmpdir("invalid");
        let mut b = brief();
        b.width_mm = -1.0;
        let err = run_pipeline(&b, &dir, &FakeTools::new(&[])).unwrap_err();
        assert!(matches!(err, AtelierError::InvalidBrief { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn manifest_json_is_valid_and_describes_artifacts() {
        let dir = tmpdir("manifest");
        run_pipeline(&brief(), &dir, &FakeTools::new(&["openscad", "freecadcmd"])).unwrap();
        let manifest = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(value["kind"], "table");
        assert!(value["artifacts"].as_array().unwrap().len() >= 6);
        std::fs::remove_dir_all(&dir).ok();
    }
}
