//! Memory pressure shedding for the OODA daemon (issue #2183).
//!
//! [`rss_health`] observes and logs RSS thresholds. This module
//! *acts* on memory pressure by invoking concrete shedding steps:
//! pruning expired sensory memory, aggressive subagent session GC,
//! and cognitive-memory statistics logging for attribution.
//!
//! Called from the daemon loop when RSS exceeds the "elevated"
//! threshold (default: 8 GiB). Each call is idempotent and safe to
//! repeat every cycle — individual shedding steps are cheap no-ops
//! when there is nothing to shed.

use std::path::Path;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::rss_health;

/// Result of one emergency-GC pass.
#[derive(Clone, Debug, Default)]
pub struct ShedReport {
    /// Number of expired sensory-memory nodes pruned.
    pub sensory_pruned: usize,
    /// Number of subagent-session registry entries removed.
    pub sessions_pruned: usize,
    /// Snapshot of cognitive-memory node counts (for attribution logging).
    pub cognitive_stats_summary: String,
    /// RSS reading *before* shedding (bytes). `None` on non-Linux.
    pub rss_before: Option<u64>,
    /// RSS reading *after* shedding (bytes). `None` on non-Linux.
    pub rss_after: Option<u64>,
}

impl ShedReport {
    /// One-line summary for daemon log.
    pub fn summary(&self) -> String {
        let rss_delta = match (self.rss_before, self.rss_after) {
            (Some(before), Some(after)) => {
                let before_mb = before / (1024 * 1024);
                let after_mb = after / (1024 * 1024);
                format!(" (RSS {before_mb}→{after_mb} MiB)")
            }
            _ => String::new(),
        };
        format!(
            "memory shed: sensory_pruned={}, sessions_pruned={}, cognitive=[{}]{}",
            self.sensory_pruned, self.sessions_pruned, self.cognitive_stats_summary, rss_delta,
        )
    }
}

/// Default elevated threshold: 8 GiB.
const DEFAULT_ELEVATED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Read the elevated-threshold from env or return the default.
pub fn elevated_threshold_bytes() -> u64 {
    std::env::var("SIMARD_RSS_ELEVATED_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ELEVATED_BYTES)
}

/// Returns `true` when current RSS exceeds the elevated threshold and
/// shedding should be attempted.
pub fn should_shed() -> bool {
    rss_health::read_rss_bytes()
        .map(|rss| rss >= elevated_threshold_bytes())
        .unwrap_or(false)
}

/// Run all shedding steps.
///
/// `memory`: cognitive-memory handle (daemon holds an `Arc<dyn CognitiveMemoryOps>`)
/// `state_root`: for logging context
///
/// Each sub-step is best-effort: failures are logged but do not abort the
/// overall pass.
pub fn run_emergency_shed(memory: &dyn CognitiveMemoryOps, _state_root: &Path) -> ShedReport {
    let mut report = ShedReport {
        rss_before: rss_health::read_rss_bytes(),
        ..Default::default()
    };

    // 1. Prune expired sensory memory (TTL-based eviction already in the
    //    trait — we just ensure it runs at shedding time too).
    match memory.prune_expired_sensory() {
        Ok(n) => report.sensory_pruned = n,
        Err(e) => tracing::warn!(
            target: "simard::memory_health",
            error = %e,
            "sensory prune failed during emergency shed",
        ),
    }

    // 2. Aggressive subagent-session GC: use the tight retention constant
    //    (1 hour) instead of the default 24 h.
    match crate::subagent_sessions::gc_with_retention(
        &crate::subagent_sessions::TmuxProbe,
        crate::subagent_sessions::TIGHT_RETENTION_SECONDS,
    ) {
        Ok(n) => report.sessions_pruned = n,
        Err(e) => tracing::warn!(
            target: "simard::memory_health",
            error = %e,
            "subagent session GC failed during emergency shed",
        ),
    }

    // 3. Snapshot cognitive-memory stats for attribution.
    match memory.get_statistics() {
        Ok(stats) => {
            report.cognitive_stats_summary = format!(
                "sensory={} working={} episodic={} semantic={} procedural={} prospective={} total={}",
                stats.sensory_count,
                stats.working_count,
                stats.episodic_count,
                stats.semantic_count,
                stats.procedural_count,
                stats.prospective_count,
                stats.total(),
            );
        }
        Err(e) => {
            report.cognitive_stats_summary = format!("stats unavailable: {e}");
        }
    }

    report.rss_after = rss_health::read_rss_bytes();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shed_report_summary_format() {
        let report = ShedReport {
            sensory_pruned: 5,
            sessions_pruned: 2,
            cognitive_stats_summary: "total=42".to_string(),
            rss_before: Some(8 * 1024 * 1024 * 1024),
            rss_after: Some(7 * 1024 * 1024 * 1024 + 512 * 1024 * 1024),
        };
        let s = report.summary();
        assert!(s.contains("sensory_pruned=5"), "got: {s}");
        assert!(s.contains("sessions_pruned=2"), "got: {s}");
        assert!(s.contains("total=42"), "got: {s}");
        assert!(s.contains("RSS"), "got: {s}");
    }

    #[test]
    fn shed_report_summary_no_rss() {
        let report = ShedReport {
            cognitive_stats_summary: "n/a".to_string(),
            ..Default::default()
        };
        let s = report.summary();
        assert!(!s.contains("RSS"), "no RSS data → no RSS in summary: {s}");
    }

    #[test]
    fn elevated_threshold_default() {
        // When env var not set, should return 8 GiB.
        let threshold = elevated_threshold_bytes();
        assert_eq!(threshold, 8 * 1024 * 1024 * 1024);
    }
}
