use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

use crate::base_types::BaseTypeCapability;
use crate::runtime::{RuntimeState, RuntimeTopology};
use crate::session::SessionPhase;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimardError {
    MissingRequiredConfig {
        key: String,
        help: String,
    },
    NonUnicodeConfigValue {
        key: String,
    },
    InvalidConfigValue {
        key: String,
        value: String,
        help: String,
    },
    UnknownIdentity {
        requested: String,
    },
    InvalidManifestContract {
        field: String,
        reason: String,
    },
    InvalidSessionId {
        value: String,
        reason: String,
    },
    PromptAssetMissing {
        asset_id: String,
        path: PathBuf,
    },
    PromptAssetRead {
        path: PathBuf,
        reason: String,
    },
    InvalidPromptAssetPath {
        asset_id: String,
        path: PathBuf,
        reason: String,
    },
    UnsupportedBaseType {
        identity: String,
        base_type: String,
    },
    AdapterNotRegistered {
        base_type: String,
    },
    MissingCapability {
        base_type: String,
        capability: BaseTypeCapability,
    },
    UnsupportedTopology {
        base_type: String,
        topology: RuntimeTopology,
    },
    InvalidRuntimeTransition {
        from: RuntimeState,
        to: RuntimeState,
    },
    RuntimeStopped {
        action: String,
    },
    InvalidSessionTransition {
        from: SessionPhase,
        to: SessionPhase,
    },
    StoragePoisoned {
        store: String,
    },
    ClockBeforeUnixEpoch {
        reason: String,
    },
}

pub type SimardResult<T> = Result<T, SimardError>;

impl Display for SimardError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequiredConfig { key, help } => {
                write!(f, "missing required configuration '{key}': {help}")
            }
            Self::NonUnicodeConfigValue { key } => {
                write!(f, "configuration '{key}' must be valid UTF-8")
            }
            Self::InvalidConfigValue { key, value, help } => {
                write!(
                    f,
                    "invalid value '{value}' for configuration '{key}': {help}"
                )
            }
            Self::UnknownIdentity { requested } => {
                write!(f, "identity '{requested}' is not registered")
            }
            Self::InvalidManifestContract { field, reason } => {
                write!(f, "invalid manifest contract field '{field}': {reason}")
            }
            Self::InvalidSessionId { value, reason } => {
                write!(f, "invalid session id '{value}': {reason}")
            }
            Self::PromptAssetMissing { asset_id, path } => {
                write!(
                    f,
                    "prompt asset '{asset_id}' was not found at {}",
                    path.display()
                )
            }
            Self::PromptAssetRead { path, reason } => {
                write!(
                    f,
                    "failed to read prompt asset {}: {reason}",
                    path.display()
                )
            }
            Self::InvalidPromptAssetPath {
                asset_id,
                path,
                reason,
            } => write!(
                f,
                "invalid prompt asset path for '{asset_id}' at {}: {reason}",
                path.display()
            ),
            Self::UnsupportedBaseType {
                identity,
                base_type,
            } => write!(
                f,
                "identity '{identity}' does not allow base type '{base_type}'"
            ),
            Self::AdapterNotRegistered { base_type } => {
                write!(f, "no adapter is registered for base type '{base_type}'")
            }
            Self::MissingCapability {
                base_type,
                capability,
            } => write!(
                f,
                "base type '{base_type}' does not provide required capability '{capability}'"
            ),
            Self::UnsupportedTopology {
                base_type,
                topology,
            } => write!(
                f,
                "base type '{base_type}' does not support topology '{topology}'"
            ),
            Self::InvalidRuntimeTransition { from, to } => {
                write!(f, "invalid runtime transition from '{from}' to '{to}'")
            }
            Self::RuntimeStopped { action } => {
                write!(f, "runtime is stopped and cannot '{action}'")
            }
            Self::InvalidSessionTransition { from, to } => {
                write!(f, "invalid session transition from '{from}' to '{to}'")
            }
            Self::StoragePoisoned { store } => {
                write!(f, "storage lock for '{store}' is poisoned")
            }
            Self::ClockBeforeUnixEpoch { reason } => {
                write!(f, "system clock is before UNIX epoch: {reason}")
            }
        }
    }
}

impl Error for SimardError {}
