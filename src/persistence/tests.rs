use super::{TempFileGuard, persist_bytes, persist_json};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{label}-{unique}"));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
#[test]
fn persist_json_ignores_planted_legacy_temp_symlink() {
    let temp_dir = TestDir::new("simard-persistence");
    let victim_path = temp_dir.path().join("victim.txt");
    let store_path = temp_dir.path().join("memory_records.json");
    let legacy_temp_path = temp_dir.path().join("memory_records.json.tmp");
    fs::write(&victim_path, "leave-me-alone").expect("victim file should exist");
    symlink(&victim_path, &legacy_temp_path).expect("legacy temp symlink should exist");

    persist_json("memory", &store_path, &vec!["fresh"])
        .expect("persistence should succeed without following the planted symlink");

    let victim_contents =
        fs::read_to_string(&victim_path).expect("victim file should remain readable");
    let store_contents =
        fs::read_to_string(&store_path).expect("store file should be written directly");

    assert_eq!(victim_contents, "leave-me-alone");
    assert!(
        store_contents.contains("fresh"),
        "store payload should be written to the requested destination"
    );
}

#[test]
fn temp_file_guard_removes_uncommitted_temp_file_on_drop() {
    let temp_dir = TestDir::new("simard-persistence-cleanup");
    let store_path = temp_dir.path().join("memory_records.json");
    let temp_path = {
        let mut temp_file =
            TempFileGuard::new("memory", &store_path).expect("temp file guard should open");
        temp_file
            .file_mut()
            .expect("temp file should still be open")
            .write_all(br#"["pending"]"#)
            .expect("temporary payload should be writable");
        let temp_path = temp_file.path().to_path_buf();
        assert!(
            temp_path.is_file(),
            "temporary persistence file should exist before the guard drops"
        );
        temp_path
    };

    assert!(
        !temp_path.exists(),
        "dropping an uncommitted temp file guard should remove the leaked temp file"
    );
    assert!(
        !store_path.exists(),
        "cleanup must not create the destination file before rename succeeds"
    );
}

#[test]
fn load_json_or_default_returns_default_for_missing_file() {
    let temp_dir = TestDir::new("simard-load-missing");
    let path = temp_dir.path().join("nonexistent.json");
    let result: Vec<String> =
        super::load_json_or_default("test", &path).expect("should return default");
    assert!(result.is_empty());
}

#[test]
fn load_json_or_default_reads_valid_file() {
    let temp_dir = TestDir::new("simard-load-valid");
    let path = temp_dir.path().join("data.json");
    fs::write(&path, r#"["alpha","beta"]"#).expect("write test file");
    let result: Vec<String> =
        super::load_json_or_default("test", &path).expect("should parse valid JSON");
    assert_eq!(result, vec!["alpha", "beta"]);
}

#[test]
fn load_json_or_default_rejects_corrupt_file() {
    let temp_dir = TestDir::new("simard-load-corrupt");
    let path = temp_dir.path().join("bad.json");
    fs::write(&path, "not-json!!!").expect("write corrupt file");
    let result: Result<Vec<String>, _> = super::load_json_or_default("test", &path);
    assert!(result.is_err(), "corrupt JSON should produce an error");
}

#[test]
fn persist_json_roundtrip() {
    let temp_dir = TestDir::new("simard-persist-roundtrip");
    let path = temp_dir.path().join("roundtrip.json");
    let data = vec!["one".to_string(), "two".to_string()];
    persist_json("test", &path, &data).expect("persist should succeed");
    let loaded: Vec<String> =
        super::load_json_or_default("test", &path).expect("load should succeed");
    assert_eq!(loaded, data);
}

#[test]
fn persist_json_creates_parent_dirs() {
    let temp_dir = TestDir::new("simard-persist-parents");
    let path = temp_dir
        .path()
        .join("a")
        .join("b")
        .join("c")
        .join("deep.json");
    persist_json("test", &path, &42u32).expect("persist with nested dirs should succeed");
    assert!(
        path.exists(),
        "file should exist in deeply nested directory"
    );
    let loaded: u32 = super::load_json_or_default("test", &path).expect("should load nested file");
    assert_eq!(loaded, 42);
}

#[test]
fn persist_json_overwrites_existing() {
    let temp_dir = TestDir::new("simard-persist-overwrite");
    let path = temp_dir.path().join("overwrite.json");
    persist_json("test", &path, &"first").expect("first write");
    persist_json("test", &path, &"second").expect("second write");
    let loaded: String =
        super::load_json_or_default("test", &path).expect("should load overwritten");
    assert_eq!(loaded, "second");
}

#[test]
fn unique_temp_path_produces_distinct_paths() {
    let parent = Path::new("/tmp");
    let p1 = super::unique_temp_path(parent, "test.json", 0);
    let p2 = super::unique_temp_path(parent, "test.json", 0);
    assert_ne!(p1, p2, "sequential calls should produce unique paths");
}

#[test]
fn temp_file_guard_file_mut_after_close_returns_error() {
    let temp_dir = TestDir::new("simard-guard-closed");
    let store_path = temp_dir.path().join("data.json");
    let mut guard = TempFileGuard::new("test", &store_path).expect("should create guard");
    guard.close();
    let result = guard.file_mut();
    assert!(result.is_err(), "file_mut after close should fail");
}

#[test]
fn temp_file_guard_persist_renames_to_destination() {
    let temp_dir = TestDir::new("simard-guard-persist");
    let store_path = temp_dir.path().join("final.json");
    let mut guard = TempFileGuard::new("test", &store_path).expect("should create guard");
    let temp_path = guard.path().to_path_buf();
    guard
        .file_mut()
        .expect("file open")
        .write_all(b"payload")
        .expect("write");
    guard
        .persist("test", &store_path)
        .expect("persist should succeed");
    assert!(
        store_path.exists(),
        "destination should exist after persist"
    );
    assert!(
        !temp_path.exists(),
        "temp file should be gone after persist"
    );
    let contents = fs::read_to_string(&store_path).expect("read destination");
    assert_eq!(contents, "payload");
}

#[test]
fn persist_json_with_nested_struct() {
    use std::collections::HashMap;
    let temp_dir = TestDir::new("simard-persist-nested");
    let path = temp_dir.path().join("nested.json");
    let mut data = HashMap::new();
    data.insert("key1".to_string(), vec![1, 2, 3]);
    data.insert("key2".to_string(), vec![4, 5]);
    persist_json("test", &path, &data).expect("persist nested");
    let loaded: HashMap<String, Vec<i32>> =
        super::load_json_or_default("test", &path).expect("load nested");
    assert_eq!(loaded, data);
}

#[test]
fn persist_json_empty_value() {
    let temp_dir = TestDir::new("simard-persist-empty");
    let path = temp_dir.path().join("empty.json");
    let data: Vec<String> = vec![];
    persist_json("test", &path, &data).expect("persist empty vec");
    let loaded: Vec<String> = super::load_json_or_default("test", &path).expect("load empty vec");
    assert!(loaded.is_empty());
}

#[test]
fn persist_json_preserves_unicode() {
    let temp_dir = TestDir::new("simard-persist-unicode");
    let path = temp_dir.path().join("unicode.json");
    let data = vec![
        "héllo".to_string(),
        "wörld".to_string(),
        "日本語".to_string(),
    ];
    persist_json("test", &path, &data).expect("persist unicode");
    let loaded: Vec<String> = super::load_json_or_default("test", &path).expect("load unicode");
    assert_eq!(loaded, data);
}

#[test]
fn load_json_or_default_with_boolean() {
    let temp_dir = TestDir::new("simard-load-bool");
    let path = temp_dir.path().join("bool.json");
    fs::write(&path, "true").expect("write bool");
    let loaded: bool = super::load_json_or_default("test", &path).expect("load bool");
    assert!(loaded);
}

#[test]
fn load_json_or_default_type_mismatch_fails() {
    let temp_dir = TestDir::new("simard-load-mismatch");
    let path = temp_dir.path().join("mismatch.json");
    fs::write(&path, r#"{"key": "value"}"#).expect("write object");
    let result: Result<Vec<String>, _> = super::load_json_or_default("test", &path);
    assert!(result.is_err(), "type mismatch should produce error");
}

#[test]
fn unique_temp_path_includes_attempt_number() {
    let parent = Path::new("/tmp");
    let p0 = super::unique_temp_path(parent, "test.json", 0);
    let p5 = super::unique_temp_path(parent, "test.json", 5);
    let p0_str = p0.to_string_lossy();
    let p5_str = p5.to_string_lossy();
    assert!(p0_str.ends_with(".0"), "attempt 0 should end path with .0");
    assert!(p5_str.ends_with(".5"), "attempt 5 should end path with .5");
}

#[test]
fn persist_json_multiple_rapid_writes() {
    let temp_dir = TestDir::new("simard-persist-rapid");
    let path = temp_dir.path().join("rapid.json");
    for i in 0..10 {
        persist_json("test", &path, &i).expect("rapid write should succeed");
    }
    let loaded: i32 = super::load_json_or_default("test", &path).expect("load last write");
    assert_eq!(loaded, 9);
}

#[test]
fn temp_file_guard_new_creates_file() {
    let temp_dir = TestDir::new("simard-guard-creates");
    let store_path = temp_dir.path().join("target.json");
    let guard = TempFileGuard::new("test", &store_path).expect("should create guard");
    assert!(
        guard.path().exists(),
        "temp file should exist after guard creation"
    );
    // guard dropped here, temp cleaned up
}

// ────────────────────────────────────────────────────────────────────────────
// Crash-durability tests (#1918)
// ────────────────────────────────────────────────────────────────────────────

/// Persisting into a freshly-created deep nested directory exercises the new
/// parent-directory fsync path on every component up to the leaf. If
/// `fsync_parent_dir` returned an error for any reason (bad fd, EBADF, ENOTDIR)
/// this test would fail; today it must succeed end-to-end.
#[test]
fn persist_json_durable_under_freshly_created_nested_parent() {
    let temp_dir = TestDir::new("simard-persist-fsync-nested");
    let path = temp_dir
        .path()
        .join("level-a")
        .join("level-b")
        .join("durable.json");
    persist_json("memory", &path, &vec!["alpha", "beta"]).expect("durable persist");
    assert!(
        path.is_file(),
        "destination should exist after durable persist"
    );
    let loaded: Vec<String> =
        super::load_json_or_default("memory", &path).expect("load nested persist");
    assert_eq!(loaded, vec!["alpha".to_string(), "beta".to_string()]);
}

/// Simulate a crash mid-write: write an "old" payload through the durable
/// pipeline, then create a `TempFileGuard` for the same destination, write
/// "new" bytes into the temp file, but drop the guard WITHOUT calling
/// `persist()`. The destination must still hold the old payload, and the
/// temp file must be cleaned up so a recovering process never sees a torn
/// or partial state.
#[test]
fn dropping_temp_file_guard_preserves_prior_destination() {
    let temp_dir = TestDir::new("simard-persist-crash-mid-write");
    let store_path = temp_dir.path().join("ledger.json");

    persist_json("memory", &store_path, &"old-payload").expect("baseline persist of old payload");

    let temp_path = {
        let mut guard = TempFileGuard::new("memory", &store_path).expect("open temp guard");
        guard
            .file_mut()
            .expect("temp file open")
            .write_all(br#""new-payload-that-never-commits""#)
            .expect("write partial new payload");
        let temp_path = guard.path().to_path_buf();
        assert!(temp_path.is_file(), "temp file present before crash");
        temp_path
        // guard dropped here without `.persist()` — simulates a crash between
        // open() and the rename(). The destination MUST still hold the old
        // payload; the loader MUST NEVER observe a torn or empty state.
    };

    assert!(
        !temp_path.exists(),
        "uncommitted temp file should be removed on drop"
    );
    let loaded: String =
        super::load_json_or_default("memory", &store_path).expect("load after simulated crash");
    assert_eq!(
        loaded, "old-payload",
        "destination must still hold the pre-rename payload after a simulated crash"
    );
}

/// Drive `persist_json` in a tight overwrite loop and assert the loader can
/// always decode a complete value at the destination — never an empty file,
/// never a half-written one. This is the "loader never observes a torn
/// state" property from the issue acceptance criteria, exercised on the
/// real persistence pipeline.
#[test]
fn persist_json_overwrite_never_observes_torn_state() {
    let temp_dir = TestDir::new("simard-persist-torn");
    let path = temp_dir.path().join("counter.json");
    persist_json("memory", &path, &0u64).expect("baseline write");
    for value in 1u64..=64 {
        persist_json("memory", &path, &value).expect("overwrite must succeed");
        let observed: u64 = super::load_json_or_default("memory", &path)
            .expect("loader must always observe a fully-formed value");
        assert!(
            observed <= value,
            "loader saw a value from the future ({observed} > {value}); pipeline is not atomic"
        );
    }
}

/// `persist_bytes` is the byte-oriented entry point used by callers that
/// produce their own serialized form (pretty JSON, fixed-format envelopes).
/// Verify it goes through the same durable pipeline as `persist_json` and
/// publishes bit-exact payloads.
#[test]
fn persist_bytes_writes_bit_exact_payload_atomically() {
    let temp_dir = TestDir::new("simard-persist-bytes");
    let path = temp_dir.path().join("blob.bin");
    let payload: &[u8] = b"\x00\x01\x02hello\xff\xfe";
    persist_bytes("test", &path, payload).expect("persist_bytes must succeed");
    let read_back = fs::read(&path).expect("read bit-exact payload");
    assert_eq!(
        read_back, payload,
        "persist_bytes must publish the exact byte sequence given"
    );
}

/// Whenever `persist_bytes` overwrites an existing file, the post-call
/// state must be exclusively the new payload. No leftover temp file may
/// remain, and no concurrent reader could observe a mix.
#[test]
fn persist_bytes_overwrites_existing_atomically() {
    let temp_dir = TestDir::new("simard-persist-bytes-overwrite");
    let path = temp_dir.path().join("blob.bin");
    persist_bytes("test", &path, b"old-bytes").expect("baseline");
    persist_bytes("test", &path, b"new-bytes-longer-than-old").expect("overwrite");
    let read_back = fs::read(&path).expect("read overwritten payload");
    assert_eq!(read_back, b"new-bytes-longer-than-old");
    let stray: Vec<_> = fs::read_dir(temp_dir.path())
        .expect("read parent dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            s.starts_with('.') && s.contains(".tmp.")
        })
        .collect();
    assert!(
        stray.is_empty(),
        "no temp file may remain after a successful persist_bytes; found {stray:?}"
    );
}
