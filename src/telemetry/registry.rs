//! Bounded, thread-safe, in-process metrics registry — the source of truth read
//! by [`crate::status`] with no external collector.
//!
//! The facade functions ([`counter_add`], [`gauge_set`], [`histogram_record`])
//! dual-write here and (once wired in `init_tracing`) into OpenTelemetry
//! instruments. This module keeps only the in-process side: an atomic registry
//! whose current values are snapshotted by [`capture`].
//!
//! ## Bounds (a bug must never grow this without limit)
//! - Attribute values are normalized: capped to [`MAX_ATTR_VALUE_LEN`] and
//!   stripped of control characters.
//! - Cardinality per `(metric, attribute-key)` is capped at
//!   [`MAX_VALUES_PER_KEY`]; a value beyond the cap is folded into the
//!   [`names::OTHER_BUCKET`] series and increments the overflow counter.
//! - The total number of distinct series is capped at [`MAX_SERIES`].

use std::collections::{BTreeSet, HashMap};
use std::sync::{LazyLock, Mutex};

use super::names;
use super::snapshot::{
    CounterSeries, GaugeSeries, HistogramBucket, HistogramSeries, HistogramValue, MetricsSnapshot,
    SCHEMA_VERSION, now_rfc3339,
};

/// Maximum retained length of a single attribute value.
pub const MAX_ATTR_VALUE_LEN: usize = 64;
/// Maximum distinct attribute values retained per `(metric, attribute-key)`
/// before further values fold into `other`.
pub const MAX_VALUES_PER_KEY: usize = 16;
/// Global cap on the number of distinct series the registry will hold.
pub const MAX_SERIES: usize = 4096;

/// Default histogram bucket boundaries used for every histogram series (the
/// only histogram today is the OODA cycle duration).
const DEFAULT_BUCKETS: &[f64] = names::DAEMON_CYCLE_DURATION_BUCKETS;

type Attrs = Vec<(String, String)>;
type SeriesKey = (String, Attrs);

#[derive(Default)]
struct HistogramAccum {
    count: u64,
    sum: f64,
    /// Per-bucket tallies aligned to `DEFAULT_BUCKETS`, with one trailing
    /// `+Inf` bucket.
    bucket_counts: Vec<u64>,
}

#[derive(Default)]
struct Registry {
    counters: HashMap<SeriesKey, u64>,
    gauges: HashMap<SeriesKey, i64>,
    histograms: HashMap<SeriesKey, HistogramAccum>,
    /// Distinct values seen per `(metric, attribute-key)`, for the cardinality
    /// bound.
    value_sets: HashMap<(String, String), BTreeSet<String>>,
    overflow_series: u64,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

fn lock() -> std::sync::MutexGuard<'static, Registry> {
    // A poisoned lock only means a prior test panicked mid-write; the registry
    // is still structurally valid, so recover rather than propagate the panic
    // into unrelated telemetry callers on a hot path.
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Add `value` to counter `name` for the given attribute set.
///
/// Dual-writes: the in-process registry (below) plus the OTel counter of the
/// same name (a no-op until [`super::otel::init`] runs).
pub fn counter_add(name: &str, value: u64, attrs: &[(&str, &str)]) {
    super::otel::record_counter(name, value, attrs);
    let mut reg = lock();
    let key = intern_key(&mut reg, name, attrs);
    let exists = reg.counters.contains_key(&key);
    if !admit_series(&mut reg, exists) {
        return;
    }
    let slot = reg.counters.entry(key).or_insert(0);
    *slot = slot.saturating_add(value);
}

/// Set gauge `name` to `value` for the given attribute set (last write wins).
///
/// Dual-writes: the in-process registry plus the OTel gauge of the same name.
pub fn gauge_set(name: &str, value: i64, attrs: &[(&str, &str)]) {
    super::otel::record_gauge(name, value, attrs);
    let mut reg = lock();
    let key = intern_key(&mut reg, name, attrs);
    let exists = reg.gauges.contains_key(&key);
    if !admit_series(&mut reg, exists) {
        return;
    }
    reg.gauges.insert(key, value);
}

/// Record one observation `value` into histogram `name` for the attribute set.
///
/// Dual-writes: the in-process registry plus the OTel histogram of the same
/// name.
pub fn histogram_record(name: &str, value: f64, attrs: &[(&str, &str)]) {
    super::otel::record_histogram(name, value, attrs);
    let mut reg = lock();
    let key = intern_key(&mut reg, name, attrs);
    let exists = reg.histograms.contains_key(&key);
    if !admit_series(&mut reg, exists) {
        return;
    }
    let accum = reg.histograms.entry(key).or_insert_with(|| HistogramAccum {
        count: 0,
        sum: 0.0,
        bucket_counts: vec![0; DEFAULT_BUCKETS.len() + 1],
    });
    accum.count = accum.count.saturating_add(1);
    accum.sum += value;
    let idx = DEFAULT_BUCKETS
        .iter()
        .position(|&b| value <= b)
        .unwrap_or(DEFAULT_BUCKETS.len());
    accum.bucket_counts[idx] = accum.bucket_counts[idx].saturating_add(1);
}

/// Snapshot the current registry into a serializable [`MetricsSnapshot`].
pub fn capture() -> MetricsSnapshot {
    let reg = lock();

    let mut counters: Vec<CounterSeries> = reg
        .counters
        .iter()
        .map(|((name, attrs), value)| CounterSeries {
            name: name.clone(),
            attrs: attrs.clone(),
            value: *value,
        })
        .collect();
    counters.sort_by(|a, b| (a.name.as_str(), &a.attrs).cmp(&(b.name.as_str(), &b.attrs)));

    let mut gauges: Vec<GaugeSeries> = reg
        .gauges
        .iter()
        .map(|((name, attrs), value)| GaugeSeries {
            name: name.clone(),
            attrs: attrs.clone(),
            value: *value,
        })
        .collect();
    gauges.sort_by(|a, b| (a.name.as_str(), &a.attrs).cmp(&(b.name.as_str(), &b.attrs)));

    let mut histograms: Vec<HistogramSeries> = reg
        .histograms
        .iter()
        .map(|((name, attrs), accum)| HistogramSeries {
            name: name.clone(),
            attrs: attrs.clone(),
            value: accum.to_value(),
        })
        .collect();
    histograms.sort_by(|a, b| (a.name.as_str(), &a.attrs).cmp(&(b.name.as_str(), &b.attrs)));

    MetricsSnapshot {
        schema_version: SCHEMA_VERSION,
        captured_at: now_rfc3339(),
        counters,
        gauges,
        histograms,
        overflow_series: reg.overflow_series,
        enrichment: None,
    }
}

/// Number of attribute values folded into `other` by the cardinality bound.
pub fn overflow_count() -> u64 {
    lock().overflow_series
}

/// Clear all series. Intended for test isolation; the daemon never calls it.
pub fn reset() {
    let mut reg = lock();
    *reg = Registry::default();
}

impl HistogramAccum {
    fn to_value(&self) -> HistogramValue {
        let mut cumulative = 0u64;
        let mut buckets = Vec::with_capacity(DEFAULT_BUCKETS.len());
        for (i, &le) in DEFAULT_BUCKETS.iter().enumerate() {
            cumulative = cumulative.saturating_add(self.bucket_counts[i]);
            buckets.push(HistogramBucket {
                le,
                count: cumulative,
            });
        }
        HistogramValue {
            count: self.count,
            sum: self.sum,
            buckets,
        }
    }
}

/// Admission control for the global series cap. `key_exists` says whether the
/// series is already tracked. Updates to an existing series are always admitted;
/// a *new* series that would exceed [`MAX_SERIES`] is rejected — this records an
/// overflow and returns `false`, so callers drop the write. Centralizing the cap
/// here keeps the "a bug must never grow the registry without limit" invariant in
/// one place across counters, gauges, and histograms.
fn admit_series(reg: &mut Registry, key_exists: bool) -> bool {
    if key_exists {
        return true;
    }
    if reg.counters.len() + reg.gauges.len() + reg.histograms.len() >= MAX_SERIES {
        reg.overflow_series = reg.overflow_series.saturating_add(1);
        return false;
    }
    true
}

/// Normalize + cardinality-bound the attribute set and return the interned
/// series key. Mutates the registry's overflow counter and per-key value sets.
fn intern_key(reg: &mut Registry, name: &str, attrs: &[(&str, &str)]) -> SeriesKey {
    let mut bounded: Attrs = Vec::with_capacity(attrs.len());
    for (k, v) in attrs {
        let key = normalize_value(k);
        let value = normalize_value(v);
        let final_value = bound_cardinality(reg, name, &key, value);
        bounded.push((key, final_value));
    }
    bounded.sort();
    (name.to_string(), bounded)
}

/// Fold an out-of-catalog value into `other` once the per-key cardinality cap
/// is reached, bumping the overflow counter.
fn bound_cardinality(reg: &mut Registry, name: &str, attr_key: &str, value: String) -> String {
    if value == names::OTHER_BUCKET {
        return value;
    }
    let set = reg
        .value_sets
        .entry((name.to_string(), attr_key.to_string()))
        .or_default();
    if set.contains(&value) {
        return value;
    }
    if set.len() < MAX_VALUES_PER_KEY {
        set.insert(value.clone());
        return value;
    }
    reg.overflow_series = reg.overflow_series.saturating_add(1);
    names::OTHER_BUCKET.to_string()
}

/// Cap length and strip control characters from an attribute key or value.
fn normalize_value(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if cleaned.chars().count() <= MAX_ATTR_VALUE_LEN {
        cleaned
    } else {
        cleaned.chars().take(MAX_ATTR_VALUE_LEN).collect()
    }
}
