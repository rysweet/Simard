//! Error type for the Atelier design pipeline.

use std::fmt::{self, Display, Formatter};

/// Errors produced while turning a product brief into a fabrication package.
#[derive(Debug)]
pub enum AtelierError {
    /// The brief file could not be read or written to disk.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// The brief JSON was malformed.
    Parse { context: String, reason: String },
    /// The brief was structurally valid JSON but physically/semantically
    /// invalid (e.g. a negative dimension, thickness thicker than the panel).
    InvalidBrief { reason: String },
    /// An external CAD tool (openscad/freecad/blender) failed to run.
    Tool { tool: String, reason: String },
    /// The produced package failed verification.
    Verification { reason: String },
}

impl AtelierError {
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

impl Display for AtelierError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "atelier io error ({context}): {source}"),
            Self::Parse { context, reason } => {
                write!(f, "atelier brief parse error ({context}): {reason}")
            }
            Self::InvalidBrief { reason } => write!(f, "invalid product brief: {reason}"),
            Self::Tool { tool, reason } => write!(f, "atelier tool '{tool}' failed: {reason}"),
            Self::Verification { reason } => {
                write!(f, "atelier package verification failed: {reason}")
            }
        }
    }
}

impl std::error::Error for AtelierError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Result alias for the Atelier pipeline.
pub type AtelierResult<T> = Result<T, AtelierError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_covers_all_variants() {
        let io = AtelierError::io("read brief", std::io::Error::other("boom"));
        assert!(io.to_string().contains("read brief"));
        let parse = AtelierError::parse("brief", "bad json");
        assert!(parse.to_string().contains("bad json"));
        let invalid = AtelierError::invalid_brief("width must be > 0");
        assert!(invalid.to_string().contains("width must be > 0"));
        let tool = AtelierError::tool("openscad", "exit 1");
        assert!(tool.to_string().contains("openscad"));
        let ver = AtelierError::verification("stl empty");
        assert!(ver.to_string().contains("stl empty"));
    }

    #[test]
    fn io_error_exposes_source() {
        use std::error::Error;
        let err = AtelierError::io("ctx", std::io::Error::other("inner"));
        assert!(err.source().is_some());
    }

    #[test]
    fn non_io_error_has_no_source() {
        use std::error::Error;
        let err = AtelierError::verification("x");
        assert!(err.source().is_none());
    }
}
