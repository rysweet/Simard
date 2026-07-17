//! Error type for the Kinema animation pipeline.

use std::fmt::{self, Display, Formatter};

/// Errors produced while turning a shot brief into a rendered animated sequence.
#[derive(Debug)]
pub enum KinemaError {
    /// The brief file could not be read or an artifact could not be written.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// The brief JSON was malformed.
    Parse { context: String, reason: String },
    /// The brief was structurally valid JSON but semantically invalid
    /// (e.g. a non-positive duration, an empty resolution, no objects to draw).
    InvalidBrief { reason: String },
    /// An external animation tool (blender/synfig/natron) failed to run.
    Tool { tool: String, reason: String },
    /// The produced sequence failed verification.
    Verification { reason: String },
}

impl KinemaError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub(crate) fn parse(context: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Parse {
            context: context.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn invalid_brief(reason: impl Into<String>) -> Self {
        Self::InvalidBrief {
            reason: reason.into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn tool(tool: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Tool {
            tool: tool.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn verification(reason: impl Into<String>) -> Self {
        Self::Verification {
            reason: reason.into(),
        }
    }
}

impl Display for KinemaError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "kinema io error ({context}): {source}"),
            Self::Parse { context, reason } => {
                write!(f, "kinema brief parse error ({context}): {reason}")
            }
            Self::InvalidBrief { reason } => write!(f, "invalid shot brief: {reason}"),
            Self::Tool { tool, reason } => write!(f, "kinema tool '{tool}' failed: {reason}"),
            Self::Verification { reason } => {
                write!(f, "kinema sequence verification failed: {reason}")
            }
        }
    }
}

impl std::error::Error for KinemaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Result alias for the Kinema pipeline.
pub type KinemaResult<T> = Result<T, KinemaError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_covers_all_variants() {
        let io = KinemaError::io("read brief", std::io::Error::other("boom"));
        assert!(io.to_string().contains("read brief"));
        let parse = KinemaError::parse("brief", "bad json");
        assert!(parse.to_string().contains("bad json"));
        let invalid = KinemaError::invalid_brief("duration must be > 0");
        assert!(invalid.to_string().contains("duration must be > 0"));
        let tool = KinemaError::tool("blender", "exit 1");
        assert!(tool.to_string().contains("blender"));
        let ver = KinemaError::verification("no frames");
        assert!(ver.to_string().contains("no frames"));
    }

    #[test]
    fn io_error_exposes_source() {
        use std::error::Error;
        let err = KinemaError::io("ctx", std::io::Error::other("inner"));
        assert!(err.source().is_some());
    }

    #[test]
    fn non_io_error_has_no_source() {
        use std::error::Error;
        let err = KinemaError::verification("x");
        assert!(err.source().is_none());
    }
}
