//! Harness executor — the wiring to COIN's real `coin evaluate` / `coin verify`
//! contract (issue #3001).
//!
//! The executor is the **objective oracle**: whether a submitted input actually
//! reaches the target line is decided by **executing the code** on a
//! coverage-instrumented build. The Gym **never re-implements reach-checking**;
//! it delegates to COIN's own pipeline and only *reads back* the `reached`
//! verdict COIN's verifier wrote. See `docs/reference/coin-benchmark.md` for the
//! code-verified contract this module implements.
//!
//! The real flow (LOCAL only — never submitted anywhere) is:
//!
//! 1. `coin evaluate --dataset <repo> --revision <tag> [--split ...]
//!    [--project ...] [--source rebuild|image]` runs the agent-under-test in a
//!    container; the agent writes its final answer to the bind-mounted
//!    `/answer/` directory (`blob.bin` + `blob.harness`, or `UNREACHABLE.md` to
//!    abstain). Evaluate mints an **experiment id**.
//! 2. `coin verify --experiment <id>` replays each submission against the
//!    project's coverage-instrumented build and writes `reached` (bool) into
//!    each work item's `result.json`.
//! 3. The Gym reads `reached` from each `result.json` to derive the per-target
//!    outcome.
//!
//! Steps 1–2 require Docker + a pulled snapshot on a provisioned host, which is
//! **Phase 3** (issue #2823). This module therefore keeps the *contract*
//! (argv construction, the `/answer/` submission writer, and the `result.json`
//! reader) fully unit-testable offline, while the live Docker delegate
//! ([`CoinEvaluateExecutor::grade`]) surfaces a clear Phase-3 gate error rather
//! than a fake verdict. For pure offline scaffold runs a [`MockHarnessExecutor`]
//! returns deterministic verdicts from a ground-truth table — a test double, not
//! a reachability engine.
//!
//! **LOCAL-ONLY guardrail.** Nothing in this module submits results externally,
//! writes a leaderboard entry, or provisions infrastructure. The only `coin`
//! subcommands it ever constructs are `evaluate` and `verify`; it never
//! constructs `publish`/`--hf-repo`/`--registry`. See [`LOCAL_ONLY`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::types::{CoinGymError, CoinGymResult, OutcomeCode, Target};

/// Compile-time assertion that this crate uses COIN as a **local** measurement
/// substrate only: no external submission, no leaderboard entry, no external VM
/// provisioning originates here. Kept as a `const` so it is greppable and can be
/// asserted by tests and downstream callers.
pub const LOCAL_ONLY: bool = true;

/// The `coin` subcommands this crate is permitted to drive. Deliberately limited
/// to the read/measure path — `publish` and any submission command are excluded
/// by the LOCAL-ONLY guardrail.
pub const ALLOWED_COIN_SUBCOMMANDS: &[&str] = &["evaluate", "verify"];

/// The submission-contract file the agent writes with the raw input **bytes**.
pub const ANSWER_BLOB_BIN: &str = "blob.bin";
/// The submission-contract file naming the harness binary the blob feeds.
pub const ANSWER_BLOB_HARNESS: &str = "blob.harness";
/// The abstention marker: written **instead of** `blob.bin` to decline.
pub const ANSWER_UNREACHABLE_MD: &str = "UNREACHABLE.md";

/// The oracle's verdict for a single **submitted** input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GradeResult {
    /// The input drove execution to the target line.
    Reached,
    /// The input was valid but did not reach the target line.
    WrongInput,
    /// Grading exceeded the per-target budget.
    TimedOut,
    /// Grading errored (harness crash, build failure, etc.).
    Error,
}

impl GradeResult {
    /// Map a grade of a *submitted* input to its outcome code.
    #[must_use]
    pub fn to_outcome_code(self) -> OutcomeCode {
        match self {
            Self::Reached => OutcomeCode::Reached,
            Self::WrongInput => OutcomeCode::WrongInput,
            Self::TimedOut => OutcomeCode::TimedOut,
            Self::Error => OutcomeCode::Error,
        }
    }
}

/// Grades a submitted input against a target. Implementors are the objective
/// oracle; they must not be spoofed by the agent under test.
pub trait HarnessExecutor {
    /// Grade `input` against `target`.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Executor`] when grading cannot be performed at all
    /// (e.g. `coin evaluate` is unavailable because the Phase-3 VM is not
    /// provisioned, or a `result.json` is missing). A *reached/not-reached*
    /// verdict is returned as `Ok(GradeResult)`, not an error.
    fn grade(&self, target: &Target, input: &str) -> CoinGymResult<GradeResult>;

    /// Whether this executor is an offline scaffold (mock) rather than a real
    /// COIN grade. Recorded on the [`crate::coin_gym::types::RunReport`] so
    /// offline runs are never mistaken for graded results. Executors that read
    /// **real** `coin verify` output (e.g. [`CoinResultsExecutor`]) return
    /// `false`.
    fn is_offline_scaffold(&self) -> bool {
        false
    }
}

// ── The submission contract (`/answer/`) ─────────────────────────────────────

/// The agent-under-test's final answer, per COIN's submission contract
/// (`coin/stages/stage7/evaluate/prompt.py`). Either a concrete **attempt**
/// (raw bytes + the harness they feed) or a precision-preserving **abstention**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentAnswer {
    /// A concrete attempt: `blob.bin` = raw input bytes; `blob.harness` = the
    /// single harness-binary name (from the provided list) to feed it to.
    Attempt {
        /// Raw input bytes written to `blob.bin`.
        blob: Vec<u8>,
        /// Harness binary name written (as one line) to `blob.harness`.
        harness: String,
    },
    /// Abstain: write **only** `UNREACHABLE.md` (with evidence) and NO
    /// `blob.bin`. If `blob.bin` exists it is treated as a normal attempt and
    /// `UNREACHABLE.md` is ignored, so [`write_answer`] removes any stale blob.
    Abstain {
        /// Evidence markdown written to `UNREACHABLE.md`.
        unreachable_md: String,
    },
}

/// Write the agent's final answer into `answer_dir` per the COIN submission
/// contract, keeping the directory unambiguous:
///
/// - **Attempt** ⇒ write `blob.bin` (bytes) + `blob.harness` (one line), and
///   remove any stale `UNREACHABLE.md` so the attempt is honoured.
/// - **Abstain** ⇒ write `UNREACHABLE.md`, and remove any stale `blob.bin` /
///   `blob.harness` so the abstention is not silently overridden (COIN treats a
///   present `blob.bin` as an attempt regardless of `UNREACHABLE.md`).
///
/// # Errors
/// Returns [`CoinGymError::Usage`] if the harness name is empty or multi-line
/// (the contract is *one line*), or [`CoinGymError::Io`] on a filesystem error.
pub fn write_answer(answer_dir: &Path, answer: &AgentAnswer) -> CoinGymResult<()> {
    std::fs::create_dir_all(answer_dir)
        .map_err(|e| CoinGymError::Io(format!("create {}: {e}", answer_dir.display())))?;
    let blob_bin = answer_dir.join(ANSWER_BLOB_BIN);
    let blob_harness = answer_dir.join(ANSWER_BLOB_HARNESS);
    let unreachable = answer_dir.join(ANSWER_UNREACHABLE_MD);
    match answer {
        AgentAnswer::Attempt { blob, harness } => {
            if harness.trim().is_empty() {
                return Err(CoinGymError::Usage(
                    "blob.harness must name a harness binary (got empty)".to_string(),
                ));
            }
            if harness.contains('\n') || harness.contains('\r') {
                return Err(CoinGymError::Usage(
                    "blob.harness must be a single line (the harness binary name)".to_string(),
                ));
            }
            remove_if_present(&unreachable)?;
            write_bytes(&blob_bin, blob)?;
            write_bytes(&blob_harness, harness.as_bytes())?;
        }
        AgentAnswer::Abstain { unreachable_md } => {
            remove_if_present(&blob_bin)?;
            remove_if_present(&blob_harness)?;
            write_bytes(&unreachable, unreachable_md.as_bytes())?;
        }
    }
    Ok(())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> CoinGymResult<()> {
    std::fs::write(path, bytes)
        .map_err(|e| CoinGymError::Io(format!("write {}: {e}", path.display())))
}

fn remove_if_present(path: &Path) -> CoinGymResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoinGymError::Io(format!("remove {}: {e}", path.display()))),
    }
}

// ── Reading `reached` from a work item's `result.json` ───────────────────────

/// The subset of a COIN work-item `result.json` the Gym reads. `coin verify`
/// writes `reached` (bool); the evaluation phase records the submission
/// disposition. All fields are optional so a partially-written or older
/// `result.json` parses without error and maps to a conservative outcome.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct CoinResultJson {
    /// The target this result is for (`<project>:<harness>:<file>:<lines>`).
    #[serde(default)]
    pub target_id: Option<String>,
    /// The verified reach verdict written by `coin verify`. `None` means verify
    /// has not (yet) run for this item.
    #[serde(default)]
    pub reached: Option<bool>,
    /// The disposition recorded during evaluation, e.g. `submitted`, `abstained`
    /// (`UNREACHABLE.md`), `no_submission`, `timeout`, `error`.
    #[serde(default)]
    pub status: Option<String>,
    /// Whether the agent submitted a `blob.bin` at all.
    #[serde(default)]
    pub submitted: Option<bool>,
    /// The harness the blob was fed to (from `blob.harness`).
    #[serde(default)]
    pub harness: Option<String>,
}

impl CoinResultJson {
    /// Parse a `result.json` payload.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Parse`] on malformed JSON.
    pub fn parse(raw: &str) -> CoinGymResult<Self> {
        serde_json::from_str(raw).map_err(|e| CoinGymError::Parse(format!("result.json: {e}")))
    }
}

fn status_kind(status: Option<&str>) -> Option<OutcomeCode> {
    let s = status?.trim().to_ascii_lowercase();
    match s.as_str() {
        "error" | "failed" | "crash" => Some(OutcomeCode::Error),
        "timeout" | "timed_out" | "timed-out" => Some(OutcomeCode::TimedOut),
        "abstained" | "abstain" | "unreachable" => Some(OutcomeCode::Abstained),
        "no_submission" | "no-submission" | "none" | "missing" | "empty" => {
            Some(OutcomeCode::NoSubmission)
        }
        _ => None,
    }
}

/// Map a parsed `result.json` to a full per-target [`OutcomeCode`]
/// (`R/W/A/T/N/E`). A decisive `status` (error/timeout/abstained/no-submission)
/// wins; otherwise the verified `reached` bool decides Reached vs WrongInput.
/// When neither is present the outcome is `Error` (verify did not produce a
/// verdict — an honest failure, never a silent pass).
#[must_use]
pub fn outcome_from_result(result: &CoinResultJson) -> OutcomeCode {
    if let Some(code) = status_kind(result.status.as_deref()) {
        return code;
    }
    match result.reached {
        Some(true) => OutcomeCode::Reached,
        Some(false) => OutcomeCode::WrongInput,
        None => {
            if result.submitted == Some(false) {
                OutcomeCode::NoSubmission
            } else {
                OutcomeCode::Error
            }
        }
    }
}

/// Map a parsed `result.json` to a [`GradeResult`] for a **submitted** input
/// (the executor only grades submissions; abstain / no-submission are decided
/// upstream). Abstain / no-submission dispositions collapse to `Error` here
/// because they should never reach the grader.
#[must_use]
pub fn grade_from_result(result: &CoinResultJson) -> GradeResult {
    match outcome_from_result(result) {
        OutcomeCode::Reached => GradeResult::Reached,
        OutcomeCode::WrongInput => GradeResult::WrongInput,
        OutcomeCode::TimedOut => GradeResult::TimedOut,
        OutcomeCode::Abstained | OutcomeCode::NoSubmission | OutcomeCode::Error => {
            GradeResult::Error
        }
    }
}

// ── Real evaluate/verify delegate (Phase-3 gated) ────────────────────────────

/// Which build the evaluator runs against: rebuild from the row's pins (default,
/// gated on the functional precheck) or the digest-pinned prebuilt image.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum EvaluateSource {
    /// `--source rebuild` (default): rebuild from the dataset row's pins.
    #[default]
    Rebuild,
    /// `--source image`: always pull the digest-pinned runtime image.
    Image,
}

impl EvaluateSource {
    /// The `--source` flag value.
    #[must_use]
    pub fn as_flag(self) -> &'static str {
        match self {
            Self::Rebuild => "rebuild",
            Self::Image => "image",
        }
    }
}

/// Configuration for the real `coin evaluate` / `coin verify` delegate.
///
/// Mirrors the snapshot-consumption path in the reference: `--dataset <repo>
/// --revision <tag>` selects the pinned snapshot; `--split` / `--project` narrow
/// the target set; `--source` picks rebuild-vs-image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoinEvaluateConfig {
    /// Path/name of the `coin` binary (default `coin`).
    pub binary: String,
    /// Hugging Face dataset repo, e.g. `COIN-Bench/coin`.
    pub dataset: String,
    /// Snapshot revision tag, e.g. `v2026-07`.
    pub revision: String,
    /// Repeatable `--split` filters (empty ⇒ all splits).
    pub splits: Vec<String>,
    /// Repeatable `--project` filters (empty ⇒ all projects).
    pub projects: Vec<String>,
    /// The evaluation build source.
    pub source: EvaluateSource,
}

impl CoinEvaluateConfig {
    /// Build a config for `dataset` at `revision` using the default `coin`
    /// binary, all splits/projects, and the default `rebuild` source.
    #[must_use]
    pub fn new(dataset: impl Into<String>, revision: impl Into<String>) -> Self {
        Self {
            binary: "coin".to_string(),
            dataset: dataset.into(),
            revision: revision.into(),
            splits: Vec::new(),
            projects: Vec::new(),
            source: EvaluateSource::Rebuild,
        }
    }

    /// Parse the `<repo>@<revision>` shorthand (e.g. `COIN-Bench/coin@v2026-07`)
    /// into a config, matching the reference's `--dataset you/coin@v1` sugar.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Usage`] if the string has no `@<revision>` or
    /// either side is empty.
    pub fn from_dataset_ref(dataset_ref: &str) -> CoinGymResult<Self> {
        let (repo, rev) = dataset_ref.rsplit_once('@').ok_or_else(|| {
            CoinGymError::Usage(format!(
                "dataset ref '{dataset_ref}' must be '<repo>@<revision>' (e.g. COIN-Bench/coin@v2026-07)"
            ))
        })?;
        if repo.is_empty() || rev.is_empty() {
            return Err(CoinGymError::Usage(format!(
                "dataset ref '{dataset_ref}' must have a non-empty repo and revision"
            )));
        }
        Ok(Self::new(repo, rev))
    }

    /// Override the `coin` binary path/name.
    #[must_use]
    pub fn with_binary(mut self, binary: impl Into<String>) -> Self {
        self.binary = binary.into();
        self
    }

    /// Add a repeatable `--split` filter.
    #[must_use]
    pub fn with_split(mut self, split: impl Into<String>) -> Self {
        self.splits.push(split.into());
        self
    }

    /// Add a repeatable `--project` filter.
    #[must_use]
    pub fn with_project(mut self, project: impl Into<String>) -> Self {
        self.projects.push(project.into());
        self
    }

    /// Set the evaluation build source.
    #[must_use]
    pub fn with_source(mut self, source: EvaluateSource) -> Self {
        self.source = source;
        self
    }
}

/// Delegates grading to COIN's own oracle via `coin evaluate` + `coin verify`.
///
/// This is the production path. It is a thin shell around the external tool —
/// the reachability judgement lives entirely in COIN's instrumented replay,
/// never here. Actually invoking it requires Docker + a pulled snapshot
/// (Phase 3, issue #2823); until then [`Self::grade`] surfaces a clear Phase-3
/// gate error while the delegation contract ([`Self::build_evaluate_argv`],
/// [`Self::build_verify_argv`]) and the `/answer/` + `result.json` wiring stay
/// unit-testable offline.
#[derive(Clone, Debug)]
pub struct CoinEvaluateExecutor {
    config: CoinEvaluateConfig,
}

impl CoinEvaluateExecutor {
    /// Create the delegate from a config.
    #[must_use]
    pub fn new(config: CoinEvaluateConfig) -> Self {
        Self { config }
    }

    /// The config this delegate drives.
    #[must_use]
    pub fn config(&self) -> &CoinEvaluateConfig {
        &self.config
    }

    /// The argv for `coin evaluate` against the configured snapshot — the exact
    /// contract from the reference:
    /// `coin evaluate --dataset <repo> --revision <tag> [--split ...]
    /// [--project ...] --source <rebuild|image>`.
    #[must_use]
    pub fn build_evaluate_argv(&self) -> Vec<String> {
        let mut argv = vec![
            self.config.binary.clone(),
            "evaluate".to_string(),
            "--dataset".to_string(),
            self.config.dataset.clone(),
            "--revision".to_string(),
            self.config.revision.clone(),
        ];
        for split in &self.config.splits {
            argv.push("--split".to_string());
            argv.push(split.clone());
        }
        for project in &self.config.projects {
            argv.push("--project".to_string());
            argv.push(project.clone());
        }
        argv.push("--source".to_string());
        argv.push(self.config.source.as_flag().to_string());
        argv
    }

    /// The argv for `coin verify` on the experiment `coin evaluate` minted:
    /// `coin verify --experiment <id> [--max-concurrent <n>]`.
    #[must_use]
    pub fn build_verify_argv(
        &self,
        experiment_id: &str,
        max_concurrent: Option<u32>,
    ) -> Vec<String> {
        let mut argv = vec![
            self.config.binary.clone(),
            "verify".to_string(),
            "--experiment".to_string(),
            experiment_id.to_string(),
        ];
        if let Some(n) = max_concurrent {
            argv.push("--max-concurrent".to_string());
            argv.push(n.to_string());
        }
        argv
    }
}

/// Extract the experiment id `coin evaluate` mints from its output. Evaluate
/// prints it (and creates `output/experiments/<id>/`); `coin verify` needs it.
/// Recognises `experiment: <id>`, `experiment_id=<id>`, and
/// `output/experiments/<id>/` forms. Returns the first match.
#[must_use]
pub fn parse_experiment_id(evaluate_output: &str) -> Option<String> {
    for line in evaluate_output.lines() {
        let line = line.trim();
        if let Some(rest) = line
            .strip_prefix("experiment:")
            .or_else(|| line.strip_prefix("experiment_id="))
            .or_else(|| line.strip_prefix("experiment_id:"))
        {
            // The id is the first whitespace-delimited token, unquoted, so a
            // trailing note (`experiment: exp-1 (2 items)`) does not leak in.
            if let Some(id) = rest.trim().trim_matches('"').split_whitespace().next()
                && !id.is_empty()
            {
                return Some(id.to_string());
            }
        }
        if let Some(idx) = line.find("output/experiments/") {
            let after = &line[idx + "output/experiments/".len()..];
            let id: String = after
                .chars()
                .take_while(|c| *c != '/' && !c.is_whitespace())
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
}

impl HarnessExecutor for CoinEvaluateExecutor {
    fn grade(&self, _target: &Target, _input: &str) -> CoinGymResult<GradeResult> {
        // Phase 3 gate: real grading needs `coin evaluate` + `coin verify` on a
        // Docker host with a pulled snapshot. Surfacing an explicit error
        // (rather than a silent fake verdict) keeps the harness honest — an
        // offline run must use MockHarnessExecutor (scaffold) or read real
        // `coin verify` output via CoinResultsExecutor.
        Err(CoinGymError::Executor(format!(
            "`{bin} evaluate`/`{bin} verify` require a Docker host + pulled snapshot \
             ({dataset}@{rev}) — Phase 3 (azlin VM, issue #2823). Use MockHarnessExecutor for \
             offline scaffold runs, or CoinResultsExecutor to read a completed run's result.json",
            bin = self.config.binary,
            dataset = self.config.dataset,
            rev = self.config.revision,
        )))
    }
}

// ── Results reader: read `reached` from a completed run's `result.json` ───────

/// Reads the **real** grades `coin verify` wrote into per-work-item
/// `result.json` files under an experiment results directory. This is the
/// read-side of the real contract: it never re-implements reach-checking, it
/// only parses the `reached` verdict COIN already computed by replaying the
/// harness on the instrumented build.
///
/// Layout: `<results_dir>/<target-slug>/result.json`, where `target-slug` is the
/// target id with path-unsafe characters replaced (so a `project:harness:file`
/// id never escapes `results_dir`). Because these are real grades, this executor
/// is **not** an offline scaffold.
#[derive(Clone, Debug)]
pub struct CoinResultsExecutor {
    results_dir: PathBuf,
}

impl CoinResultsExecutor {
    /// Create a reader over a completed experiment's results directory.
    #[must_use]
    pub fn new(results_dir: impl Into<PathBuf>) -> Self {
        Self {
            results_dir: results_dir.into(),
        }
    }

    /// The directory-safe slug for a target id used as a path component.
    #[must_use]
    pub fn target_slug(target_id: &str) -> String {
        target_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }

    /// The `result.json` path for a target under the results directory.
    #[must_use]
    pub fn result_path(&self, target_id: &str) -> PathBuf {
        self.results_dir
            .join(Self::target_slug(target_id))
            .join("result.json")
    }

    /// Read and parse the `result.json` for `target_id`.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Executor`] if the file is missing (the run did
    /// not grade this target) or [`CoinGymError::Parse`] on malformed JSON.
    pub fn read_result(&self, target_id: &str) -> CoinGymResult<CoinResultJson> {
        let path = self.result_path(target_id);
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            CoinGymError::Executor(format!(
                "cannot read result.json for '{target_id}' at {}: {e}",
                path.display()
            ))
        })?;
        CoinResultJson::parse(&raw)
    }

    /// The full per-target outcome (`R/W/A/T/N/E`) for a target, read from its
    /// `result.json`. Unlike [`HarnessExecutor::grade`] this can also report
    /// `Abstained` / `NoSubmission`, so it is the honest reader for a whole run.
    ///
    /// # Errors
    /// Propagates read/parse errors from [`Self::read_result`].
    pub fn outcome(&self, target_id: &str) -> CoinGymResult<OutcomeCode> {
        Ok(outcome_from_result(&self.read_result(target_id)?))
    }
}

impl HarnessExecutor for CoinResultsExecutor {
    fn grade(&self, target: &Target, _input: &str) -> CoinGymResult<GradeResult> {
        // The input was already submitted to `/answer/` and replayed by
        // `coin verify`; the verdict lives in result.json. We only read it.
        Ok(grade_from_result(&self.read_result(&target.id)?))
    }

    fn is_offline_scaffold(&self) -> bool {
        false
    }
}

// ── Mock executor: deterministic test double ─────────────────────────────────

/// A deterministic oracle test double backed by a ground-truth lookup table.
///
/// It does **not** compute reachability — it simply checks the submitted input
/// against a known reaching input per target (and optional injected
/// timeout/error sets). This lets the full pipeline run offline without a VM
/// while keeping the real oracle (`coin evaluate` + `coin verify`) the only
/// thing that ever judges reachability for real.
#[derive(Clone, Debug, Default)]
pub struct MockHarnessExecutor {
    reaching_input: HashMap<String, String>,
    timeout_ids: HashSet<String>,
    error_ids: HashSet<String>,
}

impl MockHarnessExecutor {
    /// An empty mock (every submission grades as `WrongInput`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a mock from a `target_id -> reaching_input` ground-truth map.
    #[must_use]
    pub fn from_oracle(reaching_input: HashMap<String, String>) -> Self {
        Self {
            reaching_input,
            ..Self::default()
        }
    }

    /// Register a reaching input for a target.
    #[must_use]
    pub fn with_reaching_input(
        mut self,
        target_id: impl Into<String>,
        input: impl Into<String>,
    ) -> Self {
        self.reaching_input.insert(target_id.into(), input.into());
        self
    }

    /// Force a target to grade as `TimedOut` regardless of input.
    #[must_use]
    pub fn with_timeout(mut self, target_id: impl Into<String>) -> Self {
        self.timeout_ids.insert(target_id.into());
        self
    }

    /// Force a target to grade as `Error` regardless of input.
    #[must_use]
    pub fn with_error(mut self, target_id: impl Into<String>) -> Self {
        self.error_ids.insert(target_id.into());
        self
    }
}

impl HarnessExecutor for MockHarnessExecutor {
    fn grade(&self, target: &Target, input: &str) -> CoinGymResult<GradeResult> {
        if self.error_ids.contains(&target.id) {
            return Ok(GradeResult::Error);
        }
        if self.timeout_ids.contains(&target.id) {
            return Ok(GradeResult::TimedOut);
        }
        match self.reaching_input.get(&target.id) {
            Some(expected) if expected == input => Ok(GradeResult::Reached),
            _ => Ok(GradeResult::WrongInput),
        }
    }

    fn is_offline_scaffold(&self) -> bool {
        true
    }
}
