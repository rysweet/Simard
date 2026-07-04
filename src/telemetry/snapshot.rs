//! Serializable point-in-time view of the in-process metrics registry, plus the
//! degrade-safe on-disk `metrics_snapshot.json` reader/writer.
//!
//! The daemon flushes a [`MetricsSnapshot`] to
//! `~/.simard/telemetry/metrics_snapshot.json` once per OODA cycle; the CLI and
//! TUI **read** it (never write it). Readers tolerate a missing, truncated, or
//! corrupt file by returning `None` rather than panicking.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Bumped whenever the serialized shape changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

/// Hard cap on the on-disk snapshot we will read into memory (bytes). A
/// pathologically large file degrades to `None` instead of exhausting memory.
pub const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;

/// A single counter series: monotonically increasing total for one attribute
/// set.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CounterSeries {
    pub name: String,
    /// Sorted, normalized `(key, value)` attribute pairs.
    pub attrs: Vec<(String, String)>,
    pub value: u64,
}

/// A single gauge series: the last value written for one attribute set.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GaugeSeries {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub value: i64,
}

/// A single histogram series: count/sum plus cumulative bucket tallies.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HistogramSeries {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub value: HistogramValue,
}

/// Aggregated histogram observation.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HistogramValue {
    pub count: u64,
    pub sum: f64,
    /// Cumulative `(le, count)` buckets, ascending by `le`.
    pub buckets: Vec<HistogramBucket>,
}

/// One cumulative histogram bucket (`count` observations with value `<= le`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HistogramBucket {
    pub le: f64,
    pub count: u64,
}

/// A serializable snapshot of every metric series known to the in-process
/// registry at `captured_at`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MetricsSnapshot {
    pub schema_version: u32,
    /// RFC3339 timestamp of when the snapshot was captured.
    pub captured_at: String,
    #[serde(default)]
    pub counters: Vec<CounterSeries>,
    #[serde(default)]
    pub gauges: Vec<GaugeSeries>,
    #[serde(default)]
    pub histograms: Vec<HistogramSeries>,
    /// Count of attribute values folded into the `other` bucket by the
    /// cardinality bound — a non-zero value signals emitter misuse.
    #[serde(default)]
    pub overflow_series: u64,
}

impl MetricsSnapshot {
    /// An empty snapshot stamped now.
    pub fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            captured_at: now_rfc3339(),
            counters: Vec::new(),
            gauges: Vec::new(),
            histograms: Vec::new(),
            overflow_series: 0,
        }
    }

    /// Current total of counter `name` with exactly `attrs` (order-independent).
    pub fn counter(&self, name: &str, attrs: &[(&str, &str)]) -> Option<u64> {
        let want = sorted_owned(attrs);
        self.counters
            .iter()
            .find(|s| s.name == name && s.attrs == want)
            .map(|s| s.value)
    }

    /// Current value of gauge `name` with exactly `attrs`.
    pub fn gauge(&self, name: &str, attrs: &[(&str, &str)]) -> Option<i64> {
        let want = sorted_owned(attrs);
        self.gauges
            .iter()
            .find(|s| s.name == name && s.attrs == want)
            .map(|s| s.value)
    }

    /// Current histogram value of `name` with exactly `attrs`.
    pub fn histogram(&self, name: &str, attrs: &[(&str, &str)]) -> Option<&HistogramValue> {
        let want = sorted_owned(attrs);
        self.histograms
            .iter()
            .find(|s| s.name == name && s.attrs == want)
            .map(|s| &s.value)
    }
}

fn sorted_owned(attrs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = attrs
        .iter()
        .map(|(k, val)| ((*k).to_string(), (*val).to_string()))
        .collect();
    v.sort();
    v
}

/// RFC3339 timestamp for "now" (UTC, second precision).
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Canonical on-disk path of the metrics snapshot under a state root:
/// `<state_root>/telemetry/metrics_snapshot.json`. The daemon writes it; the
/// CLI and TUI read it.
pub fn snapshot_path(state_root: &Path) -> std::path::PathBuf {
    state_root.join("telemetry").join("metrics_snapshot.json")
}

/// Atomically and privately write `snapshot` to `path`.
///
/// Creates the parent directory `0700`, writes a `0600` temp file, `fsync`s it,
/// then `rename`s over the target — so `path` is never briefly world-readable
/// and readers never see a partial document.
pub fn write_atomic(path: &Path, snapshot: &MetricsSnapshot) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let body = serde_json::to_vec_pretty(snapshot)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let tmp = path.with_extension("json.tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read a snapshot from `path`, degrading to `None` on any problem.
///
/// Returns `None` when the file is missing, larger than [`MAX_SNAPSHOT_BYTES`],
/// unreadable, or not parseable — **never** panics. Freshness (`live`/`stale`)
/// is a judgement the caller makes from [`MetricsSnapshot::captured_at`]; this
/// function only materializes the document.
pub fn read(path: &Path) -> Option<MetricsSnapshot> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SNAPSHOT_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<MetricsSnapshot>(&bytes).ok()
}
