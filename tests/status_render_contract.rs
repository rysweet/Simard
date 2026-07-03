//! Content-pin test for the `simard status` terminal renderer (issue #2528).
//!
//! Pins the operator-approved layout in `docs/howto/simard-status.md`: every
//! section header and label the dashboard and TUI mirror, and the "no silent
//! zeros" rule — an absent section renders `unavailable`/`absent`, never a
//! fabricated `0`.

use simard::status::render::{self, SECTION_HEADERS};
use simard::status::{
    CognitiveHealth, CompletedWork, CopilotTurn, Daemon, DiskUsage, EdgeCounts, GoalBoard,
    GoalItem, Gym, LedgerWindow, LlmUsage, MemoryBrain, NodeCounts, PrItem, Reconciliation,
    RepoPrs, Resources, SectionEnvelope, SelfImprovement, StatusSnapshot, TelemetrySignals,
    WorkItem,
};

fn populated() -> StatusSnapshot {
    let mut snap = StatusSnapshot::empty();
    snap.generated_at = "2026-07-03T03:55:05Z".into();

    snap.daemon = SectionEnvelope::live(
        Daemon {
            state: "active (running)".into(),
            version: "0.24.0".into(),
            main_pid: Some(48291),
            deployed_commit: Some("e5764c6d".into()),
            instance_uptime: Some("2h 14m 33s".into()),
            n_restarts: Some(0),
            running_since: Some("2026-07-03T01:40:31Z".into()),
        },
        None,
    );
    snap.resources = SectionEnvelope::live(
        Resources {
            cpu_pct: Some(3.2),
            rss_bytes: Some(184 * 1024 * 1024),
            cgroup_mem_peak_bytes: Some(402 * 1024 * 1024),
            load_1: Some(0.41),
            load_5: Some(0.55),
            load_15: Some(0.60),
            sys_mem_used_bytes: Some(6 * 1024 * 1024 * 1024),
            sys_mem_total_bytes: Some(16 * 1024 * 1024 * 1024),
            sys_mem_avail_bytes: Some(9 * 1024 * 1024 * 1024),
            disk_home: Some(DiskUsage {
                free_bytes: 118 * 1024 * 1024 * 1024,
                total_bytes: 256 * 1024 * 1024 * 1024,
            }),
            disk_tmp: Some(DiskUsage {
                free_bytes: 14 * 1024 * 1024 * 1024,
                total_bytes: 16 * 1024 * 1024 * 1024,
            }),
            live_engineers: Some(2),
        },
        None,
    );
    snap.llm = SectionEnvelope::live(
        LlmUsage {
            copilot_turn: Some(CopilotTurn {
                tokens_in: 4120,
                tokens_cached: 1900,
                tokens_out: 880,
                ai_credits: 12,
            }),
            ledger_today: Some(LedgerWindow {
                cost_usd: 1.87,
                tokens_in: 412000,
                tokens_out: 88000,
            }),
            ledger_7d: Some(LedgerWindow {
                cost_usd: 11.42,
                tokens_in: 2_740_000,
                tokens_out: 610_000,
            }),
            ledger_all_time: Some(LedgerWindow {
                cost_usd: 208.91,
                tokens_in: 51_300_000,
                tokens_out: 9_900_000,
            }),
            daily_budget_usd: Some(25.0),
            reconciliation: Some(Reconciliation {
                ledger_usd: 1.87,
                credits: 940,
                delta_flag: "ok".into(),
            }),
        },
        None,
    );
    snap.memory = SectionEnvelope::live(
        MemoryBrain {
            store_path: "/home/azureuser/.simard/cognitive".into(),
            store_size_bytes: Some(38 * 1024 * 1024),
            backend: "amplihack-memory-lib".into(),
            nodes_total: Some(1842),
            nodes: NodeCounts {
                episodic: Some(1204),
                semantic: Some(380),
                prospective: Some(44),
                working: Some(12),
                procedural: Some(190),
                sensory: Some(12),
            },
            edges: EdgeCounts {
                derives_from: Some(512),
                similar_to: Some(233),
                supersedes: Some(61),
            },
            cognitive_processes: CognitiveHealth {
                distillation: Some("OK".into()),
                consolidation: Some("OK".into()),
                introspection: Some("OK".into()),
            },
            brains_llm_backed: Some("3/3".into()),
            brain_fallbacks: Some(0),
            decide_ladder_exhausted: Some(0),
        },
        None,
    );
    snap.gym = SectionEnvelope::live(
        Gym {
            skip_gym: false,
            configured_scenarios: Some(7),
            self_eval_state: "idle".into(),
        },
        None,
    );
    snap.goals = SectionEnvelope::live(
        GoalBoard {
            active: vec![GoalItem {
                short_id: "goal-2f9c".into(),
                priority: "p0".into(),
                status: "in-progress".into(),
                summary: "Rationalize telemetry onto OpenTelemetry".into(),
            }],
        },
        None,
    );
    snap.workstreams = SectionEnvelope::live(
        simard::status::Workstreams {
            operator_recipes: vec![WorkItem {
                label: "ooda-cycle".into(),
                status: "running — decide phase, cycle 47".into(),
            }],
            engineer_workstreams: vec![WorkItem {
                label: "eng-alpha (goal-2f9c)".into(),
                status: "running — 0h4m, editing src/telemetry/".into(),
            }],
        },
        None,
    );
    snap.completed = SectionEnvelope::live(
        CompletedWork {
            repos: vec![RepoPrs {
                repo: "rysweet/Simard".into(),
                prs: vec![PrItem {
                    number: 2526,
                    summary: "char-boundary-safe truncation".into(),
                    status: "merged".into(),
                }],
            }],
        },
        None,
    );
    snap.self_improvement = SectionEnvelope::live(
        SelfImprovement {
            merged: vec![PrItem {
                number: 2523,
                summary: "char-boundary-safe truncation".into(),
                status: "merged".into(),
            }],
            running: vec![WorkItem {
                label: "self-quality-audit".into(),
                status: "auditing exception handling".into(),
            }],
            pending: vec![],
        },
        None,
    );
    snap.telemetry = SectionEnvelope::live(
        TelemetrySignals {
            window: "last 1h".into(),
            distill_fail_pct: Some(0.0),
            restart_churn: Some(0),
            gym_skipped: false,
            budget_flag: "OK".into(),
            parse_fix_holding: Some(true),
            anomalies: vec![],
        },
        None,
    );
    snap
}

#[test]
fn renders_title_and_every_section_header() {
    let out = render::to_terminal(&populated());
    assert!(out.contains("SIMARD STATUS"), "missing title:\n{out}");
    for header in SECTION_HEADERS {
        assert!(
            out.contains(header),
            "renderer missing section '{header}':\n{out}"
        );
    }
}

#[test]
fn renders_operator_approved_labels_and_values() {
    let out = render::to_terminal(&populated());
    // A representative label from every section — these are the pinned labels
    // the dashboard/TUI mirror.
    for needle in [
        "state",
        "NRestarts",
        "main PID",
        "load avg",
        "system mem",
        "live engineers",
        "copilot turn",
        "ledger today",
        "daily budget",
        "reconciliation",
        "nodes",
        "edges",
        "cognitive",
        "brains",
        "SIMARD_SKIP_GYM",
        "scenarios",
        "recipe",
        "engineer",
        "merged",
        "running",
        "pending",
        "parse-fix holding",
        "restart churn",
        "gym skipped",
        "budget",
        "anomalies",
    ] {
        assert!(
            out.contains(needle),
            "renderer missing label '{needle}':\n{out}"
        );
    }
    // Real values render.
    assert!(out.contains("48291"), "main PID value missing");
    assert!(out.contains("goal-2f9c"), "goal short id missing");
    assert!(out.contains("#2526"), "completed PR number missing");
    assert!(out.contains("rysweet/Simard"), "completed repo missing");
}

#[test]
fn absent_sections_render_unavailable_not_zero() {
    // A fully-empty snapshot: every section absent.
    let out = render::to_terminal(&StatusSnapshot::empty());

    // Headers always render so the operator sees the full frame.
    for header in SECTION_HEADERS {
        assert!(
            out.contains(header),
            "absent frame missing header '{header}'"
        );
    }
    // But no section fabricates data: the detail labels of absent sections must
    // NOT appear, and an absence marker must.
    assert!(
        out.contains("unavailable"),
        "absent sections must be marked:\n{out}"
    );
    assert!(
        !out.contains("NRestarts"),
        "absent daemon must not render its NRestarts detail line:\n{out}"
    );
    assert!(
        !out.contains("ledger today"),
        "absent llm must not render a fabricated ledger:\n{out}"
    );
}

#[test]
fn stale_section_is_marked_stale() {
    let mut snap = StatusSnapshot::empty();
    snap.daemon = SectionEnvelope::stale(
        Daemon {
            state: "active (running)".into(),
            version: "0.24.0".into(),
            ..Default::default()
        },
        None,
    );
    let out = render::to_terminal(&snap);
    assert!(
        out.contains("stale"),
        "a stale section must carry a stale marker:\n{out}"
    );
}
