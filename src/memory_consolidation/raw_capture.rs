//! Env-gated, default-off raw-capture of a distillation *parse failure*'s raw
//! recipe-runner stdout, so a **real currently-failing sample** can be harvested
//! and turned into a regression test (2026-07-02 operator-review priority 1).
//!
//! # Status: implemented (Wave 1)
//!
//! The contract below (see `docs/reference/distill-raw-capture-on-parse-failure.md`)
//! is fully implemented and covered by the `#[cfg(test)]` suite at the bottom of
//! this file. Every function has a real body; the capture path is `pub` in a
//! `pub mod`, so it carries no `dead_code` and the release build stays green.
//!
//! # Contract (what the implementation must satisfy)
//!
//! * Capture is **off unless** `SIMARD_DISTILL_RAW_CAPTURE` is truthy
//!   (`1`/`true`/`yes`/`on`, case-insensitive).
//! * A sample is written **only** for `failure_class == "parse-failure"` — the
//!   one class that reached (and failed) output parsing.
//! * Samples live under a mode-`0700` directory as mode-`0600` files named
//!   `distill-parsefail-<UTC>-<6hex>.txt`, size-capped and rotation-bounded.
//! * The raw stdout is written **verbatim** below a self-describing header — it
//!   is exactly what a regression fixture must reproduce.
//! * The path is **best-effort and panic-free**: any I/O error is logged and
//!   swallowed (`Ok(None)`), never turning a recoverable distill miss into a
//!   hard error, and untrusted agent bytes are never parsed, only truncated.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Master toggle env var. Truthy (`1`/`true`/`yes`/`on`, case-insensitive)
/// enables capture; anything else (incl. unset/empty/`0`/`false`) leaves it off.
pub const ENV_ENABLE: &str = "SIMARD_DISTILL_RAW_CAPTURE";
/// Per-sample byte cap env var (clamped to `[1024, 4_194_304]`).
pub const ENV_MAX_BYTES: &str = "SIMARD_DISTILL_RAW_CAPTURE_MAX_BYTES";
/// Rotation-ring size env var (clamped to `[0, 10_000]`; `0` disables pruning).
pub const ENV_KEEP: &str = "SIMARD_DISTILL_RAW_CAPTURE_KEEP";
/// Capture-directory override env var (relative paths resolve under state home).
pub const ENV_DIR: &str = "SIMARD_DISTILL_RAW_CAPTURE_DIR";

/// Default per-sample byte cap (64 KiB).
pub const DEFAULT_MAX_BYTES: usize = 65_536;
/// Default rotation-ring size.
pub const DEFAULT_KEEP: usize = 20;
/// Lower/upper clamp bounds for the per-sample byte cap.
pub const MAX_BYTES_FLOOR: usize = 1_024;
pub const MAX_BYTES_CEIL: usize = 4_194_304;
/// Upper clamp bound for the rotation-ring size.
pub const KEEP_CEIL: usize = 10_000;

/// Resolved, validated capture settings. Built once per pass from the
/// environment via [`RawCaptureConfig::from_env`], applying the documented
/// defaults and clamps. Never panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCaptureConfig {
    /// `true` iff [`ENV_ENABLE`] was truthy.
    pub enabled: bool,
    /// Absolute (or state-home-resolved) directory samples are written to.
    pub dir: PathBuf,
    /// Per-sample byte cap after clamping.
    pub max_bytes: usize,
    /// Rotation-ring size after clamping (`0` = unbounded).
    pub keep: usize,
}

impl RawCaptureConfig {
    /// Read + validate all `SIMARD_DISTILL_RAW_CAPTURE*` vars, applying the
    /// documented defaults and clamps. Never panics.
    ///
    /// * `enabled` ← [`ENV_ENABLE`] truthiness.
    /// * `max_bytes` ← [`ENV_MAX_BYTES`] clamped to `[MAX_BYTES_FLOOR, MAX_BYTES_CEIL]`;
    ///   invalid/`0`/out-of-range → [`DEFAULT_MAX_BYTES`].
    /// * `keep` ← [`ENV_KEEP`] clamped to `[0, KEEP_CEIL]`; invalid → [`DEFAULT_KEEP`].
    /// * `dir` ← [`ENV_DIR`] override, else `<state-home>/distill-captures`.
    pub fn from_env() -> Self {
        let enabled = std::env::var(ENV_ENABLE)
            .ok()
            .is_some_and(|v| is_truthy(&v));

        // Numeric knobs: a parseable value is clamped to its documented range;
        // an unset/empty/non-numeric value falls back to the default. `max_bytes`
        // has a floor (a `0`/below-floor cap is meaningless), while `keep == 0`
        // is a legal "never prune" value, so `keep` has no floor.
        let max_bytes = std::env::var(ENV_MAX_BYTES)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .map(|n| n.clamp(MAX_BYTES_FLOOR, MAX_BYTES_CEIL))
            .unwrap_or(DEFAULT_MAX_BYTES);
        let keep = std::env::var(ENV_KEEP)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .map(|n| n.min(KEEP_CEIL))
            .unwrap_or(DEFAULT_KEEP);

        let dir = std::env::var_os(ENV_DIR)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(default_capture_dir);

        Self {
            enabled,
            dir,
            max_bytes,
            keep,
        }
    }
}

/// `true` for the case-insensitive truthy tokens `1`/`true`/`yes`/`on`; every
/// other value (incl. unset/empty/`0`/`false`/`off`/`no`/arbitrary) is falsy.
fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Default capture dir: `<state-root>/distill-captures`, a sibling of the
/// operator's `metrics/` sink.
fn default_capture_dir() -> PathBuf {
    state_root().join("distill-captures")
}

/// `<state-root>`: `SIMARD_STATE_ROOT` if set to a non-empty value, else
/// `$HOME/.simard` — the same canonical location as the rest of Simard's state
/// (see [`crate::runtime_config`]). Note this treats an *empty* `SIMARD_STATE_ROOT`
/// as unset (falling back to `$HOME/.simard`) rather than honoring the empty
/// value verbatim, so a blank override can never redirect captures to the
/// process CWD.
fn state_root() -> PathBuf {
    if let Ok(v) = std::env::var("SIMARD_STATE_ROOT")
        && !v.trim().is_empty()
    {
        return PathBuf::from(v);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| {
        tracing::warn!(
            "HOME env var not set; defaulting distill-capture state root to /home/azureuser"
        );
        "/home/azureuser".to_string()
    });
    PathBuf::from(home).join(".simard")
}

/// Metadata recorded in the sample header. Mirrors the distill metrics context
/// (`build_distill_success_context`) so a capture correlates 1:1 with the
/// `metrics.jsonl` event it came from.
#[derive(Debug, Clone, Copy)]
pub struct CaptureMeta<'a> {
    /// Stable failure-class label — capture only fires for `"parse-failure"`.
    pub failure_class: &'a str,
    /// Whether the recipe process exited `0`.
    pub recipe_exited_ok: bool,
    /// 1-based runner invocation count for the pass.
    pub attempt: u32,
    /// `true` iff a success followed at least one transient retry.
    pub recovered_after_retry: bool,
    /// Episodes fed to the pass.
    pub input_count: u32,
    /// Facts extracted (`0` on failure).
    pub fact_count: u32,
}

/// Persist `raw` stdout for a parse failure.
///
/// No-op (returns `Ok(None)`, writes nothing) when capture is disabled or
/// `meta.failure_class != "parse-failure"`. On success returns
/// `Ok(Some(path))` with the written sample path.
///
/// **Best-effort:** on any I/O error it logs via `tracing::warn!` and returns
/// `Ok(None)` rather than surfacing the error — capturing a diagnostic must
/// never turn a recoverable distill miss into a hard error.
pub fn capture_parse_failure(
    cfg: &RawCaptureConfig,
    meta: &CaptureMeta<'_>,
    raw: &str,
) -> std::io::Result<Option<PathBuf>> {
    // Gate 1: the operator opt-in. Gate 2: only the ONE class that reached (and
    // failed) output parsing — every other class never produced parseable stdout
    // worth harvesting.
    if !cfg.enabled || meta.failure_class != PARSE_FAILURE_CLASS {
        return Ok(None);
    }

    // Best-effort: a diagnostic must NEVER turn a recoverable distill miss into a
    // hard error. Any I/O failure is logged and swallowed to `Ok(None)`.
    match write_sample(cfg, meta, raw) {
        Ok(path) => Ok(Some(path)),
        Err(e) => {
            tracing::warn!(
                error = %e,
                dir = ?cfg.dir,
                "distill raw-capture failed (non-fatal); dropping sample"
            );
            Ok(None)
        }
    }
}

/// The single failure class captured — the one that exited `0`, ran a step, and
/// still yielded no parseable facts object. Matches
/// [`crate::memory_consolidation::distillation::DistillFailureClass::as_str`].
const PARSE_FAILURE_CLASS: &str = "parse-failure";

/// Process-unique suffix source, so two captures within the same wall-clock
/// nanosecond (or a clock that fails to advance) still get distinct filenames.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Do the actual work: create the `0700` dir, render header + (capped) verbatim
/// raw into a `0600` file, then prune the rotation ring. Returns the written
/// path. All fallible I/O bubbles up for the caller to swallow.
fn write_sample(
    cfg: &RawCaptureConfig,
    meta: &CaptureMeta<'_>,
    raw: &str,
) -> std::io::Result<PathBuf> {
    ensure_capture_dir(&cfg.dir)?;
    let body = render_sample(cfg, meta, raw);
    let path = cfg.dir.join(sample_filename());
    write_private_file(&path, body.as_bytes())?;
    prune_ring(&cfg.dir, cfg.keep);
    Ok(path)
}

/// Create (if needed) and enforce mode-`0700` on the capture directory.
#[cfg(unix)]
fn ensure_capture_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    // Enforce 0700 even if the dir pre-existed with looser bits.
    let mut perm = std::fs::metadata(dir)?.permissions();
    if perm.mode() & 0o777 != 0o700 {
        perm.set_mode(0o700);
        std::fs::set_permissions(dir, perm)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_capture_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Write `bytes` to `path` as a mode-`0600` file (create/truncate).
#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

/// `distill-parsefail-<UTC>-<seq-hex>.txt`. The compact UTC prefix sorts
/// chronologically; the monotonic `seq` guarantees uniqueness even on identical
/// timestamps.
fn sample_filename() -> String {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("distill-parsefail-{stamp}-{seq:06x}.txt")
}

/// Render the self-describing header (mirroring the distill metrics context) plus
/// the verbatim raw stdout, truncated at `cfg.max_bytes` on a char boundary.
fn render_sample(cfg: &RawCaptureConfig, meta: &CaptureMeta<'_>, raw: &str) -> String {
    let raw_bytes = raw.len();
    let (payload, note) = if raw_bytes > cfg.max_bytes {
        let end = floor_char_boundary(raw, cfg.max_bytes);
        (
            &raw[..end],
            format!(" (truncated to {end} of {raw_bytes} bytes)"),
        )
    } else {
        (raw, String::new())
    };

    let mut s = String::with_capacity(payload.len() + 256);
    s.push_str("# distill parse-failure raw capture\n");
    s.push_str("# See docs/reference/distill-raw-capture-on-parse-failure.md\n");
    s.push_str(&format!("# failure_class: {}\n", meta.failure_class));
    s.push_str(&format!("# recipe_exited_ok: {}\n", meta.recipe_exited_ok));
    s.push_str(&format!("# attempt: {}\n", meta.attempt));
    s.push_str(&format!(
        "# recovered_after_retry: {}\n",
        meta.recovered_after_retry
    ));
    s.push_str(&format!("# input_count: {}\n", meta.input_count));
    s.push_str(&format!("# fact_count: {}\n", meta.fact_count));
    s.push_str(&format!(
        "# captured_at: {}\n",
        chrono::Utc::now().to_rfc3339()
    ));
    s.push_str(&format!("# raw_bytes: {raw_bytes}{note}\n"));
    s.push_str("# ---- raw recipe-runner stdout (verbatim) ----\n");
    s.push_str(payload);
    s
}

/// Largest byte index `<= max` that lands on a UTF-8 char boundary, so a byte
/// cap can never split a multi-byte character (`str::floor_char_boundary` is
/// still unstable).
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Keep only the newest `keep` `distill-parsefail-*.txt` samples (`keep == 0`
/// disables pruning). Best-effort: any listing/removal error is ignored — the
/// ring is hygiene, never correctness.
fn prune_ring(dir: &Path, keep: usize) {
    if keep == 0 {
        return;
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    let mut samples: Vec<(SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| is_sample_name(&e.file_name().to_string_lossy()))
        .map(|e| {
            let mtime = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (mtime, e.path())
        })
        .collect();
    if samples.len() <= keep {
        return;
    }
    // Oldest first (mtime, then path as a stable tiebreak for identical mtimes).
    samples.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let remove = samples.len() - keep;
    for (_, path) in samples.into_iter().take(remove) {
        let _ = std::fs::remove_file(path);
    }
}

/// `true` for a `distill-parsefail-*.txt` sample filename.
fn is_sample_name(name: &str) -> bool {
    name.starts_with("distill-parsefail-") && name.ends_with(".txt")
}

#[cfg(test)]
mod tests {
    //! Contract tests (Wave 1). These exercise the documented behavior of the
    //! implemented module. Env-mutating tests are `#[serial]`; each uses
    //! [`EnvGuard`] so the process-global env is restored on drop.

    use super::*;
    use serial_test::serial;
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    // ── scoped env override (restores on drop) ──────────────────────────
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: env-mutating tests in this module are `#[serial]`.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: env-mutating tests in this module are `#[serial]`.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: env-mutating tests in this module are `#[serial]`.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    /// A `CaptureMeta` for the parse-failure case the diagnostic exists to catch.
    fn parse_failure_meta() -> CaptureMeta<'static> {
        CaptureMeta {
            failure_class: "parse-failure",
            recipe_exited_ok: true,
            attempt: 2,
            recovered_after_retry: false,
            input_count: 34,
            fact_count: 0,
        }
    }

    /// A capture config that is enabled and writes to `dir` with generous caps.
    fn enabled_cfg(dir: PathBuf) -> RawCaptureConfig {
        RawCaptureConfig {
            enabled: true,
            dir,
            max_bytes: DEFAULT_MAX_BYTES,
            keep: DEFAULT_KEEP,
        }
    }

    fn parsefail_samples(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = match fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("distill-parsefail-") && n.ends_with(".txt"))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        v.sort();
        v
    }

    // ─────────────────────────── from_env ───────────────────────────────

    #[test]
    #[serial(simard_distill_raw_capture_env, cognitive_memory)]
    fn from_env_defaults_to_disabled_when_unset() {
        let _e = EnvGuard::unset(ENV_ENABLE);
        let _mb = EnvGuard::unset(ENV_MAX_BYTES);
        let _k = EnvGuard::unset(ENV_KEEP);
        let cfg = RawCaptureConfig::from_env();
        assert!(
            !cfg.enabled,
            "capture must default OFF when the toggle is unset"
        );
        assert_eq!(cfg.max_bytes, DEFAULT_MAX_BYTES);
        assert_eq!(cfg.keep, DEFAULT_KEEP);
    }

    #[test]
    #[serial(simard_distill_raw_capture_env, cognitive_memory)]
    fn from_env_truthy_variants_enable_capture() {
        for v in ["1", "true", "TRUE", "yes", "Yes", "on", "ON"] {
            let _e = EnvGuard::set(ENV_ENABLE, v);
            assert!(
                RawCaptureConfig::from_env().enabled,
                "{v:?} must enable capture"
            );
        }
    }

    #[test]
    #[serial(simard_distill_raw_capture_env, cognitive_memory)]
    fn from_env_falsy_variants_leave_capture_off() {
        for v in ["", "0", "false", "off", "no", "banana"] {
            let _e = EnvGuard::set(ENV_ENABLE, v);
            assert!(
                !RawCaptureConfig::from_env().enabled,
                "{v:?} must NOT enable capture"
            );
        }
    }

    #[test]
    #[serial(simard_distill_raw_capture_env, cognitive_memory)]
    fn from_env_max_bytes_clamps_and_defaults() {
        let _e = EnvGuard::set(ENV_ENABLE, "1");

        // In-range value is honoured verbatim.
        let _mb = EnvGuard::set(ENV_MAX_BYTES, "16384");
        assert_eq!(RawCaptureConfig::from_env().max_bytes, 16_384);

        // Zero / below-floor → floor.
        let _mb0 = EnvGuard::set(ENV_MAX_BYTES, "0");
        assert_eq!(RawCaptureConfig::from_env().max_bytes, MAX_BYTES_FLOOR);

        // Above ceiling → ceiling.
        let _mbc = EnvGuard::set(ENV_MAX_BYTES, "999999999");
        assert_eq!(RawCaptureConfig::from_env().max_bytes, MAX_BYTES_CEIL);

        // Non-numeric → default.
        let _mbx = EnvGuard::set(ENV_MAX_BYTES, "not-a-number");
        assert_eq!(RawCaptureConfig::from_env().max_bytes, DEFAULT_MAX_BYTES);
    }

    #[test]
    #[serial(simard_distill_raw_capture_env, cognitive_memory)]
    fn from_env_keep_clamps_and_defaults() {
        let _e = EnvGuard::set(ENV_ENABLE, "1");

        let _k = EnvGuard::set(ENV_KEEP, "5");
        assert_eq!(RawCaptureConfig::from_env().keep, 5);

        // 0 is a legal value (disables pruning), not the default.
        let _k0 = EnvGuard::set(ENV_KEEP, "0");
        assert_eq!(RawCaptureConfig::from_env().keep, 0);

        // Above ceiling → ceiling.
        let _kc = EnvGuard::set(ENV_KEEP, "100000");
        assert_eq!(RawCaptureConfig::from_env().keep, KEEP_CEIL);

        // Non-numeric → default.
        let _kx = EnvGuard::set(ENV_KEEP, "lots");
        assert_eq!(RawCaptureConfig::from_env().keep, DEFAULT_KEEP);
    }

    #[test]
    #[serial(simard_distill_raw_capture_env, cognitive_memory)]
    fn from_env_dir_override_is_honoured_absolute() {
        let tmp = tempfile::tempdir().unwrap();
        let abs = tmp.path().join("captures-here");
        let _e = EnvGuard::set(ENV_ENABLE, "1");
        let _d = EnvGuard::set(ENV_DIR, abs.to_str().unwrap());
        assert_eq!(RawCaptureConfig::from_env().dir, abs);
    }

    #[test]
    #[serial(simard_distill_raw_capture_env, cognitive_memory)]
    fn from_env_default_dir_is_under_state_home() {
        let _e = EnvGuard::set(ENV_ENABLE, "1");
        let _d = EnvGuard::unset(ENV_DIR);
        let dir = RawCaptureConfig::from_env().dir;
        assert!(
            dir.ends_with("distill-captures"),
            "default capture dir must be the state-home `distill-captures` subdir, got {dir:?}"
        );
    }

    // ─────────────────────── capture_parse_failure ──────────────────────

    #[test]
    fn capture_is_noop_when_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = RawCaptureConfig {
            enabled: false,
            ..enabled_cfg(tmp.path().to_path_buf())
        };
        let out = capture_parse_failure(&cfg, &parse_failure_meta(), "raw stdout")
            .expect("best-effort capture never surfaces an error");
        assert!(out.is_none(), "disabled capture must write nothing");
        assert!(
            parsefail_samples(tmp.path()).is_empty(),
            "no sample file must be created when disabled"
        );
    }

    #[test]
    fn capture_is_noop_for_non_parse_failure_class() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = enabled_cfg(tmp.path().to_path_buf());
        for class in [
            "spawn-failure",
            "copilot-terminal-failure",
            "recipe-reported-failure",
            "serialize-failure",
            "other",
        ] {
            let meta = CaptureMeta {
                failure_class: class,
                ..parse_failure_meta()
            };
            let out = capture_parse_failure(&cfg, &meta, "raw stdout")
                .expect("best-effort capture never surfaces an error");
            assert!(
                out.is_none(),
                "class {class:?} never reached parsing — must not be captured"
            );
        }
        assert!(
            parsefail_samples(tmp.path()).is_empty(),
            "no sample file for any non-parse-failure class"
        );
    }

    #[test]
    fn capture_writes_sample_with_header_and_verbatim_raw() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = enabled_cfg(tmp.path().to_path_buf());
        // A raw payload carrying exactly the launch-banner noise a real failing
        // sample would — it must be preserved byte-for-byte for the fixture.
        let raw = "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference).\n\
                   INFO launching copilot binary=/x version=\"GitHub Copilot CLI 1.0.66-2.\"\n\
                   Run 'copilot update' to check for updates.\n\
                   I could not find any facts worth distilling in this batch.";

        let path = capture_parse_failure(&cfg, &parse_failure_meta(), raw)
            .expect("best-effort capture never surfaces an error")
            .expect("an enabled parse-failure capture must write a sample");

        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("distill-parsefail-") && name.ends_with(".txt"),
            "unexpected sample filename: {name}"
        );

        let body = fs::read_to_string(&path).unwrap();
        // Header mirrors the metrics context so a sample is self-describing.
        assert!(
            body.contains("# failure_class: parse-failure"),
            "body:\n{body}"
        );
        assert!(body.contains("# attempt: 2"), "body:\n{body}");
        assert!(body.contains("# input_count: 34"), "body:\n{body}");
        assert!(body.contains("# fact_count: 0"), "body:\n{body}");
        // The raw stdout is present verbatim (below the fence), banner intact.
        assert!(
            body.contains(raw),
            "the exact failing bytes must be preserved verbatim; body:\n{body}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn capture_dir_is_0700_and_file_is_0600() {
        let tmp = tempfile::tempdir().unwrap();
        // Point at a not-yet-created subdir so the module creates it with 0700.
        let dir = tmp.path().join("distill-captures");
        let cfg = enabled_cfg(dir.clone());

        let path = capture_parse_failure(&cfg, &parse_failure_meta(), "raw")
            .expect("best-effort capture never surfaces an error")
            .expect("an enabled parse-failure capture must write a sample");

        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "capture dir must be created mode 0700");

        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "each sample file must be mode 0600");
    }

    #[test]
    fn capture_truncates_payload_at_byte_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = RawCaptureConfig {
            max_bytes: MAX_BYTES_FLOOR, // 1 KiB — smallest legal cap
            ..enabled_cfg(tmp.path().to_path_buf())
        };
        let raw = "X".repeat(MAX_BYTES_FLOOR * 4); // well over the cap

        let path = capture_parse_failure(&cfg, &parse_failure_meta(), &raw)
            .expect("best-effort capture never surfaces an error")
            .expect("an enabled parse-failure capture must write a sample");

        let body = fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("truncated"),
            "an over-cap payload must carry a truncation marker; body head:\n{}",
            &body[..body.len().min(200)]
        );
        assert!(
            body.len() < raw.len(),
            "captured file must be smaller than the un-capped raw payload"
        );
    }

    #[test]
    fn capture_rotation_ring_keeps_only_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let keep = 3usize;
        let cfg = RawCaptureConfig {
            keep,
            ..enabled_cfg(tmp.path().to_path_buf())
        };
        // Write more than `keep` samples; the ring must prune the oldest.
        for i in 0..keep + 4 {
            capture_parse_failure(&cfg, &parse_failure_meta(), &format!("sample {i}"))
                .expect("best-effort capture never surfaces an error")
                .expect("each enabled parse-failure capture writes a sample");
        }
        let remaining = parsefail_samples(tmp.path()).len();
        assert_eq!(
            remaining, keep,
            "rotation ring must retain exactly `keep` newest samples, found {remaining}"
        );
    }

    #[test]
    fn capture_is_panic_free_on_adversarial_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = enabled_cfg(tmp.path().to_path_buf());
        // Embedded control bytes, a stray NUL, and unbalanced braces must be
        // written verbatim, never parsed — the path handles untrusted stdout.
        let raw = "line1\n{unbalanced\x00\u{1b}[31m\r\n\"quote";
        let out = capture_parse_failure(&cfg, &parse_failure_meta(), raw)
            .expect("best-effort capture never surfaces an error");
        assert!(
            out.is_some(),
            "adversarial-but-valid parse-failure is still captured"
        );
    }
}
