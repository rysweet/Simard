//! TDD (Step 7 — write tests first) for Problem 6: the **PR Backlog Triage**
//! workflow (`rysweet/Simard`). These tests pin the contract documented in the
//! `pr-triage-docs.md` design artifact. They are RED against the current tree
//! (the `simard::pr_triage` module does not exist yet) and go GREEN once Step 8
//! implements the module to satisfy this contract.
//!
//! The feature triages an explicit allow-list of conflicting (DIRTY) PRs and
//! produces, per PR, exactly one of three dispositions:
//!   * `rebased-and-green`     — conflicts resolved, required checks pass.
//!   * `closed-with-rationale` — obsolete/superseded; closed with a comment.
//!   * `triage-note`           — relevance unclear; comment posted, PR left open.
//!
//! Contract surface this file pins (the shape Step 8 must build):
//!
//!   pub const DEFAULT_ALLOWLIST: &[u64];
//!
//!   pub enum OutputFormat { Text, Json }              // FromStr
//!   pub enum Mergeable    { Mergeable, Conflicting, Unknown }
//!   pub enum MergeStateStatus { Clean, Dirty, Blocked, Behind, Unstable, Unknown }
//!
//!   pub struct TriageConfig {
//!       repo: String, prs: Vec<u64>, dry_run: bool,
//!       format: OutputFormat, serialize_globs: Vec<String>,
//!       worktrees_root: PathBuf,
//!   }   // Default, validate(), is_serialized(paths), action_set(discovered),
//!       // will_mutate()
//!
//!   pub struct ActionSet { to_act: Vec<u64>, candidates: Vec<u64> }
//!
//!   pub struct PrState { number, mergeable, merge_state_status, checks_green }
//!       // needs_rescue(), merge_eligible()
//!
//!   pub struct FreshPoll(PrState);            // poll-before-every-mutation guard
//!
//!   pub struct PrRef { number, title, created: chrono::NaiveDate }
//!   pub fn supersession_closures(group) -> Vec<(u64 /*close*/, u64 /*favor*/)>;
//!
//!   pub fn merge_command(pr) -> Vec<String>;  // never --admin / --force
//!   pub fn close_command(pr, rationale) -> Result<Vec<String>, _>; // rationale required
//!
//!   pub enum Disposition { RebasedAndGreen, ClosedWithRationale{..}, TriageNote{..} }
//!       // action_str()
//!   pub struct DispositionRecord { pr, disposition, detail }
//!   pub struct Escalation { id, kind, detail }
//!   pub fn default_escalations() -> Vec<Escalation>;
//!   pub struct TriageReport { repo, dispositions, escalations }
//!       // to_text(), + serde Serialize (JSON)
//!
//! Fully hermetic: no network, no `gh` calls — pure logic + one source-scan
//! guarantee test.

use simard::pr_triage::{
    self, ActionSet, DEFAULT_ALLOWLIST, Disposition, DispositionRecord, Escalation, FreshPoll,
    MergeStateStatus, Mergeable, OutputFormat, PrRef, PrState, TriageConfig, TriageReport,
    close_command, default_escalations, merge_command, supersession_closures,
};

// ---------------------------------------------------------------------------
// Allow-list & defaults
// ---------------------------------------------------------------------------

#[test]
fn default_allowlist_is_the_nine_documented_prs_in_order() {
    // Exactly the PRs named in the requirements, in the documented order.
    assert_eq!(
        DEFAULT_ALLOWLIST,
        &[4351, 4346, 4334, 4324, 4319, 4303, 4296, 4269, 4230]
    );
}

#[test]
fn config_default_matches_documented_defaults() {
    let cfg = TriageConfig::default();
    assert_eq!(cfg.repo, "rysweet/Simard");
    assert_eq!(cfg.prs, DEFAULT_ALLOWLIST.to_vec());
    assert!(!cfg.dry_run, "default is a real run, not dry-run");
    assert!(matches!(cfg.format, OutputFormat::Text));
    assert_eq!(cfg.worktrees_root, std::path::PathBuf::from("./worktrees/"));
    // --serialize default covers ooda_* and overseer surfaces.
    let globs = cfg.serialize_globs.join(",");
    assert!(
        globs.contains("ooda"),
        "serialize default must cover ooda_*"
    );
    assert!(
        globs.contains("overseer"),
        "serialize default must cover overseer surfaces"
    );
}

#[test]
fn output_format_parses_from_str() {
    assert!(matches!(
        "text".parse::<OutputFormat>(),
        Ok(OutputFormat::Text)
    ));
    assert!(matches!(
        "json".parse::<OutputFormat>(),
        Ok(OutputFormat::Json)
    ));
    assert!("yaml".parse::<OutputFormat>().is_err());
}

// ---------------------------------------------------------------------------
// Least-privilege: act ONLY on the explicit allow-list
// ---------------------------------------------------------------------------

#[test]
fn discovered_dirty_prs_outside_allowlist_are_never_auto_added() {
    let cfg = TriageConfig::default();
    // Discovery surfaces two extra DIRTY PRs not on the allow-list, plus one
    // that is already on it.
    let discovered = [9999_u64, 8888, 4351];
    let ActionSet { to_act, candidates } = cfg.action_set(&discovered);

    // The action set is EXACTLY the allow-list — nothing auto-promoted.
    assert_eq!(to_act, cfg.prs, "must act only on the explicit allow-list");
    // Out-of-scope discoveries are reported as operator-review candidates.
    assert!(candidates.contains(&9999));
    assert!(candidates.contains(&8888));
    assert!(
        !candidates.contains(&4351),
        "an already-in-scope PR is not also a candidate"
    );
}

#[test]
fn empty_pr_list_yields_no_actions() {
    let cfg = TriageConfig {
        prs: vec![],
        ..TriageConfig::default()
    };
    let ActionSet { to_act, .. } = cfg.action_set(&[]);
    assert!(to_act.is_empty());
}

#[test]
fn validate_rejects_non_positive_pr_numbers() {
    let cfg = TriageConfig {
        prs: vec![4351, 0],
        ..TriageConfig::default()
    };
    assert!(cfg.validate().is_err(), "PR number 0 is invalid");

    let ok = TriageConfig {
        prs: vec![4351, 4303],
        ..TriageConfig::default()
    };
    assert!(ok.validate().is_ok());
}

// ---------------------------------------------------------------------------
// Re-poll live merge state — never trust the stale DIRTY label
// ---------------------------------------------------------------------------

#[test]
fn conflicting_live_state_needs_rescue_clean_does_not() {
    let dirty = PrState {
        number: 4230,
        mergeable: Mergeable::Conflicting,
        merge_state_status: MergeStateStatus::Dirty,
        checks_green: false,
    };
    assert!(dirty.needs_rescue());
    assert!(!dirty.merge_eligible());

    let clean = PrState {
        number: 4230,
        mergeable: Mergeable::Mergeable,
        merge_state_status: MergeStateStatus::Clean,
        checks_green: true,
    };
    assert!(!clean.needs_rescue());
    assert!(clean.merge_eligible());
}

#[test]
fn a_pr_labelled_dirty_but_live_clean_is_acted_on_from_live_state() {
    // The stale label said DIRTY; the live re-poll returns CLEAN + green.
    // Decisions MUST follow the live poll, so this PR is merge-eligible and
    // does NOT need a rescue.
    let live = FreshPoll::poll(PrState {
        number: 4303,
        mergeable: Mergeable::Mergeable,
        merge_state_status: MergeStateStatus::Clean,
        checks_green: true,
    });
    assert!(live.merge_eligible());
    assert!(!live.needs_rescue());
}

#[test]
fn merge_eligible_requires_clean_mergeable_and_green_checks() {
    // Mergeable but checks not yet green → not eligible (must wait / drive green).
    let unstable = PrState {
        number: 4303,
        mergeable: Mergeable::Mergeable,
        merge_state_status: MergeStateStatus::Unstable,
        checks_green: false,
    };
    assert!(!unstable.merge_eligible());
}

// ---------------------------------------------------------------------------
// OODA-core serialization: shared done-gate surfaces rebased one-at-a-time
// ---------------------------------------------------------------------------

#[test]
fn ooda_core_and_overseer_paths_are_serialized() {
    let cfg = TriageConfig::default();
    assert!(cfg.is_serialized(&["src/ooda_actions/advance_goal/mod.rs"]));
    assert!(cfg.is_serialized(&["src/ooda_scheduler/tick.rs"]));
    assert!(cfg.is_serialized(&["src/ooda_loop.rs"]));
    assert!(cfg.is_serialized(&["src/ooda_brain/mod.rs"]));
    assert!(cfg.is_serialized(&["src/overseer/merge_authority.rs"]));
}

#[test]
fn non_ooda_paths_are_parallel_safe() {
    let cfg = TriageConfig::default();
    // #4324 — Specs markdown only.
    assert!(!cfg.is_serialized(&["Specs/agent-kgpacks-rs-parity.md"]));
    // #4230 — plain module, no done-gate overlap.
    assert!(!cfg.is_serialized(&["src/fact_reliability.rs"]));
    // A PR touching a mix is serialized if ANY path is a done-gate surface.
    assert!(cfg.is_serialized(&["README.md", "src/ooda_actions/mod.rs"]));
}

// ---------------------------------------------------------------------------
// Supersession groups: older duplicate closed in favor of the newer
// ---------------------------------------------------------------------------

#[test]
fn identical_title_group_closes_the_older_pr_in_favor_of_newer() {
    let group = vec![
        PrRef {
            number: 4269,
            title: "kg relevance stopword filtering".to_string(),
            created: "2026-07-17".parse().unwrap(),
        },
        PrRef {
            number: 4303,
            title: "kg relevance stopword filtering".to_string(),
            created: "2026-07-18".parse().unwrap(),
        },
    ];
    let closures = supersession_closures(&group);
    assert_eq!(
        closures,
        vec![(4269, 4303)],
        "older #4269 closed in favor of newer #4303"
    );
}

#[test]
fn distinct_titles_are_not_a_supersession_group() {
    let group = vec![
        PrRef {
            number: 4319,
            title: "adaptive scaling".to_string(),
            created: "2026-07-16".parse().unwrap(),
        },
        PrRef {
            number: 4296,
            title: "cost tracking".to_string(),
            created: "2026-07-17".parse().unwrap(),
        },
    ];
    assert!(supersession_closures(&group).is_empty());
}

// ---------------------------------------------------------------------------
// Merge/close command safety — least privilege, never bypass protection
// ---------------------------------------------------------------------------

#[test]
fn merge_command_never_uses_admin_or_force() {
    let cmd = merge_command(4303);
    let joined = cmd.join(" ");
    assert!(joined.contains("4303"), "targets the PR number");
    assert!(
        !cmd.iter().any(|a| a == "--admin"),
        "never bypass protection"
    );
    assert!(
        !cmd.iter().any(|a| a == "-f" || a == "--force"),
        "never force-merge"
    );
}

#[test]
fn close_requires_a_nonempty_rationale_comment() {
    assert!(
        close_command(4269, "").is_err(),
        "closing without rationale is rejected (auditable)"
    );
    let ok = close_command(4269, "Superseded by #4303 (same title, newer)")
        .expect("valid rationale accepted");
    let joined = ok.join(" ");
    assert!(joined.contains("4269"));
    assert!(
        joined.contains("4303"),
        "rationale references superseding PR"
    );
}

#[test]
fn dry_run_config_performs_no_mutation() {
    let cfg = TriageConfig {
        dry_run: true,
        ..TriageConfig::default()
    };
    assert!(!cfg.will_mutate(), "dry-run reports only, never mutates");

    let real = TriageConfig::default();
    assert!(real.will_mutate());
}

// ---------------------------------------------------------------------------
// Disposition enum + report output (text & JSON parity)
// ---------------------------------------------------------------------------

#[test]
fn disposition_action_strings_are_the_three_canonical_values() {
    assert_eq!(
        Disposition::RebasedAndGreen.action_str(),
        "rebased-and-green"
    );
    assert_eq!(
        Disposition::ClosedWithRationale {
            superseded_by: Some(4303),
            reason: "Superseded by #4303".to_string(),
        }
        .action_str(),
        "closed-with-rationale"
    );
    assert_eq!(
        Disposition::TriageNote {
            reason: "Relevance unclear; comment posted".to_string(),
        }
        .action_str(),
        "triage-note"
    );
}

fn sample_report() -> TriageReport {
    TriageReport {
        repo: "rysweet/Simard".to_string(),
        dispositions: vec![
            DispositionRecord {
                pr: 4269,
                disposition: Disposition::ClosedWithRationale {
                    superseded_by: Some(4303),
                    reason: "Superseded by #4303".to_string(),
                },
                detail: "Superseded by #4303 (same title, newer)".to_string(),
            },
            DispositionRecord {
                pr: 4334,
                disposition: Disposition::RebasedAndGreen,
                detail: "OODA-core done-gate; serialized, CI settled".to_string(),
            },
            DispositionRecord {
                pr: 4296,
                disposition: Disposition::TriageNote {
                    reason: "Relevance unclear; comment posted".to_string(),
                },
                detail: "Non-trivial conflict, relevance unclear".to_string(),
            },
        ],
        escalations: default_escalations(),
    }
}

#[test]
fn text_output_lists_every_disposition_and_an_escalations_section() {
    let text = sample_report().to_text();
    assert!(text.contains("PR"), "has a header column");
    assert!(text.contains("4269") && text.contains("closed-with-rationale"));
    assert!(text.contains("4334") && text.contains("rebased-and-green"));
    assert!(text.contains("4296") && text.contains("triage-note"));
    // Text and JSON must represent the SAME feature — escalations appear in both.
    assert!(text.contains("Escalations"));
    assert!(text.contains("problem-3") && text.contains("problem-4"));
}

#[test]
fn json_output_has_documented_shape() {
    let report = sample_report();
    let v = serde_json::to_value(&report).expect("report is Serialize");

    assert_eq!(v["repo"], "rysweet/Simard");

    let disps = v["dispositions"].as_array().expect("dispositions array");
    assert_eq!(disps.len(), 3);
    assert_eq!(disps[0]["pr"], 4269);
    assert_eq!(disps[0]["action"], "closed-with-rationale");
    assert_eq!(disps[1]["action"], "rebased-and-green");
    assert_eq!(disps[2]["action"], "triage-note");

    let esc = v["escalations"].as_array().expect("escalations array");
    assert_eq!(esc.len(), 2);
    let ids: Vec<&str> = esc.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"problem-3"));
    assert!(ids.contains(&"problem-4"));
    // Each escalation carries kind + detail.
    assert!(
        esc.iter()
            .all(|e| e["kind"].is_string() && e["detail"].is_string())
    );
}

#[test]
fn default_escalations_route_release_and_overseer_problems() {
    let esc: Vec<Escalation> = default_escalations();
    let release = esc
        .iter()
        .find(|e| e.id == "problem-3")
        .expect("release-adoption escalation present");
    assert_eq!(release.kind, "release-adoption");
    assert!(
        release.detail.contains("0.33.1"),
        "names the target release"
    );

    let overseer = esc
        .iter()
        .find(|e| e.id == "problem-4")
        .expect("overseer-cadence escalation present");
    assert_eq!(overseer.kind, "overseer-cadence");
}

// ---------------------------------------------------------------------------
// Guarantee: no print!/println! and no new `Bridge` naming in the module.
// (Source-scan meta-test, mirroring the repo's existing guarantee tests.)
// ---------------------------------------------------------------------------

#[test]
fn pr_triage_module_uses_tracing_not_print_and_no_bridge_naming() {
    use std::path::PathBuf;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        root.join("src/pr_triage.rs"),
        root.join("src/pr_triage/mod.rs"),
    ];
    let path = candidates.iter().find(|p| p.exists()).unwrap_or_else(|| {
        panic!("pr_triage module source must exist (src/pr_triage.rs or src/pr_triage/mod.rs)")
    });

    let src = std::fs::read_to_string(path).expect("module source readable");
    assert!(
        !src.contains("println!") && !src.contains("print!"),
        "observability must be structured tracing/OTel, not print!/println!"
    );
    assert!(
        !src.contains("Bridge"),
        "no new `Bridge` naming is permitted"
    );

    // The module is expected to observe the token-exchange-style critical steps
    // via tracing spans/events, so at least one tracing macro must be present.
    let uses_tracing = ["tracing::", "info!", "warn!", "error!", "debug!", "span!"]
        .iter()
        .any(|needle| src.contains(needle));
    assert!(uses_tracing, "module must emit structured tracing");
}

// Silence unused-import warnings for the `self`/`pr_triage` alias if the
// compiler doesn't otherwise reference it (kept for documentation clarity).
#[allow(unused_imports)]
use pr_triage as _pr_triage_module_marker;
