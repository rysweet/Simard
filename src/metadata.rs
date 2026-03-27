use std::any::type_name;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{SimardError, SimardResult};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Provenance {
    pub source: String,
    pub locator: String,
}

impl Provenance {
    pub fn new(source: impl Into<String>, locator: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            locator: locator.into(),
        }
    }

    pub fn builtin(locator: impl Into<String>) -> Self {
        Self::new("builtin", locator)
    }

    pub fn injected(locator: impl Into<String>) -> Self {
        Self::new("injected", locator)
    }

    pub fn runtime(locator: impl Into<String>) -> Self {
        Self::new("runtime", locator)
    }

    pub fn runtime_type<T>(detail: impl AsRef<str>) -> Self {
        let detail = detail.as_ref().trim();
        let locator = if detail.is_empty() {
            type_name::<T>().to_string()
        } else {
            format!("{}::{detail}", type_name::<T>())
        };
        Self::runtime(locator)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshnessState {
    Current,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Freshness {
    pub state: FreshnessState,
    pub observed_at_unix_ms: u64,
}

impl Freshness {
    pub fn current() -> SimardResult<Self> {
        Self::from_system_time(FreshnessState::Current, SystemTime::now())
    }

    pub fn now() -> SimardResult<Self> {
        Self::current()
    }

    pub fn stale() -> SimardResult<Self> {
        Self::from_system_time(FreshnessState::Stale, SystemTime::now())
    }

    pub fn observed(state: FreshnessState) -> SimardResult<Self> {
        Self::from_system_time(state, SystemTime::now())
    }

    pub fn from_system_time(state: FreshnessState, observed_at: SystemTime) -> SimardResult<Self> {
        let duration = observed_at.duration_since(UNIX_EPOCH).map_err(|error| {
            SimardError::ClockBeforeUnixEpoch {
                reason: error.to_string(),
            }
        })?;
        let observed_at_unix_ms = duration.as_millis().min(u128::from(u64::MAX)) as u64;
        Ok(Self {
            state,
            observed_at_unix_ms,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendDescriptor {
    pub identity: String,
    pub provenance: Provenance,
    pub freshness: Freshness,
}

impl BackendDescriptor {
    pub fn new(identity: impl Into<String>, provenance: Provenance, freshness: Freshness) -> Self {
        Self {
            identity: identity.into(),
            provenance,
            freshness,
        }
    }

    pub fn for_runtime_type<T>(
        identity: impl Into<String>,
        detail: impl AsRef<str>,
        freshness: Freshness,
    ) -> Self {
        Self::new(identity, Provenance::runtime_type::<T>(detail), freshness)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Freshness, FreshnessState, UNIX_EPOCH};
    use crate::error::SimardError;

    #[test]
    fn freshness_rejects_times_before_unix_epoch() {
        let error = Freshness::from_system_time(
            FreshnessState::Current,
            UNIX_EPOCH - Duration::from_millis(1),
        )
        .expect_err("times before the unix epoch should fail");

        assert!(matches!(error, SimardError::ClockBeforeUnixEpoch { .. }));
    }
}
