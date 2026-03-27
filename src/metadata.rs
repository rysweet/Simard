use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Freshness {
    pub observed_at_unix_ms: u64,
}

impl Freshness {
    pub fn now() -> Self {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        let observed_at_unix_ms = duration.as_millis().min(u128::from(u64::MAX)) as u64;
        Self {
            observed_at_unix_ms,
        }
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
}
