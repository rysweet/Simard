use std::path::Path;
use std::process::Command;

use super::types::{GateResult, RelaunchConfig, RelaunchGate};
use crate::error::SimardResult;

/// Verify a canary binary against a sequence of gates (does not short-circuit).
///
/// Every FAILING gate emits its own attributed `tracing::warn!` at
/// `target: "self_relaunch::gates"` carrying a concrete `gate` and a sanitized
/// `detail`, so a red canary can never be diagnosed as a bare "red canary"
/// without a named gate behind it (#4422). Passing gates stay quiet.
pub fn verify_canary(
    binary: &Path,
    gates: &[RelaunchGate],
    config: &RelaunchConfig,
) -> SimardResult<Vec<GateResult>> {
    let mut results = Vec::with_capacity(gates.len());

    for &gate in gates {
        let result = run_gate(binary, gate, config);
        if !result.passed {
            tracing::warn!(
                target: "self_relaunch::gates",
                gate = %result.gate,
                detail = %sanitize_gate_detail(&result.detail),
                "canary gate failed",
            );
        }
        results.push(result);
    }

    Ok(results)
}

pub fn all_gates_passed(results: &[GateResult]) -> bool {
    results.iter().all(|r| r.passed)
}

fn run_gate(binary: &Path, gate: RelaunchGate, config: &RelaunchConfig) -> GateResult {
    match gate {
        RelaunchGate::Smoke => run_smoke_gate(binary),
        RelaunchGate::UnitTest => run_unit_test_gate(config),
        RelaunchGate::GymBaseline => run_gym_baseline_gate(binary),
        RelaunchGate::RpcHealth => run_rpc_health_gate(binary, config),
    }
}

fn run_smoke_gate(binary: &Path) -> GateResult {
    match Command::new(binary).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: format!("version: {}", stdout.trim()),
            }
        }
        Ok(output) => GateResult {
            gate: RelaunchGate::Smoke,
            passed: false,
            detail: format!(
                "binary exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::Smoke,
            passed: false,
            detail: format!("failed to execute binary: {e}"),
        },
    }
}

/// Build the argv for the `UnitTest` gate: a hermetic, NON-recursive
/// `cargo test --lib` scoped to an isolated `--target-dir` (#4422).
///
/// Root-cause fix: the gate previously ran the candidate's FULL `cargo test`
/// (all integration/bench targets) inside the canary target-dir while the
/// host-wide self-deploy `BuildLock` was held. That made it environment-
/// sensitive and self-referential — the integration suite can re-enter the
/// self-deploy/canary path and deadlock or flake against the held lock, reddening
/// the canary for reasons unrelated to the candidate's correctness. `--lib`
/// restricts the gate to the crate's library unit tests: a deterministic,
/// non-recursive candidate check that never re-runs the deploy suite. Kept as a
/// pure, testable seam so the invocation shape is asserted without shelling out.
fn unit_test_gate_argv(config: &RelaunchConfig) -> Vec<String> {
    vec![
        "test".to_string(),
        "--lib".to_string(),
        "--manifest-path".to_string(),
        config
            .manifest_dir
            .join("Cargo.toml")
            .to_string_lossy()
            .into_owned(),
        "--target-dir".to_string(),
        config.canary_target_dir.to_string_lossy().into_owned(),
    ]
}

fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    match Command::new("cargo")
        .args(unit_test_gate_argv(config))
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs())
        .output()
    {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: true,
            detail: "all tests passed".to_string(),
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let truncated = truncate_output(&stderr, 200);
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("tests failed (exit {}): {}", output.status, truncated),
            }
        }
        Err(e) => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: format!("cargo test failed to run: {e}"),
        },
    }
}

fn run_gym_baseline_gate(binary: &Path) -> GateResult {
    match Command::new(binary).args(["gym", "list"]).output() {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: true,
            detail: "gym list succeeded".to_string(),
        },
        Ok(output) => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: false,
            detail: format!(
                "gym probe failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: false,
            detail: format!("gym probe failed to run: {e}"),
        },
    }
}

fn run_rpc_health_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let timeout_secs = config.health_timeout.as_secs().to_string();
    match Command::new(binary)
        .args(["probe", "rpc", "--timeout", &timeout_secs])
        .output()
    {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::RpcHealth,
            passed: true,
            detail: "rpc health check passed".to_string(),
        },
        Ok(output) => GateResult {
            gate: RelaunchGate::RpcHealth,
            passed: false,
            detail: format!(
                "rpc health failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::RpcHealth,
            passed: false,
            detail: format!("rpc health probe failed to run: {e}"),
        },
    }
}

fn truncate_output(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.trim().to_string()
    } else {
        // Use char-boundary-safe truncation to avoid panic on multi-byte UTF-8.
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i < max_len)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        format!("{}...", s[..boundary].trim())
    }
}

/// Hard cap on candidate-supplied gate detail admitted into a struct field or a
/// structured tracing/OTel event. Bounds telemetry payload size (#4422).
const MAX_GATE_DETAIL_BYTES: usize = 1024;
/// Explicit, non-silent truncation marker (contains the substring `[truncated]`
/// callers assert on).
const GATE_DETAIL_TRUNCATION_MARKER: &str = "…[truncated]";
/// Placeholder substituted for a redacted credential (mirrors `journal::jargon`).
const GATE_SECRET_PLACEHOLDER: &str = "[redacted secret]";

/// Sanitize untrusted candidate gate stdout/stderr before it enters any struct
/// field or structured tracing/OTel event (#4422). Defense-in-depth, applied in
/// order:
///   1. redact credential-shaped substrings — PEM blocks and GitHub tokens via
///      the shared [`crate::journal::scrub_secrets`], plus AWS access keys and
///      `key=value` secrets here — so a leaked secret never reaches a log/span;
///   2. strip ANSI escape sequences and CR/LF/other control bytes so a candidate
///      can never forge a log line or smuggle terminal control codes;
///   3. hard-cap the result at [`MAX_GATE_DETAIL_BYTES`], marking any truncation
///      explicitly (never a silent drop).
pub fn sanitize_gate_detail(raw: &str) -> String {
    let redacted = redact_kv_secrets(&redact_aws_keys(&crate::journal::scrub_secrets(raw)));
    let cleaned = strip_ansi_and_control(&redacted);
    let trimmed = cleaned.trim();
    if trimmed.len() <= MAX_GATE_DETAIL_BYTES {
        return trimmed.to_string();
    }
    let budget = MAX_GATE_DETAIL_BYTES - GATE_DETAIL_TRUNCATION_MARKER.len();
    let mut end = budget.min(trimmed.len());
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &trimmed[..end], GATE_DETAIL_TRUNCATION_MARKER)
}

/// Strip ANSI escape sequences and collapse/drop control bytes so candidate
/// output cannot forge log lines (CR/LF) or emit terminal control codes.
fn strip_ansi_and_control(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI escape: ESC '[' … final byte in 0x40..=0x7E. Consume the whole
            // sequence; a lone ESC is simply dropped.
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7e}').contains(&nc) {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\r' || c == '\n' || c == '\t' {
            out.push(' ');
            continue;
        }
        if c.is_control() {
            continue;
        }
        out.push(c);
    }
    out
}

/// Redact AWS access-key-shaped tokens (`AKIA` + a run of ≥16 upper/digit chars).
fn redact_aws_keys(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if let Some(body) = rest.strip_prefix("AKIA") {
            let body_len = body
                .bytes()
                .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                .count();
            if body_len >= 16 {
                out.push_str(GATE_SECRET_PLACEHOLDER);
                i += "AKIA".len() + body_len;
                continue;
            }
        }
        let ch = rest.chars().next().expect("non-empty remainder");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Credential-introducing keys for `key=value` / `key: value` secret redaction.
const SECRET_KV_KEYS: &[&str] = &[
    "authorization",
    "client_secret",
    "access_key",
    "secret_key",
    "private_key",
    "api_key",
    "apikey",
    "password",
    "passwd",
    "secret",
    "token",
    "pwd",
];

/// Redact the value of a `key=value` / `key: value` credential assignment,
/// keeping the key so the log still reads sensibly.
fn redact_kv_secrets(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'scan: while i < s.len() {
        for key in SECRET_KV_KEYS {
            if !lower[i..].starts_with(key) {
                continue;
            }
            let boundary_before = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            if !boundary_before {
                continue;
            }
            let after = &s[i + key.len()..];
            let sep_ws = after.bytes().take_while(|b| *b == b' ').count();
            let after_ws = &after[sep_ws..];
            let sep = after_ws.as_bytes().first().copied();
            if sep != Some(b'=') && sep != Some(b':') {
                continue;
            }
            let val_area = &after_ws[1..];
            let vlead = val_area
                .bytes()
                .take_while(|b| *b == b' ' || *b == b'"' || *b == b'\'')
                .count();
            let val = &val_area[vlead..];
            let vlen = val
                .bytes()
                .take_while(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b'+')
                })
                .count();
            if vlen >= 8 {
                out.push_str(&s[i..i + key.len()]);
                out.push('=');
                out.push_str(GATE_SECRET_PLACEHOLDER);
                i += key.len() + sep_ws + 1 + vlead + vlen;
                continue 'scan;
            }
        }
        let ch = s[i..].chars().next().expect("non-empty remainder");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_gate_handles_missing_binary() {
        let result = run_smoke_gate(Path::new("/tmp/no-such-binary-48291"));
        assert!(!result.passed);
    }

    // --- truncate_output ---

    #[test]
    fn truncate_output_short_string_unchanged() {
        let result = truncate_output("hello world", 100);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn truncate_output_exact_length() {
        let input = "abcde";
        let result = truncate_output(input, 5);
        assert_eq!(result, "abcde");
    }

    #[test]
    fn truncate_output_over_limit_appends_ellipsis() {
        let input = "abcdefghij";
        let result = truncate_output(input, 5);
        assert!(
            result.ends_with("..."),
            "should end with ellipsis: {result}"
        );
        assert!(result.len() <= 8, "should be truncated: {result}");
    }

    #[test]
    fn truncate_output_trims_whitespace() {
        let result = truncate_output("  hello  ", 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_output_empty_string() {
        let result = truncate_output("", 100);
        assert_eq!(result, "");
    }

    #[test]
    fn truncate_output_multibyte_utf8_safe() {
        let input = "héllo wörld café";
        let result = truncate_output(input, 8);
        assert!(
            result.ends_with("..."),
            "should end with ellipsis: {result}"
        );
        // Must not panic on multi-byte boundary
    }

    #[test]
    fn truncate_output_zero_max_len() {
        let result = truncate_output("hello", 0);
        assert_eq!(result, "...");
    }

    // --- all_gates_passed ---

    #[test]
    fn all_gates_passed_empty_is_true() {
        assert!(all_gates_passed(&[]));
    }

    #[test]
    fn all_gates_passed_all_true() {
        let results = vec![
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: "ok".to_string(),
            },
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: true,
                detail: "ok".to_string(),
            },
        ];
        assert!(all_gates_passed(&results));
    }

    #[test]
    fn all_gates_passed_one_false() {
        let results = vec![
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: "ok".to_string(),
            },
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: "fail".to_string(),
            },
            GateResult {
                gate: RelaunchGate::GymBaseline,
                passed: true,
                detail: "ok".to_string(),
            },
        ];
        assert!(!all_gates_passed(&results));
    }

    // --- verify_canary ---

    #[test]
    fn verify_canary_with_missing_binary() {
        let config = RelaunchConfig::default();
        let results = verify_canary(
            Path::new("/no-such-binary-99999"),
            &[RelaunchGate::Smoke],
            &config,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            !results[0].passed,
            "smoke gate should fail for missing binary"
        );
    }

    #[test]
    fn verify_canary_runs_all_gates_without_short_circuit() {
        // Use a curated gate list (excludes RelaunchGate::UnitTest, which
        // would recursively invoke `cargo test` and run for 30+ minutes
        // when this test itself is executed under `cargo test`).
        let config = RelaunchConfig::default();
        let gates = [
            RelaunchGate::Smoke,
            RelaunchGate::GymBaseline,
            RelaunchGate::RpcHealth,
        ];
        let results = verify_canary(Path::new("/no-such-binary-99999"), &gates, &config).unwrap();
        assert_eq!(
            results.len(),
            3,
            "should run all 3 selected gates even if first fails"
        );
        assert!(
            results.iter().all(|r| !r.passed),
            "all gates should fail for missing binary"
        );
    }

    #[test]
    fn verify_canary_empty_gates() {
        let config = RelaunchConfig::default();
        let results = verify_canary(Path::new("/no-such-binary"), &[], &config).unwrap();
        assert!(results.is_empty());
    }
}

// ───────────────── structured per-gate attribution (#4422) ──────────────────
//
// TDD contract for the gate-level half of the fix:
//   * Every FAILING gate emits its own attributed `tracing::warn!` at
//     `target: "self_relaunch::gates"` carrying a concrete `gate` field and a
//     `detail` — so a red canary can never be diagnosed as a bare "red canary"
//     without a named gate behind it.
//   * The `UnitTest` gate is scoped to `cargo test --lib`: a NON-recursive,
//     hermetic candidate check that does not re-enter the canary path or the
//     host-wide self-deploy BuildLock. `unit_test_gate_argv` is the pure,
//     testable seam that constructs that invocation.
//
// Written test-first; FAILS until the implementation lands.
#[cfg(test)]
mod attribution_tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::{Layer, Registry};

    #[derive(Clone, Debug)]
    struct Captured {
        target: String,
        level: String,
        fields: BTreeMap<String, String>,
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<Captured>>>);

    struct Grab<'a>(&'a mut BTreeMap<String, String>);
    impl Visit for Grab<'_> {
        fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
            self.0.insert(f.name().to_string(), format!("{v:?}"));
        }
        fn record_str(&mut self, f: &Field, v: &str) {
            self.0.insert(f.name().to_string(), v.to_string());
        }
        fn record_u64(&mut self, f: &Field, v: u64) {
            self.0.insert(f.name().to_string(), v.to_string());
        }
        fn record_i64(&mut self, f: &Field, v: i64) {
            self.0.insert(f.name().to_string(), v.to_string());
        }
        fn record_bool(&mut self, f: &Field, v: bool) {
            self.0.insert(f.name().to_string(), v.to_string());
        }
    }

    impl<S: tracing::Subscriber> Layer<S> for Sink {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut fields = BTreeMap::new();
            event.record(&mut Grab(&mut fields));
            self.0.lock().unwrap().push(Captured {
                target: event.metadata().target().to_string(),
                level: event.metadata().level().to_string(),
                fields,
            });
        }
    }

    #[test]
    fn failing_gates_emit_structured_per_gate_warn() {
        let sink = Sink::default();
        let subscriber = Registry::default().with(sink.clone());
        let config = RelaunchConfig::default();
        tracing::subscriber::with_default(subscriber, || {
            // Smoke + GymBaseline against a missing binary both fail
            // deterministically (no cargo/recursion needed).
            let _ = verify_canary(
                Path::new("/no-such-binary-attr-4422"),
                &[RelaunchGate::Smoke, RelaunchGate::GymBaseline],
                &config,
            )
            .unwrap();
        });

        let events = sink.0.lock().unwrap().clone();
        let gate_warns: Vec<&Captured> = events
            .iter()
            .filter(|e| e.target == "self_relaunch::gates" && e.level == "WARN")
            .collect();

        assert!(
            gate_warns.len() >= 2,
            "each failing gate must emit its own attributed WARN at \
             target=\"self_relaunch::gates\", got {}: {events:?}",
            gate_warns.len()
        );
        assert!(
            gate_warns
                .iter()
                .all(|e| e.fields.contains_key("gate") && e.fields.contains_key("detail")),
            "every gate WARN must carry both a concrete `gate` and a `detail` field: {gate_warns:?}"
        );
        let gates_seen: Vec<String> = gate_warns
            .iter()
            .filter_map(|e| e.fields.get("gate").cloned())
            .collect();
        assert!(
            gates_seen
                .iter()
                .any(|g| g.to_lowercase().contains("smoke")),
            "the smoke gate failure must be attributed by name: {gates_seen:?}"
        );
        assert!(
            gates_seen.iter().any(|g| g.to_lowercase().contains("gym")),
            "the gym-baseline gate failure must be attributed by name: {gates_seen:?}"
        );
    }

    #[test]
    fn passing_gates_emit_no_warn() {
        // A gate that PASSES must not spam a WARN — attribution fires only on red.
        // The smoke gate runs `<binary> --version`; the Rust libtest harness that
        // hosts this test rejects `--version`, so drive the gate with `cargo`
        // (`env!("CARGO")`, always present under `cargo test`) whose `--version`
        // exits 0 — a deterministic, hermetic green gate.
        let sink = Sink::default();
        let subscriber = Registry::default().with(sink.clone());
        let config = RelaunchConfig::default();
        let pass_bin = std::path::PathBuf::from(env!("CARGO"));
        tracing::subscriber::with_default(subscriber, || {
            let results = verify_canary(&pass_bin, &[RelaunchGate::Smoke], &config).unwrap();
            assert!(
                results[0].passed,
                "smoke against `cargo --version` should pass: {:?}",
                results[0].detail
            );
        });
        let events = sink.0.lock().unwrap().clone();
        assert!(
            events
                .iter()
                .filter(|e| e.target == "self_relaunch::gates" && e.level == "WARN")
                .all(|e| e
                    .fields
                    .get("gate")
                    .map(|g| !g.to_lowercase().contains("smoke"))
                    .unwrap_or(true)),
            "a passing smoke gate must not emit a failure WARN: {events:?}"
        );
    }

    #[test]
    fn unit_test_gate_is_scoped_to_lib_and_hermetic() {
        let config = RelaunchConfig {
            manifest_dir: std::path::PathBuf::from("/repo"),
            canary_target_dir: std::path::PathBuf::from("/canary-target"),
            ..Default::default()
        };
        let argv = unit_test_gate_argv(&config);

        assert!(
            argv.iter().any(|a| a.as_str() == "test"),
            "the UnitTest gate must invoke `cargo test`: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a.as_str() == "--lib"),
            "the UnitTest gate MUST be scoped to `--lib` — a non-recursive, \
             hermetic candidate check that never re-runs the full suite (which \
             would re-enter the canary path under the held BuildLock): {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a.as_str() == "--target-dir"),
            "the gate must pin an isolated --target-dir so it never fights the \
             host build: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.as_str() == "--all-targets"),
            "must NOT pull in integration/bench targets that re-enter the canary: {argv:?}"
        );
    }
}
