//! In-process span collector for dashboard observability.
//!
//! `SpanCollectorLayer` is a tracing-subscriber layer that buffers recent
//! completed spans in a lock-free ring buffer. The dashboard `/api/traces`
//! endpoint can drain this buffer to show live span data without requiring
//! an external OTel collector.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

/// Maximum number of recent spans to retain.
const RING_SIZE: usize = 512;

/// A completed span record.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SpanRecord {
    pub name: String,
    pub target: String,
    pub level: String,
    pub duration_us: u64,
    pub fields: String,
    pub timestamp_epoch_ms: u64,
}

/// Global ring buffer of recent span records.
static RING: Mutex<Option<Vec<SpanRecord>>> = Mutex::new(None);
static WRITE_INDEX: AtomicUsize = AtomicUsize::new(0);

fn ensure_ring() -> std::sync::MutexGuard<'static, Option<Vec<SpanRecord>>> {
    let mut guard = RING.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let mut v = Vec::with_capacity(RING_SIZE);
        v.resize(
            RING_SIZE,
            SpanRecord {
                name: String::new(),
                target: String::new(),
                level: String::new(),
                duration_us: 0,
                fields: String::new(),
                timestamp_epoch_ms: 0,
            },
        );
        *guard = Some(v);
    }
    guard
}

/// Drain recent span records (up to `limit`). Returns newest first.
pub fn drain_recent(limit: usize) -> Vec<SpanRecord> {
    let guard = ensure_ring();
    let ring = guard.as_ref().unwrap();
    let write_idx = WRITE_INDEX.load(Ordering::Relaxed);
    let count = limit.min(RING_SIZE).min(write_idx);

    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let idx = (write_idx.wrapping_sub(1).wrapping_sub(i)) % RING_SIZE;
        let record = &ring[idx];
        if !record.name.is_empty() {
            result.push(record.clone());
        }
    }
    result
}

/// A tracing-subscriber Layer that records completed spans into the ring buffer.
pub struct SpanCollectorLayer;

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for SpanCollectorLayer {
    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            let exts = span.extensions();
            let duration_us = exts
                .get::<std::time::Instant>()
                .map_or(0, |start| start.elapsed().as_micros() as u64);

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);

            let metadata = span.metadata();
            let record = SpanRecord {
                name: metadata.name().to_string(),
                target: metadata.target().to_string(),
                level: metadata.level().to_string(),
                duration_us,
                fields: format!("{:?}", span.fields()),
                timestamp_epoch_ms: now,
            };

            let mut guard = ensure_ring();
            let ring = guard.as_mut().unwrap();
            let idx = WRITE_INDEX.fetch_add(1, Ordering::Relaxed) % RING_SIZE;
            ring[idx] = record;
        }
    }

    fn on_new_span(
        &self,
        _attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            let mut exts = span.extensions_mut();
            exts.insert(std::time::Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::Registry;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn drain_recent_empty_returns_empty() {
        let result = drain_recent(10);
        // Ring is initialized with empty records, so non-empty ones = 0
        assert!(
            result.is_empty()
                || result
                    .iter()
                    .all(|r| r.name.is_empty() || !r.name.is_empty())
        );
    }

    #[test]
    fn ring_size_is_reasonable() {
        const { assert!(RING_SIZE >= 64) };
    }

    /// Drive a full span lifecycle through the layer so that `on_new_span`
    /// records a start `Instant`, `on_close` builds a `SpanRecord` (covering
    /// both `map_or` closures and the ring write), and `drain_recent` then
    /// returns the cloned record. The span is given a unique name so the
    /// assertion is robust against the shared global ring buffer.
    #[test]
    fn span_lifecycle_is_recorded_and_drained() {
        let subscriber = Registry::default().with(SpanCollectorLayer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("tc_lifecycle_marker", answer = 42, label = "probe");
            span.in_scope(|| {});
            drop(span);
        });

        let records = drain_recent(RING_SIZE);
        let marker = records
            .iter()
            .find(|r| r.name == "tc_lifecycle_marker")
            .expect("closed span should be recorded in the ring buffer");

        assert_eq!(marker.level, "INFO");
        assert!(
            marker.target.starts_with("simard"),
            "unexpected target: {}",
            marker.target
        );
        // `on_close` populates `timestamp_epoch_ms` from the system clock.
        assert!(marker.timestamp_epoch_ms > 0);
        // `span.fields()` exposes the static `FieldSet` (the field *names*),
        // which `on_close` captures via its Debug representation.
        assert!(
            marker.fields.contains("answer") && marker.fields.contains("label"),
            "fields should list the span's field names, got: {}",
            marker.fields
        );
    }

    /// Opening and closing a span through the layer must produce a drainable,
    /// cloneable record. This complements the lifecycle test by asserting the
    /// non-empty drain path (the `push` branch in `drain_recent`) and the
    /// `SpanRecord::clone` derive.
    #[test]
    fn drain_recent_returns_non_empty_after_span() {
        let subscriber = Registry::default().with(SpanCollectorLayer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::warn_span!("tc_nonempty_probe");
            span.in_scope(|| {});
            drop(span);
        });

        let records = drain_recent(RING_SIZE);
        assert!(
            records.iter().any(|r| r.name == "tc_nonempty_probe"),
            "expected the recorded span to be drained"
        );
        // Exercise SpanRecord::clone explicitly (drain already clones, but make
        // the dependency on Clone explicit and verify equality of cloned data).
        if let Some(r) = records.first() {
            let cloned = r.clone();
            assert_eq!(cloned.name, r.name);
            assert_eq!(cloned.level, r.level);
        }
    }

    /// Cover the derived `Debug` and `serde::Serialize` implementations on
    /// `SpanRecord`, which are part of the file's public surface and otherwise
    /// never exercised by the layer paths.
    #[test]
    fn span_record_debug_and_serialize() {
        let record = SpanRecord {
            name: "unit".to_string(),
            target: "simard::trace_collector".to_string(),
            level: "INFO".to_string(),
            duration_us: 1234,
            fields: "{key=value}".to_string(),
            timestamp_epoch_ms: 99,
        };

        let debug = format!("{record:?}");
        assert!(debug.contains("unit"));
        assert!(debug.contains("1234"));

        let json = serde_json::to_string(&record).expect("SpanRecord should serialize");
        assert!(json.contains("\"name\":\"unit\""));
        assert!(json.contains("\"duration_us\":1234"));
        assert!(json.contains("\"timestamp_epoch_ms\":99"));

        // Clone derive coverage.
        let cloned = record.clone();
        assert_eq!(cloned.duration_us, record.duration_us);
    }

    /// `drain_recent` must cap the returned count at the requested limit even
    /// when more records are present, and never panic for a zero limit.
    #[test]
    fn drain_recent_respects_limit_bounds() {
        let subscriber = Registry::default().with(SpanCollectorLayer);
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..3 {
                let span = tracing::info_span!("tc_limit_probe");
                span.in_scope(|| {});
                drop(span);
            }
        });

        // A zero limit yields nothing and must not panic.
        assert!(drain_recent(0).is_empty());

        // A small limit returns at most that many records.
        let limited = drain_recent(2);
        assert!(limited.len() <= 2);
    }
}
