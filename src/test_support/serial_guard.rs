//! Regression-guard meta-test for the `serial(cognitive_memory)` contract
//! (issue [#2360](https://github.com/rysweet/Simard/issues/2360)).
//!
//! The `cargo test --lib` binary runs many tests concurrently in ONE process.
//! The OS environment (`environ`) is process-global and glibc `setenv`/`getenv`
//! are not thread-safe, so a test that mutates a process-global env var can tear
//! a *concurrent* env read in an unrelated test — including the dashboard goal
//! handlers' read of `SIMARD_STATE_ROOT` via `resolve_state_root()`. That race
//! is what intermittently failed
//! `operator_commands_dashboard::tests_goals_crud::full_goal_lifecycle_crud`.
//!
//! The fix is a single rule (the "Annotation Decision Rule"): every lib-binary
//! test that mutates *any* process-global environment variable, or reads the
//! cognitive-memory state-root environment / opens cognitive memory at the
//! env-derived default path, MUST carry the `cognitive_memory` serial key so an
//! env mutation is never concurrent with an env read. Enforcement was widened
//! from the state-root surface to every variable in issue
//! [#2375](https://github.com/rysweet/Simard/issues/2375): a `setenv` on *any*
//! name can `realloc(environ)` and free the array a concurrent `getenv` is
//! mid-read, so the writer and reader need not touch the same variable to race.
//!
//! This module parses the source tree with `syn` (AST-based, robust to
//! multi-line attributes, ordering, raw strings, and `#[cfg]` gating) and fails
//! the build if a hand-written `#[test]` violates the rule. It NEVER emits a
//! false positive: an offender is reported only when a concrete trigger call is
//! observed without the key. See
//! `docs/testing/cognitive-memory-serial-isolation.md` for the full contract,
//! the known false-negative blind spots, and the allowlist mechanism.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use proc_macro2::TokenTree;
use syn::visit::Visit;
use syn::{Attribute, Expr, ExprCall, ExprStruct, ImplItem, ItemFn, ItemImpl, Lit, Meta, Type};

/// The serial key that serializes every cognitive-memory env reader/writer.
const REQUIRED_KEY: &str = "cognitive_memory";

/// State-root / provider variables whose *read* can be torn by a concurrent
/// mutation — the reads that actually surfaced the #2360 flake.
/// `SIMARD_LLM_PROVIDER` is included because the dashboard agent-session
/// resolver (`open_dashboard_agent_session`) and the
/// `open_agent_session_returns_none_without_provider_config` test read it, and
/// the `ooda_actions` / `self_improve` / `disk_health` tests mutate it.
/// `SIMARD_MEETINGS_DIR` / `SIMARD_MEETINGS_ROOT` are included because the
/// meeting-persistence resolver (`meetings_dir`, used by `write_auto_save` /
/// `write_transcript` / `write_meeting_bundle`) consults them *before* falling
/// through to `SIMARD_STATE_ROOT`; a concurrent write to either (e.g. the
/// `write_meeting_bundle_*` bundle tests) tears a meetings-resolver read and
/// routed `write_auto_save_lands_under_simard_state_root`'s autosave into the
/// wrong directory (the #2360 race class, re-surfaced in CI).
/// `SIMARD_HANDOFF_DIR` is included for symmetry: the handoff resolver
/// (`default_handoff_dir`, read by `load_carried_meeting_decisions`) consults
/// it, and its writers in `ooda_loop` / `operator_cli` / `meeting_backend` race
/// the `tests_meeting_decisions` reader (which joined `cognitive_memory` because
/// it also writes `SIMARD_MEETINGS_ROOT`). `HOME`
/// is NOT here: a torn *read* of `HOME` is not the cognitive-memory race; only
/// a *write* to `HOME` (which can tear a `SIMARD_STATE_ROOT` read) is in scope,
/// and writes are handled by [`EnvWatch`].
const READ_WATCHED_VARS: &[&str] = &[
    "SIMARD_STATE_ROOT",
    "SIMARD_MEMORY_SOCKET",
    "SIMARD_LLM_PROVIDER",
    "SIMARD_MEETINGS_DIR",
    "SIMARD_MEETINGS_ROOT",
    "SIMARD_HANDOFF_DIR",
];

/// Env-reading async dashboard route handlers from
/// `operator_commands_dashboard/goals.rs` (each resolves the state root
/// internally via `resolve_state_root()`), plus the meeting-persistence
/// resolvers from `meeting_backend/persist` (`write_auto_save` /
/// `write_transcript` / `write_meeting_bundle` all resolve through
/// `meetings_dir()` -> `SIMARD_MEETINGS_DIR` / `SIMARD_MEETINGS_ROOT` /
/// `SIMARD_STATE_ROOT`). Calling one is an env read of the cognitive-memory
/// state-root surface.
const ENV_READING_HANDLERS: &[&str] = &[
    "seed_goals",
    "add_goal",
    "remove_goal",
    "update_goal_status",
    "promote_backlog_item",
    "demote_goal",
    "write_auto_save",
    "write_transcript",
    "write_meeting_bundle",
];

// ---------------------------------------------------------------------------
// Public(crate) audit API
// ---------------------------------------------------------------------------

/// Which env-var mutations trip the rule.
#[derive(Debug, Clone)]
pub(crate) enum EnvWatch {
    /// The legacy narrower policy (the default before #2375): the
    /// cognitive-memory state-root resolution surface
    /// — the variables `resolve_state_root()` / `socket_path_for` consult
    /// (`SIMARD_STATE_ROOT`, `SIMARD_MEMORY_SOCKET`, and the `HOME` fallback),
    /// plus `SIMARD_LLM_PROVIDER` (the dashboard agent-session / provider
    /// resolution surface, whose readers race the provider mutators in
    /// `ooda_actions` / `self_improve` / `disk_health`) and
    /// `SIMARD_MEETINGS_DIR` / `SIMARD_MEETINGS_ROOT` (the meeting-persistence
    /// resolver `meetings_dir()`, which falls through to `SIMARD_STATE_ROOT`;
    /// its writers — the `write_meeting_bundle_*` / `meetings_*` tests — race
    /// the autosave/transcript readers), plus `SIMARD_HANDOFF_DIR` (the
    /// `default_handoff_dir()` / handoff-bundle surface; its writers in
    /// `ooda_loop` / `operator_cli` / `meeting_backend` race the
    /// `load_carried_meeting_decisions` reader in `tests_meeting_decisions`,
    /// which shares the `cognitive_memory` group because it also writes
    /// `SIMARD_MEETINGS_ROOT`). This is the demonstrated race surface
    /// for #2360.
    StateRootSurface,
    /// Watch a specific set of variable names.
    Vars(BTreeSet<String>),
    /// Fully var-agnostic: any process-global mutation trips the rule. This is
    /// the shipped default since issue #2375 — a `setenv` on any name may
    /// `realloc`/free the whole `environ`, so every env writer must be mutually
    /// exclusive with every cognitive-memory env reader, regardless of which
    /// variable it touches.
    AnyVar,
}

impl EnvWatch {
    fn watches_mutation(&self, var: &str) -> bool {
        match self {
            EnvWatch::AnyVar => true,
            EnvWatch::StateRootSurface => {
                matches!(
                    var,
                    "SIMARD_STATE_ROOT"
                        | "SIMARD_MEMORY_SOCKET"
                        | "HOME"
                        | "SIMARD_LLM_PROVIDER"
                        | "SIMARD_MEETINGS_DIR"
                        | "SIMARD_MEETINGS_ROOT"
                        | "SIMARD_HANDOFF_DIR"
                )
            }
            EnvWatch::Vars(set) => set.contains(var),
        }
    }
}

/// Configuration for [`audit_env_mutating_tests`].
#[derive(Debug, Clone)]
pub(crate) struct AuditOptions {
    /// Source roots to scan, relative to `CARGO_MANIFEST_DIR` (or absolute).
    pub roots: Vec<PathBuf>,
    /// Path prefixes (relative to the manifest, `/`-separated) that are NOT
    /// part of the lib test binary and so are exempt. Each `src/bin/<tool>` is
    /// a separate process with its own `environ`.
    pub excluded_prefixes: Vec<String>,
    /// Which variable mutations trip the rule.
    pub watched: EnvWatch,
    /// Tests exempted with a machine-checked justification. Each entry is
    /// `(test_name_or_path, justification)`; an empty justification is itself
    /// an audit failure so exemptions cannot be added silently.
    pub allowlist: Vec<(String, String)>,
}

impl Default for AuditOptions {
    fn default() -> Self {
        Self {
            roots: vec![PathBuf::from("src")],
            excluded_prefixes: vec!["src/bin".to_string()],
            watched: EnvWatch::AnyVar,
            allowlist: Vec::new(),
        }
    }
}

/// Why a test was flagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reason {
    /// Mutates a watched process-global variable via `set_var`/`remove_var`
    /// (or the `EnvGuard` / `SkipGuard` test helpers), or constructs a guard
    /// type whose `impl` methods (constructor or `Drop` teardown) mutate one.
    MutatesEnv { var: String },
    /// Reads the state-root/socket env default (directly, or via
    /// `resolve_state_root` / `default_state_root` / `simard_state_root`).
    ReadsStateRootDefault,
    /// Constructs a `HermeticState`, which sets `SIMARD_STATE_ROOT` and unsets
    /// `SIMARD_MEMORY_SOCKET` in process-global env.
    ConstructsHermeticState,
    /// Invokes an env-reading async dashboard goal route handler or a
    /// meeting-persistence resolver (`write_auto_save` / `write_transcript` /
    /// `write_meeting_bundle`).
    CallsEnvReadingHandler { handler: String },
    /// An allowlist entry was added without a justification.
    EmptyAllowlistJustification,
}

impl Reason {
    fn describe(&self) -> String {
        match self {
            Reason::MutatesEnv { var } => {
                format!("mutates {var} via std::env::set_var/remove_var")
            }
            Reason::ReadsStateRootDefault => {
                "reads the state-root env default (resolve_state_root/default_state_root)"
                    .to_string()
            }
            Reason::ConstructsHermeticState => {
                "constructs HermeticState (sets SIMARD_STATE_ROOT / unsets SIMARD_MEMORY_SOCKET)"
                    .to_string()
            }
            Reason::CallsEnvReadingHandler { handler } => {
                format!(
                    "calls env-reading handler `{handler}` (resolves the state-root / meetings surface)"
                )
            }
            Reason::EmptyAllowlistJustification => {
                "allowlist entry has no justification".to_string()
            }
        }
    }

    /// Lower number wins when a test trips several rules — pick the most
    /// actionable reason for the report.
    fn priority(&self) -> u8 {
        match self {
            Reason::EmptyAllowlistJustification => 0,
            Reason::MutatesEnv { .. } => 1,
            Reason::ConstructsHermeticState => 2,
            Reason::CallsEnvReadingHandler { .. } => 3,
            Reason::ReadsStateRootDefault => 4,
        }
    }

    /// Whether this reason is an env *mutation* (a race *cause*). Only mutation
    /// reasons propagate along the call graph: a test that reaches a `set_var`
    /// / `HermeticState` through a same-file helper is a writer and must be
    /// serialized. Reads (a race *victim*) are flagged only when they appear
    /// *directly* in the test body, never propagated through branchy production
    /// dispatchers (which read state root only in untaken code paths).
    fn is_mutation(&self) -> bool {
        matches!(
            self,
            Reason::MutatesEnv { .. } | Reason::ConstructsHermeticState
        )
    }
}

/// One offending test.
#[derive(Debug, Clone)]
pub(crate) struct Offender {
    pub file: PathBuf,
    pub line: usize,
    pub test_name: String,
    pub reason: Reason,
}

/// Parse the source tree and return every `#[test]`/`#[tokio::test]` function
/// that trips the Annotation Decision Rule without carrying the
/// `cognitive_memory` serial key. Pure and side-effect-free.
pub(crate) fn audit_env_mutating_tests(opts: &AuditOptions) -> Vec<Offender> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();

    // An allowlist entry without a justification is itself a failure.
    for (name, justification) in &opts.allowlist {
        if justification.trim().is_empty() {
            offenders.push(Offender {
                file: PathBuf::from("<allowlist>"),
                line: 0,
                test_name: name.clone(),
                reason: Reason::EmptyAllowlistJustification,
            });
        }
    }
    let allowed: BTreeSet<&str> = opts
        .allowlist
        .iter()
        .filter(|(_, j)| !j.trim().is_empty())
        .map(|(name, _)| name.as_str())
        .collect();

    let mut files = Vec::new();
    for root in &opts.roots {
        let abs = if root.is_absolute() {
            root.clone()
        } else {
            manifest.join(root)
        };
        collect_rs_files(&abs, &mut files);
    }
    files.sort();

    // Each file's audit (read → parse → scan) is independent and writes no
    // shared state, so this parse-bound scan of the whole source tree fans out
    // across the available cores via scoped threads (std-only, no extra deps).
    // The deterministic final sort below makes the result independent of thread
    // scheduling and chunk boundaries.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len().max(1));
    let chunk_len = files.len().div_ceil(threads).max(1);
    let allowed = &allowed;
    let per_thread: Vec<Vec<Offender>> = std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk_len)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut local = Vec::new();
                    for file in chunk {
                        audit_path(file, manifest, opts, allowed, &mut local);
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("serial_guard audit worker panicked"))
            .collect()
    });
    for local in per_thread {
        offenders.extend(local);
    }

    offenders.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.test_name.cmp(&b.test_name))
    });
    offenders
}

/// Read, parse, and audit a single source file, appending any offenders to
/// `out`. Files under an excluded prefix are skipped; files that are unreadable
/// or that don't parse with `syn` (rare on stable) are skipped rather than
/// failing the audit spuriously — they remain a documented blind spot. Pure and
/// side-effect-free apart from `out`, so it is safe to run concurrently across
/// disjoint file chunks.
fn audit_path(
    file: &Path,
    manifest: &Path,
    opts: &AuditOptions,
    allowed: &BTreeSet<&str>,
    out: &mut Vec<Offender>,
) {
    let rel = file.strip_prefix(manifest).unwrap_or(file).to_path_buf();
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if opts
        .excluded_prefixes
        .iter()
        .any(|p| rel_str == *p || rel_str.starts_with(&format!("{p}/")))
    {
        return;
    }

    let Ok(src) = std::fs::read_to_string(file) else {
        return;
    };
    let Ok(ast) = syn::parse_file(&src) else {
        return;
    };

    audit_file(&rel, &ast, &opts.watched, allowed, out);
}

// ---------------------------------------------------------------------------
// Per-file two-pass audit
// ---------------------------------------------------------------------------
//
// Pass 1 collects every fn in the file (tests AND helpers) with its direct
// trigger reason and the set of same-file fn names it calls. Pass 2 propagates
// "env-touching" along the call graph to a fixpoint, so a test that reaches a
// watched mutation through a same-file helper (e.g. `with_state_root`,
// `with_temp_home`) is flagged just like a direct mutator. Cross-file helpers
// remain a documented blind spot.

struct FnInfo {
    name: String,
    is_test: bool,
    has_key: bool,
    line: usize,
    direct_reason: Option<Reason>,
    calls: BTreeSet<String>,
}

fn audit_file(
    file: &Path,
    ast: &syn::File,
    watched: &EnvWatch,
    allowed: &BTreeSet<&str>,
    out: &mut Vec<Offender>,
) {
    // Pass 0: discover env-mutating guard TYPES in this file — types whose
    // `impl` methods (a constructor, a helper, or the `Drop` teardown) mutate a
    // watched process-global env var. A test that merely *constructs* such a
    // type is an env mutator even though its own body issues no `set_var`: the
    // mutation lives in the guard's `impl`, which `FnCollector` never visits and
    // whose type name is not one of the hard-coded helper recognizers. This is
    // the indirect Drop-teardown blind spot behind the #4519 canary exit-101
    // flake. The discovery is file-local (cross-file guard types remain a
    // documented blind spot, like cross-file helpers).
    let mut guard_collector = GuardTypeCollector {
        watched,
        env_mutating_types: BTreeMap::new(),
    };
    guard_collector.visit_file(ast);
    let guard_types = guard_collector.env_mutating_types;

    let mut collector = FnCollector {
        watched,
        guard_types: &guard_types,
        fns: Vec::new(),
    };
    collector.visit_file(ast);
    let fns = collector.fns;

    // Pass 2: fixpoint propagation of the env-touching reason along calls.
    let mut reason_of: std::collections::HashMap<String, Reason> = std::collections::HashMap::new();
    for f in &fns {
        if let Some(r) = &f.direct_reason {
            // Keep the strongest direct reason per name.
            match reason_of.get(&f.name) {
                Some(existing) if existing.priority() <= r.priority() => {}
                _ => {
                    reason_of.insert(f.name.clone(), r.clone());
                }
            }
        }
    }
    loop {
        let mut changed = false;
        for f in &fns {
            if reason_of.contains_key(&f.name) {
                continue;
            }
            for callee in &f.calls {
                if let Some(r) = reason_of.get(callee) {
                    // Only mutation reasons propagate (see Reason::is_mutation).
                    if !r.is_mutation() {
                        continue;
                    }
                    let r = r.clone();
                    reason_of.insert(f.name.clone(), r);
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }

    for f in &fns {
        if !f.is_test || f.has_key || allowed.contains(f.name.as_str()) {
            continue;
        }
        if let Some(reason) = reason_of.get(&f.name) {
            out.push(Offender {
                file: file.to_path_buf(),
                line: f.line,
                test_name: f.name.clone(),
                reason: reason.clone(),
            });
        }
    }
}

struct FnCollector<'a> {
    watched: &'a EnvWatch,
    guard_types: &'a BTreeMap<String, String>,
    fns: Vec<FnInfo>,
}

impl<'a, 'ast> Visit<'ast> for FnCollector<'a> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let mut body = BodyScan {
            watched: self.watched,
            guard_types: self.guard_types,
            reasons: Vec::new(),
        };
        body.visit_block(&node.block);
        let mut calls = CallCollector {
            names: BTreeSet::new(),
        };
        calls.visit_block(&node.block);
        self.fns.push(FnInfo {
            name: node.sig.ident.to_string(),
            is_test: is_test_fn(&node.attrs),
            has_key: serial_keys(&node.attrs).contains(REQUIRED_KEY),
            line: node.sig.ident.span().start().line,
            direct_reason: body.strongest_reason(),
            calls: calls.names,
        });
        // Do not recurse into the body: module traversal already reaches all
        // module-level fns, and nested items are covered by the scans above.
    }
}

/// Collects the names of same-file free functions called in a body (single
/// path-segment calls), used for call-graph propagation.
struct CallCollector {
    names: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for CallCollector {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(path) = node.func.as_ref()
            && path.qself.is_none()
            && path.path.segments.len() == 1
        {
            self.names.insert(path.path.segments[0].ident.to_string());
        }
        syn::visit::visit_expr_call(self, node);
    }
}

struct BodyScan<'a> {
    watched: &'a EnvWatch,
    guard_types: &'a BTreeMap<String, String>,
    reasons: Vec<Reason>,
}

impl<'a> BodyScan<'a> {
    fn strongest_reason(mut self) -> Option<Reason> {
        self.reasons.sort_by_key(|r| r.priority());
        self.reasons.into_iter().next()
    }
}

impl<'a, 'ast> Visit<'ast> for BodyScan<'a> {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(path) = node.func.as_ref() {
            let segs = &path.path.segments;
            let last = segs.last().map(|s| s.ident.to_string()).unwrap_or_default();
            let penult = segs
                .iter()
                .rev()
                .nth(1)
                .map(|s| s.ident.to_string())
                .unwrap_or_default();

            // (B) direct env mutation via the primitive `set_var`/`remove_var`
            // or the by-name `EnvGuard`/`SkipGuard` helper mutators.
            if let Some(var) = direct_mutation_var(node, self.watched) {
                self.reasons.push(Reason::MutatesEnv { var });
            }

            // (B') generic env-mutating guard construction: `T::method(...)`
            // where T's `impl` methods (a constructor or the `Drop` teardown)
            // mutate a watched env var. The mutation is invisible to the direct
            // scan because it lives in the guard's `impl`, not the test body —
            // the indirect Drop-teardown blind spot behind the #4519 canary
            // exit-101 flake. `penult` is the type name for an associated-fn
            // call (`StateRootGuard::set` → penult == "StateRootGuard").
            if !penult.is_empty()
                && let Some(var) = self.guard_types.get(&penult)
            {
                self.reasons.push(Reason::MutatesEnv { var: var.clone() });
            }

            // (A) HermeticState constructor.
            if penult == "HermeticState"
                && matches!(
                    last.as_str(),
                    "new" | "new_in" | "default" | "new_with_temp"
                )
            {
                self.reasons.push(Reason::ConstructsHermeticState);
            }

            // (C) state-root default resolvers. Match `resolve_state_root`
            // (a single distinctive free fn) and the cognitive-memory
            // `default_state_root` / `simard_state_root`, but NOT same-named
            // associated fns on unrelated types (e.g. `BuildLock::default_state_root`,
            // which resolves a build-lock path, not the cognitive-memory root).
            if is_cog_state_root_resolver(&last, &penult) {
                self.reasons.push(Reason::ReadsStateRootDefault);
            }

            // (C) direct read of a watched state-root/socket var.
            if (last == "var" || last == "var_os")
                && penult == "env"
                && let Some(var) = node.args.first().and_then(arg_to_var)
                && READ_WATCHED_VARS.contains(&var.as_str())
            {
                self.reasons.push(Reason::ReadsStateRootDefault);
            }

            // (D) env-reading async goal route handlers and meeting-persistence
            // resolvers (write_auto_save / write_transcript / write_meeting_bundle).
            if segs.len() == 1 && ENV_READING_HANDLERS.contains(&last.as_str()) {
                self.reasons
                    .push(Reason::CallsEnvReadingHandler { handler: last });
            }
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast ExprStruct) {
        let last = node
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if last == "HermeticState" {
            self.reasons.push(Reason::ConstructsHermeticState);
        }
        // Generic env-mutating guard constructed via struct literal `T { .. }`.
        if let Some(var) = self.guard_types.get(&last) {
            self.reasons.push(Reason::MutatesEnv { var: var.clone() });
        }
        syn::visit::visit_expr_struct(self, node);
    }
}

// ---------------------------------------------------------------------------
// Env-mutating guard-type discovery (the indirect Drop-teardown blind spot)
// ---------------------------------------------------------------------------

/// Detect a *direct* watched env mutation at a call site — the primitive
/// `std::env::set_var`/`remove_var`, or the by-name `EnvGuard`/`SkipGuard`
/// helper mutators (whose own `set_var`/`remove_var` is hidden in their impl
/// methods, so the var name is implicit). Returns the mutated watched var name,
/// or `None`. Shared by the per-test body scan and the guard-type discovery
/// pass so both agree on what "directly mutates env" means. Conservative:
/// unresolvable (dynamic) var names never match, so it never emits a false
/// positive.
fn direct_mutation_var(node: &ExprCall, watched: &EnvWatch) -> Option<String> {
    let Expr::Path(path) = node.func.as_ref() else {
        return None;
    };
    let segs = &path.path.segments;
    let last = segs.last().map(|s| s.ident.to_string()).unwrap_or_default();
    let penult = segs
        .iter()
        .rev()
        .nth(1)
        .map(|s| s.ident.to_string())
        .unwrap_or_default();

    // std::env::set_var / remove_var(VAR, ..)
    if (last == "set_var" || last == "remove_var")
        && let Some(var) = node.args.first().and_then(arg_to_var)
        && watched.watches_mutation(&var)
    {
        return Some(var);
    }
    // EnvGuard::set / unset(VAR, ..)
    if penult == "EnvGuard"
        && (last == "set" || last == "unset")
        && let Some(var) = node.args.first().and_then(arg_to_var)
        && watched.watches_mutation(&var)
    {
        return Some(var);
    }
    // SkipGuard::set / clear() — always the hard-coded `SIMARD_SKIP_GYM`, so the
    // var name is implicit (not an argument).
    if penult == "SkipGuard"
        && (last == "set" || last == "clear")
        && watched.watches_mutation("SIMARD_SKIP_GYM")
    {
        return Some("SIMARD_SKIP_GYM".to_string());
    }
    None
}

/// File-level pre-pass: find guard TYPES whose `impl` methods (a constructor, a
/// helper, or the `Drop` teardown) directly mutate a watched process-global env
/// var. Maps each such type name to a representative watched var it mutates.
///
/// This closes the indirect Drop-teardown blind spot behind the #4519 canary
/// exit-101 flake: a guard's methods are `ImplItemFn`, never visited by
/// `FnCollector` (which only visits free `ItemFn`), and the guard's *type* name
/// need not be one of the hard-coded `EnvGuard`/`SkipGuard`/`HermeticState`
/// recognizers. A test that merely constructs such a type is therefore an env
/// mutator that the direct scan alone cannot see. Discovery is file-local, so
/// this stays a pure AST scan with no cross-file coupling (cross-file guard
/// types remain a documented blind spot, like cross-file helpers).
struct GuardTypeCollector<'a> {
    watched: &'a EnvWatch,
    env_mutating_types: BTreeMap<String, String>,
}

impl<'a, 'ast> Visit<'ast> for GuardTypeCollector<'a> {
    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        if let Some(ty) = impl_self_type_name(&node.self_ty) {
            let mut scan = FirstMutationScan {
                watched: self.watched,
                var: None,
            };
            for item in &node.items {
                if let ImplItem::Fn(method) = item {
                    scan.visit_block(&method.block);
                    if scan.var.is_some() {
                        break;
                    }
                }
            }
            if let Some(var) = scan.var {
                self.env_mutating_types.entry(ty).or_insert(var);
            }
        }
        syn::visit::visit_item_impl(self, node);
    }
}

/// Walks a code block for the first *direct* watched env mutation (via
/// [`direct_mutation_var`]). Used to decide whether a guard type's `impl`
/// methods mutate the environment.
struct FirstMutationScan<'a> {
    watched: &'a EnvWatch,
    var: Option<String>,
}

impl<'a, 'ast> Visit<'ast> for FirstMutationScan<'a> {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if self.var.is_none()
            && let Some(v) = direct_mutation_var(node, self.watched)
        {
            self.var = Some(v);
        }
        syn::visit::visit_expr_call(self, node);
    }
}

/// The last path segment of an `impl` block's self type (`impl Drop for T`,
/// `impl T`), unwrapping references / groups / parens. `None` for exotic
/// self types (tuples, trait objects, …), which are never env guards here.
fn impl_self_type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        Type::Reference(r) => impl_self_type_name(&r.elem),
        Type::Group(g) => impl_self_type_name(&g.elem),
        Type::Paren(p) => impl_self_type_name(&p.elem),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A function is a test if any attribute's path ends in `test`
/// (`#[test]`, `#[tokio::test]`, `#[test_log::test]`, …).
fn is_test_fn(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .map(|s| s.ident == "test")
            .unwrap_or(false)
    })
}

/// Collect the named `serial_test::serial(...)` keys across all attributes.
/// Bare `#[serial]` contributes no named key.
fn serial_keys(attrs: &[Attribute]) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for attr in attrs {
        let is_serial = attr
            .path()
            .segments
            .last()
            .map(|s| s.ident == "serial")
            .unwrap_or(false);
        if !is_serial {
            continue;
        }
        if let Meta::List(list) = &attr.meta {
            for tt in list.tokens.clone() {
                if let TokenTree::Ident(id) = tt {
                    keys.insert(id.to_string());
                }
            }
        }
    }
    keys
}

/// Resolve the variable name a `set_var`/`remove_var`/`env::var` argument
/// refers to: a string literal verbatim, or a known env-name constant mapped to
/// its value. Returns `None` for dynamic expressions (conservative — never a
/// false positive).
fn arg_to_var(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        Expr::Reference(r) => arg_to_var(&r.expr),
        Expr::Group(g) => arg_to_var(&g.expr),
        Expr::Path(p) => {
            let last = p.path.segments.last()?.ident.to_string();
            Some(match last.as_str() {
                "STATE_ROOT_ENV" => "SIMARD_STATE_ROOT".to_string(),
                "MEMORY_SOCKET_ENV" => "SIMARD_MEMORY_SOCKET".to_string(),
                "TEST_ALLOW_LIVE_STATE_ENV" => "SIMARD_TEST_ALLOW_LIVE_STATE".to_string(),
                other => other.to_string(),
            })
        }
        _ => None,
    }
}

/// True when a called fn is the cognitive-memory state-root resolver, not a
/// same-named associated fn on an unrelated type. `resolve_state_root` is a
/// single distinctive free fn; `default_state_root` / `simard_state_root` are
/// only treated as the cognitive-memory resolvers when called bare (imported
/// free fn) or via a recognized module path — never as `SomeType::default_state_root`.
fn is_cog_state_root_resolver(last: &str, penult: &str) -> bool {
    match last {
        "resolve_state_root" => {
            penult.is_empty() || matches!(penult, "routes" | "crate" | "self" | "super")
        }
        "default_state_root" | "simard_state_root" => {
            penult.is_empty()
                || matches!(
                    penult,
                    "memory_ipc" | "state_root" | "crate" | "self" | "super"
                )
        }
        _ => false,
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Never descend into build artifacts.
            if path.file_name().map(|n| n == "target").unwrap_or(false) {
                continue;
            }
            collect_rs_files(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// The enforcement test
// ---------------------------------------------------------------------------

/// Fails the build if any hand-written lib-binary test mutates *any*
/// process-global env var, or reads the cognitive-memory state-root environment
/// (or opens cognitive memory at the env-derived default path), without the
/// `cognitive_memory` serial key.
///
/// This meta-test reads source files only; it touches no env and no cognitive
/// memory, so it intentionally carries NO serial key.
#[test]
fn every_env_mutating_test_is_serialized() {
    let offenders = audit_env_mutating_tests(&AuditOptions::default());
    if offenders.is_empty() {
        return;
    }

    let mut report = String::new();
    report.push_str(&format!(
        "serial-guard: {} test(s) mutate a process-global env var (or read the \
cognitive-memory state-root env) without the `{REQUIRED_KEY}` serial key.\n\
Every such test in the lib binary must share that key so env mutation is never \
concurrent with an env read.\n\
See docs/testing/cognitive-memory-serial-isolation.md.\n\n",
        offenders.len()
    ));
    for o in &offenders {
        report.push_str(&format!(
            "  {}:{}  fn {}\n      reason: {}\n      fix:    add #[serial_test::serial({})] \
(or append `{}` to its existing serial keys)\n",
            o.file.display(),
            o.line,
            o.test_name,
            o.reason.describe(),
            REQUIRED_KEY,
            REQUIRED_KEY,
        ));
    }
    panic!("{report}");
}

/// Regression guard for issue
/// [#2375](https://github.com/rysweet/Simard/issues/2375): the production audit
/// must treat *every* process-global env mutation as a race against the
/// cognitive-memory state-root reader (`EnvWatch::AnyVar`), not only the
/// state-root surface — a `setenv` on any variable can `realloc`/free the whole
/// `environ` array while a concurrent `getenv` of `SIMARD_STATE_ROOT`/`HOME` is
/// mid-read. This pins the shipped default and proves an unrelated env writer in
/// another concurrent test "session" is isolated from the cognitive-memory
/// readers only when it shares the `cognitive_memory` serial key.
///
/// Reads in-memory source fixtures only; it mutates no env, so it carries no
/// serial key (and the meta-test above does not flag it).
#[test]
fn anyvar_default_isolates_every_env_writer_across_sessions() {
    // The shipped production policy must stay var-agnostic, so no future edit
    // can silently narrow it back to the state-root surface.
    assert!(
        matches!(AuditOptions::default().watched, EnvWatch::AnyVar),
        "production guard must watch AnyVar so no env writer can race a \
         cognitive-memory state-root read"
    );

    // Two fixtures model a second concurrent session that mutates an *unrelated*
    // (non-state-root) variable: one missing the key, one carrying it.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("fixture.rs"),
        "#[test]\n\
         #[serial]\n\
         fn unrelated_writer_without_key() {\n\
         unsafe { std::env::set_var(\"SOME_UNRELATED_VAR\", \"x\"); }\n\
         }\n\
         #[test]\n\
         #[serial_test::serial(cognitive_memory)]\n\
         fn unrelated_writer_with_key() {\n\
         unsafe { std::env::set_var(\"SOME_UNRELATED_VAR\", \"x\"); }\n\
         }\n",
    )
    .unwrap();

    let opts = AuditOptions {
        roots: vec![dir.path().to_path_buf()],
        excluded_prefixes: Vec::new(),
        watched: EnvWatch::AnyVar,
        allowlist: Vec::new(),
    };
    let flagged: BTreeSet<String> = audit_env_mutating_tests(&opts)
        .into_iter()
        .map(|o| o.test_name)
        .collect();
    assert!(
        flagged.contains("unrelated_writer_without_key"),
        "AnyVar must flag an unrelated env writer lacking the key: {flagged:?}"
    );
    assert!(
        !flagged.contains("unrelated_writer_with_key"),
        "a writer sharing the cognitive_memory key must be accepted: {flagged:?}"
    );

    // Sanity: under the pre-#2375 state-root-only policy the same unrelated
    // writer slipped through — this is precisely the residual class #2375 closes.
    let legacy = AuditOptions {
        watched: EnvWatch::StateRootSurface,
        ..opts.clone()
    };
    let legacy_flagged: BTreeSet<String> = audit_env_mutating_tests(&legacy)
        .into_iter()
        .map(|o| o.test_name)
        .collect();
    assert!(
        !legacy_flagged.contains("unrelated_writer_without_key"),
        "pre-#2375 StateRootSurface policy must ignore unrelated vars: \
         {legacy_flagged:?}"
    );
}

/// Regression guard for the `SkipGuard` env-helper blind spot (the
/// `SIMARD_SKIP_GYM` writers in `src/gym_runner_client.rs`). Their `set_var`/
/// `remove_var` is hidden inside `SkipGuard`'s impl methods, so the direct-call
/// scan never sees it; before this recognizer a caller could drop back to the
/// bare `#[serial]` (an independent lock) and silently re-introduce the
/// environ-realloc race that tore `cost_tracking::ledger_path`'s `HOME` read in
/// the meeting-turn cost regression. The `SkipGuard::set`/`clear` recognizer
/// makes that an auto-caught build failure, not an author obligation.
///
/// Reads in-memory source fixtures only; it mutates no env, so it carries no
/// serial key.
#[test]
fn skip_guard_helper_mutation_requires_the_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("fixture.rs"),
        "#[test]\n\
         #[serial]\n\
         fn skip_guard_without_key() {\n\
         let _g = SkipGuard::set(\"1\");\n\
         }\n\
         #[test]\n\
         #[serial]\n\
         fn skip_guard_clear_without_key() {\n\
         let _g = SkipGuard::clear();\n\
         }\n\
         #[test]\n\
         #[serial_test::serial(cognitive_memory)]\n\
         fn skip_guard_with_key() {\n\
         let _g = SkipGuard::set(\"1\");\n\
         }\n",
    )
    .unwrap();

    let opts = AuditOptions {
        roots: vec![dir.path().to_path_buf()],
        excluded_prefixes: Vec::new(),
        watched: EnvWatch::AnyVar,
        allowlist: Vec::new(),
    };
    let flagged: BTreeSet<String> = audit_env_mutating_tests(&opts)
        .into_iter()
        .map(|o| o.test_name)
        .collect();
    assert!(
        flagged.contains("skip_guard_without_key"),
        "SkipGuard::set without the cognitive_memory key must be flagged: {flagged:?}"
    );
    assert!(
        flagged.contains("skip_guard_clear_without_key"),
        "SkipGuard::clear without the cognitive_memory key must be flagged: {flagged:?}"
    );
    assert!(
        !flagged.contains("skip_guard_with_key"),
        "a SkipGuard writer sharing the cognitive_memory key must be accepted: {flagged:?}"
    );
}

// ---------------------------------------------------------------------------
// FAILING TDD SPEC — the indirect **Drop-teardown** blind spot
// (self-deploy canary exit-101 flake, issue #4519 /
//  docs/testing/canary-gate-drop-test-determinism.md).
//
// The canary-only exit-101 came from a test whose *only* env mutation lives
// inside a custom RAII guard's `Drop` (the guard restores/removes
// `SIMARD_STATE_ROOT` when it falls out of scope). Under the scrubbed canary
// env that teardown `set_var`/`remove_var` ran concurrently with a sibling
// test's env read and tore it → panic → exit 101 → RED canary.
//
// The audit is blind to this because `FnCollector::visit_item_fn` only visits
// FREE functions (`ItemFn`). A guard's `new`/`drop` are `impl` methods
// (`ImplItemFn`), never visited, and the guard's TYPE name is not one of the
// hard-coded `EnvGuard` / `SkipGuard` / `HermeticState` recognizers — so the
// mutation is invisible and the constructing test is not flagged.
//
// These two tests pin the strengthened contract:
//   * A test constructing an env-mutating-`Drop` guard type WITHOUT the
//     `cognitive_memory` key must be flagged (fails today; passes once the
//     audit learns the pattern).
//   * A benign guard whose `Drop` touches no watched env var must NOT be
//     flagged — the strengthening keeps the doc's "zero false positives"
//     guarantee and must not degrade into "flag every guard construction".
//
// Like every meta-test here, the fixtures are written to a TempDir and are
// never real `#[test]`s, so the real-tree audit is unaffected and this module
// still mutates no process-global env (hence no serial key).
// ---------------------------------------------------------------------------

/// A guard type whose `Drop` restores/removes `SIMARD_STATE_ROOT` is an env
/// mutator even though its constructing test never calls `set_var` directly:
/// the mutation lives in the guard's `impl` methods, and the teardown
/// `set_var`/`remove_var` in `Drop` is what raced the reader under the canary.
/// The audit must therefore flag a `#[test]` that constructs such a guard
/// unless it carries the `cognitive_memory` key — regardless of the guard's
/// type name (it is NOT one of the hard-coded helper names).
///
/// Reads in-memory source fixtures only; mutates no env, so it carries no
/// serial key.
#[test]
fn drop_teardown_guard_mutation_requires_the_key() {
    let dir = tempfile::tempdir().unwrap();
    // A hand-rolled restore-on-Drop env guard (the exact shape of the local
    // `EnvGuard` in tests_hermetic_state.rs, but under a DIFFERENT, unrecognized
    // type name so only Drop/impl analysis — not a name allowlist — can catch
    // it). `set_guard_without_key` mutates SIMARD_STATE_ROOT purely through the
    // guard's constructor + `Drop`; `remove_guard_without_key` exercises the
    // `remove_var` teardown path; the `_with_key` sibling is correctly keyed.
    std::fs::write(
        dir.path().join("fixture.rs"),
        r#"
struct StateRootGuard {
    prev: Option<std::ffi::OsString>,
}

impl StateRootGuard {
    fn set(value: &str) -> Self {
        let prev = std::env::var_os("SIMARD_STATE_ROOT");
        unsafe {
            std::env::set_var("SIMARD_STATE_ROOT", value);
        }
        Self { prev }
    }

    fn cleared() -> Self {
        let prev = std::env::var_os("SIMARD_STATE_ROOT");
        unsafe {
            std::env::remove_var("SIMARD_STATE_ROOT");
        }
        Self { prev }
    }
}

impl Drop for StateRootGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
                None => std::env::remove_var("SIMARD_STATE_ROOT"),
            }
        }
    }
}

#[test]
#[serial]
fn set_guard_without_key() {
    let _g = StateRootGuard::set("/tmp/hermetic");
}

#[test]
#[serial]
fn remove_guard_without_key() {
    let _g = StateRootGuard::cleared();
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn set_guard_with_key() {
    let _g = StateRootGuard::set("/tmp/hermetic");
}
"#,
    )
    .unwrap();

    let opts = AuditOptions {
        roots: vec![dir.path().to_path_buf()],
        excluded_prefixes: Vec::new(),
        watched: EnvWatch::AnyVar,
        allowlist: Vec::new(),
    };
    let flagged: BTreeSet<String> = audit_env_mutating_tests(&opts)
        .into_iter()
        .map(|o| o.test_name)
        .collect();

    assert!(
        flagged.contains("set_guard_without_key"),
        "a test whose only env mutation is a custom guard's Drop-teardown \
         (set_var restore) must be flagged without the cognitive_memory key — \
         this is the indirect Drop-teardown blind spot behind the #4519 canary \
         exit-101 flake: {flagged:?}"
    );
    assert!(
        flagged.contains("remove_guard_without_key"),
        "the remove_var teardown path of a custom Drop guard must also be \
         flagged without the key: {flagged:?}"
    );
    assert!(
        !flagged.contains("set_guard_with_key"),
        "a test constructing the same env-mutating guard while sharing the \
         cognitive_memory key must be accepted (no false positive): {flagged:?}"
    );
}

/// The Drop-teardown strengthening must not degrade into flagging *every* RAII
/// guard construction. A guard whose `Drop` performs no watched env mutation
/// (here it only touches a struct field) is hermetic and must never be
/// reported — this pins the doc's "zero false positives" guarantee against an
/// over-broad fix.
///
/// Reads in-memory source fixtures only; mutates no env, so it carries no
/// serial key.
#[test]
fn drop_teardown_detection_has_zero_false_positives() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("fixture.rs"),
        r#"
struct Spinner {
    ticks: u32,
}

impl Spinner {
    fn new() -> Self {
        Self { ticks: 0 }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        // No env mutation whatsoever — a purely in-process guard.
        self.ticks = self.ticks.wrapping_add(1);
    }
}

#[test]
#[serial]
fn benign_guard_without_key() {
    let mut s = Spinner::new();
    s.ticks += 1;
    let _ = s.ticks;
}
"#,
    )
    .unwrap();

    let opts = AuditOptions {
        roots: vec![dir.path().to_path_buf()],
        excluded_prefixes: Vec::new(),
        watched: EnvWatch::AnyVar,
        allowlist: Vec::new(),
    };
    let flagged: BTreeSet<String> = audit_env_mutating_tests(&opts)
        .into_iter()
        .map(|o| o.test_name)
        .collect();

    assert!(
        flagged.is_empty(),
        "a guard whose Drop performs no watched env mutation must never be \
         flagged; the Drop-teardown strengthening must stay a zero-false-positive \
         static scan: {flagged:?}"
    );
}
