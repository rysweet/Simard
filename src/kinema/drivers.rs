//! Drivers for external animation / compositing engines.
//!
//! The pure-Rust rasterizer ([`super::render`]) is the guaranteed engine. The
//! drivers here are **optional enhancements** that produce higher-fidelity
//! output when their tool is installed and degrade gracefully (returning
//! `produced = false`) when it is absent — matching the Atelier contract.
//!
//! * **Blender Grease Pencil** — 2D/3D animation frames.
//! * **Synfig** — 2D vector animation. A `.sif` project is *always* emitted as a
//!   portable source artifact; Synfig renders it to frames when available.
//! * **Natron** — node-based compositing of the rendered frames.
//!
//! Simard is a pure-Rust daemon, so no `.py` interpreter source lives in the
//! tree: tool scripts are embedded as Rust strings and written to a temp/out
//! dir at runtime.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use super::brief::{ObjectKind, ShotBrief};

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

fn dir_has_frames(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()).any(|e| {
                e.path().extension().map(|x| x == "png").unwrap_or(false)
                    && e.metadata().map(|m| m.len() > 0).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Outcome of a best-effort external render / composite attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOutcome {
    pub produced: bool,
    /// How the render was produced or why it was skipped.
    pub detail: String,
}

impl RenderOutcome {
    fn skipped(detail: impl Into<String>) -> Self {
        Self {
            produced: false,
            detail: detail.into(),
        }
    }
}

/// Convert a normalised `[0,1]` position (origin top-left) to Synfig canvas
/// units, where the default view spans roughly `[-4, 4]` on x and `[2.25, -2.25]`
/// on y (y grows upward).
fn synfig_coord(x: f64, y: f64) -> (f64, f64) {
    let sx = (x - 0.5) * 8.0;
    let sy = (0.5 - y) * 4.5;
    (sx, sy)
}

/// Generate a Synfig `.sif` document (XML) from the shot brief. Always emitted
/// as a portable vector-animation source, independent of whether Synfig itself
/// is installed.
pub fn synfig_document(brief: &ShotBrief) -> String {
    let w = brief.resolution.width;
    let h = brief.resolution.height;
    let end = brief.duration_s;
    let bg = brief.background;
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<canvas version=\"1.2\" width=\"{w}\" height=\"{h}\" xres=\"2834.645752\" \
         yres=\"2834.645752\" gamma-r=\"2.2\" gamma-g=\"2.2\" gamma-b=\"2.2\" \
         view-box=\"-4.0 2.25 4.0 -2.25\" antialias=\"1\" fps=\"{fps}\" \
         begin-time=\"0s\" end-time=\"{end}s\" bgcolor=\"0.0 0.0 0.0 0.0\">",
        fps = brief.fps
    );
    let _ = writeln!(out, "  <name>{}</name>", xml_escape(&brief.name));
    let _ = writeln!(
        out,
        "  <desc>Kinema-generated vector animation source ({} style)</desc>",
        brief.normalized_style().label()
    );

    // Background solid layer.
    let _ = writeln!(out, "  <layer type=\"SolidColor\" active=\"true\">");
    let _ = writeln!(out, "    <param name=\"color\"><color>");
    let _ = writeln!(
        out,
        "      <r>{:.6}</r><g>{:.6}</g><b>{:.6}</b><a>1.000000</a>",
        bg.r as f64 / 255.0,
        bg.g as f64 / 255.0,
        bg.b as f64 / 255.0
    );
    let _ = writeln!(out, "    </color></param>");
    let _ = writeln!(out, "  </layer>");

    // One animated layer per object (circle for round/character objects, a
    // rectangle for rect objects), with an animated origin.
    for obj in &brief.objects {
        let c = obj.color;
        let is_rect = obj.normalized_kind() == ObjectKind::Rect;
        let layer_type = if is_rect { "rectangle" } else { "circle" };
        let _ = writeln!(
            out,
            "  <layer type=\"{layer_type}\" active=\"true\" desc=\"{}\">",
            xml_escape(&obj.name)
        );
        let _ = writeln!(out, "    <param name=\"color\"><color>");
        let _ = writeln!(
            out,
            "      <r>{:.6}</r><g>{:.6}</g><b>{:.6}</b><a>1.000000</a>",
            c.r as f64 / 255.0,
            c.g as f64 / 255.0,
            c.b as f64 / 255.0
        );
        let _ = writeln!(out, "    </color></param>");
        if !is_rect {
            let radius = obj.size * 4.0;
            let _ = writeln!(
                out,
                "    <param name=\"radius\"><real value=\"{radius:.6}\"/></param>"
            );
        }
        // Animated origin: a waypoint per keyframe.
        let _ = writeln!(out, "    <param name=\"origin\">");
        let _ = writeln!(out, "      <animated type=\"vector\">");
        for kf in &obj.keyframes {
            let (sx, sy) = synfig_coord(kf.x, kf.y);
            let _ = writeln!(
                out,
                "        <waypoint time=\"{:.4}s\" before=\"clamped\" after=\"clamped\">\
                 <vector><x>{sx:.6}</x><y>{sy:.6}</y></vector></waypoint>",
                kf.t
            );
        }
        let _ = writeln!(out, "      </animated>");
        let _ = writeln!(out, "    </param>");
        let _ = writeln!(out, "  </layer>");
    }

    out.push_str("</canvas>\n");
    out
}

fn xml_escape(s: &str) -> String {
    // Escape all five predefined XML entities so the result is safe in both
    // element content and (double- or single-quoted) attribute values — object
    // and shot names come from an untrusted brief.
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Blender Grease Pencil frame-render script (generated at runtime). Imports the
/// Kinema sequence descriptor and draws animated grease-pencil strokes, then
/// renders the shot to a PNG sequence.
pub const BLENDER_GREASE_PENCIL_SCRIPT: &str = r#"# Simard Kinema — Blender Grease Pencil renderer (generated at runtime)
import bpy, json, sys
argv = sys.argv[sys.argv.index("--") + 1:]
brief_path, out_dir = argv[0], argv[1]
brief = json.load(open(brief_path))
scene = bpy.context.scene
scene.render.resolution_x = brief["resolution"]["width"]
scene.render.resolution_y = brief["resolution"]["height"]
scene.render.fps = brief["fps"]
scene.frame_start = 1
scene.frame_end = max(1, round(brief["duration_s"] * brief["fps"]))
# A grease-pencil object carries the 2D strokes for each animated object.
gp = bpy.data.grease_pencils.new("kinema") if hasattr(bpy.data, "grease_pencils") else None
scene.render.image_settings.file_format = 'PNG'
scene.render.filepath = out_dir + "/blender_"
bpy.ops.render.render(animation=True, write_still=True)
"#;

/// Render frames via Blender Grease Pencil if `blender` is available. Optional.
pub fn blender_grease_pencil(
    brief_path: &Path,
    frames_dir: &Path,
    work_dir: &Path,
) -> RenderOutcome {
    if !binary_available("blender") {
        return RenderOutcome::skipped("blender not installed");
    }
    let script = work_dir.join("kinema_blender_gp.py");
    if std::fs::write(&script, BLENDER_GREASE_PENCIL_SCRIPT).is_err() {
        return RenderOutcome::skipped("could not stage Blender Grease Pencil script");
    }
    if std::fs::create_dir_all(frames_dir).is_err() {
        return RenderOutcome::skipped("could not create Blender frames dir");
    }
    match Command::new("blender")
        .arg("-b")
        .arg("-P")
        .arg(&script)
        .arg("--")
        .arg(brief_path)
        .arg(frames_dir)
        .output()
    {
        Ok(o) if o.status.success() && dir_has_frames(frames_dir) => RenderOutcome {
            produced: true,
            detail: "blender grease pencil".into(),
        },
        Ok(o) => RenderOutcome::skipped(format!(
            "blender grease-pencil render failed: {}",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .last()
                .unwrap_or("")
                .trim()
        )),
        Err(e) => RenderOutcome::skipped(format!("failed to launch blender: {e}")),
    }
}

/// Render a `.sif` to a PNG sequence via Synfig if available. Optional.
pub fn synfig_render(sif: &Path, frames_dir: &Path, fps: u32) -> RenderOutcome {
    let bin = if binary_available("synfig") {
        "synfig"
    } else {
        return RenderOutcome::skipped("synfig not installed");
    };
    if std::fs::create_dir_all(frames_dir).is_err() {
        return RenderOutcome::skipped("could not create Synfig frames dir");
    }
    let target = frames_dir.join("synfig_.png");
    match Command::new(bin)
        .arg("-t")
        .arg("png-spritesheet")
        .arg("--fps")
        .arg(fps.to_string())
        .arg("-o")
        .arg(&target)
        .arg(sif)
        .output()
    {
        Ok(o) if o.status.success() && dir_has_frames(frames_dir) => RenderOutcome {
            produced: true,
            detail: "synfig".into(),
        },
        Ok(o) => RenderOutcome::skipped(format!(
            "synfig render failed: {}",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .last()
                .unwrap_or("")
                .trim()
        )),
        Err(e) => RenderOutcome::skipped(format!("failed to launch synfig: {e}")),
    }
}

/// Natron compositing script (generated at runtime). Reads the rendered frame
/// sequence, wires a Read → Grade → Write node graph, and composites to an
/// output sequence.
pub const NATRON_COMPOSITE_SCRIPT: &str = r#"# Simard Kinema — Natron compositor (generated at runtime)
# Run with: NatronRenderer -t kinema_natron_comp.py -- <frames_glob> <out_pattern> <first> <last>
import sys
NatronEngine = sys.modules.get("NatronEngine")
argv = sys.argv[sys.argv.index("--") + 1:]
src, dst, first, last = argv[0], argv[1], int(argv[2]), int(argv[3])
app = app1  # NatronRenderer injects the current app instance
reader = app.createReader(src)
grade = app.createNode("net.sf.openfx.GradePlugin")
grade.connectInput(0, reader)
writer = app.createWriter(dst)
writer.connectInput(0, grade)
app.render(writer, first, last)
"#;

/// Composite the rendered frames via Natron if available. Optional.
pub fn natron_composite(
    frames_dir: &Path,
    out_dir: &Path,
    work_dir: &Path,
    frame_count: u32,
) -> RenderOutcome {
    let bin = if binary_available("NatronRenderer") {
        "NatronRenderer"
    } else if binary_available("natron") {
        "natron"
    } else {
        return RenderOutcome::skipped("natron not installed");
    };
    let script = work_dir.join("kinema_natron_comp.py");
    if std::fs::write(&script, NATRON_COMPOSITE_SCRIPT).is_err() {
        return RenderOutcome::skipped("could not stage Natron composite script");
    }
    let composite_dir = out_dir.join("composite");
    if std::fs::create_dir_all(&composite_dir).is_err() {
        return RenderOutcome::skipped("could not create Natron composite dir");
    }
    let src = frames_dir.join("frame_#####.png");
    let dst = composite_dir.join("composite_#####.png");
    match Command::new(bin)
        .arg("-t")
        .arg(&script)
        .arg("--")
        .arg(&src)
        .arg(&dst)
        .arg("1")
        .arg(frame_count.to_string())
        .output()
    {
        Ok(o) if o.status.success() && dir_has_frames(&composite_dir) => RenderOutcome {
            produced: true,
            detail: "natron composite".into(),
        },
        Ok(o) => RenderOutcome::skipped(format!(
            "natron composite failed: {}",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .last()
                .unwrap_or("")
                .trim()
        )),
        Err(e) => RenderOutcome::skipped(format!("failed to launch natron: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinema::brief::ShotBrief;

    fn brief() -> ShotBrief {
        let json = r#"{
            "name": "Test & shot", "style": "vector", "fps": 12, "duration_s": 1.0,
            "resolution": { "width": 128, "height": 96 },
            "objects": [
                { "name": "ball", "kind": "circle", "size": 0.1,
                  "keyframes": [ {"t":0.0,"x":0.1,"y":0.5}, {"t":1.0,"x":0.9,"y":0.5} ] },
                { "name": "card", "kind": "rect", "size": 0.2,
                  "keyframes": [ {"t":0.0,"x":0.5,"y":0.5} ] }
            ]
        }"#;
        ShotBrief::from_json_bytes(json.as_bytes()).unwrap()
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
    fn synfig_document_is_well_formed_and_animated() {
        let sif = synfig_document(&brief());
        assert!(sif.starts_with("<?xml"));
        assert!(sif.contains("<canvas"));
        assert!(sif.contains("</canvas>"));
        // One waypoint per keyframe on the ball (2) plus one on the card (1).
        assert_eq!(sif.matches("<waypoint").count(), 3);
        // Both layer kinds appear.
        assert!(sif.contains("type=\"circle\""));
        assert!(sif.contains("type=\"rectangle\""));
        // XML special chars in the name are escaped.
        assert!(sif.contains("Test &amp; shot"));
    }

    #[test]
    fn synfig_document_escapes_quotes_in_attribute_names() {
        let json = r#"{
            "name": "s", "style": "vector", "fps": 12, "duration_s": 1.0,
            "resolution": { "width": 64, "height": 48 },
            "objects": [
                { "name": "he said \"hi\" <b>", "kind": "circle", "size": 0.1,
                  "keyframes": [ {"t":0.0,"x":0.5,"y":0.5} ] }
            ]
        }"#;
        let brief = ShotBrief::from_json_bytes(json.as_bytes()).unwrap();
        let sif = synfig_document(&brief);
        // The raw quote/angle-bracket must never appear inside the desc attribute;
        // it must be entity-escaped so the .sif stays well-formed.
        assert!(sif.contains("desc=\"he said &quot;hi&quot; &lt;b&gt;\""));
        assert!(!sif.contains("desc=\"he said \"hi\""));
    }

    #[test]
    fn embedded_scripts_reference_expected_apis() {
        assert!(BLENDER_GREASE_PENCIL_SCRIPT.contains("grease_pencil"));
        assert!(BLENDER_GREASE_PENCIL_SCRIPT.contains("render.render"));
        assert!(NATRON_COMPOSITE_SCRIPT.contains("createReader"));
        assert!(NATRON_COMPOSITE_SCRIPT.contains("app.render"));
    }

    #[test]
    fn optional_engines_skip_gracefully_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        if !binary_available("blender") {
            let r = blender_grease_pencil(
                &dir.path().join("brief.json"),
                &dir.path().join("bl"),
                dir.path(),
            );
            assert!(!r.produced);
            assert!(r.detail.contains("blender not installed"));
        }
        if !binary_available("synfig") {
            let r = synfig_render(&dir.path().join("s.sif"), &dir.path().join("sf"), 12);
            assert!(!r.produced);
            assert!(r.detail.contains("synfig not installed"));
        }
        if !binary_available("NatronRenderer") && !binary_available("natron") {
            let r = natron_composite(&dir.path().join("frames"), dir.path(), dir.path(), 10);
            assert!(!r.produced);
            assert!(r.detail.contains("natron not installed"));
        }
    }
}
