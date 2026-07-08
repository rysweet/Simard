//! The one shared brick behind the ten reflective threads (issue #5).
//!
//! `RecipeInvoker` is a synchronous "run one recipe, get its strict-JSON
//! stdout" seam: given a recipe name and context variables it resolves + spawns
//! `recipe-runner-rs`, reads stdout, and returns a **classified** result
//! ([`InvokeResult`]). Every recipe-backed thread depends on this one trait so
//! every thread's unit test runs offline through a fake. Alongside the trait the
//! brick exports the three small security helpers every rail applies before a
//! durable write ([`sanitize_value`], [`fence_untrusted`], [`secret_scrub`]),
//! the concept-key validator ([`validate_concept_key`]), and the double
//! env-gate predicate ([`env_gate_open`]).
//!
//! **Status (issue #5, TDD):** the type surface, the trait, and the offline
//! classification helpers are the stable studs; the security-bearing bodies
//! ([`sanitize_value`], [`fence_untrusted`], [`secret_scrub`],
//! [`validate_concept_key`], [`env_gate_open`], and
//! [`RecipeRunnerInvoker::invoke`]) are `todo!()` stubs pinned RED by the tests
//! in `tests_catalog` until the implementation step fills them in.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use serde_json::Value;

use super::thread::ThreadOutcome;

/// The master env gate that must be truthy for *any* cognitive thread to run.
pub const MASTER_GATE_ENV: &str = "SIMARD_COGNITIVE_THREADS_ENABLED";

/// Hard cap on the parsed stdout the invoker will accept from a recipe, so a
/// runaway recipe cannot exhaust memory or flood a durable sink (SR-11).
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Maximum length of an LLM-derived concept key (SR-7). Longer keys are
/// rejected rather than truncated so a partial key can never collide.
pub const MAX_CONCEPT_KEY_LEN: usize = 128;

/// Synchronous "run one recipe, get its strict-JSON stdout" seam.
pub trait RecipeInvoker: Send {
    /// Resolve + spawn `<recipe_name>.yaml`, pass each `(k, v)` as a **distinct**
    /// `-c k=v` argv pair (no shell), read stdout under [`MAX_OUTPUT_BYTES`], and
    /// return the classified result. NEVER silently degrades: infra and semantic
    /// misses are distinct, both non-success.
    fn invoke(&self, recipe_name: &str, ctx_vars: &[(&str, String)]) -> InvokeResult;
}

/// The three-way classification of a recipe run — the crux of the
/// no-silent-degradation contract (invariant I4 / SR-9).
#[derive(Clone, Debug)]
pub enum InvokeResult {
    /// exit 0, non-empty stdout, parsed as strict JSON within the size cap.
    Json(Value),
    /// exit 0, non-empty stdout, but unparseable / typeless / oversized envelope.
    SemanticMiss {
        /// The raw (bounded) stdout, for diagnostics only — never written durably.
        raw: String,
    },
    /// spawn error | non-zero exit | empty stdout.
    InfraFailure {
        /// Human-readable failure detail, for diagnostics only.
        detail: String,
    },
}

impl InvokeResult {
    /// `true` only for [`InvokeResult::Json`] — the single case a rail may write.
    pub fn is_success(&self) -> bool {
        matches!(self, InvokeResult::Json(_))
    }

    /// Borrow the parsed JSON, if this was a successful `Json` result.
    pub fn json(&self) -> Option<&Value> {
        match self {
            InvokeResult::Json(v) => Some(v),
            _ => None,
        }
    }

    /// Map a non-success result to a [`ThreadOutcome::failed`] with a stable
    /// summary so every rail surfaces the same asymmetry: both `SemanticMiss`
    /// and `InfraFailure` fail the tick and perform **zero** writes.
    pub fn into_failed_outcome(
        self,
        recipe_name: &str,
        duration: std::time::Duration,
    ) -> ThreadOutcome {
        match self {
            InvokeResult::Json(_) => ThreadOutcome::ok(format!("{recipe_name}: ok"), duration),
            InvokeResult::SemanticMiss { .. } => {
                ThreadOutcome::failed(format!("{recipe_name}: semantic miss"), duration)
            }
            InvokeResult::InfraFailure { detail } => {
                ThreadOutcome::failed(format!("{recipe_name}: infra failure: {detail}"), duration)
            }
        }
    }
}

/// Strip newlines, carriage returns, NUL, and other control characters from a
/// value before it can reach an argv pair or the prompt context (SR-7, SR-8).
///
/// Contract (pinned by tests, `todo!()` until implemented):
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
        .replace("<<END_UNTRUSTED>>", "<<END_UNTRUSTED_>>")
        .replace("<<UNTRUSTED_MEMORY>>", "<<UNTRUSTED_MEMORY_>>");
    format!("<<UNTRUSTED_MEMORY>>\n{neutralized}\n<<END_UNTRUSTED>>")
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
/// Rejection rules (pinned by tests, `todo!()` until implemented):
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

/// The double env gate as a **pure** predicate (SR-12 / S8): a thread is enabled
/// iff BOTH the master gate and its per-thread gate are truthy. Env gates are
/// rollout controls, not an authorization boundary — see
/// `docs/concepts/salience-and-decide.md`.
///
/// Truthy values are `1`, `true`, `TRUE`, `yes`, `on` (leading/trailing
/// whitespace ignored). Any other value — including `None` — is false.
pub fn env_gate_open(_master: Option<&str>, _thread: Option<&str>) -> bool {
    fn truthy(v: Option<&str>) -> bool {
        v.map(|s| matches!(s.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false)
    }
    truthy(_master) && truthy(_thread)
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

/// The one shared invoke step every recipe-backed rail runs: invoke `recipe`
/// with `ctx_vars` and return the parsed strict-JSON envelope, or the mapped
/// [`ThreadOutcome::failed`] when the run was not a clean `Json` success.
///
/// This is the no-silent-degradation crux (I4 / SR-9): **both** a
/// [`InvokeResult::SemanticMiss`] and an [`InvokeResult::InfraFailure`] short-
/// circuit to a failed tick with **zero** durable writes — the rail simply
/// `?`-style returns the `Err`. Only `Json` reaches the write phase.
pub fn invoke_for_envelope(
    invoker: &dyn RecipeInvoker,
    recipe: &str,
    ctx_vars: &[(&str, String)],
    started: Instant,
) -> Result<Value, ThreadOutcome> {
    match invoker.invoke(recipe, ctx_vars) {
        InvokeResult::Json(v) => Ok(v),
        other => Err(other.into_failed_outcome(recipe, started.elapsed())),
    }
}

/// Propose at most one goal onto the shared board through the single capacity-
/// checked path (`goal_board_store::mutate`), preserving the global
/// `MAX_ACTIVE_GOALS` cap across every thread proposer. Returns `true` iff a new
/// goal was actually appended.
///
/// A thread-proposed goal is **enforcement-equivalent** to an operator goal: no
/// privileged path, deduplicated by id, and blockable by the overseer exactly
/// like any other (invariant S3). Best-effort — a locked/unwritable board
/// never fails the calling tick.
pub fn propose_goal_if_capacity(
    state_root: &Path,
    id: &str,
    description: &str,
    priority: u32,
) -> bool {
    use crate::goal_curation::ActiveGoal;
    crate::goal_board_store::mutate(state_root, |state| {
        if state.board.active_slots_remaining() > 0
            && !state.board.active.iter().any(|g| g.id == id)
        {
            state
                .board
                .active
                .push(ActiveGoal::new(id, description, priority));
            true
        } else {
            false
        }
    })
    .unwrap_or(false)
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
    fn invoke(&self, _recipe_name: &str, _ctx_vars: &[(&str, String)]) -> InvokeResult {
        // 1. Sanitize every context value so no newline/NUL can smuggle a second
        //    `-c` pair or a fresh prompt instruction (SR-7/SR-8).
        let sanitized: Vec<(String, String)> = _ctx_vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), sanitize_value(v)))
            .collect();

        // 2. Resolve hot-vs-in-tree (SR-4).
        let (path, _from_hot) = match self.resolve_recipe_path(_recipe_name) {
            Some(p) => p,
            None => {
                return InvokeResult::InfraFailure {
                    detail: format!(
                        "recipe `{_recipe_name}` not found in hot-reload or in-tree paths"
                    ),
                };
            }
        };

        let agent_binary = match crate::runtime_config::RuntimeConfig::load() {
            Ok(cfg) => cfg.llm_provider.agent_binary_value().to_string(),
            Err(e) => {
                return InvokeResult::InfraFailure {
                    detail: format!("runtime config load failed: {e}"),
                };
            }
        };

        // 3. Spawn with argv discipline — each `-c k=v` is a DISTINCT argv pair,
        //    never a shell string (SR-8).
        let mut cmd = Command::new("recipe-runner-rs");
        cmd.arg(path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary);
        for (k, v) in &sanitized {
            cmd.arg("-c").arg(format!("{k}={v}"));
        }

        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                return InvokeResult::InfraFailure {
                    detail: format!("recipe-runner-rs spawn failed: {e}"),
                };
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return InvokeResult::InfraFailure {
                detail: format!(
                    "recipe-runner-rs exited {}: {}",
                    output.status,
                    bound_diag(&stderr)
                ),
            };
        }

        // 4. Size-cap stdout (SR-11) then classify (SR-9): only a clean strict-
        //    JSON object is a success; anything else is a SemanticMiss.
        let mut bytes = output.stdout;
        if bytes.len() > MAX_OUTPUT_BYTES {
            bytes.truncate(MAX_OUTPUT_BYTES);
        }
        let raw = String::from_utf8_lossy(&bytes);
        if raw.trim().is_empty() {
            return InvokeResult::InfraFailure {
                detail: "recipe produced empty stdout".to_string(),
            };
        }
        classify_recipe_stdout(&raw)
    }
}

/// Bound a diagnostic string so an error path can never itself flood a log or a
/// durable sink (these strings are for diagnostics only, never persisted).
fn bound_diag(s: &str) -> String {
    const MAX: usize = 500;
    s.chars().take(MAX).collect::<String>().trim().to_string()
}

/// Classify recipe-runner-rs stdout into the success/miss contract: prefer the
/// runner's JSON envelope's last step output, fall back to a bare object in
/// stdout, and require a strict-JSON **object** to count as `Json`.
fn classify_recipe_stdout(raw: &str) -> InvokeResult {
    let payload =
        parse_step_output(raw).or_else(|| crate::recipe_output::extract::extract_json_payload(raw));
    match payload {
        Some(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => InvokeResult::Json(v),
            _ => InvokeResult::SemanticMiss {
                raw: bound_diag(raw),
            },
        },
        None => InvokeResult::SemanticMiss {
            raw: bound_diag(raw),
        },
    }
}

/// Pull the strict-JSON object out of the recipe-runner-rs `--output-format
/// json` envelope's last step, or `None` if the envelope is absent, reported
/// `success=false`, or has no JSON-object step output.
fn parse_step_output(raw: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct StepResult {
        output: String,
    }
    #[derive(serde::Deserialize)]
    struct RecipeEnvelope {
        success: bool,
        #[serde(default)]
        step_results: Vec<StepResult>,
    }
    let envelope: RecipeEnvelope = serde_json::from_str(raw).ok()?;
    if !envelope.success {
        return None;
    }
    let last = envelope.step_results.last()?;
    crate::recipe_output::extract::extract_json_payload(&last.output)
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
