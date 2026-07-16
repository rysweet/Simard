//! Drivers for external CAD/render tools.
//!
//! OpenSCAD is the primary engine (STL mesh + PNG render). FreeCAD (STEP solid)
//! and Blender (photoreal render) are optional enhancements. All tool scripts
//! are embedded as Rust strings and written to a temp dir at runtime — no
//! interpreter source is committed to the repo (the project is a pure-Rust
//! daemon, so no `.py`/`.js` may live in the tree).

use std::path::Path;
use std::process::Command;

use super::error::{AtelierError, AtelierResult};
use super::geometry::Assembly;

/// Availability + version of an external tool.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolReport {
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
}

/// Return true if `bin` can be launched (resolves on PATH).
pub fn binary_available(bin: &str) -> bool {
    // `command -v` is a POSIX builtin and does not execute the target.
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort first line of `bin --version`.
fn tool_version(bin: &str) -> Option<String> {
    let output = Command::new(bin).arg("--version").output().ok()?;
    let text: String = String::from_utf8_lossy(&output.stdout).into_owned();
    let text = if text.trim().is_empty() {
        String::from_utf8_lossy(&output.stderr).into_owned()
    } else {
        text
    };
    text.lines().next().map(|l| l.trim().to_string())
}

/// Probe a tool's availability and version.
pub fn probe(bin: &str) -> ToolReport {
    let available = binary_available(bin);
    ToolReport {
        name: bin.to_string(),
        available,
        version: if available { tool_version(bin) } else { None },
    }
}

fn file_non_empty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// Export an STL mesh from an OpenSCAD source file. Required export — errors if
/// OpenSCAD is missing, fails, or produces an empty file.
pub fn openscad_stl(scad: &Path, stl: &Path) -> AtelierResult<()> {
    if !binary_available("openscad") {
        return Err(AtelierError::tool(
            "openscad",
            "openscad is not installed; it is required to export the STL mesh",
        ));
    }
    let output = Command::new("openscad")
        .arg("-o")
        .arg(stl)
        .arg(scad)
        .output()
        .map_err(|e| AtelierError::tool("openscad", format!("failed to launch: {e}")))?;
    if !output.status.success() {
        return Err(AtelierError::tool(
            "openscad",
            format!(
                "STL export failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    if !file_non_empty(stl) {
        return Err(AtelierError::tool(
            "openscad",
            "STL export produced an empty file",
        ));
    }
    Ok(())
}

/// Outcome of a best-effort render attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOutcome {
    pub produced: bool,
    /// How the render was produced or why it was skipped.
    pub detail: String,
}

/// Render a PNG preview from an OpenSCAD source file. Best-effort: OpenSCAD's
/// PNG export needs an OpenGL context, so we prefer `xvfb-run` when present and
/// degrade gracefully (returning `produced = false`) rather than failing the
/// whole package when no display is available.
pub fn openscad_png(scad: &Path, png: &Path, width: u32, height: u32) -> RenderOutcome {
    if !binary_available("openscad") {
        return RenderOutcome {
            produced: false,
            detail: "openscad not installed".into(),
        };
    }
    let imgsize = format!("--imgsize={width},{height}");
    let use_xvfb = binary_available("xvfb-run");
    let mut command = if use_xvfb {
        let mut c = Command::new("xvfb-run");
        c.arg("-a").arg("openscad");
        c
    } else {
        Command::new("openscad")
    };
    command
        .arg("-o")
        .arg(png)
        .arg(&imgsize)
        .arg("--autocenter")
        .arg("--viewall")
        .arg("--colorscheme=Tomorrow")
        .arg(scad);

    match command.output() {
        Ok(output) if output.status.success() && file_non_empty(png) => RenderOutcome {
            produced: true,
            detail: if use_xvfb {
                "openscad via xvfb-run".into()
            } else {
                "openscad direct".into()
            },
        },
        Ok(output) => RenderOutcome {
            produced: false,
            detail: format!(
                "openscad render unavailable (no GL context?): {}",
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
            ),
        },
        Err(e) => RenderOutcome {
            produced: false,
            detail: format!("failed to launch renderer: {e}"),
        },
    }
}

/// Serialize assembly boxes as JSON for the FreeCAD STEP script.
pub fn boxes_json(assembly: &Assembly) -> String {
    let boxes: Vec<serde_json::Value> = assembly
        .panels
        .iter()
        .flat_map(|p| {
            p.instances.iter().map(|i| {
                serde_json::json!({
                    "origin": i.origin,
                    "size": i.size,
                })
            })
        })
        .collect();
    serde_json::Value::Array(boxes).to_string()
}

/// FreeCAD script: build a fused solid from a JSON box list and export STEP.
/// Written to a temp file at runtime; never committed as a `.py` source file.
pub const FREECAD_STEP_SCRIPT: &str = r#"# Simard Atelier — FreeCAD STEP exporter (generated at runtime)
import json, sys
import Part
boxes = json.load(open(sys.argv[1]))
out = sys.argv[2]
solid = None
for b in boxes:
    o = b["origin"]; s = b["size"]
    box = Part.makeBox(s[0], s[1], s[2], App.Vector(o[0], o[1], o[2]))
    solid = box if solid is None else solid.fuse(box)
if solid is not None:
    solid.exportStep(out)
"#;

/// Export a STEP solid via FreeCAD if `freecadcmd` is available. Optional.
pub fn freecad_step(assembly: &Assembly, step: &Path, work_dir: &Path) -> RenderOutcome {
    if !binary_available("freecadcmd") {
        return RenderOutcome {
            produced: false,
            detail: "freecadcmd not installed".into(),
        };
    }
    let script = work_dir.join("atelier_step.py");
    let boxes = work_dir.join("atelier_boxes.json");
    if std::fs::write(&script, FREECAD_STEP_SCRIPT).is_err()
        || std::fs::write(&boxes, boxes_json(assembly)).is_err()
    {
        return RenderOutcome {
            produced: false,
            detail: "could not stage FreeCAD script".into(),
        };
    }
    match Command::new("freecadcmd")
        .arg(&script)
        .arg(&boxes)
        .arg(step)
        .output()
    {
        Ok(o) if o.status.success() && file_non_empty(step) => RenderOutcome {
            produced: true,
            detail: "freecadcmd".into(),
        },
        Ok(o) => RenderOutcome {
            produced: false,
            detail: format!(
                "freecad STEP export failed: {}",
                String::from_utf8_lossy(&o.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
            ),
        },
        Err(e) => RenderOutcome {
            produced: false,
            detail: format!("failed to launch freecadcmd: {e}"),
        },
    }
}

/// Blender script: import an STL and render a PNG. Written to temp at runtime.
pub const BLENDER_RENDER_SCRIPT: &str = r#"# Simard Atelier — Blender STL render (generated at runtime)
import bpy, sys
argv = sys.argv[sys.argv.index("--") + 1:]
stl_path, out_path = argv[0], argv[1]
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_mesh.stl(filepath=stl_path)
bpy.ops.object.camera_add(location=(3, -3, 2))
cam = bpy.context.object
bpy.context.scene.camera = cam
bpy.ops.object.light_add(type='SUN', location=(4, -4, 6))
scene = bpy.context.scene
scene.render.image_settings.file_format = 'PNG'
scene.render.filepath = out_path
bpy.ops.render.render(write_still=True)
"#;

/// Render a PNG via Blender if available. Optional enhancement over OpenSCAD.
pub fn blender_render(stl: &Path, png: &Path, work_dir: &Path) -> RenderOutcome {
    if !binary_available("blender") {
        return RenderOutcome {
            produced: false,
            detail: "blender not installed".into(),
        };
    }
    let script = work_dir.join("atelier_blender.py");
    if std::fs::write(&script, BLENDER_RENDER_SCRIPT).is_err() {
        return RenderOutcome {
            produced: false,
            detail: "could not stage Blender script".into(),
        };
    }
    match Command::new("blender")
        .arg("-b")
        .arg("-P")
        .arg(&script)
        .arg("--")
        .arg(stl)
        .arg(png)
        .output()
    {
        Ok(o) if o.status.success() && file_non_empty(png) => RenderOutcome {
            produced: true,
            detail: "blender".into(),
        },
        Ok(o) => RenderOutcome {
            produced: false,
            detail: format!(
                "blender render failed: {}",
                String::from_utf8_lossy(&o.stderr)
                    .lines()
                    .last()
                    .unwrap_or("")
                    .trim()
            ),
        },
        Err(e) => RenderOutcome {
            produced: false,
            detail: format!("failed to launch blender: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atelier::brief::ProductBrief;
    use crate::atelier::geometry::generate;

    fn sample_assembly() -> Assembly {
        let json = r#"{"name":"t","kind":"box","dimensions_mm":{"width":100,"depth":80,"height":60},
            "material":{"name":"ply","thickness_mm":6}}"#;
        generate(&ProductBrief::from_json_bytes(json.as_bytes()).unwrap())
    }

    #[test]
    fn binary_available_true_for_sh() {
        assert!(binary_available("sh"));
    }

    #[test]
    fn binary_available_false_for_nonsense() {
        assert!(!binary_available("this-binary-does-not-exist-xyz"));
    }

    #[test]
    fn probe_missing_tool_has_no_version() {
        let report = probe("this-binary-does-not-exist-xyz");
        assert!(!report.available);
        assert!(report.version.is_none());
    }

    #[test]
    fn boxes_json_matches_instance_count() {
        let a = sample_assembly();
        let json: serde_json::Value = serde_json::from_str(&boxes_json(&a)).unwrap();
        assert_eq!(json.as_array().unwrap().len() as u32, a.instance_count());
        let first = &json[0];
        assert!(first.get("origin").is_some());
        assert!(first.get("size").is_some());
    }

    #[test]
    fn embedded_scripts_reference_expected_apis() {
        assert!(FREECAD_STEP_SCRIPT.contains("exportStep"));
        assert!(FREECAD_STEP_SCRIPT.contains("Part.makeBox"));
        assert!(BLENDER_RENDER_SCRIPT.contains("import_mesh.stl"));
        assert!(BLENDER_RENDER_SCRIPT.contains("render.render"));
    }

    #[test]
    fn optional_exports_skip_gracefully_when_absent() {
        // These tests run in an environment without freecad/blender.
        let a = sample_assembly();
        let dir = tempfile::tempdir().unwrap();
        if !binary_available("freecadcmd") {
            let r = freecad_step(&a, &dir.path().join("m.step"), dir.path());
            assert!(!r.produced);
            assert!(r.detail.contains("freecadcmd not installed"));
        }
        if !binary_available("blender") {
            let r = blender_render(
                &dir.path().join("m.stl"),
                &dir.path().join("r.png"),
                dir.path(),
            );
            assert!(!r.produced);
            assert!(r.detail.contains("blender not installed"));
        }
    }
}
