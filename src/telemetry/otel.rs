//! OpenTelemetry metrics adapter for the unified telemetry facade.
//!
//! The facade ([`super::counter_add`] / [`super::gauge_set`] /
//! [`super::histogram_record`]) dual-writes: always into the in-process
//! [`super::registry`] (the source of truth read by [`crate::status`]) and,
//! through this module, into OpenTelemetry instruments on an
//! [`SdkMeterProvider`].
//!
//! ## Export is endpoint-gated, the provider is always installed
//! [`init`] always installs an `SdkMeterProvider` as the global meter provider
//! (so instruments are real, not the global no-op). An OTLP `PeriodicReader` is
//! attached **only** when `OTEL_EXPORTER_OTLP_ENDPOINT` is set — identical to the
//! tracer. With no endpoint (the production default) nothing leaves the process:
//! the provider has no reader and the in-process registry is the only consumer.
//!
//! Instruments are created lazily and cached by name. Before [`init`] runs (e.g.
//! in unit tests that never configure telemetry) the global meter is the SDK
//! no-op, so recording is a cheap no-op and the in-process registry is
//! unaffected.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry_sdk::metrics::SdkMeterProvider;

use super::names;

/// The installed provider, retained so [`shutdown`] can flush it on exit.
static PROVIDER: OnceLock<SdkMeterProvider> = OnceLock::new();

/// Cached instruments, keyed by metric name. OTel instruments are `Arc`-backed
/// (cheap to clone, `Send + Sync`); caching avoids recreating them on the hot
/// path.
static COUNTERS: LazyLock<Mutex<HashMap<String, Counter<u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static GAUGES: LazyLock<Mutex<HashMap<String, Gauge<i64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static HISTOGRAMS: LazyLock<Mutex<HashMap<String, Histogram<f64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn meter() -> Meter {
    opentelemetry::global::meter("simard")
}

/// Install the global `SdkMeterProvider`. OTLP export is attached only when
/// `endpoint` is `Some` (the same gate as the tracer); otherwise the provider
/// has no reader and metrics stay entirely in-process.
///
/// Idempotent: a second call is a no-op (the first-installed provider wins).
pub fn init(endpoint: Option<&str>) {
    if PROVIDER.get().is_some() {
        return;
    }

    use opentelemetry::KeyValue as KV;
    use opentelemetry_sdk::Resource;

    let resource = Resource::new(vec![
        KV::new("service.name", "simard"),
        KV::new("service.version", env!("CARGO_PKG_VERSION")),
    ]);

    let mut builder = SdkMeterProvider::builder().with_resource(resource);

    if let Some(ep) = endpoint {
        match build_otlp_reader(ep) {
            Ok(reader) => builder = builder.with_reader(reader),
            Err(e) => eprintln!("[simard] OTEL metrics exporter init failed: {e}"),
        }
    }

    let provider = builder.build();
    opentelemetry::global::set_meter_provider(provider.clone());
    let _ = PROVIDER.set(provider);
}

/// Flush and shut down the meter provider so pending export is not lost on exit.
/// Safe to call when [`init`] was never invoked.
pub fn shutdown() {
    if let Some(provider) = PROVIDER.get() {
        let _ = provider.shutdown();
    }
}

fn build_otlp_reader(
    endpoint: &str,
) -> Result<opentelemetry_sdk::metrics::PeriodicReader, Box<dyn std::error::Error + Send + Sync>> {
    use opentelemetry_otlp::{MetricExporter, WithExportConfig};
    use opentelemetry_sdk::metrics::PeriodicReader;

    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    Ok(PeriodicReader::builder(exporter, opentelemetry_sdk::runtime::Tokio).build())
}

fn to_kv(attrs: &[(&str, &str)]) -> Vec<KeyValue> {
    attrs
        .iter()
        .map(|(k, v)| KeyValue::new(k.to_string(), v.to_string()))
        .collect()
}

/// Mirror a counter add into the OTel counter of the same name.
///
/// The cache is keyed by name; a hit (the steady state, since the metric
/// catalog is small and fixed) does a borrowed lookup and allocates no key,
/// so only the first sighting of each name pays for interning.
pub(super) fn record_counter(name: &str, value: u64, attrs: &[(&str, &str)]) {
    let mut cache = COUNTERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(counter) = cache.get(name) {
        counter.add(value, &to_kv(attrs));
        return;
    }
    let counter = meter().u64_counter(name.to_string()).build();
    counter.add(value, &to_kv(attrs));
    cache.insert(name.to_string(), counter);
}

/// Mirror a gauge set into the OTel gauge of the same name.
pub(super) fn record_gauge(name: &str, value: i64, attrs: &[(&str, &str)]) {
    let mut cache = GAUGES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(gauge) = cache.get(name) {
        gauge.record(value, &to_kv(attrs));
        return;
    }
    let gauge = meter().i64_gauge(name.to_string()).build();
    gauge.record(value, &to_kv(attrs));
    cache.insert(name.to_string(), gauge);
}

/// Mirror a histogram observation into the OTel histogram of the same name.
///
/// All histograms use the shared [`names::DAEMON_CYCLE_DURATION_BUCKETS`]
/// boundaries — the only histogram today is the OODA cycle duration.
pub(super) fn record_histogram(name: &str, value: f64, attrs: &[(&str, &str)]) {
    let mut cache = HISTOGRAMS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hist) = cache.get(name) {
        hist.record(value, &to_kv(attrs));
        return;
    }
    let hist = meter()
        .f64_histogram(name.to_string())
        .with_boundaries(names::DAEMON_CYCLE_DURATION_BUCKETS.to_vec())
        .build();
    hist.record(value, &to_kv(attrs));
    cache.insert(name.to_string(), hist);
}
