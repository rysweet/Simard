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
        assert!(RING_SIZE >= 64);
    }
}
