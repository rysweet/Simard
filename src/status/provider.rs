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
use crate::overseer::activity::{self, OverseerActivity};
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
        resources: assemble_resources(main_pid, &opts.state_root),
        memory: assemble_memory(metrics.as_ref(), &opts.state_root),
        gym: assemble_gym(gym_skipped),
        goals: SectionEnvelope::absent("goal board read deferred (see dashboard/TUI goal board)"),
        workstreams: SectionEnvelope::absent("engineer registry not read in this context"),
        completed: SectionEnvelope::absent("gh: not queried in this context"),
        self_improvement: SectionEnvelope::absent("gh: not queried in this context"),
        telemetry: assemble_telemetry(metrics.as_ref(), gym_skipped, n_restarts, budget_over),
        llm,
        overseer: assemble_overseer(&opts.state_root),
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

/// Age (seconds) beyond which the `daemon_health.json` heartbeat is considered
/// `stale`. Matches the `/api/status` threshold (`routes.rs`): cycle interval
/// (300s) + max cycle runtime (~600s). With the heartbeat stamped at cycle
/// start, a healthy daemon's heartbeat stays well under this.
const DAEMON_HEARTBEAT_STALE_SECS: i64 = 900;

fn assemble_daemon(opts: &AssembleOptions) -> SectionEnvelope<Daemon> {
    // Prefer systemd when a unit is actually loaded; otherwise fall back to the
    // durable `daemon_health.json` heartbeat so the snapshot stays
    // process-agnostic in non-systemd deployments (dev / worktree / container).
    match assemble_daemon_from_systemctl(opts) {
        Some(env) => env,
        None => assemble_daemon_from_heartbeat(),
    }
}

/// Assemble the daemon section from `systemctl show`. Returns `Some(..)` only
/// when the unit is genuinely loaded; `None` (unavailable / not-found /
/// not-loaded) signals the caller to fall back to the durable heartbeat.
fn assemble_daemon_from_systemctl(opts: &AssembleOptions) -> Option<SectionEnvelope<Daemon>> {
    let output = std::process::Command::new("systemctl")
        .args([
            "show",
            &opts.service_unit,
            "--property=LoadState,ActiveState,SubState,MainPID,NRestarts,ExecMainStartTimestamp",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        // systemctl ran but errored, or is unavailable — try the heartbeat.
        Ok(_) | Err(_) => return None,
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut props = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            props.insert(k.to_string(), v.to_string());
        }
    }

    // `systemctl show` exits 0 even for an unknown unit (reporting
    // `LoadState=not-found`); treat anything not actually loaded as "no unit"
    // and fall back to the heartbeat rather than rendering a phantom daemon.
    match props.get("LoadState").map(String::as_str) {
        Some("loaded") => {}
        _ => return None,
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
    Some(SectionEnvelope::live(daemon, None))
}

/// Path to the durable OODA heartbeat the daemon flushes each cycle
/// (`dirs::data_local_dir()/simard/daemon_health.json`). This is the same file
/// `/api/status`, `/api/activity`, and `/api/workboard` read, and — unlike the
/// telemetry snapshot — it is *not* under `SIMARD_STATE_ROOT`, so it is resolved
/// from the OS data-local dir rather than `opts.state_root`.
fn daemon_health_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/var/tmp"))
        .join("simard")
        .join("daemon_health.json")
}

/// Assemble the daemon section from the durable `daemon_health.json` heartbeat.
/// Fail-visible: a missing / unreadable / unparseable heartbeat degrades this
/// one section to `absent` with the honest `systemctl: unit not loaded` note
/// (never panics, never fabricates a running daemon).
fn assemble_daemon_from_heartbeat() -> SectionEnvelope<Daemon> {
    read_daemon_heartbeat(&daemon_health_path(), chrono::Utc::now())
}

/// Read + map the heartbeat at `path` as of `now`. Split from
/// [`assemble_daemon_from_heartbeat`] so tests can inject a temp path without
/// mutating process-global `HOME` / `XDG_DATA_HOME`.
fn read_daemon_heartbeat(
    path: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
) -> SectionEnvelope<Daemon> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return SectionEnvelope::absent("systemctl: unit not loaded"),
    };
    let health: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return SectionEnvelope::absent("systemctl: unit not loaded"),
    };
    daemon_from_heartbeat(&health, now)
}

/// Pure mapping from a parsed `daemon_health.json` heartbeat to the daemon
/// section, given `now` (so it is deterministically testable). The heartbeat's
/// `timestamp` drives freshness against [`DAEMON_HEARTBEAT_STALE_SECS`]:
/// - fresh  → `state = "running (<phase>)"`, `freshness = live`
/// - stale  → `state = "stale (<phase>)"`,   `freshness = stale`
/// - no/invalid timestamp → `state = "unknown"`, `freshness = stale`
///
/// `as_of` is the heartbeat `timestamp`. `main_pid` / `n_restarts` are not
/// recorded in the heartbeat, so they stay `None` (honest) rather than guessed.
fn daemon_from_heartbeat(
    health: &serde_json::Value,
    now: chrono::DateTime<chrono::Utc>,
) -> SectionEnvelope<Daemon> {
    let timestamp = health.get("timestamp").and_then(|t| t.as_str());
    let phase = health
        .get("cycle_phase")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .trim();

    let fresh = timestamp
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|ts| now.signed_duration_since(ts.with_timezone(&chrono::Utc)))
        .map(|age| age.num_seconds() < DAEMON_HEARTBEAT_STALE_SECS);

    let base = match fresh {
        Some(true) => "running",
        Some(false) => "stale",
        None => "unknown",
    };
    let state = if phase.is_empty() {
        base.to_string()
    } else {
        format!("{base} ({phase})")
    };

    let daemon = Daemon {
        state,
        version: env!("CARGO_PKG_VERSION").to_string(),
        main_pid: None,
        deployed_commit: None,
        instance_uptime: None,
        running_since: None,
        n_restarts: None,
    };

    let as_of = timestamp.map(|s| s.to_string());
    if fresh == Some(true) {
        SectionEnvelope::live(daemon, as_of)
    } else {
        SectionEnvelope::stale(daemon, as_of)
    }
}

// ── resources ───────────────────────────────────────────────────────────────

fn assemble_resources(
    daemon_pid: Option<u32>,
    state_root: &std::path::Path,
) -> SectionEnvelope<Resources> {
    let (load_1, load_5, load_15) = read_loadavg();
    let (total, avail) = read_meminfo();
    let used = match (total, avail) {
        (Some(t), Some(a)) => Some(t.saturating_sub(a)),
        _ => None,
    };
    let rss_bytes = daemon_pid.and_then(read_process_rss_bytes);
    // #2432 design G4/G5: the authoritative live-engineer count is the set of
    // live worktree dispatch claims (a sentinel PID per real engineer worktree,
    // verified alive), NOT a fragile process-name grep. The retired pgrep pattern
    // matched `simard-engineer` (hyphen) while engineers actually run as
    // `simard engineer run single-process …` (space), silently undercounting the
    // live fleet (observed 17 real → 1 matched). The claim file is written by the
    // spawn path itself, so it cannot drift the way a name pattern can.
    let live_engineers = Some(crate::ooda_brain::count_live_engineer_claims(state_root));

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

    // Single-source the ceiling through the canonical resolver (bug #6): the
    // daily budget is always guarded (default `DEFAULT_DAILY_BUDGET_USD`), so
    // the display must reflect the *enforced* ceiling — the same value the
    // Overseer's `BudgetGate` reads — rather than parsing the raw env and
    // falsely reporting "unset (no guard)" when the reading process lacks the
    // var. Always `Some(resolved)`.
    let daily_budget = Some(crate::overseer::config::daily_budget_usd());

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

// ── overseer activity feed (#2419) ───────────────────────────────────────────

/// Build the OVERSEER section from the durable
/// [activity feed](crate::overseer::activity), distinguishing the honest states
/// the feed reference pins. The acting-Overseer gate
/// ([`overseer_acting_enabled`](crate::overseer::config::overseer_acting_enabled))
/// is the live source of truth for `enabled`, so a disabled Overseer is a
/// *present* state (`Overseer: disabled`), never a blank panel — even when no
/// feed file exists yet. Assembled in isolation; never panics.
///
/// Freshness is **cadence-relative** (`2 × cadence_secs`), NOT the fixed
/// [`SNAPSHOT_FRESHNESS_SECS`]: the Overseer ticks on its own (default 15-minute)
/// cadence, so reusing the 300 s telemetry threshold would mark a healthy feed
/// `stale` for two-thirds of every cadence window.
fn assemble_overseer(state_root: &std::path::Path) -> SectionEnvelope<OverseerActivity> {
    use crate::overseer::config;

    let enabled = config::overseer_acting_enabled();
    let path = activity::activity_path(state_root);
    let file_exists = path.is_file();

    match activity::read(&path) {
        Some(mut feed) => {
            // The config gate is the live truth for the acting Overseer; the
            // stored flag only reflects the state at the last write.
            feed.enabled = enabled;
            let note = format!("Overseer: {}", feed.status_summary());

            if !enabled {
                // Disabled is PRESENT (live), so the UI says "disabled" plainly
                // rather than pretending there is no data.
                let as_of = feed.last_tick_at.clone();
                let mut env = SectionEnvelope::live(feed, as_of);
                env.note = Some(note);
                return env;
            }

            match feed.last_tick_at.clone() {
                Some(ts) => {
                    let stale = feed_is_stale(&ts, feed.cadence_secs);
                    let mut env = if stale {
                        SectionEnvelope::stale(feed, Some(ts))
                    } else {
                        SectionEnvelope::live(feed, Some(ts))
                    };
                    env.note = Some(note);
                    env
                }
                // Enabled, file present, but no ticks recorded in it.
                None => SectionEnvelope::absent("Overseer: no ticks recorded yet"),
            }
        }
        None => {
            if !enabled {
                // Disabled with no readable file: synthesize a present, honest
                // "disabled" section from config so the surfaces still say so.
                let feed = OverseerActivity {
                    enabled: false,
                    cadence_secs: config::overseer_interval_secs(),
                    author_login: config::overseer_author_login(),
                    last_tick_at: None,
                    ..OverseerActivity::default()
                };
                let mut env = SectionEnvelope::live(feed, None);
                env.note = Some("Overseer: disabled".to_string());
                return env;
            }
            // Enabled but unreachable: distinguish "never ticked" (no file) from
            // "unreadable/corrupt" (file present but did not parse) honestly.
            if file_exists {
                SectionEnvelope::absent("Overseer activity feed unavailable")
            } else {
                SectionEnvelope::absent("Overseer: no ticks recorded yet")
            }
        }
    }
}

/// Whether a feed's last tick is older than `2 × cadence_secs`. An unparseable
/// timestamp is treated as fresh (not stale), matching the telemetry provider's
/// tolerant behavior.
fn feed_is_stale(last_tick_at: &str, cadence_secs: u64) -> bool {
    match chrono::DateTime::parse_from_rfc3339(last_tick_at) {
        Ok(ts) => {
            let age = chrono::Utc::now().signed_duration_since(ts.with_timezone(&chrono::Utc));
            let window = 2 * cadence_secs.max(1) as i64;
            age.num_seconds() > window
        }
        Err(_) => false,
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[cfg(test)]
mod live_engineers_tests {
    use super::*;

    /// Issue #2432 (design G4/G5): `resources.live_engineers` must derive from the
    /// authoritative live worktree dispatch-claim set — a sentinel PID per real
    /// engineer worktree, verified alive — NOT the retired `pgrep 'simard-engineer'`
    /// (hyphen) pattern that never matched the real `simard engineer` (space) argv.
    /// A live claim under the state root must be counted.
    #[test]
    fn live_engineers_derives_from_live_worktree_claims() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("status-live-engineers-test");
        let _ = std::fs::remove_dir_all(&dir);
        let wt = dir
            .join(crate::engineer_worktree::WORKTREES_SUBDIR)
            .join("goal-live-1");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE),
            format!("{}\n", std::process::id()),
        )
        .unwrap();

        let resources = assemble_resources(None, &dir);
        let _ = std::fs::remove_dir_all(&dir);

        let data = resources
            .data
            .expect("resources section should be live on a host with /proc");
        assert_eq!(
            data.live_engineers,
            Some(1),
            "the one live worktree dispatch claim must be counted as a live engineer"
        );
    }
}

#[cfg(test)]
mod pure_helper_tests {
    use super::*;
    use crate::status::{Availability, Freshness, SCHEMA_VERSION};

    #[test]
    fn parse_kb_reads_leading_number_as_kib() {
        assert_eq!(parse_kb("2048 kB"), Some(2048 * 1024));
        assert_eq!(parse_kb("15"), Some(15 * 1024));
        assert_eq!(parse_kb("MemFree: junk"), None);
        assert_eq!(parse_kb(""), None);
    }

    #[test]
    fn clamp_u64_floors_negatives_at_zero() {
        assert_eq!(clamp_u64(-1), 0);
        assert_eq!(clamp_u64(i64::MIN), 0);
        assert_eq!(clamp_u64(0), 0);
        assert_eq!(clamp_u64(123), 123);
    }

    #[test]
    fn add_window_accumulates_cost_and_tokens() {
        let mut w = LedgerWindow::default();
        add_window(&mut w, 1.5, 10, 5);
        add_window(&mut w, 2.5, 20, 7);
        assert_eq!(w.tokens_in, 30);
        assert_eq!(w.tokens_out, 12);
        assert!((w.cost_usd - 4.0).abs() < 1e-9);
    }

    #[test]
    fn llm_over_budget_compares_spend_to_budget() {
        let over = SectionEnvelope::live(
            LlmUsage {
                daily_budget_usd: Some(100.0),
                ledger_today: Some(LedgerWindow {
                    cost_usd: 150.0,
                    ..Default::default()
                }),
                ..Default::default()
            },
            None,
        );
        assert_eq!(llm_over_budget(&over), Some(true));

        let under = SectionEnvelope::live(
            LlmUsage {
                daily_budget_usd: Some(100.0),
                ledger_today: Some(LedgerWindow {
                    cost_usd: 50.0,
                    ..Default::default()
                }),
                ..Default::default()
            },
            None,
        );
        assert_eq!(llm_over_budget(&under), Some(false));

        let no_budget = SectionEnvelope::live(
            LlmUsage {
                daily_budget_usd: None,
                ledger_today: Some(LedgerWindow::default()),
                ..Default::default()
            },
            None,
        );
        assert_eq!(llm_over_budget(&no_budget), None);

        assert_eq!(
            llm_over_budget(&SectionEnvelope::<LlmUsage>::absent("no ledger")),
            None
        );
    }

    #[test]
    fn assemble_gym_reports_skip_flag_as_live_section() {
        let on = assemble_gym(true);
        assert!(on.is_present());
        let g = on.data.as_ref().unwrap();
        assert!(g.skip_gym);
        assert_eq!(g.self_eval_state, "idle");

        let off = assemble_gym(false);
        assert!(!off.data.unwrap().skip_gym);
    }

    #[test]
    fn snapshot_is_stale_uses_freshness_window() {
        assert!(!snapshot_is_stale(&snapshot::now_rfc3339()));
        let old = (chrono::Utc::now() - chrono::Duration::seconds(SNAPSHOT_FRESHNESS_SECS + 60))
            .to_rfc3339();
        assert!(snapshot_is_stale(&old));
        // Tolerant: an unparseable timestamp is treated as fresh, not stale.
        assert!(!snapshot_is_stale("not-a-timestamp"));
    }

    #[test]
    fn feed_is_stale_uses_double_cadence_window() {
        let cadence = 900u64;
        assert!(!feed_is_stale(&snapshot::now_rfc3339(), cadence));
        let old =
            (chrono::Utc::now() - chrono::Duration::seconds(2 * cadence as i64 + 60)).to_rfc3339();
        assert!(feed_is_stale(&old, cadence));
        assert!(!feed_is_stale("bogus", cadence));
    }

    #[test]
    fn snapshot_section_picks_live_when_fresh_and_stale_when_old() {
        let fresh = snapshot::now_rfc3339();
        let live = snapshot_section(Gym::default(), &fresh);
        assert_eq!(live.availability, Availability::Ok);
        assert_eq!(live.freshness, Freshness::Live);
        assert_eq!(live.as_of.as_deref(), Some(fresh.as_str()));

        let old = (chrono::Utc::now() - chrono::Duration::seconds(SNAPSHOT_FRESHNESS_SECS + 60))
            .to_rfc3339();
        let stale = snapshot_section(Gym::default(), &old);
        assert_eq!(stale.freshness, Freshness::Stale);
    }

    #[test]
    fn assemble_memory_is_absent_without_metrics_or_gauges() {
        let root = std::path::Path::new("/nonexistent-simard-state");
        assert!(!assemble_memory(None, root).is_present());

        let empty = snapshot::MetricsSnapshot::empty();
        let env = assemble_memory(Some(&empty), root);
        assert!(!env.is_present());
        assert_eq!(env.note.as_deref(), Some("memory gauges: not in snapshot"));
    }

    #[test]
    fn assemble_memory_present_sums_node_gauges() {
        let mut m = snapshot::MetricsSnapshot::empty();
        for (ty, value) in [("episodic", 10), ("semantic", 20)] {
            m.gauges.push(snapshot::GaugeSeries {
                name: names::MEMORY_NODES.to_string(),
                attrs: vec![(names::ATTR_TYPE.to_string(), ty.to_string())],
                value,
            });
        }
        let env = assemble_memory(Some(&m), std::path::Path::new("/nonexistent-simard-state"));
        assert!(env.is_present());
        let data = env.data.as_ref().unwrap();
        assert_eq!(data.nodes.episodic, Some(10));
        assert_eq!(data.nodes.semantic, Some(20));
        assert_eq!(data.nodes_total, Some(30));
        assert_eq!(data.backend, "amplihack-memory-lib");
    }

    #[test]
    fn assemble_telemetry_absent_without_metrics() {
        assert!(!assemble_telemetry(None, false, None, None).is_present());
    }

    #[test]
    fn assemble_telemetry_derives_flags_and_anomalies() {
        let mut m = snapshot::MetricsSnapshot::empty();
        m.counters.push(snapshot::CounterSeries {
            name: names::DISTILL_RUNS.to_string(),
            attrs: vec![(names::ATTR_RESULT.to_string(), "ok".to_string())],
            value: 9,
        });
        m.counters.push(snapshot::CounterSeries {
            name: names::DISTILL_RUNS.to_string(),
            attrs: vec![(names::ATTR_RESULT.to_string(), "parse_fail".to_string())],
            value: 1,
        });
        let env = assemble_telemetry(Some(&m), true, Some(3), Some(true));
        assert!(env.is_present());
        let t = env.data.as_ref().unwrap();
        assert_eq!(t.budget_flag, "over");
        assert_eq!(t.restart_churn, Some(3));
        assert!(t.gym_skipped);
        assert_eq!(t.parse_fix_holding, Some(false));
        assert!(t.distill_fail_pct.is_some());
        assert!(
            t.anomalies
                .iter()
                .any(|a| a.contains("daily LLM budget exceeded"))
        );
        assert!(t.anomalies.iter().any(|a| a.contains("distill parse-fail")));
    }

    #[test]
    fn assemble_is_total_and_degrades_unwired_sources_to_absent() {
        let dir =
            std::env::temp_dir().join(format!("simard-status-assemble-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = AssembleOptions::with_state_root(dir.clone());
        let snap = assemble(&opts);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(snap.schema_version, SCHEMA_VERSION);
        assert!(!snap.generated_at.is_empty());
        // Sources not wired in this process context degrade to absent, never panic.
        assert!(!snap.goals.is_present());
        assert!(!snap.completed.is_present());
        assert!(!snap.self_improvement.is_present());
    }

    #[test]
    fn assemble_options_with_state_root_overrides_only_the_root() {
        let root = std::path::PathBuf::from("/tmp/simard-status-opts");
        let opts = AssembleOptions::with_state_root(root.clone());
        assert_eq!(opts.state_root, root);
        assert_eq!(opts.service_unit, "simard.service");
        assert!(opts.sections.is_none());
    }

    // ── daemon heartbeat fallback (#4215) ─────────────────────────────
    // In non-systemd deployments (dev / worktree / container) `systemctl show`
    // reports no loaded unit, so `assemble_daemon` falls back to the durable
    // `daemon_health.json` heartbeat instead of marking the daemon absent.

    /// A fresh heartbeat renders the daemon as running (with phase) and live.
    #[test]
    fn daemon_from_heartbeat_fresh_is_running_live() {
        let now = chrono::Utc::now();
        let health = serde_json::json!({
            "timestamp": (now - chrono::Duration::seconds(30)).to_rfc3339(),
            "status": "running",
            "cycle_phase": "sleep",
            "cycle_number": 1723,
        });
        let env = daemon_from_heartbeat(&health, now);
        assert_eq!(env.availability, Availability::Ok);
        assert_eq!(env.freshness, Freshness::Live);
        let d = env.data.as_ref().expect("running daemon carries data");
        assert_eq!(d.state, "running (sleep)");
        assert!(env.as_of.is_some(), "as_of carries the heartbeat timestamp");
        // Fields the heartbeat does not record stay honestly None.
        assert!(d.main_pid.is_none());
        assert!(d.n_restarts.is_none());
    }

    /// A heartbeat older than the staleness window renders as stale, not absent.
    #[test]
    fn daemon_from_heartbeat_old_is_stale() {
        let now = chrono::Utc::now();
        let health = serde_json::json!({
            "timestamp": (now
                - chrono::Duration::seconds(DAEMON_HEARTBEAT_STALE_SECS + 60))
            .to_rfc3339(),
            "cycle_phase": "orient",
        });
        let env = daemon_from_heartbeat(&health, now);
        assert_eq!(env.availability, Availability::Ok);
        assert_eq!(env.freshness, Freshness::Stale);
        assert_eq!(env.data.as_ref().unwrap().state, "stale (orient)");
    }

    /// No `cycle_phase` yields a bare state without an empty `( )` suffix.
    #[test]
    fn daemon_from_heartbeat_without_phase_has_no_suffix() {
        let now = chrono::Utc::now();
        let health = serde_json::json!({
            "timestamp": now.to_rfc3339(),
        });
        let env = daemon_from_heartbeat(&health, now);
        assert_eq!(env.data.as_ref().unwrap().state, "running");
    }

    /// A missing / invalid timestamp is "unknown" and stale — never fabricated
    /// as running.
    #[test]
    fn daemon_from_heartbeat_missing_timestamp_is_unknown_stale() {
        let now = chrono::Utc::now();
        let health = serde_json::json!({ "cycle_phase": "decide" });
        let env = daemon_from_heartbeat(&health, now);
        assert_eq!(env.freshness, Freshness::Stale);
        assert_eq!(env.data.as_ref().unwrap().state, "unknown (decide)");
        assert!(env.as_of.is_none());
    }

    /// Fail-visible: a missing heartbeat file degrades this one section to
    /// absent with the honest systemctl note (never panics, never fabricates).
    #[test]
    fn read_daemon_heartbeat_missing_file_is_absent() {
        let missing = std::env::temp_dir().join(format!(
            "simard-status-nohealth-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let env = read_daemon_heartbeat(&missing, chrono::Utc::now());
        assert_eq!(env.availability, Availability::Unavailable);
        assert_eq!(env.freshness, Freshness::Absent);
        assert_eq!(env.note.as_deref(), Some("systemctl: unit not loaded"));
    }

    /// A readable, fresh heartbeat file reads back as a running/live daemon.
    #[test]
    fn read_daemon_heartbeat_reads_fresh_file_as_running() {
        let now = chrono::Utc::now();
        let dir = std::env::temp_dir().join(format!(
            "simard-status-health-{}-{}",
            std::process::id(),
            now.timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon_health.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "timestamp": now.to_rfc3339(),
                "status": "running",
                "cycle_phase": "act",
            })
            .to_string(),
        )
        .unwrap();
        let env = read_daemon_heartbeat(&path, now);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(env.availability, Availability::Ok);
        assert_eq!(env.freshness, Freshness::Live);
        assert_eq!(env.data.as_ref().unwrap().state, "running (act)");
    }

    /// A corrupt (non-JSON) heartbeat also degrades to the honest absent note.
    #[test]
    fn read_daemon_heartbeat_corrupt_file_is_absent() {
        let now = chrono::Utc::now();
        let dir = std::env::temp_dir().join(format!(
            "simard-status-badhealth-{}-{}",
            std::process::id(),
            now.timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon_health.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        let env = read_daemon_heartbeat(&path, now);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(env.availability, Availability::Unavailable);
        assert_eq!(env.note.as_deref(), Some("systemctl: unit not loaded"));
    }
}
