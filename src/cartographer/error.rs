//! Error type for the Cartographer data-storytelling pipeline.

use std::fmt::{self, Display, Formatter};

/// Errors produced while turning a dataset + question into a dashboard package.
#[derive(Debug)]
pub enum CartographerError {
    /// A file (brief, dataset, artifact) could not be read or written.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// A brief or dataset document was malformed.
    Parse { context: String, reason: String },
    /// The brief was structurally valid but semantically invalid (e.g. an
    /// empty question, a missing dataset path, or an empty dataset).
    InvalidBrief { reason: String },
    /// An optional external delivery tool (streamlit/python) failed to run.
    Tool { tool: String, reason: String },
    /// The produced package failed verification.
    Verification { reason: String },
    /// The static dashboard server failed to bind or serve.
    Serve { reason: String },
}

impl CartographerError {
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

    pub(crate) fn serve(reason: impl Into<String>) -> Self {
        Self::Serve {
            reason: reason.into(),
        }
    }
}

impl Display for CartographerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => {
                write!(f, "cartographer io error ({context}): {source}")
            }
            Self::Parse { context, reason } => {
                write!(f, "cartographer parse error ({context}): {reason}")
            }
            Self::InvalidBrief { reason } => write!(f, "invalid analysis brief: {reason}"),
            Self::Tool { tool, reason } => {
                write!(f, "cartographer tool '{tool}' failed: {reason}")
            }
            Self::Verification { reason } => {
                write!(f, "cartographer package verification failed: {reason}")
            }
            Self::Serve { reason } => write!(f, "cartographer serve error: {reason}"),
        }
    }
}

impl std::error::Error for CartographerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Result alias for the Cartographer pipeline.
pub type CartographerResult<T> = Result<T, CartographerError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn display_covers_all_variants() {
        let io = CartographerError::io("read brief", std::io::Error::other("boom"));
        assert!(io.to_string().contains("read brief"));
        let parse = CartographerError::parse("dataset", "bad csv");
        assert!(parse.to_string().contains("bad csv"));
        let invalid = CartographerError::invalid_brief("question must not be empty");
        assert!(invalid.to_string().contains("question must not be empty"));
        let tool = CartographerError::tool("streamlit", "exit 1");
        assert!(tool.to_string().contains("streamlit"));
        let ver = CartographerError::verification("dashboard empty");
        assert!(ver.to_string().contains("dashboard empty"));
        let serve = CartographerError::serve("port in use");
        assert!(serve.to_string().contains("port in use"));
    }

    #[test]
    fn io_error_exposes_source() {
        let err = CartographerError::io("ctx", std::io::Error::other("inner"));
        assert!(err.source().is_some());
    }

    #[test]
    fn non_io_error_has_no_source() {
        let err = CartographerError::verification("x");
        assert!(err.source().is_none());
    }
}
