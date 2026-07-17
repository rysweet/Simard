//! Shot-brief data model.
//!
//! A [`ShotBrief`] is the untrusted input to the Kinema pipeline: a description
//! of an animated shot to be produced. It is parsed from JSON, validated for
//! sanity, and then drives storyboard generation, rig derivation, and frame
//! rendering.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::{KinemaError, KinemaResult};

/// Upper bound on total rendered frames accepted from an untrusted brief.
/// Guards against a hostile brief driving unbounded rendering work / disk use.
pub const MAX_FRAMES: u32 = 3600;
/// Upper bound on canvas dimension in pixels.
pub const MAX_DIMENSION: u32 = 4096;
/// Upper bound on animated objects in a single shot.
pub const MAX_OBJECTS: usize = 256;
/// Upper bound on keyframes on a single object.
pub const MAX_KEYFRAMES: usize = 4096;

/// A shot to be animated, parsed from a brief JSON document.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShotBrief {
    /// Human-readable shot name.
    pub name: String,
    /// Animation style. Unknown values fall back to motion graphics.
    pub style: String,
    /// Frames per second.
    pub fps: u32,
    /// Shot duration in seconds.
    pub duration_s: f64,
    /// Output resolution in pixels.
    pub resolution: Resolution,
    /// Background colour. Defaults to dark slate.
    #[serde(default = "default_background")]
    pub background: Color,
    /// The animated objects that make up the shot.
    #[serde(default)]
    pub objects: Vec<AnimatedObject>,
    /// Optional artistic notes carried into the storyboard.
    #[serde(default)]
    pub notes: Option<String>,
    /// Any additional metadata, preserved for documentation.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_background() -> Color {
    Color {
        r: 18,
        g: 22,
        b: 33,
    }
}

/// Output resolution in pixels.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

/// An 8-bit-per-channel RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
    };

    /// Blend `self` over `bg` with the given alpha in `[0, 1]` (src-over).
    pub fn over(self, bg: Color, alpha: f64) -> Color {
        let a = alpha.clamp(0.0, 1.0);
        let mix = |s: u8, b: u8| ((s as f64) * a + (b as f64) * (1.0 - a)).round() as u8;
        Color {
            r: mix(self.r, bg.r),
            g: mix(self.g, bg.g),
            b: mix(self.b, bg.b),
        }
    }
}

/// A single animated object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnimatedObject {
    /// Object name (used for the storyboard, rig, and diagnostics).
    pub name: String,
    /// Object family. Unknown values fall back to a circle.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Fill colour. Defaults to white.
    #[serde(default = "default_object_color")]
    pub color: Color,
    /// Base size as a fraction of the smaller canvas dimension.
    /// Interpreted as a radius for circles/characters and as width/height for
    /// rectangles.
    #[serde(default = "default_size")]
    pub size: f64,
    /// Animation keyframes. Must contain at least one.
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
}

fn default_kind() -> String {
    "circle".to_string()
}

fn default_object_color() -> Color {
    Color::WHITE
}

fn default_size() -> f64 {
    0.1
}

/// A single keyframe. Positions are normalised to `[0, 1]` with the origin at
/// the top-left of the canvas.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct Keyframe {
    /// Time in seconds from the start of the shot.
    pub t: f64,
    /// Horizontal centre position, `0.0` = left edge, `1.0` = right edge.
    pub x: f64,
    /// Vertical centre position, `0.0` = top edge, `1.0` = bottom edge.
    pub y: f64,
    /// Uniform scale multiplier applied to `size`.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Opacity in `[0, 1]`.
    #[serde(default = "default_opacity")]
    pub opacity: f64,
}

fn default_scale() -> f64 {
    1.0
}

fn default_opacity() -> f64 {
    1.0
}

/// The kind of object to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// A filled circle.
    Circle,
    /// An axis-aligned filled rectangle.
    Rect,
    /// A rigged stick-figure character (head + torso + limbs).
    Character,
}

impl ObjectKind {
    pub fn classify(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rect" | "rectangle" | "box" | "card" | "shape" => Self::Rect,
            "character" | "actor" | "figure" | "person" | "puppet" => Self::Character,
            _ => Self::Circle,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Circle => "circle",
            Self::Rect => "rect",
            Self::Character => "character",
        }
    }
}

/// Animation style, selecting the preferred external engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationStyle {
    /// 2D frame animation (Blender Grease Pencil is the preferred engine).
    GreasePencil2D,
    /// 3D animation (Blender is the preferred engine).
    ThreeD,
    /// 2D vector animation (Synfig is the preferred engine).
    Vector,
    /// Motion graphics / compositing (Natron is the preferred engine).
    MotionGraphics,
}

impl AnimationStyle {
    pub fn classify(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "2d" | "grease-pencil" | "grease_pencil" | "greasepencil" | "hand-drawn" | "cel" => {
                Self::GreasePencil2D
            }
            "3d" | "three-d" | "cgi" => Self::ThreeD,
            "vector" | "synfig" | "svg" | "flash" => Self::Vector,
            _ => Self::MotionGraphics,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::GreasePencil2D => "grease-pencil-2d",
            Self::ThreeD => "3d",
            Self::Vector => "vector",
            Self::MotionGraphics => "motion-graphics",
        }
    }

    /// The external engine preferred for this style.
    pub fn preferred_engine(self) -> &'static str {
        match self {
            Self::GreasePencil2D | Self::ThreeD => "blender",
            Self::Vector => "synfig",
            Self::MotionGraphics => "natron",
        }
    }
}

impl AnimatedObject {
    pub fn normalized_kind(&self) -> ObjectKind {
        ObjectKind::classify(&self.kind)
    }
}

impl ShotBrief {
    /// Read and validate a brief from a JSON file.
    pub fn from_path(path: &Path) -> KinemaResult<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| KinemaError::io(format!("reading brief {}", path.display()), e))?;
        Self::from_json_bytes(&bytes)
    }

    /// Parse and validate a brief from JSON bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> KinemaResult<Self> {
        let brief: ShotBrief = serde_json::from_slice(bytes)
            .map_err(|e| KinemaError::parse("brief json", e.to_string()))?;
        brief.validate()?;
        Ok(brief)
    }

    /// Normalised animation style used to select the preferred engine.
    pub fn normalized_style(&self) -> AnimationStyle {
        AnimationStyle::classify(&self.style)
    }

    /// Total number of frames the shot renders to.
    pub fn frame_count(&self) -> u32 {
        // Round to the nearest frame, always at least one.
        ((self.duration_s * self.fps as f64).round() as i64).clamp(1, MAX_FRAMES as i64) as u32
    }

    /// Reject malformed or nonsensical briefs.
    pub fn validate(&self) -> KinemaResult<()> {
        if self.name.trim().is_empty() {
            return Err(KinemaError::invalid_brief("name must not be empty"));
        }
        if self.fps == 0 || self.fps > 240 {
            return Err(KinemaError::invalid_brief(format!(
                "fps must be between 1 and 240 (got {})",
                self.fps
            )));
        }
        if !self.duration_s.is_finite() || self.duration_s <= 0.0 {
            return Err(KinemaError::invalid_brief(format!(
                "duration_s must be a positive, finite number (got {})",
                self.duration_s
            )));
        }
        let Resolution { width, height } = self.resolution;
        if width == 0 || height == 0 {
            return Err(KinemaError::invalid_brief(
                "resolution width and height must be positive",
            ));
        }
        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(KinemaError::invalid_brief(format!(
                "resolution must not exceed {MAX_DIMENSION}px per side (got {width}x{height})"
            )));
        }
        // Bound total rendering work.
        let frames = (self.duration_s * self.fps as f64).round();
        if frames > MAX_FRAMES as f64 {
            return Err(KinemaError::invalid_brief(format!(
                "duration_s×fps must not exceed {MAX_FRAMES} frames (got {frames})"
            )));
        }
        if self.objects.is_empty() {
            return Err(KinemaError::invalid_brief(
                "a shot must contain at least one animated object",
            ));
        }
        if self.objects.len() > MAX_OBJECTS {
            return Err(KinemaError::invalid_brief(format!(
                "a shot must not contain more than {MAX_OBJECTS} objects (got {})",
                self.objects.len()
            )));
        }
        for obj in &self.objects {
            if obj.name.trim().is_empty() {
                return Err(KinemaError::invalid_brief("object name must not be empty"));
            }
            if !obj.size.is_finite() || obj.size <= 0.0 || obj.size > 2.0 {
                return Err(KinemaError::invalid_brief(format!(
                    "object '{}' size must be in (0, 2] fraction of the canvas (got {})",
                    obj.name, obj.size
                )));
            }
            if obj.keyframes.is_empty() {
                return Err(KinemaError::invalid_brief(format!(
                    "object '{}' must have at least one keyframe",
                    obj.name
                )));
            }
            if obj.keyframes.len() > MAX_KEYFRAMES {
                return Err(KinemaError::invalid_brief(format!(
                    "object '{}' must not have more than {MAX_KEYFRAMES} keyframes",
                    obj.name
                )));
            }
            for kf in &obj.keyframes {
                for (label, v) in [
                    ("t", kf.t),
                    ("x", kf.x),
                    ("y", kf.y),
                    ("scale", kf.scale),
                    ("opacity", kf.opacity),
                ] {
                    if !v.is_finite() {
                        return Err(KinemaError::invalid_brief(format!(
                            "object '{}' keyframe field '{label}' must be finite",
                            obj.name
                        )));
                    }
                }
                if kf.t < 0.0 {
                    return Err(KinemaError::invalid_brief(format!(
                        "object '{}' keyframe time must be non-negative (got {})",
                        obj.name, kf.t
                    )));
                }
                if kf.scale <= 0.0 {
                    return Err(KinemaError::invalid_brief(format!(
                        "object '{}' keyframe scale must be positive (got {})",
                        obj.name, kf.scale
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
            "name": "Bouncing hero",
            "style": "2d",
            "fps": 12,
            "duration_s": 2.0,
            "resolution": { "width": 320, "height": 240 },
            "objects": [
                {
                    "name": "hero",
                    "kind": "character",
                    "color": { "r": 240, "g": 200, "b": 60 },
                    "size": 0.15,
                    "keyframes": [
                        { "t": 0.0, "x": 0.1, "y": 0.5 },
                        { "t": 2.0, "x": 0.9, "y": 0.5 }
                    ]
                }
            ]
        }"#
    }

    #[test]
    fn parses_and_validates_sample() {
        let brief = ShotBrief::from_json_bytes(sample_json().as_bytes()).unwrap();
        assert_eq!(brief.name, "Bouncing hero");
        assert_eq!(brief.normalized_style(), AnimationStyle::GreasePencil2D);
        assert_eq!(brief.frame_count(), 24);
        assert_eq!(brief.objects[0].normalized_kind(), ObjectKind::Character);
    }

    #[test]
    fn default_background_when_absent() {
        let brief = ShotBrief::from_json_bytes(sample_json().as_bytes()).unwrap();
        assert_eq!(brief.background, default_background());
    }

    #[test]
    fn rejects_zero_fps() {
        let json = r#"{"name":"x","style":"2d","fps":0,"duration_s":1.0,
            "resolution":{"width":10,"height":10},
            "objects":[{"name":"o","keyframes":[{"t":0,"x":0.5,"y":0.5}]}]}"#;
        assert!(matches!(
            ShotBrief::from_json_bytes(json.as_bytes()).unwrap_err(),
            KinemaError::InvalidBrief { .. }
        ));
    }

    #[test]
    fn rejects_non_positive_duration() {
        let json = r#"{"name":"x","style":"2d","fps":12,"duration_s":0.0,
            "resolution":{"width":10,"height":10},
            "objects":[{"name":"o","keyframes":[{"t":0,"x":0.5,"y":0.5}]}]}"#;
        assert!(matches!(
            ShotBrief::from_json_bytes(json.as_bytes()).unwrap_err(),
            KinemaError::InvalidBrief { .. }
        ));
    }

    #[test]
    fn rejects_zero_resolution() {
        let json = r#"{"name":"x","style":"2d","fps":12,"duration_s":1.0,
            "resolution":{"width":0,"height":10},
            "objects":[{"name":"o","keyframes":[{"t":0,"x":0.5,"y":0.5}]}]}"#;
        assert!(matches!(
            ShotBrief::from_json_bytes(json.as_bytes()).unwrap_err(),
            KinemaError::InvalidBrief { .. }
        ));
    }

    #[test]
    fn rejects_too_many_frames() {
        let json = r#"{"name":"x","style":"2d","fps":240,"duration_s":100.0,
            "resolution":{"width":10,"height":10},
            "objects":[{"name":"o","keyframes":[{"t":0,"x":0.5,"y":0.5}]}]}"#;
        assert!(matches!(
            ShotBrief::from_json_bytes(json.as_bytes()).unwrap_err(),
            KinemaError::InvalidBrief { .. }
        ));
    }

    #[test]
    fn rejects_empty_objects() {
        let json = r#"{"name":"x","style":"2d","fps":12,"duration_s":1.0,
            "resolution":{"width":10,"height":10},"objects":[]}"#;
        assert!(matches!(
            ShotBrief::from_json_bytes(json.as_bytes()).unwrap_err(),
            KinemaError::InvalidBrief { .. }
        ));
    }

    #[test]
    fn rejects_object_without_keyframes() {
        let json = r#"{"name":"x","style":"2d","fps":12,"duration_s":1.0,
            "resolution":{"width":10,"height":10},
            "objects":[{"name":"o","keyframes":[]}]}"#;
        assert!(matches!(
            ShotBrief::from_json_bytes(json.as_bytes()).unwrap_err(),
            KinemaError::InvalidBrief { .. }
        ));
    }

    #[test]
    fn rejects_non_finite_keyframe() {
        // NaN is not representable in strict JSON, so exercise the validator directly.
        let mut brief = ShotBrief::from_json_bytes(sample_json().as_bytes()).unwrap();
        brief.objects[0].keyframes[0].x = f64::NAN;
        assert!(matches!(
            brief.validate().unwrap_err(),
            KinemaError::InvalidBrief { .. }
        ));
    }

    #[test]
    fn malformed_json_is_parse_error() {
        assert!(matches!(
            ShotBrief::from_json_bytes(b"{not json").unwrap_err(),
            KinemaError::Parse { .. }
        ));
    }

    #[test]
    fn style_and_kind_mapping_is_stable() {
        assert_eq!(
            AnimationStyle::classify("2d"),
            AnimationStyle::GreasePencil2D
        );
        assert_eq!(AnimationStyle::classify("3D"), AnimationStyle::ThreeD);
        assert_eq!(AnimationStyle::classify("vector"), AnimationStyle::Vector);
        assert_eq!(
            AnimationStyle::classify("anything"),
            AnimationStyle::MotionGraphics
        );
        assert_eq!(AnimationStyle::GreasePencil2D.preferred_engine(), "blender");
        assert_eq!(AnimationStyle::Vector.preferred_engine(), "synfig");
        assert_eq!(AnimationStyle::MotionGraphics.preferred_engine(), "natron");
        assert_eq!(ObjectKind::classify("box"), ObjectKind::Rect);
        assert_eq!(ObjectKind::classify("actor"), ObjectKind::Character);
        assert_eq!(ObjectKind::classify("blob"), ObjectKind::Circle);
    }

    #[test]
    fn color_over_blends() {
        let fg = Color { r: 255, g: 0, b: 0 };
        let bg = Color { r: 0, g: 0, b: 0 };
        assert_eq!(fg.over(bg, 1.0), fg);
        assert_eq!(fg.over(bg, 0.0), bg);
        let half = fg.over(bg, 0.5);
        assert!((half.r as i32 - 128).abs() <= 1);
    }
}
