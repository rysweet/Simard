//! Process-agnostic assembly of [`StatusSnapshot`] from durable sources.
//!
//! `assemble` reads on-disk and system sources (the telemetry snapshot file, the
//! cost ledger, `/proc`, `systemctl show`) so it returns the same result from
//! the daemon, the CLI, or the TUI. Each section is assembled in isolation: one
//! failing source degrades one section, never the whole report, and never
//! panics.
//!
//! Sources that require the live memory/goal IPC socket or `gh` (memory, goals,
//! workstreams, completed work, self-improvement) are wired incrementally; until
//! then they report `unavailable` with an honest note rather than inventing data.

use std::path::PathBuf;

use super::{
    CopilotTurn, Daemon, DiskUsage, EdgeCounts, Gym, LedgerWindow, LlmUsage, MemoryBrain,
    NodeCounts, Resources, SectionEnvelope, StatusSnapshot, TelemetrySignals,
};
use crate::telemetry::{names, snapshot};

/// How the snapshot is assembled.
#[derive(Clone, Debug)]
pub struct AssembleOptions {
    /// Durable state root to read (`telemetry/metrics_snapshot.json`, cost
    /// ledger, …). Defaults to the resolved Simard state root.
    pub state_root: PathBuf,
    /// The systemd unit whose `systemctl show` backs the daemon section.
    pub service_unit: String,
    /// Optional allowlist of section names to assemble; `None` = all.
    pub sections: Option<Vec<String>>,
}

impl Default for AssembleOptions {
    fn default() -> Self {
        Self {
            state_root: crate::state_root::simard_state_root(),
            service_unit: "simard.service".to_string(),
            sections: None,
        }
    }
}

impl AssembleOptions {
    /// Assemble reading a specific state root.
    pub fn with_state_root(state_root: PathBuf) -> Self {
        Self {
            state_root,
            ..Self::default()
        }
    }
}

/// Age (seconds) beyond which the telemetry snapshot is considered `stale`.
const SNAPSHOT_FRESHNESS_SECS: i64 = 300;

/// Assemble the full snapshot. Never panics; degraded sources become
/// `unavailable`/`absent` sections.
pub fn assemble(opts: &AssembleOptions) -> StatusSnapshot {
    let metrics = snapshot::read(&snapshot::snapshot_path(&opts.state_root));
    let gym_skipped = env_flag("SIMARD_SKIP_GYM");

    let daemon = assemble_daemon(opts);
    // The daemon's PID and restart count feed the resource + telemetry sections,
    // so extract them once from the assembled daemon view.
    let main_pid = daemon.data.as_ref().and_then(|d| d.main_pid);
    let n_restarts = daemon.data.as_ref().and_then(|d| d.n_restarts);

    let llm = assemble_llm(opts);
    let budget_over = llm_over_budget(&llm);

    StatusSnapshot {
        schema_version: super::SCHEMA_VERSION,
        generated_at: snapshot::now_rfc3339(),
        daemon,
        resources: assemble_resources(main_pid),
        memory: assemble_memory(metrics.as_ref(), &opts.state_root),
        gym: assemble_gym(gym_skipped),
        goals: SectionEnvelope::absent("goal board read deferred (see dashboard/TUI goal board)"),
        workstreams: SectionEnvelope::absent("engineer registry not read in this context"),
        completed: SectionEnvelope::absent("gh: not queried in this context"),
        self_improvement: SectionEnvelope::absent("gh: not queried in this context"),
        telemetry: assemble_telemetry(metrics.as_ref(), gym_skipped, n_restarts, budget_over),
        llm,
    }
}

/// Whether today's ledger spend has crossed the daily budget guard, when both
/// are known. `None` when either is unknown.
fn llm_over_budget(llm: &SectionEnvelope<LlmUsage>) -> Option<bool> {
    let data = llm.data.as_ref()?;
    let budget = data.daily_budget_usd?;
    let spent = data.ledger_today.as_ref().map(|w| w.cost_usd)?;
    Some(spent > budget)
}

// ── daemon ──────────────────────────────────────────────────────────────────

fn assemble_daemon(opts: &AssembleOptions) -> SectionEnvelope<Daemon> {
    let output = std::process::Command::new("systemctl")
        .args([
            "show",
            &opts.service_unit,
            "--property=LoadState,ActiveState,SubState,MainPID,NRestarts,ExecMainStartTimestamp",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(_) => return SectionEnvelope::absent("systemctl: unit not found"),
        Err(_) => return SectionEnvelope::absent("systemctl: unavailable"),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut props = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            props.insert(k.to_string(), v.to_string());
        }
    }

    // `systemctl show` exits 0 even for an unknown unit (reporting
    // `LoadState=not-found`); treat anything not actually loaded as absent
    // rather than rendering a phantom daemon.
    match props.get("LoadState").map(String::as_str) {
        Some("loaded") => {}
        _ => return SectionEnvelope::absent("systemctl: unit not loaded"),
    }

    let active = props.get("ActiveState").cloned().unwrap_or_default();
    let sub = props.get("SubState").cloned().unwrap_or_default();
    let state = if sub.is_empty() {
        active
    } else {
        format!("{active} ({sub})")
    };

    let daemon = Daemon {
        state,
        version: env!("CARGO_PKG_VERSION").to_string(),
        main_pid: props
            .get("MainPID")
            .and_then(|v| v.parse().ok())
            .filter(|p| *p != 0),
        deployed_commit: None,
        instance_uptime: None,
        running_since: props
            .get("ExecMainStartTimestamp")
            .filter(|s| !s.is_empty())
            .cloned(),
        n_restarts: props.get("NRestarts").and_then(|v| v.parse().ok()),
    };
    SectionEnvelope::live(daemon, None)
}

// ── resources ───────────────────────────────────────────────────────────────

fn assemble_resources(daemon_pid: Option<u32>) -> SectionEnvelope<Resources> {
    let (load_1, load_5, load_15) = read_loadavg();
    let (total, avail) = read_meminfo();
    let used = match (total, avail) {
        (Some(t), Some(a)) => Some(t.saturating_sub(a)),
        _ => None,
    };
    let rss_bytes = daemon_pid.and_then(read_process_rss_bytes);
    let live_engineers = count_live_engineers();

    let any = load_1.is_some() || total.is_some();
    if !any {
        return SectionEnvelope::absent("/proc: unavailable");
    }

    let resources = Resources {
        cpu_pct: None,
        rss_bytes,
        cgroup_mem_peak_bytes: None,
        load_1,
        load_5,
        load_15,
        sys_mem_used_bytes: used,
        sys_mem_total_bytes: total,
        sys_mem_avail_bytes: avail,
        disk_home: read_disk_for_home(),
        disk_tmp: read_disk("/tmp"),
        live_engineers,
    };
    SectionEnvelope::live(resources, None)
}

/// Read a process's resident set size (bytes) from `/proc/<pid>/status`
/// `VmRSS`. `None` when the process is gone or `/proc` is unavailable.
fn read_process_rss_bytes(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return parse_kb(rest);
        }
    }
    None
}

/// Count live engineer/agent subprocesses via `pgrep`. `None` when `pgrep` is
/// unavailable; `Some(0)` when it ran and found none.
fn count_live_engineers() -> Option<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", "-c", "simard-engineer|RustyClawd|copilot.*--auto"])
        .output()
        .ok()?;
    // pgrep exits 1 with "0\n" when there are no matches; still a valid count.
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .ok()
}

/// Resolve and stat the operator's home mount for the disk row.
fn read_disk_for_home() -> Option<DiskUsage> {
    let home = std::env::var("HOME").ok()?;
    read_disk(&home)
}

fn read_loadavg() -> (Option<f64>, Option<f64>, Option<f64>) {
    let Ok(text) = std::fs::read_to_string("/proc/loadavg") else {
        return (None, None, None);
    };
    let mut it = text.split_whitespace();
    (
        it.next().and_then(|s| s.parse().ok()),
        it.next().and_then(|s| s.parse().ok()),
        it.next().and_then(|s| s.parse().ok()),
    )
}

fn read_meminfo() -> (Option<u64>, Option<u64>) {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let mut total = None;
    let mut avail = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail = parse_kb(rest);
        }
    }
    (total, avail)
}

fn parse_kb(s: &str) -> Option<u64> {
    s.split_whitespace()
        .next()
        .and_then(|n| n.parse::<u64>().ok())
        .map(|kb| kb.saturating_mul(1024))
}

fn read_disk(path: &str) -> Option<DiskUsage> {
    #[cfg(unix)]
    #[allow(clippy::unnecessary_cast)] // statvfs field widths vary by platform.
    {
        use std::ffi::CString;
        use std::mem::MaybeUninit;
        let c = CString::new(path).ok()?;
        // SAFETY: `statvfs` writes into the provided buffer; we check the return
        // code before reading it.
        unsafe {
            let mut buf = MaybeUninit::<libc::statvfs>::zeroed();
            if libc::statvfs(c.as_ptr(), buf.as_mut_ptr()) != 0 {
                return None;
            }
            let buf = buf.assume_init();
            let frsize = buf.f_frsize as u64;
            Some(DiskUsage {
                free_bytes: (buf.f_bavail as u64).saturating_mul(frsize),
                total_bytes: (buf.f_blocks as u64).saturating_mul(frsize),
            })
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

// ── llm (cost ledger, read within state_root) ───────────────────────────────

fn assemble_llm(opts: &AssembleOptions) -> SectionEnvelope<LlmUsage> {
    use std::io::BufRead;

    let ledger = opts.state_root.join("costs").join("ledger.jsonl");
    let Ok(file) = std::fs::File::open(&ledger) else {
        return SectionEnvelope::absent("cost ledger: absent");
    };

    // Only the four fields the windows need; typed deserialization skips the
    // per-line `serde_json::Value` map (one allocation + seven interned keys per
    // entry) that a generic parse would build. `timestamp` stays a raw string so
    // an entry with a missing or unparseable timestamp still counts toward the
    // all-time window — matching the prior tolerant behavior — rather than being
    // dropped by a stricter typed timestamp.
    #[derive(serde::Deserialize)]
    struct LedgerLine {
        #[serde(default)]
        timestamp: Option<String>,
        #[serde(default)]
        cost_usd_est: f64,
        #[serde(default)]
        prompt_tokens_est: u64,
        #[serde(default)]
        completion_tokens_est: u64,
    }

    let now = chrono::Utc::now();
    let day_ago = now - chrono::Duration::days(1);
    let week_ago = now - chrono::Duration::days(7);

    let mut today = LedgerWindow::default();
    let mut last7 = LedgerWindow::default();
    let mut all = LedgerWindow::default();
    let mut last_turn: Option<CopilotTurn> = None;

    // Stream line-by-line so peak memory stays ~one line rather than the whole
    // append-only ledger, which grows without bound over the daemon's lifetime.
    let reader = std::io::BufReader::new(file);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<LedgerLine>(trimmed) else {
            continue;
        };
        let ts = entry
            .timestamp
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let cost = entry.cost_usd_est;
        let tin = entry.prompt_tokens_est;
        let tout = entry.completion_tokens_est;

        add_window(&mut all, cost, tin, tout);
        if let Some(ts) = ts {
            if ts >= week_ago {
                add_window(&mut last7, cost, tin, tout);
            }
            if ts >= day_ago {
                add_window(&mut today, cost, tin, tout);
            }
        }
        last_turn = Some(CopilotTurn {
            tokens_in: tin,
            tokens_cached: 0,
            tokens_out: tout,
            ai_credits: 0,
        });
    }

    let daily_budget = std::env::var("SIMARD_DAILY_BUDGET_USD")
        .ok()
        .and_then(|v| v.parse::<f64>().ok());

    let usage = LlmUsage {
        copilot_turn: last_turn,
        ledger_today: Some(today),
        ledger_7d: Some(last7),
        ledger_all_time: Some(all),
        daily_budget_usd: daily_budget,
        reconciliation: None,
    };
    SectionEnvelope::live(usage, None)
}

fn add_window(w: &mut LedgerWindow, cost: f64, tin: u64, tout: u64) {
    w.cost_usd += cost;
    w.tokens_in = w.tokens_in.saturating_add(tin);
    w.tokens_out = w.tokens_out.saturating_add(tout);
}

// ── memory / brain (node + edge gauges from the metrics snapshot) ────────────

/// Build the MEMORY / BRAIN section from the per-cycle gauges the daemon flushes
/// into the metrics snapshot (`simard.memory.nodes{type}` /
/// `simard.memory.edges{type}`). Reading through the snapshot keeps this
/// process-agnostic — no LadybugDB open from the CLI — and honest: when the
/// daemon has not flushed memory gauges the section is `absent`, never a
/// fabricated zero.
fn assemble_memory(
    metrics: Option<&snapshot::MetricsSnapshot>,
    state_root: &std::path::Path,
) -> SectionEnvelope<MemoryBrain> {
    let Some(m) = metrics else {
        return SectionEnvelope::absent("metrics snapshot: absent");
    };

    let node = |ty: &str| m.gauge(names::MEMORY_NODES, &[(names::ATTR_TYPE, ty)]);
    let edge = |ty: &str| m.gauge(names::MEMORY_EDGES, &[(names::ATTR_TYPE, ty)]);

    let nodes = NodeCounts {
        episodic: node("episodic").map(clamp_u64),
        semantic: node("semantic").map(clamp_u64),
        prospective: node("prospective").map(clamp_u64),
        working: node("working").map(clamp_u64),
        procedural: node("procedural").map(clamp_u64),
        sensory: node("sensory").map(clamp_u64),
    };
    let edges = EdgeCounts {
        derives_from: edge("DERIVES_FROM").map(clamp_u64),
        similar_to: edge("SIMILAR_TO").map(clamp_u64),
        supersedes: edge("SUPERSEDES").map(clamp_u64),
    };

    // No memory gauges in this snapshot → the daemon has not published them yet.
    let any_nodes = nodes.episodic.is_some()
        || nodes.semantic.is_some()
        || nodes.prospective.is_some()
        || nodes.working.is_some()
        || nodes.procedural.is_some()
        || nodes.sensory.is_some();
    if !any_nodes && edges.derives_from.is_none() {
        return SectionEnvelope::absent("memory gauges: not in snapshot");
    }

    let nodes_total = any_nodes.then(|| {
        [
            nodes.episodic,
            nodes.semantic,
            nodes.prospective,
            nodes.working,
            nodes.procedural,
            nodes.sensory,
        ]
        .into_iter()
        .flatten()
        .sum()
    });

    let store_path = state_root.join("cognitive");
    let store_size_bytes = std::fs::metadata(&store_path).ok().map(|meta| meta.len());

    let memory = MemoryBrain {
        store_path: store_path.display().to_string(),
        store_size_bytes,
        backend: "amplihack-memory-lib".to_string(),
        nodes_total,
        nodes,
        edges,
        cognitive_processes: super::CognitiveHealth::default(),
        brains_llm_backed: None,
        brain_fallbacks: m.counter(names::BRAIN_ESCALATIONS, &[]),
        decide_ladder_exhausted: m.counter(names::BRAIN_LADDER_EXHAUSTED, &[]),
    };

    snapshot_section(memory, &m.captured_at)
}

/// Clamp a gauge's `i64` back into a non-negative `u64` for a count field.
fn clamp_u64(v: i64) -> u64 {
    v.max(0) as u64
}

// ── gym ─────────────────────────────────────────────────────────────────────

fn assemble_gym(skip_gym: bool) -> SectionEnvelope<Gym> {
    SectionEnvelope::live(
        Gym {
            skip_gym,
            configured_scenarios: None,
            self_eval_state: "idle".to_string(),
        },
        None,
    )
}

// ── telemetry / anomalies (derived from the metrics snapshot) ────────────────

fn assemble_telemetry(
    metrics: Option<&snapshot::MetricsSnapshot>,
    gym_skipped: bool,
    n_restarts: Option<u64>,
    budget_over: Option<bool>,
) -> SectionEnvelope<TelemetrySignals> {
    let Some(m) = metrics else {
        return SectionEnvelope::absent("metrics snapshot: absent");
    };

    let ok = m
        .counter(names::DISTILL_RUNS, &[(names::ATTR_RESULT, "ok")])
        .unwrap_or(0);
    let fail = m
        .counter(names::DISTILL_RUNS, &[(names::ATTR_RESULT, "parse_fail")])
        .unwrap_or(0);
    let distill_total = ok + fail;
    let distill_fail_pct = if distill_total > 0 {
        Some((fail as f64 / distill_total as f64) * 100.0)
    } else {
        None
    };

    // Restart churn is authoritatively the systemd `NRestarts` (it survives the
    // process boundary a restart crosses); fall back to the in-process counter
    // only when the daemon section is unavailable.
    let restart_churn = n_restarts.or_else(|| m.counter(names::DAEMON_RESTART, &[]));

    let budget_flag = match budget_over {
        Some(true) => "over".to_string(),
        Some(false) => "ok".to_string(),
        None => "unknown".to_string(),
    };

    let mut anomalies = Vec::new();
    if m.overflow_series > 0 {
        anomalies.push(format!(
            "telemetry cardinality overflow ({})",
            m.overflow_series
        ));
    }
    if let Some(pct) = distill_fail_pct
        && pct > 0.0
    {
        anomalies.push(format!("distill parse-fail rate {pct:.0}%"));
    }
    if matches!(budget_over, Some(true)) {
        anomalies.push("daily LLM budget exceeded".to_string());
    }
    let ladder = m.counter(names::BRAIN_LADDER_EXHAUSTED, &[]).unwrap_or(0);
    if ladder > 0 {
        anomalies.push(format!("brain decide ladder exhausted ({ladder})"));
    }

    let signals = TelemetrySignals {
        window: "since last flush".to_string(),
        distill_fail_pct,
        restart_churn,
        gym_skipped,
        budget_flag,
        parse_fix_holding: distill_fail_pct.map(|p| p == 0.0),
        anomalies,
    };

    snapshot_section(signals, &m.captured_at)
}

/// Wrap a snapshot-derived section, choosing `live` vs `stale` from
/// `captured_at`. A readable snapshot is always `Ok`; only its freshness varies,
/// so the two constructors capture every reachable state without a
/// mutate-after-construct step.
fn snapshot_section<T>(data: T, captured_at: &str) -> SectionEnvelope<T> {
    let as_of = Some(captured_at.to_string());
    if snapshot_is_stale(captured_at) {
        SectionEnvelope::stale(data, as_of)
    } else {
        SectionEnvelope::live(data, as_of)
    }
}

/// Whether the snapshot's `captured_at` is older than the freshness window. An
/// unparseable timestamp is treated as fresh (not stale), matching the prior
/// tolerant behavior.
fn snapshot_is_stale(captured_at: &str) -> bool {
    match chrono::DateTime::parse_from_rfc3339(captured_at) {
        Ok(ts) => {
            let age = chrono::Utc::now().signed_duration_since(ts.with_timezone(&chrono::Utc));
            age.num_seconds() > SNAPSHOT_FRESHNESS_SECS
        }
        Err(_) => false,
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}
