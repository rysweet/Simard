//! Issue #2576 — TDD contract: Simard's runtime cargo features must build BY
//! DEFAULT so no shippable capability is opt-in.
//!
//! Operator directive: "none of the features need to be opt-in in the future."
//! Concretely, a plain `cargo build` / `cargo test` (no `--features`) must yield
//! a fully-capable `simard` binary with the Signal conversation channel
//! (`src/signal_conversation/`) compiled in, so `simard signal run` reaches its
//! real config-loading path instead of the feature-off stub that prints
//! "not compiled into this build".
//!
//! This file encodes the acceptance criteria as two layers of contract:
//!
//! 1. [`default_feature_set_includes_all_runtime_features`] — a manifest-level
//!    assertion that ALWAYS runs, independent of the compiled feature set. It
//!    reads `Cargo.toml` and requires the `[features] default` set to include
//!    every functional/runtime feature (`signal`, `dashboard-audit`) and to
//!    EXCLUDE `slow-tests` (the one intentional opt-in: a slow-test *selection*
//!    gate, not a shippable capability — defaulting it would force slow tests
//!    into every `cargo test` / CI run and blow up CI time). This is the genuine
//!    TDD red: it FAILS on `main` (which declares no `default` set) and passes
//!    once the change lands. It also passes harmlessly under
//!    `--no-default-features`, because it inspects the manifest text rather than
//!    the active compile features.
//!
//! 2. The behavioral `signal_*` tests in the [`signal_present`] module, gated
//!    `#[cfg(feature = "signal")]` so they run under a default `cargo test` and
//!    under `--all-features`, but AUTO-SKIP under `--no-default-features` (where
//!    the Signal channel is legitimately absent and there is nothing to prove).
//!    They drive the real binary via `assert_cmd` and assert the default build's
//!    `simard signal` subcommand is fully wired: `--help` lists `run`, and
//!    `run` (with no `[signal]` config) reaches the real missing-config error
//!    path — never the feature-off "not compiled into this build" stub.
//!
//! Isolation: the behavioral tests point `SIMARD_STATE_ROOT` at a unique
//! directory under the OS temp dir and clear the `SIMARD_SIGNAL_*` env vars, so
//! they never read or write the operator's live `~/.simard` state and never
//! depend on ambient signal configuration.
//!
//! Naming: nothing here is a "bridge"; this exercises the first-class Signal
//! conversation channel and the one-Brain / reasoner terminology only.

use std::path::PathBuf;

/// Read the crate manifest that this test is compiled from.
fn cargo_manifest() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Cargo.toml must be readable at {}: {e}", path.display()))
}

/// Return the text of the `[features]` table (from its header to the next
/// top-level table header), so key lookups cannot be confused by `default-run`
/// in `[package]` or `default`-named keys in other tables.
fn features_section(manifest: &str) -> &str {
    let header = "[features]";
    let start = if manifest.starts_with(header) {
        0
    } else {
        manifest
            .find(&format!("\n{header}"))
            .map(|i| i + 1)
            .expect("Cargo.toml must declare a [features] table")
    };
    let body = &manifest[start + header.len()..];
    let end = body
        .find("\n[")
        .map(|i| header.len() + i)
        .unwrap_or(body.len() + header.len());
    &manifest[start..start + end]
}

/// Parse the `default = [ ... ]` array from the `[features]` table. Returns an
/// empty vector when no `default` key is declared (the pre-change state on
/// `main`), which is exactly what drives this test red until the fix lands.
fn default_feature_set() -> Vec<String> {
    let manifest = cargo_manifest();
    let features = features_section(&manifest);

    let mut offset = 0usize;
    let mut key_pos: Option<usize> = None;
    for line in features.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') && trimmed.starts_with("default") {
            let after_key = trimmed["default".len()..].trim_start();
            if after_key.starts_with('=') {
                key_pos = Some(offset);
                break;
            }
        }
        offset += line.len() + 1; // +1 for the '\n' consumed by `lines()`
    }

    let key_pos = match key_pos {
        Some(pos) => pos,
        None => return Vec::new(),
    };

    let tail = &features[key_pos..];
    let open = tail
        .find('[')
        .expect("`default` feature set must be declared as an array");
    let close = open
        + tail[open..]
            .find(']')
            .expect("`default` feature array must be closed with ']'");

    tail[open + 1..close]
        .split(',')
        .map(|token| {
            token
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string()
        })
        .filter(|token| !token.is_empty())
        .collect()
}

/// The core issue #2576 contract: every functional/runtime feature is built by
/// default, and only the slow-test *selector* stays opt-in.
#[test]
fn default_feature_set_includes_all_runtime_features() {
    let defaults = default_feature_set();

    for feature in ["signal", "dashboard-audit"] {
        assert!(
            defaults.iter().any(|f| f == feature),
            "Cargo.toml `[features] default` must include the runtime feature \
             `{feature}` so a plain `cargo build` yields a fully-capable binary \
             (issue #2576: no shippable capability may be opt-in). \
             Current default set: {defaults:?}"
        );
    }

    assert!(
        !defaults.iter().any(|f| f == "slow-tests"),
        "`slow-tests` must stay OUT of the default set — it is a slow-test \
         *selection* gate, not a shippable capability; defaulting it would force \
         slow tests into every `cargo test` / CI run and blow up CI time. \
         Current default set: {defaults:?}"
    );
}

/// Behavioral proof that the *default build* has the Signal channel compiled in.
///
/// Gated on `feature = "signal"`: `assert_cmd` builds `simard` with the same
/// feature set as this test invocation, so whenever this module compiles the
/// binary under test is guaranteed to carry the Signal channel. Under
/// `--no-default-features` the module (and the binary's Signal code) drop out
/// together, so these tests correctly auto-skip instead of failing on a build
/// that legitimately omits Signal.
#[cfg(feature = "signal")]
mod signal_present {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Output;
    use std::time::{SystemTime, UNIX_EPOCH};

    use assert_cmd::Command;

    /// A unique, self-cleaning state root under the OS temp dir. Honors the
    /// directive to never touch the operator's live `~/.simard`.
    struct TempStateRoot {
        path: PathBuf,
    }

    impl TempStateRoot {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after the unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "simard-signal-default-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("temp state root should be creatable");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempStateRoot {
        fn drop(&mut self) {
            if self.path.exists() {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    /// Build a `simard` command isolated to `state_root`, with the update check
    /// disabled and every `SIMARD_SIGNAL_*` override cleared so `signal run`
    /// deterministically reaches the missing-config path rather than the
    /// operator's ambient configuration.
    fn simard(state_root: &Path) -> Command {
        let mut cmd = Command::cargo_bin("simard").expect("the `simard` binary must build");
        cmd.env("SIMARD_STATE_ROOT", state_root)
            .env("SIMARD_NO_UPDATE_CHECK", "1")
            .env_remove("SIMARD_SIGNAL_ENDPOINT")
            .env_remove("SIMARD_SIGNAL_ACCOUNT")
            .env_remove("SIMARD_SIGNAL_ALLOWLIST")
            .env_remove("SIMARD_SIGNAL_READ_ONLY_UNKNOWN");
        cmd
    }

    fn rendered(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    const NOT_COMPILED: &str = "not compiled into this build";

    #[test]
    fn signal_help_succeeds_and_lists_run() {
        let state = TempStateRoot::new();
        let output = simard(state.path())
            .args(["signal", "--help"])
            .output()
            .expect("`simard signal --help` should spawn");
        let rendered = rendered(&output);

        assert!(
            output.status.success(),
            "`simard signal --help` must exit 0 in a default build; got {:?}\n{rendered}",
            output.status.code()
        );
        assert!(
            rendered.contains("run"),
            "`simard signal --help` must document the `run` subcommand; got:\n{rendered}"
        );
        assert!(
            !rendered.contains(NOT_COMPILED),
            "a default build must have the Signal channel compiled in, so \
             `simard signal --help` must not mention the feature-off stub; got:\n{rendered}"
        );
    }

    #[test]
    fn signal_run_reaches_real_config_path_not_feature_stub() {
        let state = TempStateRoot::new();
        let output = simard(state.path())
            .args(["signal", "run"])
            .output()
            .expect("`simard signal run` should spawn");
        let rendered = rendered(&output);

        assert!(
            !output.status.success(),
            "`simard signal run` must fail when no `[signal]` config is present; \
             got success.\n{rendered}"
        );
        assert!(
            !rendered.contains(NOT_COMPILED),
            "REGRESSION (issue #2576): the default build is MISSING the Signal \
             channel — `simard signal run` fell through to the feature-off stub. \
             A plain `cargo build` must compile the `signal` feature in.\n{rendered}"
        );
        assert!(
            rendered.contains("missing required configuration 'signal."),
            "in a default build with no `[signal]` config, `simard signal run` \
             must reach the real config loader and fail with its \
             missing-config error; got:\n{rendered}"
        );
    }
}
