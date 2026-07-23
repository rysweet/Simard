//! Canonical terminal renderer for [`StatusSnapshot`].
//!
//! The section headers and labels here are the operator-approved layout the
//! dashboard and TUI mirror. A missing count is rendered `absent` /
//! `unavailable (<reason>)` — **never** a fabricated `0`.

use std::fmt::Write as _;

use super::{Availability, Freshness, SectionEnvelope, StatusSnapshot};

/// The stable section headers, in render order. Surfaces and tests pin these.
pub const SECTION_HEADERS: &[&str] = &[
    "DAEMON / UPTIME",
    "RESOURCE SNAPSHOT",
    "LLM USAGE",
    "MEMORY / BRAIN",
    "GYM",
    "GOAL BOARD",
    "ACTIVE WORKSTREAMS",
    "COMPLETED WORK",
    "SELF-IMPROVEMENT",
    "TELEMETRY / UNEXPECTED SIGNALS",
    "OVERSEER",
];

const LABEL_WIDTH: usize = 18;

/// Render the full snapshot to the canonical terminal layout.
pub fn to_terminal(snapshot: &StatusSnapshot) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "SIMARD STATUS  ·  {}", snapshot.generated_at);
    out.push('\n');

    render_daemon(&mut out, &snapshot.daemon);
    render_resources(&mut out, &snapshot.resources);
    render_llm(&mut out, &snapshot.llm);
    render_memory(&mut out, &snapshot.memory);
    render_gym(&mut out, &snapshot.gym);
    render_goals(&mut out, &snapshot.goals);
    render_workstreams(&mut out, &snapshot.workstreams);
    render_completed(&mut out, &snapshot.completed);
    render_self_improvement(&mut out, &snapshot.self_improvement);
    render_telemetry(&mut out, &snapshot.telemetry);
    render_overseer(&mut out, &snapshot.overseer);

    out
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn header(out: &mut String, title: &str) {
    let _ = writeln!(out, "{title}");
}

fn label(out: &mut String, name: &str, value: impl AsRef<str>) {
    let _ = writeln!(out, "  {name:<LABEL_WIDTH$}{}", value.as_ref());
}

/// If a section is not present, render its absence and return `false`.
fn absent_marker<T>(out: &mut String, env: &SectionEnvelope<T>) -> bool {
    if env.is_present() {
        return true;
    }
    let reason = env.note.clone().unwrap_or_else(|| "no source".to_string());
    match env.availability {
        Availability::Error => label(out, "", format!("error ({reason})")),
        _ => match env.freshness {
            Freshness::Absent => label(out, "", format!("unavailable ({reason})")),
            _ => label(out, "", format!("stale ({reason})")),
        },
    }
    false
}

/// A trailing `(stale)` marker when a present section is stale.
fn stale_suffix<T>(env: &SectionEnvelope<T>) -> &'static str {
    if env.freshness == Freshness::Stale {
        "  (stale)"
    } else {
        ""
    }
}

fn opt_u64(v: Option<u64>) -> String {
    v.map(|n| n.to_string())
        .unwrap_or_else(|| "absent".to_string())
}

fn opt_f64(v: Option<f64>, digits: usize) -> String {
    v.map(|n| format!("{n:.digits$}"))
        .unwrap_or_else(|| "absent".to_string())
}

fn opt_str(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "absent".to_string())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

fn opt_bytes(v: Option<u64>) -> String {
    v.map(format_bytes).unwrap_or_else(|| "absent".to_string())
}

// ── sections ────────────────────────────────────────────────────────────────

fn render_daemon(out: &mut String, env: &SectionEnvelope<super::Daemon>) {
    header(out, "DAEMON / UPTIME");
    if absent_marker(out, env)
        && let Some(d) = &env.data
    {
        label(out, "state", format!("{}{}", d.state, stale_suffix(env)));
        let ver = match &d.deployed_commit {
            Some(c) => format!("{}  (deployed commit {c})", d.version),
            None => d.version.clone(),
        };
        label(out, "version", ver);
        label(out, "main PID", opt_u64(d.main_pid.map(u64::from)));
        let up = match (&d.instance_uptime, &d.running_since) {
            (Some(u), Some(s)) => format!("{u}   (running since {s})"),
            (Some(u), None) => u.clone(),
            _ => "absent".to_string(),
        };
        label(out, "this-instance up", up);
        label(out, "NRestarts", opt_u64(d.n_restarts));
    }
    out.push('\n');
}

fn render_resources(out: &mut String, env: &SectionEnvelope<super::Resources>) {
    header(out, "RESOURCE SNAPSHOT");
    if absent_marker(out, env)
        && let Some(r) = &env.data
    {
        label(
            out,
            "daemon CPU / RSS",
            format!(
                "{}%  ·  {}   (cgroup mem peak {})",
                opt_f64(r.cpu_pct, 1),
                opt_bytes(r.rss_bytes),
                opt_bytes(r.cgroup_mem_peak_bytes)
            ),
        );
        label(
            out,
            "load avg",
            format!(
                "{} / {} / {}   (1 / 5 / 15m)",
                opt_f64(r.load_1, 2),
                opt_f64(r.load_5, 2),
                opt_f64(r.load_15, 2)
            ),
        );
        label(
            out,
            "system mem",
            format!(
                "{} used / {}   ({} avail)",
                opt_bytes(r.sys_mem_used_bytes),
                opt_bytes(r.sys_mem_total_bytes),
                opt_bytes(r.sys_mem_avail_bytes)
            ),
        );
        let disk = |d: &Option<super::DiskUsage>| match d {
            Some(u) => format!(
                "{} free / {}",
                format_bytes(u.free_bytes),
                format_bytes(u.total_bytes)
            ),
            None => "absent".to_string(),
        };
        label(out, "disk /home", disk(&r.disk_home));
        label(out, "disk /tmp", disk(&r.disk_tmp));
        label(
            out,
            "live engineers",
            opt_u64(r.live_engineers.map(u64::from)),
        );
    }
    out.push('\n');
}

fn render_llm(out: &mut String, env: &SectionEnvelope<super::LlmUsage>) {
    header(out, "LLM USAGE");
    if absent_marker(out, env)
        && let Some(l) = &env.data
    {
        match &l.copilot_turn {
            Some(t) => label(
                out,
                "copilot turn",
                format!(
                    "in {}  cached {}  out {}   ·  AI-credits {}",
                    t.tokens_in, t.tokens_cached, t.tokens_out, t.ai_credits
                ),
            ),
            None => label(out, "copilot turn", "absent"),
        }
        let win = |w: &Option<super::LedgerWindow>| match w {
            Some(w) => format!(
                "${:.2}   in {}  out {}",
                w.cost_usd, w.tokens_in, w.tokens_out
            ),
            None => "absent".to_string(),
        };
        label(out, "ledger today", win(&l.ledger_today));
        label(out, "ledger 7d", win(&l.ledger_7d));
        label(out, "ledger all-time", win(&l.ledger_all_time));
        match l.daily_budget_usd {
            Some(b) => {
                let spent = l.ledger_today.as_ref().map(|w| w.cost_usd).unwrap_or(0.0);
                label(out, "daily budget", format!("${spent:.2} / ${b:.2}"));
            }
            None => label(out, "daily budget", "n/a"),
        }
        match &l.reconciliation {
            Some(r) => label(
                out,
                "reconciliation",
                format!(
                    "ledger ${:.2}  vs  credits {}   ·  {}",
                    r.ledger_usd, r.credits, r.delta_flag
                ),
            ),
            None => label(out, "reconciliation", "absent"),
        }
    }
    out.push('\n');
}

fn render_memory(out: &mut String, env: &SectionEnvelope<super::MemoryBrain>) {
    header(out, "MEMORY / BRAIN");
    if absent_marker(out, env)
        && let Some(m) = &env.data
    {
        label(
            out,
            "store",
            format!(
                "{}  ·  {}  ({})",
                m.store_path,
                opt_bytes(m.store_size_bytes),
                m.backend
            ),
        );
        label(out, "nodes", format!("{} total", opt_u64(m.nodes_total)));
        let n = &m.nodes;
        label(
            out,
            "",
            format!(
                "episodic {}  ·  semantic (facts) {}  ·  prospective (triggers) {}",
                opt_u64(n.episodic),
                opt_u64(n.semantic),
                opt_u64(n.prospective)
            ),
        );
        label(
            out,
            "",
            format!(
                "working {}  ·  procedural {}  ·  sensory {}",
                opt_u64(n.working),
                opt_u64(n.procedural),
                opt_u64(n.sensory)
            ),
        );
        let e = &m.edges;
        label(
            out,
            "edges",
            format!(
                "DERIVES_FROM {}  ·  SIMILAR_TO {}  ·  SUPERSEDES {}",
                opt_u64(e.derives_from),
                opt_u64(e.similar_to),
                opt_u64(e.supersedes)
            ),
        );
        let c = &m.cognitive_processes;
        label(
            out,
            "cognitive",
            format!(
                "distillation {}  ·  consolidation {}  ·  introspection {}",
                opt_str(&c.distillation),
                opt_str(&c.consolidation),
                opt_str(&c.introspection)
            ),
        );
        label(
            out,
            "brains",
            format!(
                "LLM-backed {}  ·  fallbacks {}  ·  decide ladder_exhausted {}",
                opt_str(&m.brains_llm_backed),
                opt_u64(m.brain_fallbacks),
                opt_u64(m.decide_ladder_exhausted)
            ),
        );
    }
    out.push('\n');
}

fn render_gym(out: &mut String, env: &SectionEnvelope<super::Gym>) {
    header(out, "GYM");
    if absent_marker(out, env)
        && let Some(g) = &env.data
    {
        label(
            out,
            "SIMARD_SKIP_GYM",
            if g.skip_gym {
                "set (gym skipped)"
            } else {
                "unset (gym enabled)"
            },
        );
        label(
            out,
            "scenarios",
            format!(
                "{} configured",
                opt_u64(g.configured_scenarios.map(u64::from))
            ),
        );
        label(
            out,
            "self-eval",
            if g.self_eval_state.is_empty() {
                "unknown".to_string()
            } else {
                g.self_eval_state.clone()
            },
        );
    }
    out.push('\n');
}

fn render_goals(out: &mut String, env: &SectionEnvelope<super::GoalBoard>) {
    header(out, "GOAL BOARD");
    if absent_marker(out, env)
        && let Some(g) = &env.data
    {
        if g.active.is_empty() {
            label(out, "", "no active goals");
        }
        for goal in &g.active {
            let _ = writeln!(
                out,
                "  [{}] {:<14} {:<42} ({})",
                goal.priority, goal.status, goal.summary, goal.short_id
            );
        }
    }
    out.push('\n');
}

fn render_workstreams(out: &mut String, env: &SectionEnvelope<super::Workstreams>) {
    header(out, "ACTIVE WORKSTREAMS");
    if absent_marker(out, env)
        && let Some(w) = &env.data
    {
        if w.operator_recipes.is_empty() && w.engineer_workstreams.is_empty() {
            label(out, "", "none active");
        }
        for r in &w.operator_recipes {
            let _ = writeln!(out, "  recipe   {:<22} {}", r.label, r.status);
        }
        for e in &w.engineer_workstreams {
            let _ = writeln!(out, "  engineer {:<22} {}", e.label, e.status);
        }
    }
    out.push('\n');
}

fn render_completed(out: &mut String, env: &SectionEnvelope<super::CompletedWork>) {
    header(out, "COMPLETED WORK (merged PRs, last ~24h)");
    if absent_marker(out, env)
        && let Some(c) = &env.data
    {
        if c.repos.is_empty() {
            label(out, "", "none");
        }
        for repo in &c.repos {
            let _ = writeln!(out, "  {}", repo.repo);
            for pr in &repo.prs {
                let _ = writeln!(
                    out,
                    "    #{:<6} {:<48} {}",
                    pr.number, pr.summary, pr.status
                );
            }
        }
    }
    out.push('\n');
}

fn render_self_improvement(out: &mut String, env: &SectionEnvelope<super::SelfImprovement>) {
    header(out, "SELF-IMPROVEMENT");
    if absent_marker(out, env)
        && let Some(s) = &env.data
    {
        for pr in &s.merged {
            let _ = writeln!(out, "  merged   #{:<6} {}", pr.number, pr.summary);
        }
        for r in &s.running {
            let _ = writeln!(out, "  running  {:<22} {}", r.label, r.status);
        }
        if s.pending.is_empty() {
            let _ = writeln!(out, "  pending  —");
        }
        for p in &s.pending {
            let _ = writeln!(out, "  pending  {:<22} {}", p.label, p.status);
        }
    }
    out.push('\n');
}

fn render_telemetry(out: &mut String, env: &SectionEnvelope<super::TelemetrySignals>) {
    let title = match &env.data {
        Some(t) if !t.window.is_empty() => format!("TELEMETRY / UNEXPECTED SIGNALS ({})", t.window),
        _ => "TELEMETRY / UNEXPECTED SIGNALS".to_string(),
    };
    header(out, &title);
    if absent_marker(out, env)
        && let Some(t) = &env.data
    {
        let holding = match t.parse_fix_holding {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        };
        let pct = match t.distill_fail_pct {
            Some(p) => format!("{p:.0}%"),
            None => "unknown".to_string(),
        };
        label(
            out,
            "parse-fix holding",
            format!("{holding} (distill parse-fail {pct})"),
        );
        label(
            out,
            "restart churn",
            match t.restart_churn {
                Some(0) => "none".to_string(),
                Some(n) => format!("{n} restarts"),
                None => "unknown".to_string(),
            },
        );
        label(out, "gym skipped", if t.gym_skipped { "yes" } else { "no" });
        label(
            out,
            "budget",
            if t.budget_flag.is_empty() {
                "unknown".to_string()
            } else {
                t.budget_flag.clone()
            },
        );
        label(
            out,
            "anomalies",
            if t.anomalies.is_empty() {
                "none".to_string()
            } else {
                t.anomalies.join(", ")
            },
        );
    }
    out.push('\n');
}

// ── overseer activity feed (#2419) ────────────────────────────────────────────

fn render_overseer(
    out: &mut String,
    env: &SectionEnvelope<crate::overseer::activity::OverseerActivity>,
) {
    header(out, "OVERSEER");
    if absent_marker(out, env)
        && let Some(a) = &env.data
    {
        // Honest one-line status: "disabled" / "enabled, observing, 0
        // interventions" / "enabled, N interventions".
        label(
            out,
            "overseer",
            format!("Overseer: {}{}", a.status_summary(), stale_suffix(env)),
        );
        label(
            out,
            "cadence",
            format!(
                "every {}  ·  as {}",
                crate::overseer::activity::human_cadence(a.cadence_secs),
                a.author_login
            ),
        );
        label(out, "last tick", opt_str(&a.last_tick_at));

        // Per-thread status rows (name, on/off, last run, next due, health).
        if a.threads.is_empty() {
            label(out, "threads", "none reported");
        } else {
            for t in &a.threads {
                let _ = writeln!(
                    out,
                    "  thread {:<16} {}  ·  last {}  ·  next {}  ·  {}",
                    t.id,
                    if t.enabled { "on" } else { "off" },
                    opt_str(&t.last_run),
                    opt_str(&t.next_due),
                    t.health,
                );
            }
        }

        // Recent-activity timeline, newest-first, in plain language.
        if a.recent.is_empty() {
            label(out, "recent", "no ticks recorded yet");
        } else {
            let shown = a.recent.len().min(RECENT_ROWS);
            for r in a.recent.iter().take(RECENT_ROWS) {
                let _ = writeln!(
                    out,
                    "  {}  {}",
                    r.timestamp,
                    crate::overseer::activity::humanize_tick(&r.report)
                );
                // WHAT it observed + WHAT it did, beneath the summary (issue #21).
                let details = crate::overseer::activity::humanize_tick_details(&r.report);
                let detail_shown = details.len().min(DETAIL_ROWS);
                for d in details.iter().take(DETAIL_ROWS) {
                    let _ = writeln!(out, "      {d}");
                }
                if details.len() > detail_shown {
                    let _ = writeln!(out, "      … {} more", details.len() - detail_shown);
                }
            }
            if a.recent.len() > shown {
                label(
                    out,
                    "",
                    format!("… {} older tick(s) retained", a.recent.len() - shown),
                );
            }
        }
    }
    out.push('\n');
}

/// How many recent-activity rows the terminal render shows before summarizing.
const RECENT_ROWS: usize = 20;

/// How many per-tick detail lines the terminal render shows before summarizing.
const DETAIL_ROWS: usize = 12;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overseer::activity::{
        OverseerActivity, OverseerActivityRecord, OverseerThreadStatus,
    };
    use crate::status::{
        CognitiveHealth, CompletedWork, CopilotTurn, Daemon, DiskUsage, EdgeCounts, GoalBoard,
        GoalItem, Gym, LedgerWindow, LlmUsage, MemoryBrain, NodeCounts, PrItem, Reconciliation,
        RepoPrs, Resources, SelfImprovement, TelemetrySignals, WorkItem, Workstreams,
    };

    const AS_OF: &str = "2026-07-06T12:00:00Z";

    /// A snapshot with every section `live` and fully populated, so the rendered
    /// output exercises each section's present-data branch.
    fn full_snapshot() -> StatusSnapshot {
        let mut snap = StatusSnapshot::empty();
        snap.generated_at = AS_OF.to_string();

        snap.daemon = SectionEnvelope::live(
            Daemon {
                state: "running".to_string(),
                version: "0.27.0".to_string(),
                main_pid: Some(4242),
                deployed_commit: Some("abc123".to_string()),
                instance_uptime: Some("1h2m".to_string()),
                n_restarts: Some(3),
                running_since: Some("2026-07-06T10:58:00Z".to_string()),
            },
            Some(AS_OF.to_string()),
        );

        snap.resources = SectionEnvelope::live(
            Resources {
                cpu_pct: Some(12.5),
                rss_bytes: Some(200 * 1024 * 1024),
                cgroup_mem_peak_bytes: Some(512 * 1024 * 1024),
                load_1: Some(1.5),
                load_5: Some(1.25),
                load_15: Some(0.75),
                sys_mem_used_bytes: Some(4 * 1024 * 1024 * 1024),
                sys_mem_total_bytes: Some(16 * 1024 * 1024 * 1024),
                sys_mem_avail_bytes: Some(12 * 1024 * 1024 * 1024),
                disk_home: Some(DiskUsage {
                    free_bytes: 50 * 1024 * 1024 * 1024,
                    total_bytes: 100 * 1024 * 1024 * 1024,
                }),
                disk_tmp: Some(DiskUsage {
                    free_bytes: 1024 * 1024 * 1024,
                    total_bytes: 2 * 1024 * 1024 * 1024,
                }),
                live_engineers: Some(2),
            },
            Some(AS_OF.to_string()),
        );

        snap.llm = SectionEnvelope::live(
            LlmUsage {
                copilot_turn: Some(CopilotTurn {
                    tokens_in: 100,
                    tokens_cached: 20,
                    tokens_out: 50,
                    ai_credits: 7,
                }),
                ledger_today: Some(LedgerWindow {
                    cost_usd: 12.34,
                    tokens_in: 1000,
                    tokens_out: 500,
                }),
                ledger_7d: Some(LedgerWindow {
                    cost_usd: 56.78,
                    tokens_in: 9000,
                    tokens_out: 4000,
                }),
                ledger_all_time: Some(LedgerWindow {
                    cost_usd: 90.0,
                    tokens_in: 20000,
                    tokens_out: 8000,
                }),
                daily_budget_usd: Some(500.0),
                reconciliation: Some(Reconciliation {
                    ledger_usd: 12.34,
                    credits: 7,
                    delta_flag: "ok".to_string(),
                }),
            },
            Some(AS_OF.to_string()),
        );

        snap.memory = SectionEnvelope::live(
            MemoryBrain {
                store_path: "/state/cognitive".to_string(),
                store_size_bytes: Some(3 * 1024 * 1024),
                backend: "amplihack-memory-lib".to_string(),
                nodes_total: Some(42),
                nodes: NodeCounts {
                    episodic: Some(10),
                    semantic: Some(20),
                    prospective: Some(3),
                    working: Some(4),
                    procedural: Some(5),
                    sensory: Some(0),
                },
                edges: EdgeCounts {
                    derives_from: Some(11),
                    similar_to: Some(22),
                    supersedes: Some(2),
                },
                cognitive_processes: CognitiveHealth {
                    distillation: Some("idle".to_string()),
                    consolidation: Some("idle".to_string()),
                    introspection: Some("idle".to_string()),
                },
                brains_llm_backed: Some("decide, orient".to_string()),
                brain_fallbacks: Some(0),
                decide_ladder_exhausted: Some(0),
            },
            Some(AS_OF.to_string()),
        );

        snap.gym = SectionEnvelope::live(
            Gym {
                skip_gym: true,
                configured_scenarios: Some(9),
                self_eval_state: "idle".to_string(),
            },
            Some(AS_OF.to_string()),
        );

        snap.goals = SectionEnvelope::live(
            GoalBoard {
                active: vec![GoalItem {
                    short_id: "g1".to_string(),
                    priority: "p0".to_string(),
                    status: "in-progress".to_string(),
                    summary: "raise coverage".to_string(),
                }],
            },
            Some(AS_OF.to_string()),
        );

        snap.workstreams = SectionEnvelope::live(
            Workstreams {
                operator_recipes: vec![WorkItem {
                    label: "distill".to_string(),
                    status: "running".to_string(),
                }],
                engineer_workstreams: vec![WorkItem {
                    label: "coverage".to_string(),
                    status: "testing".to_string(),
                }],
            },
            Some(AS_OF.to_string()),
        );

        snap.completed = SectionEnvelope::live(
            CompletedWork {
                repos: vec![RepoPrs {
                    repo: "rysweet/Simard".to_string(),
                    prs: vec![PrItem {
                        number: 2633,
                        summary: "fix status".to_string(),
                        status: "merged".to_string(),
                    }],
                }],
            },
            Some(AS_OF.to_string()),
        );

        snap.self_improvement = SectionEnvelope::live(
            SelfImprovement {
                merged: vec![PrItem {
                    number: 2600,
                    summary: "journal".to_string(),
                    status: "merged".to_string(),
                }],
                running: vec![WorkItem {
                    label: "improve-x".to_string(),
                    status: "running".to_string(),
                }],
                pending: vec![WorkItem {
                    label: "improve-y".to_string(),
                    status: "queued".to_string(),
                }],
            },
            Some(AS_OF.to_string()),
        );

        snap.telemetry = SectionEnvelope::live(
            TelemetrySignals {
                window: "last 1h".to_string(),
                distill_fail_pct: Some(0.0),
                restart_churn: Some(2),
                gym_skipped: true,
                budget_flag: "ok".to_string(),
                parse_fix_holding: Some(true),
                anomalies: vec!["panic in worker".to_string()],
            },
            Some(AS_OF.to_string()),
        );

        snap.overseer = SectionEnvelope::live(overseer_activity(1), Some(AS_OF.to_string()));

        snap
    }

    /// An [`OverseerActivity`] with one thread and `n` recent ticks.
    fn overseer_activity(recent: usize) -> OverseerActivity {
        let mut a = OverseerActivity {
            last_tick_at: Some(AS_OF.to_string()),
            threads: vec![OverseerThreadStatus {
                id: "overseer".to_string(),
                enabled: true,
                last_run: Some(AS_OF.to_string()),
                next_due: Some("2026-07-06T12:15:00Z".to_string()),
                health: "ok".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        for i in 0..recent {
            a.recent.push_back(OverseerActivityRecord {
                timestamp: format!("2026-07-06T12:00:{:02}Z", i % 60),
                enabled: true,
                ..Default::default()
            });
        }
        a
    }

    #[test]
    fn empty_snapshot_includes_every_section_header() {
        let out = to_terminal(&StatusSnapshot::empty());
        for header in SECTION_HEADERS {
            assert!(
                out.contains(header),
                "rendered output is missing section header {header:?}\n{out}"
            );
        }
        assert!(out.starts_with("SIMARD STATUS"));
    }

    #[test]
    fn empty_snapshot_marks_sections_unavailable() {
        let out = to_terminal(&StatusSnapshot::empty());
        // Every section is absent, so each renders an `unavailable (...)` marker
        // rather than a fabricated zero.
        assert!(out.contains("unavailable"));
        assert!(!out.contains("  state "));
    }

    #[test]
    fn full_snapshot_renders_present_data_for_every_section() {
        let out = to_terminal(&full_snapshot());
        // A representative, stable token from each section's present branch.
        assert!(out.contains("deployed commit abc123"), "daemon");
        assert!(out.contains("running since 2026-07-06T10:58:00Z"), "uptime");
        assert!(out.contains("NRestarts"), "restarts");
        assert!(out.contains("50 GiB free / 100 GiB"), "disk /home");
        assert!(out.contains("live engineers"), "live engineers");
        assert!(out.contains("AI-credits 7"), "copilot turn");
        assert!(out.contains("$12.34 / $500.00"), "daily budget");
        assert!(
            out.contains("ledger $12.34  vs  credits 7"),
            "reconciliation"
        );
        assert!(out.contains("episodic 10"), "memory nodes");
        assert!(out.contains("DERIVES_FROM 11"), "memory edges");
        assert!(out.contains("set (gym skipped)"), "gym");
        assert!(out.contains("[p0] in-progress"), "goal board");
        assert!(out.contains("recipe   distill"), "operator recipe");
        assert!(out.contains("engineer coverage"), "engineer workstream");
        assert!(out.contains("rysweet/Simard"), "completed repo");
        assert!(out.contains("#2633"), "completed pr");
        assert!(out.contains("merged   #2600"), "self-improvement merged");
        assert!(
            out.contains("TELEMETRY / UNEXPECTED SIGNALS (last 1h)"),
            "telemetry title"
        );
        assert!(out.contains("panic in worker"), "telemetry anomaly");
        assert!(out.contains("Overseer: "), "overseer status");
        assert!(out.contains("thread overseer"), "overseer thread");
    }

    #[test]
    fn present_but_stale_section_renders_stale_suffix() {
        let mut snap = StatusSnapshot::empty();
        snap.daemon = SectionEnvelope::stale(
            Daemon {
                state: "running".to_string(),
                version: "0.27.0".to_string(),
                ..Default::default()
            },
            Some(AS_OF.to_string()),
        );
        let out = to_terminal(&snap);
        assert!(
            out.contains("running  (stale)"),
            "stale suffix missing:\n{out}"
        );
    }

    #[test]
    fn errored_section_renders_error_marker() {
        let mut snap = StatusSnapshot::empty();
        snap.resources = SectionEnvelope::error("proc read failed");
        let out = to_terminal(&snap);
        assert!(out.contains("error (proc read failed)"), "{out}");
    }

    #[test]
    fn absent_stale_section_renders_stale_marker() {
        // availability != Ok but freshness == Stale exercises absent_marker's
        // `stale (reason)` branch (distinct from `unavailable`/`error`).
        let mut snap = StatusSnapshot::empty();
        snap.gym = SectionEnvelope {
            availability: Availability::Unavailable,
            freshness: Freshness::Stale,
            as_of: None,
            note: Some("last-known".to_string()),
            data: None,
        };
        let out = to_terminal(&snap);
        assert!(out.contains("stale (last-known)"), "{out}");
    }

    #[test]
    fn present_sections_with_none_fields_render_absent_tokens() {
        // A present section whose optional fields are all `None` must render
        // `absent`, never a fabricated `0`.
        let mut snap = StatusSnapshot::empty();
        snap.resources = SectionEnvelope::live(Resources::default(), Some(AS_OF.to_string()));
        snap.daemon = SectionEnvelope::live(Daemon::default(), Some(AS_OF.to_string()));
        let out = to_terminal(&snap);
        assert!(out.contains("absent"), "expected absent tokens:\n{out}");
    }

    #[test]
    fn empty_collections_render_placeholders() {
        let mut snap = StatusSnapshot::empty();
        snap.goals = SectionEnvelope::live(GoalBoard::default(), None);
        snap.workstreams = SectionEnvelope::live(Workstreams::default(), None);
        snap.completed = SectionEnvelope::live(CompletedWork::default(), None);
        snap.self_improvement = SectionEnvelope::live(SelfImprovement::default(), None);
        snap.overseer = SectionEnvelope::live(OverseerActivity::default(), None);
        let out = to_terminal(&snap);
        assert!(out.contains("no active goals"), "goals placeholder");
        assert!(out.contains("none active"), "workstreams placeholder");
        assert!(out.contains("none"), "completed placeholder");
        assert!(
            out.contains("pending  —"),
            "self-improvement pending placeholder"
        );
        assert!(
            out.contains("none reported"),
            "overseer threads placeholder"
        );
        assert!(
            out.contains("no ticks recorded yet"),
            "overseer recent placeholder"
        );
    }

    #[test]
    fn overseer_truncates_recent_beyond_cap() {
        let mut snap = StatusSnapshot::empty();
        snap.overseer =
            SectionEnvelope::live(overseer_activity(RECENT_ROWS + 5), Some(AS_OF.to_string()));
        let out = to_terminal(&snap);
        assert!(out.contains("older tick(s) retained"), "{out}");
    }

    #[test]
    fn telemetry_placeholders_when_values_unknown() {
        let mut snap = StatusSnapshot::empty();
        snap.telemetry = SectionEnvelope::live(
            TelemetrySignals {
                window: String::new(),
                distill_fail_pct: None,
                restart_churn: Some(0),
                gym_skipped: false,
                budget_flag: String::new(),
                parse_fix_holding: None,
                anomalies: Vec::new(),
            },
            Some(AS_OF.to_string()),
        );
        let out = to_terminal(&snap);
        // Empty window keeps the plain header.
        assert!(out.contains("TELEMETRY / UNEXPECTED SIGNALS\n"));
        assert!(
            out.contains("unknown (distill parse-fail unknown)"),
            "holding"
        );
        assert!(out.contains("restart churn"), "churn label");
        assert!(out.contains("none"), "no anomalies");
    }

    #[test]
    fn format_bytes_scales_through_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3 GiB");
        assert_eq!(format_bytes(5_u64 * 1024 * 1024 * 1024 * 1024), "5 TiB");
        // Beyond TiB the unit is capped at TiB rather than overflowing the table.
        assert_eq!(
            format_bytes(2048_u64 * 1024 * 1024 * 1024 * 1024),
            "2048 TiB"
        );
    }

    #[test]
    fn opt_helpers_render_absent_for_none() {
        assert_eq!(opt_u64(None), "absent");
        assert_eq!(opt_u64(Some(7)), "7");
        assert_eq!(opt_f64(None, 2), "absent");
        assert_eq!(opt_f64(Some(1.5), 2), "1.50");
        assert_eq!(opt_str(&None), "absent");
        assert_eq!(opt_str(&Some("x".to_string())), "x");
        assert_eq!(opt_bytes(None), "absent");
        assert_eq!(opt_bytes(Some(2048)), "2 KiB");
    }
}
