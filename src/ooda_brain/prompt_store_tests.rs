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
