//! Error type for the Atelier design pipeline.

use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

/// Errors produced while turning a product brief into fabrication artifacts.
#[derive(Debug)]
pub enum AtelierError {
    /// The product brief JSON could not be parsed.
    BriefParse { reason: String },
    /// The product brief was structurally valid JSON but semantically invalid.
    InvalidBrief { field: String, reason: String },
    /// A filesystem operation failed while reading or writing artifacts.
    Io { path: PathBuf, reason: String },
}

impl AtelierError {
    pub(crate) fn io(path: impl Into<PathBuf>, err: &std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            reason: err.to_string(),
        }
    }

    pub(crate) fn invalid(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidBrief {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

impl Display for AtelierError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::BriefParse { reason } => {
                write!(f, "failed to parse product brief: {reason}")
            }
            Self::InvalidBrief { field, reason } => {
                write!(f, "invalid product brief field '{field}': {reason}")
            }
            Self::Io { path, reason } => {
                write!(f, "io error at {}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for AtelierError {}

/// Convenience result alias for the Atelier pipeline.
pub type AtelierResult<T> = Result<T, AtelierError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_covers_all_variants() {
        let e = AtelierError::BriefParse {
            reason: "eof".into(),
        };
        assert!(e.to_string().contains("failed to parse product brief"));

        let e = AtelierError::invalid("length_mm", "must be positive");
        assert!(e.to_string().contains("length_mm"));
        assert!(e.to_string().contains("must be positive"));

        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        let e = AtelierError::io("/tmp/x", &io);
        assert!(e.to_string().contains("/tmp/x"));
        assert!(e.to_string().contains("nope"));
    }

    #[test]
    fn error_is_std_error() {
        fn assert_err<E: std::error::Error>(_: &E) {}
        assert_err(&AtelierError::BriefParse { reason: "x".into() });
    }
}
