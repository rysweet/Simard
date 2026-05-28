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
