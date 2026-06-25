//! Tests for [`super::PromptStore`].
//!
//! These tests deliberately avoid touching process environment in parallel —
//! env-var resolution is exercised through the pure helper
//! [`super::resolve_dir_from_env`] guarded by a serializing mutex.

use super::prompt_store::*;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Serialize tests that mutate process environment.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn tmpdir(label: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/test-tmp"));
    let dir = base.join(format!(
        "prompt-store-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create tmpdir");
    dir
}

#[test]
fn missing_file_falls_back_to_embedded() {
    let dir = tmpdir("missing");
    let store = PromptStore::new(Some(dir));
    let prompt = store.load("ooda_brain.md");
    assert!(
        prompt.contains("ROLE"),
        "embedded fallback must be served when file is missing"
    );
    assert_eq!(prompt, embedded_fallback("ooda_brain.md").unwrap());
}

#[test]
fn disk_file_overrides_embedded_fallback() {
    let dir = tmpdir("override");
    let path = dir.join("ooda_brain.md");
    std::fs::write(&path, "# CUSTOM BRAIN PROMPT\n").unwrap();
    let store = PromptStore::new(Some(dir));
    assert_eq!(store.load("ooda_brain.md"), "# CUSTOM BRAIN PROMPT\n");
}

#[test]
fn mtime_change_invalidates_cache() {
    let dir = tmpdir("mtime");
    let path = dir.join("ooda_decide.md");
    std::fs::write(&path, "v1").unwrap();
    let store = PromptStore::new(Some(dir.clone()));
    assert_eq!(store.load("ooda_decide.md"), "v1");

    // Sleep past filesystem mtime resolution (commonly 1s on ext4 without
    // O_NOATIME tricks; 1.1s is safe across CI filesystems).
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(&path, "v2").unwrap();
    assert_eq!(
        store.load("ooda_decide.md"),
        "v2",
        "cache must invalidate when mtime advances"
    );
}

#[test]
fn unchanged_file_serves_from_cache() {
    let dir = tmpdir("cache");
    let path = dir.join("ooda_orient.md");
    std::fs::write(&path, "stable").unwrap();
    let store = PromptStore::new(Some(dir.clone()));
    assert_eq!(store.load("ooda_orient.md"), "stable");

    // Replace contents WITHOUT bumping mtime by restoring the original
    // mtime after the write. This proves the cache key is `(path, mtime)`
    // and not `(path, contents)`.
    let original = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, "altered").unwrap();
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(original).unwrap();

    assert_eq!(
        store.load("ooda_orient.md"),
        "stable",
        "unchanged mtime must serve cached value"
    );
}

#[test]
fn no_dir_means_pure_embedded() {
    let store = PromptStore::new(None);
    assert_eq!(
        store.load("ooda_brain.md"),
        embedded_fallback("ooda_brain.md").unwrap()
    );
    assert_eq!(
        store.load("ooda_decide.md"),
        embedded_fallback("ooda_decide.md").unwrap()
    );
    assert_eq!(
        store.load("ooda_orient.md"),
        embedded_fallback("ooda_orient.md").unwrap()
    );
}

#[test]
fn unknown_prompt_name_returns_empty_when_no_disk_file() {
    let store = PromptStore::new(None);
    assert_eq!(store.load("never_existed.md"), "");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn env_var_takes_precedence_over_home() {
    let _g = ENV_LOCK.lock().unwrap();
    let saved_env = std::env::var_os(ENV_VAR);
    let saved_home = std::env::var_os("HOME");

    let env_dir = tmpdir("envwin");
    // SAFETY: serialized via ENV_LOCK above.
    unsafe {
        std::env::set_var(ENV_VAR, &env_dir);
        std::env::set_var("HOME", "/nonexistent-home-for-test");
    }
    let resolved = resolve_dir_from_env();
    unsafe {
        match saved_env {
            Some(v) => std::env::set_var(ENV_VAR, v),
            None => std::env::remove_var(ENV_VAR),
        }
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    assert_eq!(resolved.as_deref(), Some(env_dir.as_path()));
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn home_used_when_env_var_unset() {
    let _g = ENV_LOCK.lock().unwrap();
    let saved_env = std::env::var_os(ENV_VAR);
    let saved_home = std::env::var_os("HOME");

    unsafe {
        std::env::remove_var(ENV_VAR);
        std::env::set_var("HOME", "/tmp/fake-home-for-test");
    }
    let resolved = resolve_dir_from_env();
    unsafe {
        match saved_env {
            Some(v) => std::env::set_var(ENV_VAR, v),
            None => std::env::remove_var(ENV_VAR),
        }
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    assert_eq!(
        resolved,
        Some(PathBuf::from(
            "/tmp/fake-home-for-test/.simard/prompt_assets/simard"
        ))
    );
}

#[test]
fn singleton_is_idempotent() {
    let a = global() as *const PromptStore;
    let b = global() as *const PromptStore;
    assert_eq!(a, b, "global() must return the same instance");
}

// --- prompt_version helper -------------------------------------------------

#[test]
fn prompt_version_is_12_lowercase_hex_chars() {
    let v = prompt_version("anything");
    assert_eq!(v.len(), 12);
    assert!(
        v.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
        "expected 12 lowercase hex chars, got {v:?}"
    );
}

#[test]
fn prompt_version_is_deterministic() {
    let a = prompt_version("ooda system prompt");
    let b = prompt_version("ooda system prompt");
    assert_eq!(a, b);
}

#[test]
fn prompt_version_changes_on_any_byte_change() {
    let a = prompt_version("hello");
    let b = prompt_version("hello\n");
    let c = prompt_version("Hello");
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert_ne!(b, c);
}

#[test]
fn prompt_version_matches_known_sha256_prefix() {
    // sha256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    assert_eq!(prompt_version(""), "e3b0c44298fc");
}

// --- goal_session_objective.md registration (TDD: issue #2152) -------------
//
// These tests specify the contract for making goal_session_objective.md
// runtime-loadable via PromptStore, matching the pattern used by the 3
// brain prompts (ooda_brain.md, ooda_decide.md, ooda_orient.md).

#[test]
fn goal_session_objective_has_embedded_fallback() {
    // Change 2: prompt_store must register goal_session_objective.md in
    // embedded_fallback() so it can be loaded at runtime with a compile-time
    // baked-in default.
    let fallback = embedded_fallback("goal_session_objective.md");
    assert!(
        fallback.is_some(),
        "embedded_fallback must return Some for goal_session_objective.md"
    );
}

#[test]
fn goal_session_objective_fallback_is_nonempty() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    assert!(
        !content.trim().is_empty(),
        "embedded fallback for goal_session_objective.md must not be empty"
    );
}

#[test]
fn goal_session_objective_contains_priority_order_section() {
    // Change 1: The prompt must contain a "Priority Order" section that
    // tells Simard to triage existing PRs before creating new work.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    assert!(
        content.contains("Priority Order") || content.contains("priority order"),
        "goal_session_objective.md must contain a Priority Order section, got:\n{content}"
    );
}

#[test]
fn goal_session_objective_priority_order_lists_merge_green_first() {
    // Tier 1: Merge green PRs first via gh pr merge --squash --delete-branch
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    assert!(
        content.contains("gh pr merge --squash --delete-branch"),
        "Priority Order must instruct merge via `gh pr merge --squash --delete-branch`"
    );
}

#[test]
fn goal_session_objective_priority_order_lists_fix_failing_second() {
    // Tier 2: Fix failing PRs (diagnose CI failure, fix, push)
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("fix") && lower.contains("failing"),
        "Priority Order must include fixing failing PRs as tier 2"
    );
}

#[test]
fn goal_session_objective_priority_order_lists_close_duplicates() {
    // Tier 3: Close duplicate PRs
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("close") && lower.contains("duplicate"),
        "Priority Order must include closing duplicate PRs as tier 3"
    );
}

#[test]
fn goal_session_objective_priority_order_new_work_last() {
    // Tier 4: New work only when no existing PRs need attention
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("new work") || lower.contains("new implementation"),
        "Priority Order must list new work as the last tier"
    );
}

#[test]
fn goal_session_objective_priority_order_precedes_response_shapes() {
    // The Priority Order section must appear BEFORE the "Two response shapes"
    // section so the agent reads triage rules before action shapes.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let priority_pos = content
        .find("Priority Order")
        .or_else(|| content.find("priority order"));
    let shapes_pos = content.find("Two response shapes");
    assert!(
        priority_pos.is_some() && shapes_pos.is_some(),
        "both Priority Order and Two response shapes sections must exist"
    );
    assert!(
        priority_pos.unwrap() < shapes_pos.unwrap(),
        "Priority Order must appear before Two response shapes"
    );
}

#[test]
fn goal_session_objective_loads_via_store_without_disk() {
    // Change 2: PromptStore::new(None) must return the embedded fallback
    // for goal_session_objective.md (pure embedded mode).
    let store = PromptStore::new(None);
    let content = store.load("goal_session_objective.md");
    assert!(
        !content.is_empty(),
        "PromptStore.load('goal_session_objective.md') must return non-empty in embedded mode"
    );
    assert_eq!(
        content,
        embedded_fallback("goal_session_objective.md").unwrap(),
        "store.load must return the same content as embedded_fallback"
    );
}

#[test]
fn goal_session_objective_disk_override_works() {
    // Change 2: A file on disk must override the embedded fallback,
    // matching the hot-reload pattern used by ooda_brain.md etc.
    let dir = tmpdir("goal-obj-override");
    let path = dir.join("goal_session_objective.md");
    std::fs::write(&path, "# CUSTOM GOAL OBJECTIVE\n").unwrap();
    let store = PromptStore::new(Some(dir));
    assert_eq!(
        store.load("goal_session_objective.md"),
        "# CUSTOM GOAL OBJECTIVE\n"
    );
}

#[test]
fn goal_session_objective_requires_merge_ready_criteria() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("merge-ready") || lower.contains("merge_ready"),
        "Priority Order must require merge-ready criteria verification before merging"
    );
    assert!(
        lower.contains("qa-team") || lower.contains("qa_team"),
        "merge-ready criteria must mention qa-team scenarios"
    );
    assert!(
        lower.contains("quality-audit") || lower.contains("quality_audit"),
        "merge-ready criteria must mention quality-audit cycles"
    );
}

#[test]
fn goal_session_objective_mentions_self_update() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("self-update") || lower.contains("self_update"),
        "goal_session_objective.md must mention self-update awareness for Simard repo merges"
    );
}

// --- Loop / stuck self-detection + proactivity (issue #2403) ----------------
//
// These tests pin the prompt CONTENT that makes Simard reason about whether she
// is making real progress or spinning in a loop, break the loop by changing
// strategy, keep open-ended goals bounded, and proactively backfill work. They
// assert wording only — no Rust logic or output-contract change.

#[test]
fn goal_session_objective_has_loop_self_detection() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("loop"),
        "goal_session_objective.md must make Simard reason about being in a loop"
    );
    assert!(
        lower.contains("real progress") && lower.contains("not progress"),
        "goal_session_objective.md must distinguish real progress from non-progress signals"
    );
    assert!(
        lower.contains("change strategy") || lower.contains("stop repeating"),
        "goal_session_objective.md must tell Simard to break the loop / change strategy when stuck"
    );
}

#[test]
fn goal_session_objective_loop_check_precedes_priority_order() {
    // The self-detection section must come BEFORE Priority Order so Simard
    // decides whether she is looping before defaulting to re-triage.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    let loop_pos = lower
        .find("looping")
        .or_else(|| lower.find("are you making progress"));
    let priority_pos = lower.find("priority order");
    assert!(
        loop_pos.is_some() && priority_pos.is_some(),
        "both the loop-detection section and Priority Order must exist"
    );
    assert!(
        loop_pos.unwrap() < priority_pos.unwrap(),
        "loop self-detection must appear before Priority Order"
    );
}

#[test]
fn goal_session_objective_triage_is_quick_first_pass_not_gate() {
    // Rebalance: triage must be a quick first pass, not a perpetual gate that
    // blocks executing new work.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("quick first pass") && lower.contains("not a perpetual gate"),
        "Priority Order must frame triage as a quick first pass, not a perpetual gate"
    );
}

#[test]
fn goal_session_objective_biases_toward_executing() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("decompose") && lower.contains("shipping"),
        "goal_session_objective.md must bias toward decomposing open-ended goals and shipping"
    );
}

#[test]
fn ooda_decide_notes_stuck_loop_in_rationale() {
    let content = embedded_fallback("ooda_decide.md").expect("ooda_decide.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("stuck loop") || lower.contains("suspected loop"),
        "ooda_decide.md must instruct surfacing a suspected stuck loop in the rationale"
    );
    // The action KIND must remain advance_goal (output contract unchanged).
    assert!(
        content.contains("advance_goal"),
        "ooda_decide.md must keep routing stuck goals to advance_goal (kind unchanged)"
    );
}

#[test]
fn progress_reviewer_escalates_stalled_open_ended_goals() {
    let content = embedded_fallback("progress_assessment_reviewer.md")
        .expect("progress_assessment_reviewer.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("stalled") && lower.contains("open-ended"),
        "progress_assessment_reviewer.md must call out stalled / open-ended goals"
    );
    assert!(
        lower.contains("decompose") || lower.contains("demote"),
        "progress reviewer must push parked goals toward decompose/complete/demote"
    );
}

#[test]
fn ooda_brain_detects_churn_vs_progress() {
    let prompt = include_str!("../../prompt_assets/simard/ooda_brain.md");
    let lower = prompt.to_lowercase();
    assert!(
        lower.contains("churn") && lower.contains("stuck loop"),
        "ooda_brain.md must distinguish churn from progress (stuck-loop detection)"
    );
    // Loop-breaking must route through existing variants — no new variants.
    assert!(
        prompt.contains("deprioritize") && prompt.contains("open_tracking_issue"),
        "ooda_brain.md must break loops via existing deprioritize / open_tracking_issue variants"
    );
}

#[test]
fn goal_curator_has_open_ended_hygiene_and_proactive_backfill() {
    let prompt = include_str!("../../prompt_assets/simard/goal_curator_system.md");
    let lower = prompt.to_lowercase();
    assert!(
        lower.contains("open-ended goal hygiene"),
        "goal_curator_system.md must teach open-ended goal hygiene (decompose to completable sub-goals)"
    );
    assert!(
        lower.contains("done-when"),
        "open-ended goals must require explicit done-when criteria"
    );
    assert!(
        lower.contains("proactive backfill") && lower.contains("open github issues"),
        "goal_curator_system.md must teach proactive backfill from own open GitHub issues"
    );
}

#[test]
fn goal_session_objective_enumerates_concrete_progress_signals() {
    // Behavior A: the loop self-detection must DEFINE concrete progress signals
    // (a new commit SHA, an opened/merged PR, a closed issue, a completion-%
    // increase backed by a shipped artifact) and an explicit non-progress list
    // (re-triaging, re-reading the same thing). Loose "real progress" wording is
    // not enough — the enumerated signals are what make the check actionable.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();

    for signal in [
        "commit sha",
        "pr opened",
        "merged",
        "issue closed",
        "shipped artifact",
    ] {
        assert!(
            lower.contains(signal),
            "goal_session_objective.md must enumerate the concrete progress signal {signal:?}"
        );
    }
    for non_signal in ["re-triaging", "re-reading"] {
        assert!(
            lower.contains(non_signal),
            "goal_session_objective.md must list {non_signal:?} as a non-progress (loop) signal"
        );
    }
}

#[test]
fn progress_reviewer_rejects_reasserted_stalled_high_pct() {
    // Behavior E: the per-cycle progress reviewer must judge whether the cycle
    // produced real progress. The enforceable verdict is that a high percent
    // re-asserted with NO new shipped artifact is REJECTED (not parked), which
    // is what feeds the demote/decompose decision next cycle.
    let content = embedded_fallback("progress_assessment_reviewer.md")
        .expect("progress_assessment_reviewer.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("re-asserts about the same high percent"),
        "progress_assessment_reviewer.md must detect a high percent re-asserted with no new work"
    );
    assert!(
        lower.contains("no new shipped artifact"),
        "the stalled-progress rule must hinge on the absence of a new shipped artifact"
    );
    assert!(
        lower.contains("reject"),
        "a re-asserted stalled high percent must be rejected, not accepted as progress"
    );
}

// ── Maximum safe parallelism — fill spare capacity (Step 6) ──────────────
//
// These tests pin the prompt guidance that makes Simard fill spare machine
// capacity with concurrent engineers on DISTINCT work items, bounded by the
// existing AIMD safety cap. The fan-out is prompt-driven (decompose an umbrella
// goal into distinct per-issue goals via `simard goal add`); the coverage
// allocator + AIMD cap then parallelize them. The Rust output contracts the
// parsers depend on (DECISION marker, Spawn-an-engineer / NO ACTION shapes)
// must remain intact.

#[test]
fn goal_session_objective_teaches_maximum_safe_parallelism() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("maximum safe parallelism"),
        "goal_session_objective.md must teach a Maximum-safe-parallelism strategy"
    );
    // Fan-out is via decomposing an umbrella into distinct per-issue goals,
    // created with `simard goal add`, so coverage can parallelize them.
    assert!(
        lower.contains("simard goal add"),
        "the fan-out must create concrete per-issue goals via `simard goal add`"
    );
    assert!(
        lower.contains("decompose") && lower.contains("distinct"),
        "must decompose an umbrella goal into distinct per-issue goals"
    );
    // Bounded by the EXISTING AIMD safety cap — not an unbounded spawn.
    assert!(
        lower.contains("aimd cap") || lower.contains("aimd safety cap"),
        "fan-out must stay bounded by the AIMD safety cap"
    );
    // The operator override that widens the resource-bounded ceiling.
    assert!(
        content.contains("SIMARD_MAX_CONCURRENT_ACTIONS"),
        "must point to SIMARD_MAX_CONCURRENT_ACTIONS for widening the ceiling"
    );
}

#[test]
fn goal_session_objective_parallelism_is_collision_safe() {
    // Parallel engineers must work DISTINCT items — never duplicate or re-triage
    // the same issue (preserves the #2404 loop-awareness).
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("one goal per issue"),
        "collision guard: exactly one goal per distinct issue"
    );
    assert!(
        lower.contains("never two engineers on the same issue"),
        "collision guard: never two engineers on the same issue"
    );
    // The umbrella delegates to per-issue goals rather than duplicating them.
    assert!(
        lower.contains("delegate"),
        "the umbrella must delegate to per-issue goals, not duplicate their work"
    );
}

#[test]
fn goal_session_objective_parallelism_keeps_response_shapes() {
    // The fan-out must reuse the existing "Spawn an engineer" response shape and
    // must NOT invent a new shape the Rust parser cannot read. Both documented
    // shapes (Spawn an engineer / NO ACTION) remain intact.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("spawn an engineer"),
        "fan-out must use the existing Spawn-an-engineer response shape"
    );
    assert!(
        content.contains("NO ACTION"),
        "the NO ACTION response shape must remain documented"
    );
}

#[test]
fn ooda_decide_explains_parallelism_without_new_variant() {
    let content = embedded_fallback("ooda_decide.md").expect("ooda_decide.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("parallelism"),
        "ooda_decide.md must explain how per-cycle parallelism is achieved"
    );
    // Parallelism comes from routing each DISTINCT goal to advance_goal — there
    // is NO new parallel/spawn-N variant (output contract unchanged).
    assert!(
        content.contains("advance_goal") && lower.contains("invent one"),
        "parallelism must route distinct goals to advance_goal with no invented variant"
    );
    assert!(
        lower.contains("aimd safety cap") || lower.contains("aimd cap"),
        "parallelism must be bounded by the AIMD safety cap"
    );
}

#[test]
fn ooda_decide_first_line_decision_contract_preserved() {
    // The Rust parser reads the FIRST non-blank line for a `DECISION:` marker.
    // Body additions must not disturb that contract.
    let content = embedded_fallback("ooda_decide.md").expect("ooda_decide.md must be registered");
    let first_non_blank = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .expect("ooda_decide.md must have a non-blank line");
    assert!(
        first_non_blank.contains("DECISION:"),
        "first non-blank line must still assert the DECISION contract, got: {first_non_blank:?}"
    );
}

#[test]
fn ooda_decide_recipe_mirrors_parallelism_note() {
    // The runtime recipe (recipe-runner-rs path) must stay in sync with the
    // embedded ooda_decide.md prompt on the parallelism guidance.
    let recipe = include_str!("../../prompt_assets/simard/recipes/ooda-decide.yaml");
    let lower = recipe.to_lowercase();
    assert!(
        lower.contains("parallelism") && recipe.contains("advance_goal"),
        "ooda-decide.yaml must mirror the parallelism note routing distinct goals to advance_goal"
    );
    assert!(
        lower.contains("aimd safety cap") || lower.contains("aimd cap"),
        "ooda-decide.yaml parallelism note must reference the AIMD safety cap"
    );
    assert!(
        lower.contains("invent one"),
        "ooda-decide.yaml must forbid inventing a new parallel action variant"
    );
}

// ── Maximum safe parallelism — additional outcome/constraint coverage (Step 7) ──
//
// The six tests above pin the headline guidance and the preserved output
// contracts. These tests pin the remaining REQUIRED OUTCOME points and HARD
// CONSTRAINTS so they cannot silently regress when the prompts are edited:
//   * fill spare capacity — no engineer slot idle while parallelizable work remains
//   * resource-aware AIMD backoff (additive-increase + halve under pressure) survives
//   * each parallel engineer gets a DISTINCT, BOUNDED work item
//   * the `rysweet`-only operator gate (Priority Order tier 0) is preserved per-issue
//   * #2404 loop-awareness is preserved (decompose-and-ship, do not re-triage)
//   * failures are surfaced, never silently re-looped (no silent degradation)

/// Collapse all runs of whitespace to single spaces so assertions on a phrase
/// are not defeated by Markdown line-wrapping.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn goal_session_objective_fills_spare_capacity_no_idle() {
    // REQUIRED OUTCOME #1: when live engineers < the AIMD cap and parallelizable
    // work exists, fill the slots — no idle capacity while work remains.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("fill spare capacity"),
        "must teach filling spare capacity"
    );
    assert!(
        lower.contains("below the aimd cap"),
        "fan-out must trigger when live engineers are below the AIMD cap (spare capacity)"
    );
    assert!(
        lower.contains("sits idle") && lower.contains("parallelizable work remains"),
        "must assert no engineer slot sits idle while parallelizable work remains"
    );
}

#[test]
fn goal_session_objective_parallelism_assigns_distinct_bounded_work() {
    // REQUIRED OUTCOME #1 & #5: each parallel engineer works a DISTINCT, BOUNDED
    // item (one issue / one bounded file-set per goal) — never duplicating work.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("done-when") || lower.contains("done when"),
        "each per-issue goal must carry an explicit done-when criterion (bounded work)"
    );
    assert!(
        lower.contains("distinct work only"),
        "must state the distinct-work-only rule"
    );
    assert!(
        lower.contains("one issue") && lower.contains("per goal"),
        "must bound each goal to one issue (or one bounded file-set) per goal"
    );
}

#[test]
fn goal_session_objective_parallelism_is_resource_aware() {
    // HARD CONSTRAINT: "fill the machine" must stay resource-aware — the AIMD cap
    // raises additively while there is headroom and BACKS OFF under CPU / memory /
    // 429 pressure. The prompt must keep describing that safety behavior so the
    // "safe" in "maximum safe parallelism" is never lost.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("additively"),
        "must describe additive increase of the cap while there is headroom"
    );
    assert!(
        lower.contains("backs off") && lower.contains("pressure"),
        "must describe the cap backing off under pressure"
    );
    assert!(
        content.contains("429"),
        "must name the 429 / rate-limit backoff signal"
    );
    assert!(
        lower.contains("shrinks automatically under load"),
        "the AIMD ceiling must shrink automatically under load (never hard-thrash)"
    );
}

#[test]
fn goal_session_objective_parallelism_preserves_rysweet_gate() {
    // REQUIRED OUTCOME #5: the per-issue fan-out must NOT bypass the operator's
    // `rysweet`-only author gate (Priority Order tier 0). Each spawned engineer
    // verifies the issue author before acting.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("priority order tier 0"),
        "the parallel fan-out must still honor Priority Order tier 0"
    );
    assert!(
        lower.contains("rysweet"),
        "the per-issue fan-out must keep the rysweet-only gate"
    );
    assert!(
        content.contains("gh issue view"),
        "each engineer must verify the issue author (gh issue view) before acting"
    );
}

#[test]
fn goal_session_objective_preserves_loop_awareness() {
    // REQUIRED OUTCOME #5 (#2404): parallel engineers must not all re-triage the
    // same thing. The decomposition IS the loop-break: decompose and ship rather
    // than re-triaging the same list every cycle.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("loop awareness still applies"),
        "loop-awareness (#2404) must be preserved in the parallelism strategy"
    );
    assert!(
        lower.contains("loop-break"),
        "the decomposition must be framed as the loop-break"
    );
    assert!(
        lower.contains("re-triage"),
        "must warn against re-triaging the same list every cycle"
    );
}

#[test]
fn goal_session_objective_surfaces_not_silently_reloops() {
    // HARD CONSTRAINT: surface failures explicitly; no silent degradation. The
    // pull-fresh-work strategy must surface the proposal and never silently
    // re-loop. (Phrase spans wrapped lines, so normalize whitespace first.)
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("surface the proposal"),
        "must surface the proposal to the operator"
    );
    assert!(
        norm.contains("never silently re-loop"),
        "must never silently re-loop (no silent degradation)"
    );
}

#[test]
fn ooda_decide_parallelism_is_resource_aware() {
    // The Decide note must also frame parallelism as resource-aware: the coverage
    // allocator spawns up to the AIMD cap, which backs off under pressure.
    let content = embedded_fallback("ooda_decide.md").expect("ooda_decide.md must be registered");
    let lower = content.to_lowercase();
    assert!(
        lower.contains("backs off") && lower.contains("pressure"),
        "ooda_decide.md parallelism note must describe the cap backing off under pressure"
    );
    assert!(
        content.contains("429"),
        "ooda_decide.md must name the 429 backoff signal in the parallelism note"
    );
}
