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
    UnsupportedMemoryPolicy {
        field: String,
        reason: String,
    },
    UnsupportedBaseType {
        identity: String,
        base_type: String,
    },
    AdapterNotRegistered {
        base_type: String,
    },
    AdapterInvocationFailed {
        base_type: String,
        reason: String,
    },
    InvalidBaseTypeSessionState {
        base_type: String,
        action: String,
        reason: String,
    },
    MissingCapability {
        base_type: String,
        capability: BaseTypeCapability,
    },
    UnsupportedTopology {
        base_type: String,
        topology: RuntimeTopology,
    },
    UnsupportedRuntimeTopology {
        topology: RuntimeTopology,
        driver: String,
    },
    InvalidRuntimeTransition {
        from: RuntimeState,
        to: RuntimeState,
    },
    RuntimeStopped {
        action: String,
    },
    RuntimeFailed {
        action: String,
    },
    InvalidSessionTransition {
        from: SessionPhase,
        to: SessionPhase,
    },
    InvalidHandoffSnapshot {
        field: String,
        reason: String,
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
            Self::InvalidConfigValue {
                key,
                value: _,
                help,
            } => {
                write!(f, "invalid value for configuration '{key}': {help}")
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
            Self::PromptAssetMissing { asset_id, path: _ } => {
                write!(
                    f,
                    "prompt asset '{asset_id}' was not found under the configured prompt root"
                )
            }
            Self::PromptAssetRead { path: _, reason } => {
                write!(f, "failed to read configured prompt asset: {reason}")
            }
            Self::InvalidPromptAssetPath {
                asset_id,
                path: _,
                reason,
            } => write!(f, "invalid prompt asset path for '{asset_id}': {reason}"),
            Self::UnsupportedMemoryPolicy { field, reason } => {
                write!(f, "unsupported memory policy '{field}': {reason}")
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
            Self::AdapterInvocationFailed { base_type, reason } => {
                write!(
                    f,
                    "base type '{base_type}' failed during invocation: {reason}"
                )
            }
            Self::InvalidBaseTypeSessionState {
                base_type,
                action,
                reason,
            } => write!(
                f,
                "base type session '{base_type}' cannot '{action}': {reason}"
            ),
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
            Self::UnsupportedRuntimeTopology { topology, driver } => write!(
                f,
                "runtime topology driver '{driver}' does not support topology '{topology}'"
            ),
            Self::InvalidRuntimeTransition { from, to } => {
                write!(f, "invalid runtime transition from '{from}' to '{to}'")
            }
            Self::RuntimeStopped { action } => {
                write!(f, "runtime is stopped and cannot '{action}'")
            }
            Self::RuntimeFailed { action } => {
                write!(
                    f,
                    "runtime is failed and cannot '{action}' until it is stopped"
                )
            }
            Self::InvalidSessionTransition { from, to } => {
                write!(f, "invalid session transition from '{from}' to '{to}'")
            }
            Self::InvalidHandoffSnapshot { field, reason } => {
                write!(f, "invalid handoff snapshot field '{field}': {reason}")
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
