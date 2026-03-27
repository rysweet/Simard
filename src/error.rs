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
    InvalidConfigValue {
        key: String,
        value: String,
        help: String,
    },
    UnknownIdentity {
        requested: String,
    },
    PromptAssetMissing {
        asset_id: String,
        path: PathBuf,
    },
    PromptAssetRead {
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
    InvalidSessionTransition {
        from: SessionPhase,
        to: SessionPhase,
    },
    StoragePoisoned {
        store: String,
    },
}

pub type SimardResult<T> = Result<T, SimardError>;

impl Display for SimardError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequiredConfig { key, help } => {
                write!(f, "missing required configuration '{key}': {help}")
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
            Self::InvalidSessionTransition { from, to } => {
                write!(f, "invalid session transition from '{from}' to '{to}'")
            }
            Self::StoragePoisoned { store } => {
                write!(f, "storage lock for '{store}' is poisoned")
            }
        }
    }
}

impl Error for SimardError {}
