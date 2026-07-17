//! Storyboard derivation.
//!
//! A [`Storyboard`] is an ordered set of key panels sampled from the shot
//! timeline — the classic "shot brief → storyboard" step. Panels are derived
//! deterministically from the union of every object's keyframe times, so a
//! reviewer can read the shot's beats before a single frame is rendered.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::brief::ShotBrief;
use super::timeline;

/// One storyboard panel: a moment in time and where every object sits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Panel {
    pub index: u32,
    pub time_s: f64,
    /// The frame number this panel corresponds to.
    pub frame: u32,
    /// One line per visible object describing its placement.
    pub beats: Vec<String>,
}

/// A shot storyboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Storyboard {
    pub shot: String,
    pub style: String,
    pub fps: u32,
    pub duration_s: f64,
    pub panels: Vec<Panel>,
}

/// Maximum number of storyboard panels, so a keyframe-dense brief still yields a
/// readable board.
const MAX_PANELS: usize = 24;

/// Build a storyboard from a shot brief.
pub fn build_storyboard(brief: &ShotBrief) -> Storyboard {
    let times = panel_times(brief);

    let panels = times
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            let frame = ((t * brief.fps as f64).round() as i64)
                .clamp(0, brief.frame_count() as i64 - 1) as u32;
            let beats = brief
                .objects
                .iter()
                .map(|obj| {
                    let s = timeline::sample(&obj.keyframes, t);
                    format!(
                        "{} ({}) at ({:.2}, {:.2}) scale {:.2} opacity {:.2}",
                        obj.name,
                        obj.normalized_kind().label(),
                        s.x,
                        s.y,
                        s.scale,
                        s.opacity,
                    )
                })
                .collect();
            Panel {
                index: i as u32,
                time_s: t,
                frame,
                beats,
            }
        })
        .collect();

    Storyboard {
        shot: brief.name.clone(),
        style: brief.normalized_style().label().to_string(),
        fps: brief.fps,
        duration_s: brief.duration_s,
        panels,
    }
}

/// Distinct, sorted panel times: every keyframe time (clamped to the shot) plus
/// the shot start and end, capped at [`MAX_PANELS`].
fn panel_times(brief: &ShotBrief) -> Vec<f64> {
    let mut times: Vec<f64> = Vec::new();
    let push_unique = |times: &mut Vec<f64>, t: f64| {
        if !times.iter().any(|&e| (e - t).abs() < 1e-6) {
            times.push(t);
        }
    };
    push_unique(&mut times, 0.0);
    for obj in &brief.objects {
        for kf in &obj.keyframes {
            let t = kf.t.clamp(0.0, brief.duration_s);
            push_unique(&mut times, t);
        }
    }
    push_unique(&mut times, brief.duration_s);
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if times.len() > MAX_PANELS {
        // Evenly subsample down to MAX_PANELS, always keeping first and last.
        let mut reduced = Vec::with_capacity(MAX_PANELS);
        for i in 0..MAX_PANELS {
            let idx = i * (times.len() - 1) / (MAX_PANELS - 1);
            reduced.push(times[idx]);
        }
        reduced.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        reduced
    } else {
        times
    }
}

impl Storyboard {
    /// Render the storyboard as JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Render the storyboard as human-readable Markdown.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Storyboard — {}", self.shot);
        let _ = writeln!(
            out,
            "\nStyle: {} · {:.2}s @ {} fps · {} panels\n",
            self.style,
            self.duration_s,
            self.fps,
            self.panels.len()
        );
        for panel in &self.panels {
            let _ = writeln!(
                out,
                "## Panel {} — t={:.2}s (frame {})",
                panel.index + 1,
                panel.time_s,
                panel.frame
            );
            for beat in &panel.beats {
                let _ = writeln!(out, "- {beat}");
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief() -> ShotBrief {
        let json = r#"{
            "name": "Walk cycle", "style": "2d", "fps": 12, "duration_s": 2.0,
            "resolution": { "width": 160, "height": 120 },
            "objects": [
                { "name": "hero", "kind": "character", "size": 0.2,
                  "keyframes": [ {"t":0.0,"x":0.1,"y":0.5}, {"t":1.0,"x":0.5,"y":0.4}, {"t":2.0,"x":0.9,"y":0.5} ] }
            ]
        }"#;
        ShotBrief::from_json_bytes(json.as_bytes()).unwrap()
    }

    #[test]
    fn storyboard_has_panels_for_each_keyframe() {
        let sb = build_storyboard(&brief());
        // 0.0, 1.0, 2.0 — start and end coincide with keyframes.
        assert_eq!(sb.panels.len(), 3);
        assert_eq!(sb.panels[0].time_s, 0.0);
        assert_eq!(sb.panels.last().unwrap().time_s, 2.0);
    }

    #[test]
    fn every_panel_describes_every_object() {
        let sb = build_storyboard(&brief());
        for p in &sb.panels {
            assert_eq!(p.beats.len(), 1);
            assert!(p.beats[0].contains("hero"));
        }
    }

    #[test]
    fn markdown_and_json_render() {
        let sb = build_storyboard(&brief());
        assert!(sb.to_markdown().contains("# Storyboard — Walk cycle"));
        assert!(sb.to_json().contains("\"shot\""));
    }

    #[test]
    fn panels_are_capped() {
        let mut kfs = String::from("[");
        for i in 0..100 {
            if i > 0 {
                kfs.push(',');
            }
            let _ = write!(kfs, "{{\"t\":{:.3},\"x\":0.5,\"y\":0.5}}", i as f64 * 0.05);
        }
        kfs.push(']');
        let json = format!(
            r#"{{"name":"x","style":"2d","fps":20,"duration_s":5.0,
            "resolution":{{"width":32,"height":32}},
            "objects":[{{"name":"o","keyframes":{kfs}}}]}}"#
        );
        let b = ShotBrief::from_json_bytes(json.as_bytes()).unwrap();
        let sb = build_storyboard(&b);
        assert!(sb.panels.len() <= 24);
        assert!(sb.panels.len() >= 2);
    }
}
