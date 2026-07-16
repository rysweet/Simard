//! Self-contained error type for the Atelier fabrication engine.
//!
//! Kept local (rather than a variant on the crate-wide `SimardError`) so the
//! Atelier identity stays a focused, regeneratable brick: it owns its own
//! failure surface and does not widen the exhaustive core error enum.

use std::fmt::{self, Display, Formatter};

/// Failure raised while turning a product brief into fabrication artifacts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AtelierError {
    /// The brief JSON could not be parsed.
    BriefParse { reason: String },
    /// The brief parsed but violated a design invariant (e.g. non-positive
    /// dimension, panel thicker than the piece).
    InvalidBrief { field: String, reason: String },
    /// An unknown product kind was requested.
    UnknownKind { requested: String },
    /// A filesystem operation failed while writing artifacts.
    Io { path: String, reason: String },
    /// An external fabrication tool (OpenSCAD/FreeCAD/Blender) failed while
    /// producing an artifact. Missing tools are NOT an error — they are skipped
    /// and recorded; this is only for a tool that ran and returned failure.
    ToolFailed { tool: String, reason: String },
}

impl Display for AtelierError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::BriefParse { reason } => {
                write!(f, "atelier: could not parse product brief: {reason}")
            }
            Self::InvalidBrief { field, reason } => {
                write!(f, "atelier: invalid brief field '{field}': {reason}")
            }
            Self::UnknownKind { requested } => {
                write!(
                    f,
                    "atelier: unknown product kind '{requested}' (expected table | shelf | box)"
                )
            }
            Self::Io { path, reason } => {
                write!(f, "atelier: io error at '{path}': {reason}")
            }
            Self::ToolFailed { tool, reason } => {
                write!(f, "atelier: tool '{tool}' failed: {reason}")
            }
        }
    }
}

impl std::error::Error for AtelierError {}

/// Convenience alias for Atelier results.
pub type AtelierResult<T> = Result<T, AtelierError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_covers_all_variants() {
        let cases = [
            AtelierError::BriefParse {
                reason: "bad json".into(),
            },
            AtelierError::InvalidBrief {
                field: "width_mm".into(),
                reason: "must be > 0".into(),
            },
            AtelierError::UnknownKind {
                requested: "spaceship".into(),
            },
            AtelierError::Io {
                path: "/tmp/x".into(),
                reason: "denied".into(),
            },
            AtelierError::ToolFailed {
                tool: "openscad".into(),
                reason: "exit 1".into(),
            },
        ];
        for case in cases {
            let msg = case.to_string();
            assert!(msg.starts_with("atelier:"), "message was: {msg}");
        }
    }

    #[test]
    fn error_is_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&AtelierError::UnknownKind {
            requested: "x".into(),
        });
    }
}
