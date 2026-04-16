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

    use super::*;
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

    #[test]
    fn freshness_from_system_time_success() {
        let time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let f = Freshness::from_system_time(FreshnessState::Current, time).unwrap();
        assert_eq!(f.state, FreshnessState::Current);
        assert_eq!(f.observed_at_unix_ms, 1_700_000_000_000);
    }

    #[test]
    fn freshness_current_returns_current_state() {
        let f = Freshness::current().unwrap();
        assert_eq!(f.state, FreshnessState::Current);
        assert!(f.observed_at_unix_ms > 0);
    }

    #[test]
    fn freshness_now_is_alias_for_current() {
        let f = Freshness::now().unwrap();
        assert_eq!(f.state, FreshnessState::Current);
    }

    #[test]
    fn freshness_stale_returns_stale_state() {
        let f = Freshness::stale().unwrap();
        assert_eq!(f.state, FreshnessState::Stale);
    }

    #[test]
    fn freshness_observed_passes_through_state() {
        let current = Freshness::observed(FreshnessState::Current).unwrap();
        assert_eq!(current.state, FreshnessState::Current);
        let stale = Freshness::observed(FreshnessState::Stale).unwrap();
        assert_eq!(stale.state, FreshnessState::Stale);
    }

    #[test]
    fn freshness_at_unix_epoch_yields_zero() {
        let f = Freshness::from_system_time(FreshnessState::Current, UNIX_EPOCH).unwrap();
        assert_eq!(f.observed_at_unix_ms, 0);
    }

    // ---- Provenance ----

    #[test]
    fn provenance_new() {
        let p = Provenance::new("test-source", "test-locator");
        assert_eq!(p.source, "test-source");
        assert_eq!(p.locator, "test-locator");
    }

    #[test]
    fn provenance_builtin() {
        let p = Provenance::builtin("identity/manifest.json");
        assert_eq!(p.source, "builtin");
        assert_eq!(p.locator, "identity/manifest.json");
    }

    #[test]
    fn provenance_injected() {
        let p = Provenance::injected("operator-cli");
        assert_eq!(p.source, "injected");
    }

    #[test]
    fn provenance_runtime() {
        let p = Provenance::runtime("session-42");
        assert_eq!(p.source, "runtime");
        assert_eq!(p.locator, "session-42");
    }

    #[test]
    fn provenance_runtime_type_with_detail() {
        let p = Provenance::runtime_type::<String>("detail");
        assert_eq!(p.source, "runtime");
        assert!(p.locator.contains("String"));
        assert!(p.locator.contains("detail"));
    }

    #[test]
    fn provenance_runtime_type_empty_detail() {
        let p = Provenance::runtime_type::<u32>("");
        assert_eq!(p.source, "runtime");
        assert!(p.locator.contains("u32"));
        assert!(!p.locator.contains("::"));
    }

    // ---- BackendDescriptor ----

    #[test]
    fn backend_descriptor_new() {
        let f = Freshness::from_system_time(
            FreshnessState::Current,
            UNIX_EPOCH + Duration::from_secs(100),
        )
        .unwrap();
        let p = Provenance::builtin("test");
        let desc = BackendDescriptor::new("test-backend", p.clone(), f);
        assert_eq!(desc.identity, "test-backend");
        assert_eq!(desc.provenance, p);
        assert_eq!(desc.freshness, f);
    }

    #[test]
    fn backend_descriptor_for_runtime_type() {
        let f = Freshness::current().unwrap();
        let desc = BackendDescriptor::for_runtime_type::<Vec<u8>>("store", "detail", f);
        assert!(desc.provenance.locator.contains("Vec"));
    }
}
