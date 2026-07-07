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

// ── Own PRs to landing — finish in-flight PRs, merged-AND-closed done-gate ──
//
// These tests pin the prompt guidance that makes Simard DRIVE the PRs she/her
// engineers open all the way to landing (CI-green → squash-merge → close the
// issue) instead of leaving them open and stalled. They are ADDITIVE to the
// #2404 (loop-awareness) and #2405 (parallel fan-out) guidance — start in
// parallel, don't loop, and now FINISH/land. The Rust output contracts the
// parsers depend on (prose-only goal_session, single-line verdict JSON) stay
// intact. Phrases are checked after `normalize_ws` so Markdown line-wrapping
// cannot defeat the assertions.

#[test]
fn goal_session_objective_owns_open_prs_to_landing() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("finish what you started")
            && norm.contains("own your open prs all the way to landing"),
        "goal_session_objective.md must teach owning open PRs all the way to landing"
    );
    assert!(
        norm.contains("own-pr-to-landing priority"),
        "must declare an own-PR-to-landing priority for goals that already have an open PR"
    );
    assert!(
        norm.contains("drive that pr to landing"),
        "the next action for a goal with an open PR must be to drive that PR to landing"
    );
    // Use her existing merge authority to merge it herself, then close the issue.
    assert!(
        norm.contains("merge it yourself"),
        "must direct her to merge the PR herself using her existing merge authority"
    );
    assert!(
        norm.contains("close the linked issue"),
        "must direct her to close the linked issue after merging"
    );
    // Finishing in-flight work is preferred over starting new work.
    assert!(
        norm.contains("prefer finishing an in-flight pr you own over starting a fresh one"),
        "must prefer finishing an in-flight PR over starting new work"
    );
}

#[test]
fn goal_session_objective_done_gate_requires_merge_and_close() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("done-gate"),
        "must contain an explicit Done-gate section"
    );
    assert!(
        norm.contains("complete only when its pr is merged and the linked issue is closed"),
        "done-gate: a fix/implement goal is complete ONLY when its PR is merged AND the issue is closed"
    );
    // A genuine external blocker is surfaced as Blocked + reason, not silently re-looped.
    assert!(
        norm.contains("record the goal as blocked with the concrete reason"),
        "an external blocker must be recorded as Blocked with a concrete reason"
    );
    assert!(
        norm.contains("silently re-loop"),
        "must forbid silently re-looping on a blocked goal"
    );
}

#[test]
fn goal_session_objective_ci_fix_priority_over_new_pr() {
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("ci-fix priority"),
        "must declare a CI-fix priority section"
    );
    assert!(
        norm.contains("higher priority than opening a new pr"),
        "fixing an own red/BLOCKED PR must outrank opening a new PR for a different issue"
    );
}

#[test]
fn engineer_system_continues_existing_pr_no_duplicate() {
    // engineer_system.md is not registered for embedded_fallback (no prompt_store
    // logic change), so assert its content directly via include_str!.
    let content = include_str!("../../prompt_assets/simard/engineer_system.md");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("continue it, never duplicate it"),
        "engineer_system.md must teach continuing the dispatched issue's PR, never duplicating it"
    );
    assert!(
        norm.contains("continue that pr"),
        "a dispatched engineer must continue an existing open PR for its issue"
    );
    assert!(
        norm.contains("never open a second pr for an issue that already has one"),
        "engineer must never open a duplicate PR for an issue that already has one"
    );
    assert!(
        norm.contains("not done until its pr is merged and the linked issue is closed"),
        "engineer cycle is not done until its PR is merged and the linked issue is closed"
    );
}

#[test]
fn progress_reviewer_open_pr_is_not_done() {
    let content = embedded_fallback("progress_assessment_reviewer.md")
        .expect("progress_assessment_reviewer.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("done means merged-and-closed, not merely opened"),
        "progress reviewer must encode the merged-and-closed done-gate"
    );
    assert!(
        norm.contains("open, un-merged pr is not completion"),
        "an open, un-merged PR must not count as completion"
    );
    assert!(
        norm.contains("reject"),
        "a near-100% completion claim on an open PR must be rejected"
    );
    // Output contract preserved: single-line verdict JSON.
    assert!(
        content.contains("\"verdict\""),
        "single-line verdict JSON output contract must be preserved"
    );
}

#[test]
fn progress_assessment_recipe_mirrors_done_gate() {
    // The runtime recipe-runner recipe must stay in sync with the embedded
    // progress_assessment_reviewer.md on the open-PR-not-done gate.
    let recipe = include_str!("../../prompt_assets/simard/recipes/progress-assessment.yaml");
    let norm = normalize_ws(recipe).to_lowercase();
    assert!(
        norm.contains("done means merged-and-closed, not merely opened"),
        "progress-assessment.yaml must mirror the merged-and-closed done-gate"
    );
    assert!(
        norm.contains("open, un-merged pr is not completion"),
        "recipe mirror: an open, un-merged PR is not completion"
    );
    // Verdict JSON output contract preserved.
    assert!(
        recipe.contains("\"verdict\""),
        "progress-assessment.yaml must keep the verdict JSON output contract"
    );
}

// ── PR-finalization review pipeline (#2410 follow-on) ───────────────────────
//
// These tests pin the prompt-content contract for the new, bounded, ordered
// PR-finalization pipeline every engineer runs at the end of a PR, AFTER the
// fix is implemented and the PR is opened/updated but BEFORE merge-ready:
//
//   1. CRUSTY REVIEW→FIX LOOP on a HIGH-END model (`$SIMARD_REVIEW_MODEL`,
//      default `gpt-5.5`, run with --reasoning-effort high --context long_context),
//      fixing every actionable finding and re-reviewing the
//      LATEST PR state until crusty emits the sentinel `NO BLOCKING FINDINGS`
//      or a bounded cap (`$SIMARD_REVIEW_MAX_ITERS`, default 3) is reached.
//   2. PR-GUIDE illustrated walkthrough (graceful-skip where unavailable).
//   3. FINAL REVIEW — one lightweight pass, no loop.
//   4. MERGE-READY → merge → close issue (the pre-existing #2410 landing path).
//
// The spec is `docs/reference/pr-finalization-pipeline.md`. The pipeline is
// PROMPT-ONLY: it is added to `engineer_system.md` (engineer PR-finalization
// instructions) with a short cross-reference note in `goal_session_objective.md`
// (so the OODA brain knows finalization runs INSIDE the engineer and must not
// spin the goal-action cycle — preserving #2404 loop-awareness, #2405 fan-out,
// and #2410 own-PRs-to-landing). These assertions FAIL until the prompt edits
// land. `engineer_system.md` is not registered for `embedded_fallback` (no
// prompt_store logic change), so it is asserted via `include_str!` exactly like
// `engineer_system_continues_existing_pr_no_duplicate` above. Phrases are
// checked after `normalize_ws` so Markdown line-wrapping cannot defeat them.

/// The engineer system prompt, read at compile time (not registered for
/// `embedded_fallback`, so assert its content directly like the existing
/// `engineer_system_continues_existing_pr_no_duplicate` test).
fn engineer_system_md() -> &'static str {
    include_str!("../../prompt_assets/simard/engineer_system.md")
}

#[test]
fn engineer_system_has_pr_finalization_pipeline_section() {
    // A discrete, named PR-finalization pipeline section that names every skill
    // it orchestrates, in order.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("pr-finalization pipeline"),
        "engineer_system.md must add an explicit, named PR-finalization pipeline section"
    );
    assert!(
        norm.contains("crusty-old-engineer"),
        "the pipeline must name the crusty-old-engineer review skill"
    );
    assert!(
        norm.contains("pr-guide"),
        "the pipeline must name the pr-guide illustrated-walkthrough skill"
    );
    assert!(
        norm.contains("merge-ready"),
        "the pipeline must name the existing merge-ready final gate"
    );
}

#[test]
fn engineer_system_crusty_loop_uses_high_end_model() {
    // Stage 1 runs crusty on a HIGH-END reasoning model, pinned via a configurable
    // env var with a verified high-end default, invoked through a pinned subprocess
    // (the engineer itself runs the default/auto model).
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("review→fix loop"),
        "stage 1 must be described as a crusty review→fix loop"
    );
    assert!(
        norm.contains("high-end"),
        "the crusty loop must run on a high-end reasoning model"
    );
    assert!(
        norm.contains("$simard_review_model"),
        "the high-end model must be configurable via $SIMARD_REVIEW_MODEL"
    );
    assert!(
        norm.contains("gpt-5.5"),
        "the high-end model must default to the verified gpt-5.5"
    );
    assert!(
        norm.contains("reasoning-effort high") && norm.contains("long_context"),
        "crusty must run with --reasoning-effort high and --context long_context"
    );
    assert!(
        norm.contains("copilot --model"),
        "crusty must be pinned to the high-end model via a copilot --model subprocess"
    );
}

#[test]
fn engineer_system_crusty_loop_is_bounded() {
    // The loop MUST terminate: a bounded, configurable iteration cap.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("$simard_review_max_iters"),
        "the iteration cap must be configurable via $SIMARD_REVIEW_MAX_ITERS"
    );
    assert!(
        norm.contains("default 3"),
        "the iteration cap must have a sensible default of 3"
    );
    assert!(
        norm.contains("cap"),
        "the loop must declare a bounded cap to prevent infinite review→fix loops"
    );
}

#[test]
fn engineer_system_crusty_loop_reviews_latest_state() {
    // Each iteration operates on the freshly-pushed PR state, gated by a
    // structural sentinel verdict (not free text) — no TOCTOU on a stale diff.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("latest pr state"),
        "each loop iteration must review the latest PR state"
    );
    assert!(
        norm.contains("stale diff"),
        "the loop must never re-review a stale diff"
    );
    assert!(
        norm.contains("no blocking findings"),
        "the satisfied signal must be the structural sentinel verdict NO BLOCKING FINDINGS"
    );
}

#[test]
fn engineer_system_crusty_loop_fixes_every_finding() {
    // Fix discipline: every actionable finding is fixed and pushed before the
    // next review, then crusty re-runs.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("every actionable finding"),
        "the loop must fix every actionable finding crusty raises"
    );
    assert!(
        norm.contains("same pr branch"),
        "fixes must be pushed to the same PR branch before re-review"
    );
    assert!(
        norm.contains("re-review"),
        "the loop must re-review after pushing fixes"
    );
}

#[test]
fn engineer_system_cap_reached_surfaces_blocker_not_merge() {
    // Bounded + honest: if the cap is hit with findings still open, record them
    // on the PR AND surface a goal blocker — never silently merge.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("remaining findings as a pr comment"),
        "cap-reached must post the remaining findings as a PR comment"
    );
    assert!(
        norm.contains("blocker"),
        "cap-reached-with-findings must surface a goal blocker"
    );
    assert!(
        norm.contains("engineer_summary"),
        "the blocker must be surfaced in cycle_summary.engineer_summary"
    );
    assert!(
        norm.contains("do not merge"),
        "the engineer must NOT merge past unsatisfied crusty findings"
    );
}

#[test]
fn engineer_system_trivial_pr_filter_is_cost_aware() {
    // Cost awareness: the full high-end loop runs only on non-trivial PRs; a
    // trivial PR gets a single lightweight pass.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("non-trivial pr"),
        "the full crusty loop must run only on non-trivial PRs"
    );
    assert!(
        norm.contains("single lightweight pass"),
        "a trivial PR must get a single lightweight pass, not the full loop"
    );
    assert!(
        norm.contains("cost"),
        "the pipeline must note cost awareness (high-end review is expensive)"
    );
}

#[test]
fn engineer_system_pr_guide_degrades_gracefully() {
    // Stage 2: run pr-guide; if unavailable in the target repo, log a note and
    // continue. This is the ONLY sanctioned skip — it must not hard-fail.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("pr-guide unavailable"),
        "the pipeline must handle pr-guide being unavailable in the target repo"
    );
    assert!(
        norm.contains("skipping illustrated guide"),
        "pr-guide unavailability must be a logged skip of the illustrated guide"
    );
    assert!(
        norm.contains("does not hard-fail"),
        "a missing pr-guide must degrade gracefully, not hard-fail the pipeline"
    );
}

#[test]
fn engineer_system_final_review_is_one_pass_no_loop() {
    // Stage 3: a single, lightweight final correctness/consistency pass after the
    // guide — explicitly NOT a second review→fix loop.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("final review"),
        "the pipeline must include a final review pass after pr-guide"
    );
    assert!(
        norm.contains("one pass, no loop"),
        "the final review must be one lightweight pass, not a second loop"
    );
}

#[test]
fn engineer_system_pipeline_gates_merge_ready() {
    // The pipeline runs BEFORE merge-ready, and the merge step is gated on the
    // pipeline having run — order-independent, semantic assertions.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("before merge-ready"),
        "stages 1–3 must run before the merge-ready gate"
    );
    assert!(
        norm.contains("only after the pr-finalization pipeline"),
        "the merge step must be gated on the PR-finalization pipeline having run"
    );
}

#[test]
fn engineer_system_pipeline_preserves_2410_landing() {
    // Regression guard: the new pipeline builds ON TOP of #2410 — the
    // own-PR-to-landing and merged-AND-closed done-gate guidance must remain.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("continue it, never duplicate it"),
        "must preserve #2410 continue-existing-PR-never-duplicate guidance"
    );
    assert!(
        norm.contains("not done until its pr is merged and the linked issue is closed"),
        "must preserve the #2410 merged-AND-closed done-gate"
    );
}

#[test]
fn goal_session_objective_finalization_runs_inside_engineer() {
    // The OODA brain must know finalization runs INSIDE the engineer's cycle: the
    // goal-action only dispatches/checks and must not spin while an engineer is
    // mid-finalization (preserving #2404 loop-awareness).
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("pr-finalization pipeline"),
        "goal_session_objective.md must reference the engineer's PR-finalization pipeline"
    );
    assert!(
        norm.contains("finalization runs inside the engineer"),
        "the note must state finalization runs inside the engineer's cycle"
    );
    assert!(
        norm.contains("only dispatches and checks"),
        "the goal-action brain only dispatches and checks; it does not run the loop"
    );
    assert!(
        norm.contains("does not run that loop"),
        "the brain must not run the review loop itself"
    );
    assert!(
        norm.contains("while its engineer is finalizing"),
        "the brain must not re-dispatch/re-loop a goal while its engineer is finalizing"
    );
    assert!(
        norm.contains("#2404"),
        "the note must tie back to #2404 loop-awareness it preserves"
    );
}

#[test]
fn goal_session_objective_finalization_preserves_prose_contract() {
    // The cross-reference note must stay additive prose: it must NOT introduce a
    // JSON verdict shape into the prose-only goal-action output, and it must not
    // regress the #2410 own-open-PRs-to-landing guidance it builds on.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("own your open prs all the way to landing"),
        "the finalization note must not regress the #2410 own-PRs-to-landing guidance"
    );
    assert!(
        !content.contains("\"verdict\""),
        "goal_session_objective.md is prose-only — it must not gain a JSON verdict contract"
    );
}

// ── Self-maintaining dependency pins (#2403 follow-on, prompt-only) ──────────
//
// Simard's root `Cargo.toml` pins several tools she maintains by EXACT git rev,
// not by branch:
//
//   amplihack-agent-eval -> rysweet/amplihack-rs        (rev = 59548a96…)
//   amplihack-memory     -> rysweet/amplihack-memory-lib (rev = 26d49bf8…)
//   rustyclawd-core      -> rysweet/RustyClawd          (rev = 43ebaa1c…)
//   rustyclawd-tools     -> rysweet/RustyClawd          (rev = 43ebaa1c…)
//
// A git-rev pin is FROZEN: when Simard lands a fix in one of those upstream
// repos, her own pin keeps pointing at the OLD commit, so the fix she just
// merged is NOT in her own running build. (The motivating case: amplihack-agent-eval
// pinned ~22 commits behind amplihack-rs main → ~9 merged PRs absent from her
// own daemon.)
//
// The fix is two prompt-only behaviours, fully specified in
// `docs/howto/self-maintain-dependency-pins.md`:
//
//   (A) REACTIVE done-gate — when an engineer LANDS an upstream change to a
//       build-dependency repo, the SAME goal is not "done" until Simard has also
//       bumped the matching `Cargo.toml` rev to the merged commit, verified
//       `cargo build`, and LANDED that bump PR against `rysweet/Simard`. Opening
//       the upstream PR is NOT the finish line; shipping it into her own running
//       build is. (Daemon redeploy stays operator-gated.)
//   (B) PROACTIVE reconcile — as low-priority idle/research-time self-maintenance,
//       detect when a pinned rev has fallen behind its upstream default branch and
//       open/update a bump follow-up.
//
// These assertions FAIL until the prompt edits land. The feature is PROMPT-ONLY
// (no Rust logic change) and must COMPOSE additively with #2404 loop-awareness,
// #2405 per-issue fan-out, #2410 own-PRs-to-landing, and #2413 finalization — the
// dep-bump is a NEW done-gate that runs AFTER landing, alongside #2410. Output
// contracts are PRESERVED: `goal_session_objective.md` stays prose-only (NO ACTION
// / PROGRESS markers intact); `progress_assessment_reviewer.md` and its recipe
// mirror keep the single-line `{"verdict": …}` JSON the Rust parser reads. Phrases
// are checked after `normalize_ws` + lowercase so Markdown wrapping cannot defeat
// them. `engineer_system.md` is asserted via `engineer_system_md()` (include_str!),
// exactly like the existing engineer-system pins above.

/// The progress-assessment recipe-runner recipe, read at compile time — the
/// runtime mirror of `progress_assessment_reviewer.md` (asserted inline exactly
/// like `progress_assessment_recipe_mirrors_done_gate`).
fn progress_assessment_recipe() -> &'static str {
    include_str!("../../prompt_assets/simard/recipes/progress-assessment.yaml")
}

// ── (A) Reactive done-gate in goal_session_objective.md ──────────────────────

#[test]
fn goal_session_objective_has_dependency_pin_done_gate() {
    // The OODA brain must carry an explicit dependency-pin done-gate: landing an
    // upstream change to a build-dependency is not the finish line; the fix must
    // ship into Simard's own running build.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("dependency-pin done-gate"),
        "goal_session_objective.md must declare an explicit dependency-pin done-gate"
    );
    assert!(
        norm.contains("build-dependency"),
        "the gate must scope to Simard's build-dependency (git-rev-pinned) repos"
    );
    assert!(
        norm.contains("opening the upstream pr is not the finish line"),
        "must teach that opening/merging the UPSTREAM PR is not the finish line"
    );
    assert!(
        norm.contains("running build"),
        "the deliverable is the fix shipping into Simard's own running build"
    );
}

#[test]
fn goal_session_objective_dep_gate_requires_bump_build_and_landing() {
    // The gate's concrete steps: bump the own Cargo.toml rev, verify `cargo build`,
    // and LAND the bump PR — the goal is not done until all three hold.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("is not done until"),
        "the gate must state the goal is not done until the own-pin bump ships"
    );
    assert!(
        norm.contains("cargo.toml"),
        "the gate must direct bumping the matching rev in the root Cargo.toml"
    );
    assert!(
        norm.contains("bump"),
        "the gate must direct a rev bump of the own dependency pin"
    );
    assert!(
        norm.contains("cargo build"),
        "the gate must require `cargo build` to verify the new rev before shipping"
    );
    assert!(
        norm.contains("bump pr"),
        "the gate must require opening/landing a bump PR for the own pin change"
    );
}

#[test]
fn goal_session_objective_dep_gate_redeploy_is_operator_gated() {
    // The done-gate guarantees the fix is in the SOURCE build; the actual daemon
    // redeploy stays operator-gated and is NOT required for the goal to be done.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("operator-gated"),
        "the daemon redeploy must be described as operator-gated"
    );
    assert!(
        norm.contains("not required for") || norm.contains("not required to"),
        "the operator redeploy must be explicitly NOT required for the goal to be done"
    );
}

#[test]
fn goal_session_objective_has_proactive_dependency_drift_note() {
    // (B) Proactive reconcile must be NOTED as acceptable low-priority idle/research
    // self-maintenance — the upstream-repo analog of the existing Self-update awareness.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("dependency-drift"),
        "must note a proactive dependency-drift reconcile activity"
    );
    assert!(
        norm.contains("fallen behind"),
        "drift means a pinned rev has fallen behind its upstream default branch"
    );
    assert!(
        norm.contains("low-priority"),
        "the proactive reconcile must be framed as LOW-priority (never preempts real work)"
    );
    assert!(
        norm.contains("self-maintenance"),
        "the proactive reconcile is self-maintenance / idle-time work"
    );
}

#[test]
fn goal_session_objective_dep_gate_preserves_prose_contract() {
    // Output-contract guard: the new gate must stay additive PROSE — it must NOT
    // introduce a JSON verdict shape, and must keep the NO ACTION / PROGRESS
    // markers the goal-session parser reads. (Combined with a new-behaviour
    // assertion so the test fails until the gate lands.)
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("dependency-pin done-gate"),
        "precondition: the dependency-pin done-gate must be present"
    );
    assert!(
        !content.contains("\"verdict\""),
        "goal_session_objective.md is prose-only — the dep-gate must not add a JSON verdict contract"
    );
    assert!(
        content.contains("NO ACTION"),
        "the prose `NO ACTION` marker the parser reads must be preserved"
    );
    assert!(
        content.contains("PROGRESS:"),
        "the prose `PROGRESS: NN` marker the parser reads must be preserved"
    );
}

#[test]
fn goal_session_objective_dep_gate_composes_with_2410_done_gate() {
    // Regression/compose guard: the dep-pin done-gate builds ON TOP of the #2410
    // merged-AND-closed done-gate — both must coexist, not replace one another.
    let content = embedded_fallback("goal_session_objective.md")
        .expect("goal_session_objective.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("complete only when its pr is merged and the linked issue is closed"),
        "must preserve the #2410 merged-AND-closed done-gate"
    );
    assert!(
        norm.contains("dependency-pin done-gate"),
        "the new dependency-pin done-gate must compose alongside the #2410 done-gate"
    );
}

// ── (A) Engineer follow-through + (B) drift directive in engineer_system.md ──

#[test]
fn engineer_system_bumps_own_pin_after_landing_upstream() {
    // The engineer that LANDED an upstream change must follow through in the same
    // cycle: bump the own Cargo.toml rev and re-verify the build.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("bump your own pin"),
        "engineer_system.md must direct bumping Simard's own pin after landing upstream"
    );
    assert!(
        norm.contains("not done when the upstream pr merges"),
        "the engineer is not done when the upstream PR merges — the fix isn't in her build yet"
    );
    assert!(
        norm.contains("cargo.toml"),
        "the engineer must edit the matching rev in the root Cargo.toml"
    );
    assert!(
        norm.contains("cargo build"),
        "the engineer must re-verify with `cargo build` (a bump that does not build is rolled back)"
    );
}

#[test]
fn engineer_system_dep_bump_pr_convention_and_dedup() {
    // The bump PR uses a deterministic, upstream-repo-keyed naming convention and
    // is de-duplicated: an already-open bump PR is UPDATED, never duplicated.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("chore/bump-"),
        "must use the deterministic `chore/bump-<upstream-repo>-pin` branch convention"
    );
    assert!(
        norm.contains("chore(deps): bump"),
        "must use the `chore(deps): bump <upstream-repo> pin to <short-sha>` title convention"
    );
    assert!(
        norm.contains("rysweet/simard"),
        "the bump PR is opened against rysweet/Simard (where Cargo.toml lives)"
    );
    assert!(
        norm.contains("already open") && norm.contains("update it"),
        "an already-open bump PR for that repo must be UPDATED, not duplicated"
    );
}

#[test]
fn engineer_system_dep_bump_atomic_for_shared_repo() {
    // Crates that pin the SAME upstream repo must be bumped together in one commit:
    // rustyclawd-core and rustyclawd-tools both pin RustyClawd, so a bump moves both
    // in the same PR — never split one upstream commit across two PRs.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("rustyclawd-core") && norm.contains("rustyclawd-tools"),
        "must name the two crates that share the RustyClawd repo pin"
    );
    assert!(
        norm.contains("together in one commit"),
        "crates sharing an upstream repo must be re-pointed together in one commit"
    );
}

#[test]
fn engineer_system_has_proactive_dependency_drift_directive() {
    // (B) The proactive reconcile lives as a low-priority dependency-drift directive
    // in engineer_system.md: compare each pinned rev to the upstream default branch.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("dependency-drift"),
        "engineer_system.md must carry a dependency-drift self-maintenance directive"
    );
    assert!(
        norm.contains("fallen behind"),
        "drift detection means a pinned rev has fallen behind its upstream default branch"
    );
    assert!(
        norm.contains("ls-remote"),
        "drift detection must use runtime git tooling (e.g. `git ls-remote`) — no new Rust subsystem"
    );
    assert!(
        norm.contains("low-priority"),
        "the drift directive must be low-priority and never preempt an active goal"
    );
}

#[test]
fn engineer_system_dep_bump_preserves_2410_landing() {
    // Regression/compose guard: the dep-bump follow-through builds ON TOP of #2410 —
    // the continue-existing-PR-never-duplicate guidance must remain.
    let norm = normalize_ws(engineer_system_md()).to_lowercase();
    assert!(
        norm.contains("continue it, never duplicate it"),
        "must preserve the #2410 continue-existing-PR-never-duplicate guidance"
    );
    assert!(
        norm.contains("bump your own pin"),
        "the new dep-bump follow-through must compose alongside the #2410 landing guidance"
    );
}

// ── (A) Reviewer enforcement in progress_assessment_reviewer.md (+ recipe) ───

#[test]
fn progress_reviewer_rejects_unbumped_dep_after_upstream_landing() {
    // The reviewer cannot diff git revs (text-only; it fails CLOSED on a
    // semantic verdict parse-miss but cannot inspect commits),
    // so the rule is EVIDENCE-ABSENCE: reject a done/100% claim that describes landing
    // an upstream build-dependency change but shows no evidence of BOTH the own
    // Cargo.toml rev bump AND a verified `cargo build`.
    let content = embedded_fallback("progress_assessment_reviewer.md")
        .expect("progress_assessment_reviewer.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("build-dependency"),
        "the reviewer must recognise an upstream build-dependency landing claim"
    );
    assert!(
        norm.contains("landing upstream is not done"),
        "the reviewer must encode that landing upstream is not done until the own-pin bump ships"
    );
    assert!(
        norm.contains("cargo.toml") && norm.contains("cargo build"),
        "the rejection must hinge on missing evidence of the Cargo.toml rev bump + cargo build"
    );
    assert!(
        norm.contains("no evidence"),
        "the rule must be phrased as evidence-absence (the reviewer cannot diff revs)"
    );
    assert!(
        norm.contains("reject"),
        "a premature done/100% upstream-landing claim must be rejected"
    );
}

#[test]
fn progress_reviewer_dep_gate_preserves_json_contract() {
    // Output-contract guard: the new rule must NOT change the single-line verdict
    // JSON the Rust reviewer parser reads. Combined with a new-behaviour assertion
    // so the test fails until the rule lands.
    let content = embedded_fallback("progress_assessment_reviewer.md")
        .expect("progress_assessment_reviewer.md must be registered");
    let norm = normalize_ws(content).to_lowercase();
    assert!(
        norm.contains("landing upstream is not done"),
        "precondition: the dep-bump rejection rule must be present"
    );
    assert!(
        content.contains("\"verdict\""),
        "the single-line verdict JSON output contract must be preserved"
    );
    assert!(
        content.contains("\"accept\"") && content.contains("\"reject\""),
        "the verdict tokens must stay exactly \"accept\" / \"reject\" for the Rust parser"
    );
}

#[test]
fn progress_assessment_recipe_mirrors_dep_bump_gate() {
    // The runtime recipe-runner recipe must stay in sync with the embedded
    // progress_assessment_reviewer.md on the dep-bump evidence gate.
    let recipe = progress_assessment_recipe();
    let norm = normalize_ws(recipe).to_lowercase();
    assert!(
        norm.contains("build-dependency"),
        "progress-assessment.yaml must mirror the upstream build-dependency landing gate"
    );
    assert!(
        norm.contains("landing upstream is not done"),
        "recipe mirror: landing upstream is not done until the own-pin bump ships"
    );
    assert!(
        norm.contains("cargo.toml") && norm.contains("cargo build"),
        "recipe mirror: the gate hinges on the Cargo.toml rev bump + cargo build evidence"
    );
    assert!(
        norm.contains("no evidence"),
        "recipe mirror: evidence-absence formulation must be preserved"
    );
    assert!(
        recipe.contains("\"verdict\""),
        "progress-assessment.yaml must keep the verdict JSON output contract"
    );
}

// ── Goal-decomposition prompt content-pin (issue #2405) ────────────────────
//
// The `decompose_goal` driver parses this prompt's output, so its wording is a
// hard contract: the prompt must instruct the model to emit a bounded set of
// sub-goals, each carrying a `done_criterion` and an optional `depends_on`
// ordering. This guards the embedded fallback for `goal_decomposition.md`
// (the prompt asset + `embedded_fallback` arm).

#[test]
fn goal_decomposition_prompt_is_embedded() {
    assert!(
        embedded_fallback("goal_decomposition.md").is_some(),
        "the goal_decomposition.md prompt must be compiled in as an embedded fallback"
    );
}

#[test]
fn goal_decomposition_prompt_pins_output_contract() {
    let prompt =
        embedded_fallback("goal_decomposition.md").expect("goal_decomposition.md embedded prompt");
    let lower = prompt.to_lowercase();

    // Intent.
    assert!(
        lower.contains("decompose"),
        "prompt must describe decomposition"
    );
    assert!(
        lower.contains("sub-goal") || lower.contains("sub goal") || lower.contains("subgoal"),
        "prompt must talk about sub-goals"
    );

    // Fan-out bound 2..=6.
    assert!(
        prompt.contains('2') && prompt.contains('6'),
        "prompt must pin the 2-to-6 fan-out bound"
    );

    // JSON output contract the parser depends on (these keys map to
    // SubGoalProposal fields).
    assert!(
        prompt.contains("done_criterion"),
        "each sub-goal must carry a done_criterion"
    );
    assert!(
        prompt.contains("depends_on"),
        "the prompt must allow an optional depends_on ordering between sub-goals"
    );
    assert!(
        prompt.contains("description"),
        "each sub-goal must carry a description"
    );
}

// Issue #2690 — the engineer-admission prompt is embedded, loads in pure
// embedded mode, and pins the overlap-reasoning + output contract the shim
// depends on.
#[test]
fn engineer_admission_prompt_is_embedded_and_pins_overlap_contract() {
    // Registered in the embedded-fallback table.
    let prompt = embedded_fallback("ooda_engineer_admission.md")
        .expect("ooda_engineer_admission.md embedded prompt");

    // Loads non-empty in pure embedded mode (no disk dir).
    let store = PromptStore::new(None);
    assert_eq!(
        store.load("ooda_engineer_admission.md"),
        prompt,
        "embedded mode must return the embedded admission prompt"
    );

    let lower = prompt.to_lowercase();
    // Reasoning intent: file-footprint overlap between the candidate and the
    // in-flight engineers.
    assert!(
        lower.contains("overlap"),
        "prompt must reason about overlap"
    );
    assert!(
        lower.contains("collide") || lower.contains("collision"),
        "prompt must name the merge-collision problem"
    );
    // The three decision variants the parser (admission_decision_from_variant)
    // depends on.
    assert!(prompt.contains("admit"), "must document the admit variant");
    assert!(prompt.contains("defer"), "must document the defer variant");
    assert!(
        prompt.contains("serialize_after"),
        "must document the serialize_after variant"
    );
    // The load-bearing envelope fields the shim reads explicitly.
    assert!(prompt.contains("blocked_by"), "defer carries blocked_by");
    assert!(
        prompt.contains("after_goal_id") && prompt.contains("overlap_files"),
        "serialize_after carries after_goal_id + overlap_files"
    );
    // Fail-OPEN polarity is pinned so a future edit cannot silently flip it.
    assert!(
        lower.contains("fail") && lower.contains("open"),
        "prompt must pin the fail-open contract"
    );
    // The canonical collisions this gate exists to catch are anchored in the
    // few-shot examples.
    assert!(
        prompt.contains("goals_status.rs"),
        "few-shot must anchor on the goals_status.rs collision"
    );
    assert!(
        lower.contains("adapter"),
        "few-shot must anchor on the Adapter-rename incident"
    );
}

// Issue #2706 — the resource-admission prompt is embedded, loads in pure
// embedded mode, and pins the resource-reasoning + output contract the shim
// depends on (disk/build-cache/load → admit | defer | reclaim_first).
#[test]
fn resource_admission_prompt_is_embedded_and_pins_resource_contract() {
    // Registered in the embedded-fallback table.
    let prompt = embedded_fallback("ooda_resource_admission.md")
        .expect("ooda_resource_admission.md embedded prompt");

    // Loads non-empty in pure embedded mode (no disk dir).
    let store = PromptStore::new(None);
    assert_eq!(
        store.load("ooda_resource_admission.md"),
        prompt,
        "embedded mode must return the embedded resource-admission prompt"
    );

    let lower = prompt.to_lowercase();
    // Reasoning intent: host resource affordability, not file overlap.
    assert!(lower.contains("disk"), "prompt must reason about disk");
    assert!(
        lower.contains("build") && lower.contains("cache"),
        "prompt must reason about build caches"
    );
    assert!(
        lower.contains("load average") || lower.contains("load_avg"),
        "prompt must reason about system load"
    );
    // The hard-rail / ENOSPC framing must be pinned so an edit cannot imply the
    // prompt owns the out-of-space guarantee (it is enforced in Rust).
    assert!(
        lower.contains("ceiling"),
        "prompt must reference the disk ceiling"
    );
    assert!(
        lower.contains("enospc")
            || lower.contains("out-of-space")
            || lower.contains("out of space"),
        "prompt must name the ENOSPC hazard"
    );
    // The three decision variants the parser
    // (resource_admission_decision_from_variant) depends on.
    assert!(prompt.contains("admit"), "must document the admit variant");
    assert!(prompt.contains("defer"), "must document the defer variant");
    assert!(
        prompt.contains("reclaim_first"),
        "must document the reclaim_first variant"
    );
    // Fail-CLOSED polarity is pinned so a future edit cannot silently flip it —
    // a resource-gate brain error must DEFER, not admit.
    assert!(
        lower.contains("fail") && (lower.contains("closed") || lower.contains("defer")),
        "prompt must pin the fail-closed contract"
    );
}
