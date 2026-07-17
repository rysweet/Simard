//! Frame-sequence rendering: the "shot brief → rendered animated sequence" core.
//!
//! For every frame in the shot, sample each object's transform, draw it onto a
//! fresh canvas, and encode the result as a PNG. This is the guaranteed engine:
//! it is pure Rust with no external dependency, so a brief always renders to a
//! real animated frame sequence even when Blender / Synfig / Natron are absent.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::brief::{AnimatedObject, Color, ObjectKind, ShotBrief};
use super::error::{KinemaError, KinemaResult};
use super::raster::Canvas;
use super::timeline;

/// Subdirectory (under the package out-dir) that holds rendered frames.
pub const FRAMES_DIR: &str = "frames";
/// The sequence descriptor file name.
pub const SEQUENCE_JSON: &str = "sequence.json";

/// One entry in the rendered sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameEntry {
    pub index: u32,
    pub time_s: f64,
    pub file: String,
    pub bytes: u64,
}

/// The rendered-sequence descriptor, persisted as `sequence.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sequence {
    pub shot: String,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub frames: Vec<FrameEntry>,
}

impl Sequence {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Zero-padded frame file name, e.g. `frame_00001.png`.
pub fn frame_file_name(index: u32) -> String {
    format!("frame_{:05}.png", index + 1)
}

/// Render the full frame sequence for `brief` into `out_dir/frames`, writing a
/// `sequence.json` descriptor into `out_dir`.
pub fn render_sequence(brief: &ShotBrief, out_dir: &Path) -> KinemaResult<Sequence> {
    let frames_dir = out_dir.join(FRAMES_DIR);
    std::fs::create_dir_all(&frames_dir)
        .map_err(|e| KinemaError::io(format!("creating {}", frames_dir.display()), e))?;

    let frame_count = brief.frame_count();
    let width = brief.resolution.width;
    let height = brief.resolution.height;
    let mut frames = Vec::with_capacity(frame_count as usize);

    for f in 0..frame_count {
        let t = f as f64 / brief.fps as f64;
        let mut canvas = Canvas::new(width, height, brief.background);
        for obj in &brief.objects {
            draw_object(&mut canvas, obj, t);
        }
        let png = canvas.to_png();
        let file = frame_file_name(f);
        let path: PathBuf = frames_dir.join(&file);
        std::fs::write(&path, &png)
            .map_err(|e| KinemaError::io(format!("writing {}", path.display()), e))?;
        frames.push(FrameEntry {
            index: f,
            time_s: t,
            file: format!("{FRAMES_DIR}/{file}"),
            bytes: png.len() as u64,
        });
    }

    let sequence = Sequence {
        shot: brief.name.clone(),
        fps: brief.fps,
        width,
        height,
        frame_count,
        frames,
    };
    let seq_path = out_dir.join(SEQUENCE_JSON);
    std::fs::write(&seq_path, sequence.to_json())
        .map_err(|e| KinemaError::io(format!("writing {}", seq_path.display()), e))?;
    Ok(sequence)
}

/// Draw one object at time `t` onto the canvas.
fn draw_object(canvas: &mut Canvas, obj: &AnimatedObject, t: f64) {
    let s = timeline::sample(&obj.keyframes, t);
    if s.opacity <= 0.0 {
        return;
    }
    let w = canvas.width() as f64;
    let h = canvas.height() as f64;
    let cx = s.x * w;
    let cy = s.y * h;
    let base = obj.size * w.min(h) * s.scale;

    match obj.normalized_kind() {
        ObjectKind::Circle => {
            canvas.fill_circle(cx, cy, base / 2.0, obj.color, s.opacity);
        }
        ObjectKind::Rect => {
            canvas.fill_rect(cx, cy, base, base, obj.color, s.opacity);
        }
        ObjectKind::Character => {
            draw_character(canvas, cx, cy, base, obj.color, s.opacity, t);
        }
    }
}

/// Draw a rigged stick figure. The figure height is `base`; limbs swing with a
/// walk-cycle phase driven by `t`, so a rigged character actually animates
/// rather than sliding rigidly.
fn draw_character(
    canvas: &mut Canvas,
    cx: f64,
    cy: f64,
    base: f64,
    color: Color,
    alpha: f64,
    t: f64,
) {
    // Local units: the character spans `base` vertically, centred on (cx, cy).
    let unit = base;
    let stroke = (base * 0.08).max(1.0);

    // Rig proportions mirror `rig::character_bones`.
    let hip = (cx, cy + unit * 0.10);
    let shoulder = (cx, cy - unit * 0.16);
    let head_c = (cx, cy - unit * 0.28);
    let head_r = unit * 0.10;

    // Torso.
    canvas.stroke_line(hip, shoulder, stroke, color, alpha);
    // Head.
    canvas.fill_circle(head_c.0, head_c.1, head_r, color, alpha);

    // Walk-cycle swing: 2 Hz gait.
    let phase = (t * 2.0 * PI * 2.0).sin();
    let swing = unit * 0.16 * phase;
    let arm_len = unit * 0.22;
    let leg_len = unit * 0.28;

    // Arms swing opposite to legs.
    canvas.stroke_line(
        shoulder,
        (shoulder.0 + swing, shoulder.1 + arm_len),
        stroke,
        color,
        alpha,
    );
    canvas.stroke_line(
        shoulder,
        (shoulder.0 - swing, shoulder.1 + arm_len),
        stroke,
        color,
        alpha,
    );
    // Legs.
    canvas.stroke_line(hip, (hip.0 - swing, hip.1 + leg_len), stroke, color, alpha);
    canvas.stroke_line(hip, (hip.0 + swing, hip.1 + leg_len), stroke, color, alpha);
}

/// Count how many frame PNGs actually exist and are non-empty under
/// `out_dir/frames` for the expected frame set. Used by verification / inspect.
pub fn count_present_frames(out_dir: &Path, expected: u32) -> u32 {
    let frames_dir = out_dir.join(FRAMES_DIR);
    (0..expected)
        .filter(|&f| {
            let path = frames_dir.join(frame_file_name(f));
            std::fs::metadata(&path)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        })
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief() -> ShotBrief {
        let json = r#"{
            "name": "seq", "style": "2d", "fps": 10, "duration_s": 0.5,
            "resolution": { "width": 48, "height": 32 },
            "objects": [
                { "name": "hero", "kind": "character", "size": 0.4,
                  "keyframes": [ {"t":0.0,"x":0.2,"y":0.5}, {"t":0.5,"x":0.8,"y":0.5} ] },
                { "name": "sun", "kind": "circle", "color": {"r":250,"g":220,"b":80}, "size": 0.15,
                  "keyframes": [ {"t":0.0,"x":0.5,"y":0.2} ] }
            ]
        }"#;
        ShotBrief::from_json_bytes(json.as_bytes()).unwrap()
    }

    #[test]
    fn frame_file_names_are_zero_padded() {
        assert_eq!(frame_file_name(0), "frame_00001.png");
        assert_eq!(frame_file_name(41), "frame_00042.png");
    }

    #[test]
    fn renders_all_frames_and_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let b = brief();
        let seq = render_sequence(&b, dir.path()).unwrap();
        assert_eq!(seq.frame_count, b.frame_count());
        assert_eq!(seq.frames.len(), b.frame_count() as usize);
        // Every listed frame exists and is a non-empty PNG.
        for entry in &seq.frames {
            let p = dir.path().join(&entry.file);
            let bytes = std::fs::read(&p).unwrap();
            assert!(bytes.len() > 8);
            assert_eq!(
                &bytes[0..8],
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
            );
        }
        assert!(dir.path().join(SEQUENCE_JSON).exists());
        assert_eq!(
            count_present_frames(dir.path(), b.frame_count()),
            b.frame_count()
        );
    }

    #[test]
    fn frames_differ_over_time() {
        // The hero moves, so the first and last frames must not be identical.
        let dir = tempfile::tempdir().unwrap();
        let b = brief();
        render_sequence(&b, dir.path()).unwrap();
        let first = std::fs::read(dir.path().join(FRAMES_DIR).join(frame_file_name(0))).unwrap();
        let last = std::fs::read(
            dir.path()
                .join(FRAMES_DIR)
                .join(frame_file_name(b.frame_count() - 1)),
        )
        .unwrap();
        assert_ne!(first, last, "animation should change across frames");
    }

    #[test]
    fn count_present_frames_reports_zero_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(count_present_frames(dir.path(), 5), 0);
    }
}
