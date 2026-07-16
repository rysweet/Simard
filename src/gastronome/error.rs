//! Error type for the Gastronome menu/event-design pipeline.

use std::fmt::{self, Display, Formatter};

/// Errors produced while turning a menu/event brief into a costed, scheduled
/// menu plan.
#[derive(Debug)]
pub enum GastronomeError {
    /// The brief file could not be read or a package artifact written to disk.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// The brief JSON was malformed.
    Parse { context: String, reason: String },
    /// The brief was structurally valid JSON but semantically invalid
    /// (e.g. zero guests, a dish with no ingredients, a negative quantity).
    InvalidBrief { reason: String },
    /// The produced menu plan failed verification.
    Verification { reason: String },
}

impl GastronomeError {
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

    pub(crate) fn verification(reason: impl Into<String>) -> Self {
        Self::Verification {
            reason: reason.into(),
        }
    }
}

impl Display for GastronomeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "gastronome io error ({context}): {source}"),
            Self::Parse { context, reason } => {
                write!(f, "gastronome brief parse error ({context}): {reason}")
            }
            Self::InvalidBrief { reason } => write!(f, "invalid menu brief: {reason}"),
            Self::Verification { reason } => {
                write!(f, "gastronome menu plan verification failed: {reason}")
            }
        }
    }
}

impl std::error::Error for GastronomeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Result alias for the Gastronome pipeline.
pub type GastronomeResult<T> = Result<T, GastronomeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_covers_all_variants() {
        let io = GastronomeError::io("read brief", std::io::Error::other("boom"));
        assert!(io.to_string().contains("read brief"));
        let parse = GastronomeError::parse("brief", "bad json");
        assert!(parse.to_string().contains("bad json"));
        let invalid = GastronomeError::invalid_brief("guests must be > 0");
        assert!(invalid.to_string().contains("guests must be > 0"));
        let ver = GastronomeError::verification("shopping list empty");
        assert!(ver.to_string().contains("shopping list empty"));
    }

    #[test]
    fn io_error_exposes_source() {
        use std::error::Error;
        let err = GastronomeError::io("ctx", std::io::Error::other("inner"));
        assert!(err.source().is_some());
    }

    #[test]
    fn non_io_error_has_no_source() {
        use std::error::Error;
        let err = GastronomeError::verification("x");
        assert!(err.source().is_none());
    }
}
