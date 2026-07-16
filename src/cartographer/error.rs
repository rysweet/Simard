//! Error type for the Cartographer data-storytelling pipeline.

use std::fmt::{self, Display, Formatter};

/// Errors produced while turning a dataset + question into a dashboard package.
#[derive(Debug)]
pub enum CartographerError {
    /// A file (brief, dataset, manifest) could not be read or written.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// The study brief or dataset was malformed (unparseable JSON/CSV).
    Parse { context: String, reason: String },
    /// The brief was structurally valid but semantically invalid (e.g. an empty
    /// question, a column hint naming a column the dataset does not contain).
    InvalidBrief { reason: String },
    /// The dataset was empty, too large, or otherwise unusable.
    InvalidDataset { reason: String },
    /// The produced dashboard package failed verification.
    Verification { reason: String },
    /// The static dashboard server could not bind or serve.
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

    pub(crate) fn invalid_dataset(reason: impl Into<String>) -> Self {
        Self::InvalidDataset {
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
            Self::InvalidBrief { reason } => write!(f, "invalid study brief: {reason}"),
            Self::InvalidDataset { reason } => write!(f, "invalid dataset: {reason}"),
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

    #[test]
    fn display_covers_all_variants() {
        let io = CartographerError::io("read brief", std::io::Error::other("boom"));
        assert!(io.to_string().contains("read brief"));
        let parse = CartographerError::parse("brief", "bad json");
        assert!(parse.to_string().contains("bad json"));
        let invalid = CartographerError::invalid_brief("question must not be empty");
        assert!(invalid.to_string().contains("question must not be empty"));
        let dataset = CartographerError::invalid_dataset("no rows");
        assert!(dataset.to_string().contains("no rows"));
        let ver = CartographerError::verification("dashboard empty");
        assert!(ver.to_string().contains("dashboard empty"));
        let serve = CartographerError::serve("port in use");
        assert!(serve.to_string().contains("port in use"));
    }

    #[test]
    fn io_error_exposes_source() {
        use std::error::Error;
        let err = CartographerError::io("ctx", std::io::Error::other("inner"));
        assert!(err.source().is_some());
    }

    #[test]
    fn non_io_error_has_no_source() {
        use std::error::Error;
        let err = CartographerError::verification("x");
        assert!(err.source().is_none());
    }
}
