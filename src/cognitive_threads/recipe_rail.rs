//! The one shared brick behind the ten reflective threads (issue #5).
//!
//! `RecipeInvoker` is a synchronous "run one recipe, get a success/failure
//! verdict" seam: given a recipe name and context variables it resolves + spawns
//! `recipe-runner-rs` and returns whether it ran ([`InvokeResult`]) — it parses
//! NOTHING from stdout, because each recipe performs every durable effect itself
//! via its own `simard …` tool calls (like `distill-episodes.yaml`). Every
//! recipe-backed thread depends on this one trait so
//! every thread's unit test runs offline through a fake. Alongside the trait the
//! brick exports the three small security helpers every rail applies before a
//! durable write ([`sanitize_value`], [`fence_untrusted`], [`secret_scrub`]),
//! the concept-key validator ([`validate_concept_key`]), and the double
//! env-gate predicate ([`env_gate_open`]).
//!
//! **Transport (issue #5):** small scalar context vars (`state_root`,
//! `repo_path`, counts) ride inline on `argv` as `-c k=v`, control-char
//! sanitized so a value can never smuggle a second `-c` pair (SR-7/SR-8). A
//! *fenced untrusted-memory* payload (see [`fence_untrusted`]) is unbounded, so
//! it is delivered out-of-band through a private temp file via [`ContextFile`] —
//! only `-c <key>_path=<abs>` touches `argv` — exactly like the journal /
//! episode-distillation seams. That keeps a large recall byte-for-byte (no
//! truncation, so no silent degradation) while `execve` can never fail with
//! `E2BIG`/"Argument list too long" (issues #2640/#2692).
#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::recipe_context_file::ContextFile;

use super::thread::ThreadOutcome;

/// The master env gate that must be truthy for *any* cognitive thread to run.
pub const MASTER_GATE_ENV: &str = "SIMARD_COGNITIVE_THREADS_ENABLED";

/// Maximum length of an LLM-derived concept key (SR-7). Longer keys are
/// rejected rather than truncated so a partial key can never collide.
pub const MAX_CONCEPT_KEY_LEN: usize = 128;

/// Synchronous "run one recipe, get a success/failure verdict" seam.
///
/// The recipe performs every durable effect ITSELF via its own `simard …` tool
/// calls (`simard memory remember` / `remember-procedure` / `goal add` /
/// `cognition salience-signal`), exactly like `distill-episodes.yaml`. So the
/// invoker parses NOTHING from stdout — it only reports whether the recipe ran.
pub trait RecipeInvoker: Send {
    /// Resolve + spawn `<recipe_name>.yaml`, pass each `(k, v)` as a **distinct**
    /// `-c k=v` argv pair (no shell), and return whether it ran: a clean exit is
    /// [`InvokeResult::Ran`], anything else is [`InvokeResult::Failed`]. NEVER
    /// silently degrades — a failed run is recorded LOUDLY, never squashed into
    /// a "ran, wrote nothing" success.
    fn invoke(&self, recipe_name: &str, ctx_vars: &[(&str, String)]) -> InvokeResult;
}

/// The two-way verdict of a recipe run — the crux of the no-silent-degradation
/// contract (invariant I4 / SR-9). There is deliberately no third "semantic
/// miss" state to swallow: the recipe's tool calls ARE its effect, so stdout is
/// never parsed and a torn envelope can no longer discard the work.
#[derive(Clone, Debug)]
pub enum InvokeResult {
    /// The recipe exited 0. Its own `simard …` tool calls performed every
    /// durable effect; the invoker read nothing back.
    Ran,
    /// spawn error | non-zero exit. Recorded LOUDLY, never a silent success.
    Failed {
        /// Human-readable failure detail, for diagnostics / health only.
        detail: String,
    },
}

impl InvokeResult {
    /// `true` only for [`InvokeResult::Ran`] — a non-zero recipe is a failure,
    /// never a silent success.
    pub fn is_success(&self) -> bool {
        matches!(self, InvokeResult::Ran)
    }

    /// Map the verdict to a [`ThreadOutcome`]: `Ran` => a successful tick,
    /// `Failed` => a failed tick surfaced LOUDLY with its detail. No third state
    /// can quietly turn a failure into a wrote-nothing success.
    pub fn into_outcome(self, recipe_name: &str, duration: std::time::Duration) -> ThreadOutcome {
        match self {
            InvokeResult::Ran => ThreadOutcome::ok(format!("{recipe_name}: ok"), duration),
            InvokeResult::Failed { detail } => {
                ThreadOutcome::failed(format!("{recipe_name}: {detail}"), duration)
            }
        }
    }
}

/// The memory IPC socket a reflective recipe inherits so a bare `simard memory
/// remember` inside it reaches the SAME live store the daemon publishes — the
/// exact seam the episode-distiller uses (issue #2679). Kept identical to
/// [`crate::memory_ipc::socket_path_for`] so the rail and the daemon never
/// disagree on the path.
pub fn memory_socket_path(state_root: &Path) -> PathBuf {
    crate::memory_ipc::socket_path_for(state_root)
}

/// Strip newlines, carriage returns, NUL, and other control characters from a
/// value before it can reach an argv pair or the prompt context (SR-7, SR-8).
///
/// Contract (pinned by tests):
/// - removes `\n`, `\r`, `\0`, and all other C0/C1 control characters;
/// - a value like `foo\n-c evil=1` therefore cannot smuggle a second `-c` pair
///   or a newline into prompt context;
/// - preserves ordinary printable content otherwise.
pub fn sanitize_value(raw: &str) -> String {
    // `char::is_control` covers the C0 range (incl. `\n`, `\r`, NUL) and the
    // C1/DEL range, so a value can carry no line break or NUL to smuggle a
    // second `-c` pair or a fresh prompt instruction.
    raw.chars().filter(|c| !c.is_control()).collect()
}

/// Opening delimiter of the untrusted-memory data region emitted by
/// [`fence_untrusted`]. A context value beginning with this marker is treated as
/// an unbounded memory payload and transported out-of-band via [`ContextFile`].
pub const UNTRUSTED_OPEN: &str = "<<UNTRUSTED_MEMORY>>";
/// Closing delimiter of the untrusted-memory data region.
pub const UNTRUSTED_CLOSE: &str = "<<END_UNTRUSTED>>";

/// Wrap memory-sourced text in the recipe's untrusted-data region so a recipe
/// treats it as data, never instructions (SR-2). The returned string is bounded
/// by the region delimiters `<<UNTRUSTED_MEMORY>> … <<END_UNTRUSTED>>` and any
/// attempt to close the region early inside `raw` is neutralized.
pub fn fence_untrusted(raw: &str) -> String {
    // Neutralize any embedded region delimiter so memory text cannot close the
    // fence early and escape into the instruction stream. The break inserts an
    // underscore inside the token so the exact delimiter no longer appears, yet
    // the content stays human-readable.
    let neutralized = raw
        .replace(UNTRUSTED_CLOSE, "<<END_UNTRUSTED_>>")
        .replace(UNTRUSTED_OPEN, "<<UNTRUSTED_MEMORY_>>");
    format!("{UNTRUSTED_OPEN}\n{neutralized}\n{UNTRUSTED_CLOSE}")
}

/// Whether a context value is a fenced untrusted-memory payload (the output of
/// [`fence_untrusted`]). Such a value is unbounded and MUST ride off `argv`
/// through a private temp file ([`build_context_args`]), never inline.
pub fn is_fenced_payload(value: &str) -> bool {
    value.starts_with(UNTRUSTED_OPEN)
}

/// Redact token-shaped substrings before writing to a fact, metric line, or
/// issue body (SR-6). A seeded token in a source episode must never be echoed
/// into a durable sink; `AMPLIHACK_AGENT_BINARY`/env values are never persisted.
pub fn secret_scrub(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut word = String::new();
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !word.is_empty() {
                out.push_str(&scrub_word(&word));
                word.clear();
            }
            out.push(ch);
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push_str(&scrub_word(&word));
    }
    out
}

/// Placeholder written in place of any redacted secret.
const REDACTED: &str = "[REDACTED]";

/// Redact a single whitespace-delimited word. First the labelled case
/// (`key=value` / `key:value` with a secret-ish key) redacts the value; then any
/// credential-shaped RUN *inside* the word (a token prefix or a long high-entropy
/// blob) is redacted individually, so a secret wrapped in quotes, parentheses, or
/// JSON punctuation cannot slip past by defeating a whole-word check (SR-6).
fn scrub_word(word: &str) -> String {
    for delim in ['=', ':'] {
        if let Some(idx) = word.find(delim) {
            let (key, rest) = word.split_at(idx);
            let value = &rest[delim.len_utf8()..];
            if !value.is_empty() && is_secret_key(key) {
                return format!("{key}{delim}{REDACTED}");
            }
        }
    }
    redact_credential_runs(word)
}

/// Redact each maximal run of credential characters within `word` that looks
/// like a secret (token prefix or high-entropy blob), preserving the surrounding
/// punctuation. This is what defeats punctuation-wrapped tokens such as
/// `"sk-…"`, `(ghp_…)`, or `["<blob>","x"]`.
fn redact_credential_runs(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut run = String::new();
    for ch in word.chars() {
        if is_credential_char(ch) {
            run.push(ch);
        } else {
            out.push_str(&scrub_run(&run));
            run.clear();
            out.push(ch);
        }
    }
    out.push_str(&scrub_run(&run));
    out
}

/// Redact `run` iff it looks like a credential; otherwise return it unchanged.
fn scrub_run(run: &str) -> String {
    if run.is_empty() {
        return String::new();
    }
    if has_secret_prefix(run) || looks_high_entropy(run) {
        REDACTED.to_string()
    } else {
        run.to_string()
    }
}

/// Characters that can appear in a base64/hex/token-shaped credential run. A
/// `.` is deliberately excluded so ordinary prose (and URLs) is not merged into
/// one giant run.
fn is_credential_char(ch: char) -> bool {
    matches!(ch, 'a'..='z' | 'A'..='Z' | '0'..='9' | '+' | '/' | '_' | '-' | '=')
}

/// Whether `key` (after dropping non-alphanumerics and lowercasing) names a
/// secret — `token`, `api_key`, `password`, `authorization`, etc.
fn is_secret_key(key: &str) -> bool {
    let norm: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    matches!(
        norm.as_str(),
        "token"
            | "tokens"
            | "secret"
            | "secrets"
            | "key"
            | "apikey"
            | "password"
            | "passwd"
            | "pass"
            | "pwd"
            | "auth"
            | "authorization"
            | "bearer"
            | "accesstoken"
            | "refreshtoken"
            | "sessionkey"
            | "clientsecret"
            | "privatekey"
    )
}

/// Whether a credential-character run begins with a well-known credential prefix
/// (GitHub tokens, OpenAI/Slack/AWS keys). Applied to runs (not whole words) so a
/// wrapping quote/paren cannot defeat the anchor.
fn has_secret_prefix(run: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "ghr_",
        "github_pat_",
        "sk-",
        "xoxb-",
        "xoxp-",
        "AKIA",
    ];
    PREFIXES.iter().any(|p| run.starts_with(p))
}

/// Whether `run` is a long, mixed-class, base64/hex-shaped blob — the shape of
/// an opaque credential even without a labelled key. Deliberately conservative
/// (>= 32 chars, at least one letter and one digit) so prose is never redacted.
fn looks_high_entropy(run: &str) -> bool {
    if run.len() < 32 {
        return false;
    }
    let mut has_alpha = false;
    let mut has_digit = false;
    for c in run.chars() {
        match c {
            'a'..='z' | 'A'..='Z' => has_alpha = true,
            '0'..='9' => has_digit = true,
            _ => {}
        }
    }
    has_alpha && has_digit
}

/// Validate + normalize an LLM-derived concept key (SR-7). Returns `Some(key)`
/// when the key is safe, or `None` when it must be rejected.
///
/// Rejection rules (pinned by tests):
/// - reject anything containing a path separator (`/` or `\`) or a `..` segment;
/// - reject keys longer than [`MAX_CONCEPT_KEY_LEN`] (no truncation — reject);
/// - strip control characters; reject if empty after stripping.
pub fn validate_concept_key(_raw: &str) -> Option<String> {
    let stripped: String = _raw.chars().filter(|c| !c.is_control()).collect();
    let key = stripped.trim();
    if key.is_empty() || key.len() > MAX_CONCEPT_KEY_LEN {
        return None;
    }
    if key.contains('/') || key.contains('\\') || key.contains("..") {
        return None;
    }
    Some(key.to_string())
}

/// The double env gate as a **pure** predicate (SR-12 / S8): default-ON opt-out.
/// A thread is enabled UNLESS its master gate or its per-thread gate is set to an
/// explicit falsy token — mirroring the existing `SIMARD_CREATIVE_IDEAS_ENABLED`
/// default-ON pattern (issue #4845, requirement A4 — THE CRUX). Env gates are
/// rollout controls, not an authorization boundary — see
/// `docs/concepts/salience-and-decide.md`.
///
/// Falsy tokens (case-insensitive, leading/trailing whitespace ignored) are
/// `0`, `false`, `no`, `off`. `None` (unset), empty, and any other value are NOT
/// an opt-out and leave the gate OPEN. The predicate fails **closed** on an
/// explicit falsy value on either gate (security T-S1): an operator opt-out is
/// always honoured.
pub fn env_gate_open(_master: Option<&str>, _thread: Option<&str>) -> bool {
    fn falsy(v: Option<&str>) -> bool {
        v.map(|s| {
            matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(false)
    }
    !falsy(_master) && !falsy(_thread)
}

/// Read the double env gate for a thread from the process environment using
/// [`env_gate_open`]. `thread_gate_env` is the per-thread var name
/// (`SIMARD_THREAD_<NAME>_ENABLED`).
pub fn thread_enabled(thread_gate_env: &str) -> bool {
    env_gate_open(
        std::env::var(MASTER_GATE_ENV).ok().as_deref(),
        std::env::var(thread_gate_env).ok().as_deref(),
    )
}

/// Production [`RecipeInvoker`] — a faithful extraction of the existing
/// progress-checker subprocess path plus the security contract (argv discipline,
/// hot-vs-in-tree path resolution + logging, size cap, classification).
pub struct RecipeRunnerInvoker {
    repo_root: PathBuf,
    state_root: PathBuf,
}

impl RecipeRunnerInvoker {
    /// Build a production invoker rooted at the repo + state roots.
    pub fn new(repo_root: impl Into<PathBuf>, state_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            state_root: state_root.into(),
        }
    }

    /// Resolve `<name>.yaml`: prefer the hot-reload dir under the state root,
    /// then the in-tree recipes dir; log which was used; reject a group/world-
    /// writable hot dir with a fallback + warning (SR-4). Returns the resolved
    /// path and whether it came from the hot dir.
    pub fn resolve_recipe_path(&self, _name: &str) -> Option<(PathBuf, bool)> {
        let filename = format!("{_name}.yaml");
        let hot_dir = self.state_root.join("prompt_assets/simard/recipes");
        let hot = hot_dir.join(&filename);
        if hot.is_file() {
            if is_insecurely_writable(&hot_dir) {
                tracing::warn!(
                    target: "simard::cognitive_threads",
                    recipe = _name,
                    dir = %hot_dir.display(),
                    "hot-reload recipe dir is group/world-writable; ignoring it and \
                     falling back to the in-tree recipe (SR-4)"
                );
            } else {
                tracing::debug!(
                    target: "simard::cognitive_threads",
                    recipe = _name,
                    source = "hot",
                    path = %hot.display(),
                    "resolved recipe from hot-reload dir"
                );
                return Some((hot, true));
            }
        }
        let in_tree = self
            .repo_root
            .join("prompt_assets/simard/recipes")
            .join(&filename);
        if in_tree.is_file() {
            tracing::debug!(
                target: "simard::cognitive_threads",
                recipe = _name,
                source = "in-tree",
                path = %in_tree.display(),
                "resolved recipe from in-tree dir"
            );
            return Some((in_tree, false));
        }
        None
    }
}

impl RecipeInvoker for RecipeRunnerInvoker {
    fn invoke(&self, recipe_name: &str, ctx_vars: &[(&str, String)]) -> InvokeResult {
        // 1. Resolve hot-vs-in-tree (SR-4).
        let (path, _from_hot) = match self.resolve_recipe_path(recipe_name) {
            Some(p) => p,
            None => {
                return InvokeResult::Failed {
                    detail: format!(
                        "recipe `{recipe_name}` not found in hot-reload or in-tree paths"
                    ),
                };
            }
        };

        let agent_binary = match crate::runtime_config::RuntimeConfig::load() {
            Ok(cfg) => cfg.llm_provider.agent_binary_value().to_string(),
            Err(e) => {
                return InvokeResult::Failed {
                    detail: format!("runtime config load failed: {e}"),
                };
            }
        };

        // 2. Build the `-c` argv values. A fenced untrusted-memory payload is
        //    unbounded, so it is written to a private temp file and only its
        //    `<key>_path=<abs>` rides on argv (E2BIG-safe, #2640/#2692); a small
        //    scalar is passed inline, control-char sanitized so it cannot smuggle
        //    a second `-c` pair (SR-7/SR-8). The `_guards` MUST outlive the spawn
        //    so the payload files exist while the recipe reads them.
        let (arg_values, _guards) = match build_context_args(recipe_name, ctx_vars) {
            Ok(pair) => pair,
            Err(e) => {
                return InvokeResult::Failed {
                    detail: format!("recipe context-file write failed: {e}"),
                };
            }
        };

        // 3. Spawn with argv discipline — each `-c …` is a DISTINCT argv pair,
        //    never a shell string (SR-8). Export SIMARD_MEMORY_SOCKET so a bare
        //    `simard memory remember` inside the recipe reaches the SAME live
        //    store the daemon publishes, through the gated write boundary — the
        //    exact episode-distiller seam (issue #2679). The recipe's own tool
        //    calls ARE its effect; this invoker reads NOTHING back from stdout.
        let mut cmd = Command::new("recipe-runner-rs");
        cmd.arg(path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .env("SIMARD_MEMORY_SOCKET", memory_socket_path(&self.state_root));
        for value in &arg_values {
            cmd.arg("-c").arg(value);
        }

        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                return InvokeResult::Failed {
                    detail: format!("recipe-runner-rs spawn failed: {e}"),
                };
            }
        };

        // 4. Success is EXIT STATUS only (SR-9). A clean exit is `Ran` — the
        //    recipe's `simard …` tool calls already performed every durable
        //    effect. A non-zero exit is a LOUD `Failed` carrying bounded stderr
        //    for health/logging; nothing is ever parsed from stdout, so a torn
        //    envelope can no longer discard the work (#2658/#2679).
        if output.status.success() {
            InvokeResult::Ran
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            InvokeResult::Failed {
                detail: format!(
                    "recipe-runner-rs exited {}: {}",
                    output.status,
                    bound_diag(&stderr)
                ),
            }
        }
    }
}

/// Build the ordered `-c` argv values for a recipe call, transporting each
/// context var by the rule tied to the security fence:
///
/// - a **fenced untrusted-memory** payload ([`is_fenced_payload`]) is unbounded,
///   so it is written verbatim to a private per-invocation `0700` temp file via
///   [`ContextFile`] and only `<key>_path=<abs>` rides on argv — the payload
///   never touches argv, so `execve` can never fail with `E2BIG`/"Argument list
///   too long" (issues #2640/#2692), and the recipe reads `{{<key>_path}}` to
///   see the full recall byte-for-byte (no truncation → no silent degradation);
/// - a **small scalar** (paths, counts) is passed inline as
///   `<key>=<sanitize_value>`, control-char stripped so it can never smuggle a
///   second `-c` pair or a newline into prompt context (SR-7/SR-8).
///
/// Returns the argv values plus the [`ContextFile`] guards, which the caller
/// MUST keep alive until `recipe-runner-rs` has finished (each guard unlinks its
/// file and directory on drop).
fn build_context_args(
    base_type: &str,
    ctx_vars: &[(&str, String)],
) -> io::Result<(Vec<String>, Vec<ContextFile>)> {
    let mut arg_values = Vec::with_capacity(ctx_vars.len());
    let mut guards = Vec::new();
    for (key, value) in ctx_vars {
        if is_fenced_payload(value) {
            let cf = ContextFile::write(base_type, key, value)?;
            arg_values.push(cf.arg_value());
            guards.push(cf);
        } else {
            arg_values.push(format!("{key}={}", sanitize_value(value)));
        }
    }
    Ok((arg_values, guards))
}

/// Bound a diagnostic string so an error path can never itself flood a log or a
/// durable sink (these strings are for diagnostics only, never persisted).
fn bound_diag(s: &str) -> String {
    const MAX: usize = 500;
    s.chars().take(MAX).collect::<String>().trim().to_string()
}

/// Whether `dir` is group- or world-writable (SR-4). On non-unix targets this
/// is conservatively `false` (the daemon only runs on Linux).
#[cfg(unix)]
fn is_insecurely_writable(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(dir)
        .map(|m| m.permissions().mode() & 0o022 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_insecurely_writable(_dir: &Path) -> bool {
    false
}

#[cfg(test)]
mod secret_scrub_tests {
    use super::secret_scrub;

    #[test]
    fn redacts_punctuation_wrapped_credentials() {
        // SR-6: a token must be redacted even when quotes / parens / brackets
        // keep it from being a bare whitespace-delimited word.
        for (input, leaked) in [
            (
                "the operator used key \"sk-proj-AbCdEf0123456789GhIjKlMnOpQrStUv\"",
                "sk-proj-AbCdEf0123456789GhIjKlMnOpQrStUv",
            ),
            ("see (ghp_AbCdEf0123456789GhIjKlMnOpQr) for auth", "ghp_"),
            (
                "array: [\"AbCdEf0123456789GhIjKlMnOpQrStUvWx012\",\"x\"]",
                "AbCdEf0123456789GhIjKlMnOpQrStUvWx012",
            ),
            (
                "value=\"AbCdEf0123456789GhIjKlMnOpQrStUvWx012\"",
                "AbCdEf0123456789GhIjKlMnOpQrStUvWx012",
            ),
        ] {
            let out = secret_scrub(input);
            assert!(
                !out.contains(leaked),
                "wrapped credential leaked: input={input:?} out={out:?}"
            );
            assert!(
                out.contains("[REDACTED]"),
                "redaction marker present: {out:?}"
            );
        }
    }

    #[test]
    fn keeps_ordinary_prose_and_urls() {
        for input in [
            "over_optimism repeated triage loop",
            "disk crisis dominates salience",
            "see https://example.com/docs for details",
            "the bearer of good news arrived",
        ] {
            let out = secret_scrub(input);
            assert_eq!(out, input, "prose must pass through unchanged: {input:?}");
        }
    }
}

#[cfg(test)]
mod context_transport_tests {
    use super::{build_context_args, fence_untrusted, is_fenced_payload};

    #[test]
    fn fenced_payload_is_recognized_scalar_is_not() {
        assert!(is_fenced_payload(&fence_untrusted("recalled fact")));
        assert!(!is_fenced_payload("/home/user/.simard"));
        assert!(!is_fenced_payload("42"));
        assert!(!is_fenced_payload(""));
    }

    #[test]
    fn scalar_vars_ride_inline_and_are_sanitized() {
        // A scalar is passed inline as `key=value`; a smuggled newline + `-c`
        // pair is stripped so it stays exactly one argv value (SR-7/SR-8).
        let vars = vec![
            ("state_root", "/home/user/.simard".to_string()),
            ("repo_path", "foo\n-c evil=1".to_string()),
        ];
        let (args, guards) = build_context_args("unit-test", &vars).expect("build args");
        assert!(guards.is_empty(), "no temp files for scalars");
        assert_eq!(args[0], "state_root=/home/user/.simard");
        assert_eq!(
            args[1], "repo_path=foo-c evil=1",
            "the newline is stripped, so `-c evil=1` stays inert text inside this \
             one argv value and can never become a second `-c` pair"
        );
        assert!(!args[1].contains('\n'), "no newline survives on argv");
    }

    #[test]
    fn fenced_payload_rides_off_argv_via_a_file_verbatim() {
        // A fenced (unbounded) payload must NOT ride on argv: only
        // `<key>_path=<abs>` does, and the file holds the payload byte-for-byte
        // (newlines preserved, no truncation) so a large recall can never fail
        // the spawn with E2BIG (#2640/#2692).
        let payload = fence_untrusted("line one\nline two\nline three");
        let vars = vec![("prior_operator_model", payload.clone())];
        let (args, guards) = build_context_args("unit-test", &vars).expect("build args");
        assert_eq!(guards.len(), 1, "one temp file backs the fenced payload");

        let arg = &args[0];
        assert!(
            arg.starts_with("prior_operator_model_path="),
            "argv carries only the file path key: {arg}"
        );
        assert!(
            !arg.contains(&payload),
            "the payload itself never touches argv"
        );

        let abs = arg
            .strip_prefix("prior_operator_model_path=")
            .expect("path value");
        let on_disk = std::fs::read_to_string(abs).expect("read context file");
        assert_eq!(
            on_disk, payload,
            "the recipe reads the full fenced payload byte-for-byte from the file"
        );
        // Guard drop unlinks the file.
        drop(guards);
        assert!(
            !std::path::Path::new(abs).exists(),
            "the per-invocation payload file is removed when the guard drops"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #4845 — TDD contract (Step 7) for the DEFAULT-ON opt-out env gate flip
// (design component C1 / requirement A4 — THE CRUX, security SR-12 / T-S1).
//
// Authored BEFORE the flip, so this module is RED until `env_gate_open` changes
// from the double-AND default-OFF predicate (`truthy(master) && truthy(thread)`)
// to a default-ON opt-out predicate: a thread is enabled UNLESS its master or
// per-thread gate is set to an explicit falsy token. This mirrors the existing
// `SIMARD_CREATIVE_IDEAS_ENABLED` default-ON pattern
// (`creative_ideas::is_falsey` — {0,false,no,off}). The predicate is PURE, so
// these tests pass `Option` args directly and never touch the process env.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod env_gate_default_on_tests {
    use super::env_gate_open;

    // --- The default: NOTHING set ⇒ ENABLED (the whole point of issue #4845). ---
    #[test]
    fn unset_master_and_thread_is_enabled_by_default() {
        assert!(
            env_gate_open(None, None),
            "default-ON opt-out: with neither gate set, a thread must be ENABLED \
             (this is the flip #4845 requires; RED under the old default-OFF gate)"
        );
    }

    // --- Explicit falsy on EITHER gate opts out (fail-closed, security T-S1). ---
    #[test]
    fn explicit_falsy_master_opts_out_the_whole_roster() {
        for falsy in ["0", "false", "FALSE", "no", "off", " 0 ", "Off", "No"] {
            assert!(
                !env_gate_open(Some(falsy), None),
                "master gate set to an explicit falsy value ({falsy:?}) must DISABLE \
                 (operator opt-out honoured; fail-closed)"
            );
        }
    }

    #[test]
    fn explicit_falsy_thread_opts_out_just_that_thread() {
        for falsy in ["0", "false", "FALSE", "no", "off", " off ", "NO"] {
            assert!(
                !env_gate_open(None, Some(falsy)),
                "per-thread gate set to an explicit falsy value ({falsy:?}) must DISABLE \
                 that thread even with the master gate at its default-ON state"
            );
        }
    }

    // --- Truthy values remain enabled (back-compat with the old opt-in). ---
    #[test]
    fn truthy_values_stay_enabled() {
        for truthy in ["1", "true", "TRUE", "yes", "on"] {
            assert!(
                env_gate_open(Some(truthy), Some(truthy)),
                "an explicitly truthy master+thread ({truthy:?}) stays ENABLED"
            );
        }
    }

    // --- Unknown / empty / garbage is NOT an opt-out ⇒ stays enabled. ---
    #[test]
    fn unknown_or_empty_value_defaults_to_enabled() {
        for garbage in ["", "  ", "maybe", "2", "enabled", "disabled", "0x0", "null"] {
            assert!(
                env_gate_open(Some(garbage), None),
                "a non-falsy master value ({garbage:?}) must NOT opt out — default-ON \
                 only honours the explicit falsy token set, everything else is enabled"
            );
            assert!(
                env_gate_open(None, Some(garbage)),
                "a non-falsy thread value ({garbage:?}) must NOT opt out"
            );
        }
    }

    // --- Master falsy dominates a truthy per-thread (master kills the roster). ---
    #[test]
    fn master_falsy_overrides_truthy_thread() {
        assert!(
            !env_gate_open(Some("0"), Some("1")),
            "an explicit master opt-out disables the thread even if its own gate is truthy"
        );
    }

    // --- Truthy master + falsy thread ⇒ that one thread stays off. ---
    #[test]
    fn thread_falsy_overrides_truthy_master() {
        assert!(
            !env_gate_open(Some("1"), Some("0")),
            "a per-thread opt-out disables that thread even under an explicitly enabled master"
        );
    }

    // --- Full truth table, exhaustively pinned (master × thread). ---
    #[test]
    fn full_truth_table() {
        // (master, thread, expected_enabled)
        let cases: &[(Option<&str>, Option<&str>, bool)] = &[
            (None, None, true),                // default-ON
            (Some(""), None, true),            // empty ≠ falsy
            (Some("garbage"), None, true),     // garbage ≠ falsy
            (Some("1"), Some("1"), true),      // both truthy
            (Some("true"), None, true),        // truthy master, unset thread
            (None, Some("on"), true),          // unset master, truthy thread
            (Some("0"), None, false),          // master opt-out
            (None, Some("0"), false),          // thread opt-out
            (Some("off"), Some("1"), false),   // master opt-out wins
            (Some("1"), Some("false"), false), // thread opt-out wins
            (Some("no"), Some("no"), false),   // both opted out
        ];
        for &(m, t, want) in cases {
            assert_eq!(
                env_gate_open(m, t),
                want,
                "env_gate_open(master={m:?}, thread={t:?}) should be {want}"
            );
        }
    }
}
