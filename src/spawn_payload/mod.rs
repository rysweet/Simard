//! The single large-payload spawn facade (issue #2640).
//!
//! The E2BIG ("Argument list too long", `errno 7`) spawn failure has been fixed
//! one launch site at a time — #2660 gave copilot/OODA a stdin prompt channel,
//! #2692/#2700 gave the journal recipe a context-file channel — yet it kept
//! recurring because there was no *single* chokepoint every agent/recipe launch
//! was forced through. This module is that chokepoint. It adds **no** new
//! byte-transport; it composes the two already-shipping primitives under one
//! policy:
//!
//! > a dynamic value whose length can reach [`ARGV_PAYLOAD_MAX_BYTES`] is
//! > delivered out-of-band — copilot **prompts** on stdin (via
//! > [`crate::prompt_delivery`]), recipe **context** on a private file
//! > referenced by `-c <key>_path=<abs>` (via
//! > [`crate::recipe_context_file::ContextFile`]) — and never appears in `argv`
//! > or `envp`.
//!
//! Two transports are required because the two runners disagree on how they
//! accept input: `copilot` reads its prompt from **stdin** when no `-p` is
//! given (its `--` positional would be misparsed as a subcommand, so an inline
//! prompt is never safe), while `recipe-runner-rs` only accepts context on
//! `argv` as `-c KEY=VALUE`, so an unbounded value must ride as a short
//! **file path** instead. One byte-transport cannot serve both; the facade is
//! the one *policy* that dispatches to the correct one.
//!
//! Every pre-exec spawn failure (the E2BIG defect has no child and no
//! `ExitStatus`, only an [`std::io::Error`]) is classified
//! ([`crate::overseer::diagnosis::classify_spawn_failure`]) and recorded into
//! the Overseer failure sink ([`record_spawn_failure`]) — the "diagnose, don't
//! silently swallow" invariant.
//!
//! The wire-level contract is pinned by `tests/spawn_payload_facade.rs` and the
//! per-path transport tests, and the anti-regression guard
//! `tests/e2big_argv_guard.rs` asserts this module keeps existing.

use std::process::Command as StdCommand;

use crate::overseer::diagnosis::classify_spawn_failure;
use crate::overseer::failure_sink;
use crate::prompt_delivery::{
    self, AppliedPromptStd, AppliedPromptTokio, PromptDelivery, PromptDeliveryError,
};
use crate::recipe_context_file::ContextFile;

/// The single policy threshold: a payload of this size or larger is delivered
/// out-of-band (stdin / file) and never inlined into `argv`.
///
/// It equals [`prompt_delivery::INLINE_MAX_BYTES`] so the "small enough to
/// inline" boundary is identical for copilot prompts and recipe context —
/// one number, one policy.
pub const ARGV_PAYLOAD_MAX_BYTES: usize = prompt_delivery::INLINE_MAX_BYTES;

// ---------------------------------------------------------------------------
// Recipe context transport (recipe-runner-rs family)
// ---------------------------------------------------------------------------

/// A resolved `recipe-runner-rs` `-c` argument for one context key.
///
/// A small value stays [`Inline`](RecipeArg::Inline) (`key=value`); an
/// unbounded value is [`Filed`](RecipeArg::Filed) — written to a private temp
/// file so only `key_path=<abs>` rides on `argv`. The [`ContextFile`] guard in
/// the `Filed` arm MUST be held alive until the recipe subprocess has finished
/// reading (its `Drop` unlinks the file), so callers keep the `RecipeArg` in
/// scope for the whole spawn — mirroring the journal file-channel (#2700).
#[derive(Debug)]
pub enum RecipeArg {
    /// A small value inlined verbatim as `key=value` (newlines collapsed for
    /// YAML safety, #2127; never truncated — it is already small).
    Inline(String),
    /// An oversized value written to a private file; `argv` carries only the
    /// short `key_path=<abs>` token via [`ContextFile::arg_value`].
    Filed(ContextFile),
}

impl RecipeArg {
    /// The `-c` value to pass to `recipe-runner-rs`: `key=value` when inline,
    /// `key_path=<abs>` when filed. Small and constant-size when filed, so it
    /// can never contribute to `ARG_MAX` regardless of payload size.
    pub fn arg_value(&self) -> String {
        match self {
            RecipeArg::Inline(value) => value.clone(),
            RecipeArg::Filed(cf) => cf.arg_value(),
        }
    }
}

/// Resolve a recipe context `(key, value)` into an E2BIG-safe [`RecipeArg`].
///
/// The single policy: a value shorter than [`ARGV_PAYLOAD_MAX_BYTES`] is
/// inlined (`key=value`, newlines collapsed via the shared sanitizer so a
/// multi-line value can never break the recipe's YAML interpolation, #2127, but
/// **never truncated** — G3: no silent content loss); a value at or above the
/// threshold is written verbatim to a private temp file and only its short path
/// rides on `argv`. Either way the payload can never overflow `ARG_MAX`.
///
/// `site` namespaces the temp directory (e.g. `"overseer"`, `"self-improve"`).
/// Returns the underlying [`std::io::Error`] if the temp-file write fails (e.g.
/// `ENOSPC`) — callers surface it, they do not silently drop the context.
pub fn recipe_context(site: &str, key: &str, value: &str) -> std::io::Result<RecipeArg> {
    if value.len() < ARGV_PAYLOAD_MAX_BYTES {
        // Collapse newlines/whitespace for YAML safety but never truncate: the
        // value is already below the inline cap, so a no-op-large ceiling keeps
        // it lossless (`ARGV_PAYLOAD_MAX_BYTES` chars >= its char count).
        let inline =
            crate::ooda_brain::sanitize::sanitize_context_var(value, ARGV_PAYLOAD_MAX_BYTES);
        Ok(RecipeArg::Inline(format!("{key}={inline}")))
    } else {
        // Oversized: write the payload verbatim (no truncation) to a private
        // file; only `key_path=<abs>` will ride on argv.
        Ok(RecipeArg::Filed(ContextFile::write(site, key, value)?))
    }
}

// ---------------------------------------------------------------------------
// Copilot prompt transport (copilot family)
// ---------------------------------------------------------------------------

/// Attach a (possibly large) copilot prompt to a [`std::process::Command`] and
/// return the RAII feed guard.
///
/// The prompt is ALWAYS delivered on **stdin** ([`PromptDelivery::Stdin`]),
/// never as an argv token: `copilot` reads its prompt from stdin when no `-p`
/// is given, so the prompt never contributes to `ARG_MAX` regardless of size,
/// and it is never misparsed as a positional subcommand. This sets the child's
/// stdin to a pipe; the caller later calls [`AppliedPromptStd::feed`] with the
/// child's `stdin` — ideally from a feeder thread so a large prompt cannot
/// deadlock against the child filling stdout.
pub fn attach_prompt_std(
    cmd: &mut StdCommand,
    prompt: &[u8],
) -> Result<AppliedPromptStd, PromptDeliveryError> {
    // Force Stdin: never Inline (copilot cannot take a positional prompt) and
    // never TempFile (keep the proven meeting/OODA stdin behavior byte-for-byte
    // — no incidental postmortem file). Size-independent by construction.
    prompt_delivery::apply_std(cmd, prompt, PromptDelivery::Stdin)
}

/// Async sibling of [`attach_prompt_std`] for [`tokio::process::Command`].
pub async fn attach_prompt_tokio(
    cmd: &mut tokio::process::Command,
    prompt: &[u8],
) -> Result<AppliedPromptTokio, PromptDeliveryError> {
    prompt_delivery::apply_tokio(cmd, prompt, PromptDelivery::Stdin).await
}

// ---------------------------------------------------------------------------
// Failure surfacing
// ---------------------------------------------------------------------------

/// Classify a pre-exec spawn [`std::io::Error`] (E2BIG / ENOSPC / ENOMEM / …)
/// and record it into the Overseer failure sink — the "diagnose, don't just
/// log" seam every launch site pairs with its own error propagation.
///
/// A spawn failure has no child and no `ExitStatus`, so the exit-code
/// classifier cannot see it; this routes it to
/// [`classify_spawn_failure`] (errno-first) and tags the evidence with the
/// launch `site` so the Overseer's Observe pass knows WHERE the failure fired.
pub fn record_spawn_failure(err: &std::io::Error, site: &str) {
    let mut diagnosis = classify_spawn_failure(err);
    diagnosis.evidence = if diagnosis.evidence.is_empty() {
        format!("[{site}]")
    } else {
        format!("[{site}] {}", diagnosis.evidence)
    };
    failure_sink::record_step_failure(diagnosis);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_matches_prompt_inline_cap() {
        assert_eq!(ARGV_PAYLOAD_MAX_BYTES, prompt_delivery::INLINE_MAX_BYTES);
        assert_eq!(ARGV_PAYLOAD_MAX_BYTES, 8 * 1024);
    }

    #[test]
    fn small_recipe_context_inlines_verbatim() {
        let arg = recipe_context("test", "goal_id", "abc-123").expect("inline");
        assert!(matches!(arg, RecipeArg::Inline(_)));
        assert_eq!(arg.arg_value(), "goal_id=abc-123");
    }

    #[test]
    fn small_recipe_context_collapses_newlines() {
        let arg = recipe_context("test", "note", "a\nb\tc").expect("inline");
        let v = arg.arg_value();
        assert_eq!(v, "note=a b c");
        assert!(!v.contains('\n'));
    }

    #[test]
    fn oversized_recipe_context_is_filed_losslessly() {
        let payload = "x".repeat(ARGV_PAYLOAD_MAX_BYTES + 1);
        let arg = recipe_context("test", "plan", &payload).expect("file");
        let cf = match &arg {
            RecipeArg::Filed(cf) => cf,
            RecipeArg::Inline(_) => panic!("must be filed"),
        };
        assert!(arg.arg_value().starts_with("plan_path="));
        assert_eq!(std::fs::read_to_string(cf.path()).unwrap(), payload);
    }
}
