//! Operator subcommand `simard memory <stats|dump>` — read-only introspection
//! of the six-type cognitive-memory store on the library backend (issue #2308
//! follow-up).
//!
//! Both commands open the store through [`open_reader_client`], the canonical
//! read-only consumer entry point: when the OODA daemon is up they route
//! through its memory socket (no lock contention); when it is down they fall
//! back to a direct open of the on-disk store. Neither command mutates memory.
//!
//! See `docs/reference/simard-memory-cli.md` for the operator reference.

use std::path::{Path, PathBuf};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::memory_cognitive::{CognitiveStatistics, GraphStats};
use crate::memory_ipc::{RemoteCognitiveMemory, open_reader_client, socket_path_for};

pub(super) const MEMORY_HELP: &str = "\
Simard memory subcommand — cognitive-memory introspection & restore

Usage:
  simard memory stats  [state-root] [--json]
  simard memory dump   [state-root] [--type=TYPE] [--limit=N] [--json]
  simard memory import <snapshot.json> [state-root]

stats  Print per-type counts (sensory, working, episodic, semantic/facts,
       procedural/procedures, prospective/triggers), a graph-edge / dedup
       (\"edges / connections\") section, plus a few sample rows.
dump   Print counts plus a larger set of sample rows per type for eyeballing
       content. --type restricts to one of: facts, episodes, procedures.
import Ingest a cognitive_snapshot.json (as written by the periodic verified
       backup under ~/.simard/backups/<ts>/) back into the store. Idempotent:
       memories already present are skipped (dedup by content), so re-running a
       restore never duplicates. Run with the OODA daemon stopped so the import
       writes to the same store the daemon serves.

stats/dump are read-only and safe to run while the OODA daemon holds the store:
they route through the daemon socket when it is up and fall back to a direct
open when it is down. With no [state-root] they resolve $SIMARD_STATE_ROOT, then
$HOME/.simard.
";

/// Default number of sample rows per type for `stats`.
const STATS_SAMPLE_LIMIT: usize = 3;
/// Default number of sample rows per type for `dump`.
const DUMP_SAMPLE_LIMIT: usize = 10;

/// Best-effort sample rows per type. Counts come from `get_statistics`; these
/// rows are an eyeballing aid only and may legitimately be empty even when the
/// corresponding count is non-zero (the enumerators are keyword/CONTAINS
/// probes, and some tiers expose no neutral enumerator).
#[derive(Debug, Clone, Default)]
struct MemorySamples {
    facts: Vec<String>,
    episodes: Vec<String>,
    procedures: Vec<String>,
    /// Note printed when episode rows could not be neutrally enumerated over
    /// the active access tier (e.g. the daemon socket exposes no `get_episodes`).
    episodes_note: Option<String>,
}

/// Access tier that served a read, for the banner and JSON. The CLI is a
/// separate process from the daemon, so it only ever observes the daemon
/// socket (tier 1) or a direct on-disk open (tier 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessTier {
    DaemonSocket,
    DirectOpen,
}

impl AccessTier {
    /// Human banner phrase (`via <human>`).
    fn human(self) -> &'static str {
        match self {
            Self::DaemonSocket => "daemon socket",
            Self::DirectOpen => "direct open",
        }
    }
    /// Stable machine token for `--json`.
    fn token(self) -> &'static str {
        match self {
            Self::DaemonSocket => "daemon-socket",
            Self::DirectOpen => "direct-open",
        }
    }
}

/// A fully-collected introspection report for one store.
#[derive(Debug, Clone)]
struct MemoryReport {
    state_root: PathBuf,
    store_path: PathBuf,
    access_tier: AccessTier,
    counts: CognitiveStatistics,
    samples: MemorySamples,
    /// Graph-edge / dedup connection counts (issue #2331). Zeroed when they
    /// could not be computed over the active tier — see [`Self::graph_note`].
    graph: GraphStats,
    /// Set when the edge counts could not be computed (e.g. over the daemon
    /// IPC socket, which exposes no graph reader). When present, the edges
    /// section prints this note instead of the (meaningless) zero counts.
    graph_note: Option<String>,
}

/// Note shown when graph stats cannot be computed over the daemon IPC socket.
/// The socket-backed `RemoteCognitiveMemory` has no graph reader (its
/// `graph_stats` is the all-zero trait default, indistinguishable from a truly
/// empty graph), so over IPC we surface this rather than misreport zeros.
const DAEMON_GRAPH_NOTE: &str = "(edges: run with daemon stopped for graph stats)";

/// Which type(s) a `dump` should sample. `Working` and `Sensory` are
/// accepted but yield no sample rows — neither has a read-only enumerator, so
/// they are count-only (the count table is always printed in full regardless).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DumpType {
    All,
    Facts,
    Episodes,
    Procedures,
    Working,
    Sensory,
}

impl DumpType {
    fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match s {
            "facts" => Ok(Self::Facts),
            "episodes" => Ok(Self::Episodes),
            "procedures" => Ok(Self::Procedures),
            "working" => Ok(Self::Working),
            "sensory" => Ok(Self::Sensory),
            other => Err(format!(
                "unknown --type '{other}' \
                 (expected facts, episodes, procedures, working, or sensory)"
            )
            .into()),
        }
    }

    fn wants_facts(self) -> bool {
        matches!(self, Self::All | Self::Facts)
    }
    fn wants_episodes(self) -> bool {
        matches!(self, Self::All | Self::Episodes)
    }
    fn wants_procedures(self) -> bool {
        matches!(self, Self::All | Self::Procedures)
    }
}

pub(crate) fn dispatch_memory_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = match args.next() {
        Some(s) => s,
        None => {
            print!("{MEMORY_HELP}");
            return Ok(());
        }
    };

    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{MEMORY_HELP}");
            Ok(())
        }
        "stats" => run_stats(args),
        "dump" => run_dump(args),
        "import" => run_import(args),
        "remember" => {
            // The distiller's per-fact WRITE tool (issue #2679). `run_remember_fact`
            // returns a precise exit code (0 stored / 2 usage / 3 no-daemon /
            // 4 quarantined); honour it by exiting the process directly so a
            // mis-invoking or blocked agent is diagnosable. `--help` short-circuits
            // before any daemon contact.
            let argv: Vec<String> = args.collect();
            if argv
                .iter()
                .any(|a| a == "--help" || a == "-h" || a == "help")
            {
                print!("{REMEMBER_HELP}");
                return Ok(());
            }
            std::process::exit(run_remember_fact(argv));
        }
        "remember-procedure" => {
            let argv: Vec<String> = args.collect();
            if argv
                .iter()
                .any(|a| a == "--help" || a == "-h" || a == "help")
            {
                print!("{REMEMBER_PROCEDURE_HELP}");
                return Ok(());
            }
            std::process::exit(run_remember_procedure(argv));
        }
        other => Err(format!("unsupported command 'memory {other}'").into()),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// `simard memory remember` — the distiller's per-fact WRITE tool (issue #2679)
// ───────────────────────────────────────────────────────────────────────────

const REMEMBER_HELP: &str = "\
Simard memory remember — write ONE semantic fact into cognitive memory.

Usage:
  simard memory remember --concept <LABEL> --content <TEXT>
        [--source-episode-id <ID> ...] [--confidence <0..1>]
        [--tags <a,b,c>] [--pass-id <ID>] [state-root]

One process writes exactly one fact. There is no batch/array/JSON-body form —
that is the point of #2679: no Simard-side document is ever deserialized. Emit N
facts with N calls.

The write routes ONLY through the OODA daemon's memory socket, where the single
authoritative write-boundary gate grounds, scores, quarantines, and dedups the
fact server-side. The client-supplied --confidence is a hint the server ignores;
the server re-derives confidence from provenance grounding + content + concept.

Exit codes:
  0  stored          the fact cleared the gate and was persisted
  2  usage error     a required flag was missing or malformed
  3  no daemon       no reachable memory daemon (no un-gated fallback exists)
  4  quarantined     the gate blocked the fact (ungrounded/empty/below threshold)
";

const REMEMBER_PROCEDURE_HELP: &str = "\
Simard memory remember-procedure — write ONE procedure into cognitive memory.

Usage:
  simard memory remember-procedure --name <NAME> --step <TEXT> [--step <TEXT> ...]
        [--prerequisite <TEXT> ...] [--source-episode-id <ID> ...]
        [--pass-id <ID>] [state-root]

Exit codes: 0 stored, 2 usage error, 3 no reachable daemon.
";

/// Parsed `simard memory remember` invocation (issue #2679). Scalar flags only —
/// one process writes one fact, so there is no envelope to deserialize.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RememberFactArgs {
    pub concept: String,
    pub content: String,
    pub source_episode_ids: Vec<String>,
    /// A hint only; the server re-derives the stored confidence.
    pub confidence: Option<f64>,
    pub tags: Vec<String>,
    pub pass_id: Option<String>,
    pub state_root: Option<PathBuf>,
}

/// Take a flag's value: inline (`--flag=value`) if present, else the next argv
/// token (`--flag value`). Errors if neither is available.
fn flag_value(
    flag: &str,
    inline: Option<String>,
    next: &mut dyn Iterator<Item = String>,
) -> Result<String, String> {
    match inline {
        Some(v) => Ok(v),
        None => next
            .next()
            .ok_or_else(|| format!("--{flag} requires a value")),
    }
}

/// Parse `simard memory remember` argv into typed [`RememberFactArgs`], packing
/// scalar flags straight into fields the CLI hands to a typed IPC request. No
/// free text is ever re-parsed as JSON.
pub(crate) fn parse_remember_fact_args(args: Vec<String>) -> Result<RememberFactArgs, String> {
    let mut concept: Option<String> = None;
    let mut content: Option<String> = None;
    let mut source_episode_ids: Vec<String> = Vec::new();
    let mut confidence: Option<f64> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut pass_id: Option<String> = None;
    let mut state_root: Option<PathBuf> = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix("--") {
            let (key, inline) = match rest.split_once('=') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (rest.to_string(), None),
            };
            match key.as_str() {
                "concept" => concept = Some(flag_value("concept", inline, &mut iter)?),
                "content" => content = Some(flag_value("content", inline, &mut iter)?),
                "source-episode-id" => {
                    source_episode_ids.push(flag_value("source-episode-id", inline, &mut iter)?)
                }
                "confidence" => {
                    let v = flag_value("confidence", inline, &mut iter)?;
                    let parsed = v.parse::<f64>().map_err(|_| {
                        format!("--confidence must be a number in [0,1], got {v:?}")
                    })?;
                    confidence = Some(parsed);
                }
                "tags" => {
                    let v = flag_value("tags", inline, &mut iter)?;
                    tags.extend(
                        v.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                    );
                }
                "pass-id" => pass_id = Some(flag_value("pass-id", inline, &mut iter)?),
                other => return Err(format!("unknown flag --{other}")),
            }
        } else if state_root.is_none() {
            state_root = Some(PathBuf::from(arg));
        } else {
            return Err(format!("unexpected extra positional argument '{arg}'"));
        }
    }

    let concept = concept.ok_or_else(|| "missing required --concept".to_string())?;
    let content = content.ok_or_else(|| "missing required --content".to_string())?;
    Ok(RememberFactArgs {
        concept,
        content,
        source_episode_ids,
        confidence,
        tags,
        pass_id,
        state_root,
    })
}

/// Run `simard memory remember`, returning the process exit code (0 stored,
/// 2 usage error, 3 no reachable daemon, 4 quarantined). Routes ONLY through the
/// daemon socket: a direct on-disk open would bypass the authoritative gate, so
/// no daemon means no gated write path.
pub(crate) fn run_remember_fact(args: Vec<String>) -> i32 {
    let parsed = match parse_remember_fact_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[simard] memory remember: {e}");
            return 2;
        }
    };

    let state_root = resolve_state_root(parsed.state_root.clone());
    let sock = socket_path_for(&state_root);
    let client = match RemoteCognitiveMemory::connect(&sock) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[simard] memory remember: no reachable memory daemon at {} ({e}); \
                 not writing un-gated",
                sock.display()
            );
            return 3;
        }
    };

    // A stable provenance label for the fact's `source_id`; the gate verifies
    // the episode ids separately for grounding.
    let source_id = match parsed.source_episode_ids.first() {
        Some(id) => format!("distill:{id}"),
        None => "distill".to_string(),
    };
    let pass_id = resolve_pass_id(parsed.pass_id.as_deref());
    // The confidence is a hint the server ignores; pass 0.0 when unset.
    let confidence_hint = parsed.confidence.unwrap_or(0.0);

    match client.remember_fact_gated(
        &parsed.concept,
        &parsed.content,
        confidence_hint,
        &parsed.tags,
        &source_id,
        &parsed.source_episode_ids,
        &pass_id,
    ) {
        Ok(outcome) if outcome.stored => {
            println!(
                "[simard] memory remember: stored concept={} confidence={:.2}{}",
                parsed.concept,
                outcome.confidence,
                outcome
                    .node_id
                    .as_deref()
                    .map(|id| format!(" node_id={id}"))
                    .unwrap_or_default()
            );
            0
        }
        Ok(outcome) => {
            eprintln!(
                "[simard] memory remember: quarantined concept={} confidence={:.2} (below gate)",
                parsed.concept, outcome.confidence
            );
            4
        }
        Err(e) => {
            eprintln!("[simard] memory remember: gated write failed: {e}");
            3
        }
    }
}

/// Run `simard memory remember-procedure`, returning the process exit code
/// (0 stored, 2 usage error, 3 no reachable daemon).
pub(crate) fn run_remember_procedure(args: Vec<String>) -> i32 {
    let mut name: Option<String> = None;
    let mut steps: Vec<String> = Vec::new();
    let mut prerequisites: Vec<String> = Vec::new();
    let mut source_episode_ids: Vec<String> = Vec::new();
    let mut pass_id: Option<String> = None;
    let mut state_root: Option<PathBuf> = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix("--") {
            let (key, inline) = match rest.split_once('=') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (rest.to_string(), None),
            };
            let res = match key.as_str() {
                "name" => flag_value("name", inline, &mut iter).map(|v| name = Some(v)),
                "step" => flag_value("step", inline, &mut iter).map(|v| steps.push(v)),
                "prerequisite" => {
                    flag_value("prerequisite", inline, &mut iter).map(|v| prerequisites.push(v))
                }
                "source-episode-id" => flag_value("source-episode-id", inline, &mut iter)
                    .map(|v| source_episode_ids.push(v)),
                "pass-id" => flag_value("pass-id", inline, &mut iter).map(|v| pass_id = Some(v)),
                other => Err(format!("unknown flag --{other}")),
            };
            if let Err(e) = res {
                eprintln!("[simard] memory remember-procedure: {e}");
                return 2;
            }
        } else if state_root.is_none() {
            state_root = Some(PathBuf::from(arg));
        } else {
            eprintln!("[simard] memory remember-procedure: unexpected extra argument '{arg}'");
            return 2;
        }
    }

    let name = match name {
        Some(n) => n,
        None => {
            eprintln!("[simard] memory remember-procedure: missing required --name");
            return 2;
        }
    };
    if steps.is_empty() {
        eprintln!("[simard] memory remember-procedure: at least one --step is required");
        return 2;
    }

    let state_root = resolve_state_root(state_root);
    let sock = socket_path_for(&state_root);
    let client = match RemoteCognitiveMemory::connect(&sock) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[simard] memory remember-procedure: no reachable memory daemon at {} ({e})",
                sock.display()
            );
            return 3;
        }
    };
    let pass_id = resolve_pass_id(pass_id.as_deref());
    match client.remember_procedure_provenance(
        &name,
        &steps,
        &prerequisites,
        &source_episode_ids,
        &pass_id,
    ) {
        Ok(id) => {
            println!("[simard] memory remember-procedure: stored name={name} node_id={id}");
            0
        }
        Err(e) => {
            eprintln!("[simard] memory remember-procedure: write failed: {e}");
            3
        }
    }
}

/// Resolve the state root: explicit argument wins, else the shared resolver
/// the daemon uses (`$SIMARD_STATE_ROOT`, then `$HOME/.simard`).
fn resolve_state_root(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(crate::state_root::simard_state_root)
}

/// Resolve the distillation pass id for a `remember` / `remember-procedure`
/// write (issue #2679).
///
/// Precedence: a non-empty explicit `--pass-id` flag wins; otherwise fall back
/// to the [`DISTILL_PASS_ID_ENV`] environment variable the distill runner
/// exports to every remember subprocess. An empty result (neither source set)
/// means "no ledger participation" — the server's ledger deliberately no-ops.
///
/// The env fallback is the fix for the silent metrics-degradation regression:
/// the distiller agent runs `simard memory remember` with only the content
/// flags and no `--pass-id`, so without this fallback the pass id resolved
/// empty, the server ledger dropped the write, and `drain_pass_ledger` returned
/// 0 — making every distill pass report `fact_count = 0` / `reduction_pct =
/// 100%` even though facts were stored. Only tests (which pass `--pass-id`
/// explicitly) exercised the ledger, hiding the production breakage.
fn resolve_pass_id(explicit: Option<&str>) -> String {
    if let Some(p) = explicit
        && !p.is_empty()
    {
        return p.to_string();
    }
    std::env::var(crate::memory_ipc::DISTILL_PASS_ID_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Best-effort access-tier label for the banner. The daemon socket is
/// authoritative when present; otherwise the read is a direct on-disk open.
fn access_tier_for(state_root: &Path) -> AccessTier {
    if socket_path_for(state_root).exists() {
        AccessTier::DaemonSocket
    } else {
        AccessTier::DirectOpen
    }
}

fn run_stats(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let (state_root_opt, json) = super::args::parse_state_root_and_json(args.collect())?;
    let state_root = resolve_state_root(state_root_opt);
    let tier = access_tier_for(&state_root);

    let reader = open_reader_client(&state_root)?;
    let report = build_report(
        reader.ops(),
        &state_root,
        tier,
        STATS_SAMPLE_LIMIT,
        DumpType::All,
    )?;

    if json {
        println!("{}", render_json(&report, false));
    } else {
        print!("{}", render_human(&report, false));
    }
    Ok(())
}

/// `simard memory import <snapshot.json> [state-root] [--json]` — restore a
/// `cognitive_snapshot.json` back into the store (issue #2550).
///
/// Idempotent: [`crate::remote_transfer::import_full_snapshot`] dedups by
/// content, so re-running an import never duplicates memories. Opens the store
/// **for write** via a direct on-disk open, which requires the daemon to be
/// stopped (it holds the store lock while running) — enforced below with a clear
/// error rather than a lock-contention failure.
fn run_import(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut snapshot_arg: Option<PathBuf> = None;
    let mut state_root_opt: Option<PathBuf> = None;
    let mut json = false;
    for arg in args {
        if arg == "--help" || arg == "-h" {
            print!("{MEMORY_HELP}");
            return Ok(());
        } else if arg == "--json" {
            json = true;
        } else if arg.starts_with("--") {
            return Err(format!("unexpected flag: {arg}").into());
        } else if snapshot_arg.is_none() {
            snapshot_arg = Some(PathBuf::from(arg));
        } else if state_root_opt.is_none() {
            state_root_opt = Some(PathBuf::from(arg));
        } else {
            return Err(format!("unexpected argument: {arg}").into());
        }
    }

    let snapshot_path = snapshot_arg.ok_or_else(|| {
        Box::<dyn std::error::Error>::from(
            "usage: simard memory import <snapshot.json> [state-root] [--json]",
        )
    })?;
    let state_root = resolve_state_root(state_root_opt);

    // A direct write open cannot coexist with the daemon's store lock. Fail
    // loudly and actionably instead of blocking on / corrupting a live store.
    if socket_path_for(&state_root).exists() {
        return Err(format!(
            "the OODA daemon appears to be running for {} (socket present); \
             stop it before importing so the restore writes to the store the \
             daemon serves",
            state_root.display()
        )
        .into());
    }

    let snapshot = crate::remote_transfer::load_full_snapshot_from_file(&snapshot_path)?;
    let items = snapshot.total_items();
    let memory = crate::cognitive_memory::LibraryCognitiveMemory::open(&state_root)?;
    let new = crate::remote_transfer::import_full_snapshot(&memory, &snapshot)?;
    // Fold the writes into the main DB so a subsequent reader (e.g. `memory
    // stats`) sees them without needing a WAL replay.
    memory.checkpoint()?;
    let deduplicated = items.saturating_sub(new);
    let store_path = state_root.join(crate::cognitive_memory::LIVE_STORE_SUBDIR);

    if json {
        println!(
            "{{\"imported\":{items},\"new\":{new},\"deduplicated\":{deduplicated},\"store\":{}}}",
            serde_json::Value::String(store_path.display().to_string())
        );
    } else {
        println!(
            "imported {items} items ({new} new, {deduplicated} deduplicated) -> {}",
            store_path.display()
        );
    }
    Ok(())
}

fn run_dump(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut state_root_opt: Option<PathBuf> = None;
    let mut dump_type = DumpType::All;
    let mut limit = DUMP_SAMPLE_LIMIT;
    let mut json = false;

    for arg in args {
        if arg == "--json" {
            json = true;
        } else if let Some(t) = arg.strip_prefix("--type=") {
            dump_type = DumpType::parse(t)?;
        } else if let Some(n) = arg.strip_prefix("--limit=") {
            limit = n
                .parse()
                .map_err(|_| format!("invalid --limit value: {n}"))?;
        } else if arg.starts_with("--") {
            return Err(format!("unexpected flag: {arg}").into());
        } else if state_root_opt.is_none() {
            state_root_opt = Some(PathBuf::from(arg));
        } else {
            return Err(format!("unexpected argument: {arg}").into());
        }
    }

    let state_root = resolve_state_root(state_root_opt);
    let tier = access_tier_for(&state_root);

    let reader = open_reader_client(&state_root)?;
    let report = build_report(reader.ops(), &state_root, tier, limit, dump_type)?;

    if json {
        println!("{}", render_json(&report, true));
    } else {
        print!("{}", render_human(&report, true));
    }
    Ok(())
}

/// Collect counts (authoritative, from `get_statistics`) plus best-effort
/// sample rows for the requested types.
fn build_report(
    ops: &dyn CognitiveMemoryOps,
    state_root: &Path,
    access_tier: AccessTier,
    sample_limit: usize,
    dump_type: DumpType,
) -> SimardResult<MemoryReport> {
    let counts = ops.get_statistics()?;
    let samples = collect_samples(
        ops,
        sample_limit,
        dump_type,
        access_tier,
        counts.episodic_count,
    );
    // Graph-edge / dedup stats (issue #2331). Gate on the access tier: over the
    // daemon socket the IPC client has no graph reader (its `graph_stats` is the
    // all-zero default), so we show a note rather than misreporting zeros. On a
    // direct on-disk open we compute the real counts — but never fail the whole
    // report if that read errors (the count table is the primary payload).
    let (graph, graph_note) = match access_tier {
        AccessTier::DaemonSocket => (GraphStats::default(), Some(DAEMON_GRAPH_NOTE.to_string())),
        AccessTier::DirectOpen => match ops.graph_stats() {
            Ok(g) => (g, None),
            Err(_) => (
                GraphStats::default(),
                Some("(edges: graph stats unavailable)".to_string()),
            ),
        },
    };
    Ok(MemoryReport {
        state_root: state_root.to_path_buf(),
        store_path: state_root.join("cognitive"),
        access_tier,
        counts,
        samples,
        graph,
        graph_note,
    })
}

/// One-line, length-capped rendering of a multi-line content blob.
fn one_line(content: &str, max: usize) -> String {
    let flat = content.replace(['\n', '\r'], " ");
    let flat = flat.trim();
    if flat.chars().count() <= max {
        flat.to_string()
    } else {
        let truncated: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

/// Best-effort sampling over whatever read-only enumerators the active tier
/// supports. Never fails the whole report: a probe error degrades to an empty
/// (or noted) sample list for that type.
fn collect_samples(
    ops: &dyn CognitiveMemoryOps,
    limit: usize,
    dump_type: DumpType,
    access_tier: AccessTier,
    episodic_count: u64,
) -> MemorySamples {
    let mut samples = MemorySamples::default();
    let cap = limit.max(1) as u32;

    if dump_type.wants_facts() {
        // `search_facts("*", …)` maps to the library's "return all" path.
        if let Ok(facts) = ops.search_facts("*", cap, 0.0) {
            samples.facts = facts
                .into_iter()
                .take(limit)
                .map(|f| format!("{}: {}", f.concept, one_line(&f.content, 80)))
                .collect();
        }
    }

    if dump_type.wants_procedures() {
        // `recall_procedure("*", …)` returns all procedures (truncated).
        if let Ok(procs) = ops.recall_procedure("*", cap) {
            samples.procedures = procs.into_iter().take(limit).map(|p| p.name).collect();
        }
    }

    if dump_type.wants_episodes() {
        // `list_undistilled_episodes` is the neutral enumerator: it is
        // implemented by the direct-open library backend but defaults to empty
        // over the daemon socket (`RemoteCognitiveMemory` does not expose it).
        match ops.list_undistilled_episodes(cap) {
            Ok(eps) if !eps.is_empty() => {
                samples.episodes = eps
                    .into_iter()
                    .take(limit)
                    .map(|e| one_line(&e.content, 80))
                    .collect();
            }
            // Empty rows but a non-zero count means the rows exist yet could not
            // be neutrally enumerated on this tier (socket) or are all already
            // distilled (direct open). A zero count needs no note at all.
            Ok(_) if episodic_count > 0 => {
                samples.episodes_note = Some(match access_tier {
                    AccessTier::DaemonSocket => {
                        "(samples unavailable over IPC — run with the daemon stopped \
                         for direct-open rows)"
                            .to_string()
                    }
                    AccessTier::DirectOpen => {
                        "(no undistilled episodes to sample; stored episodes may already \
                         be distilled)"
                            .to_string()
                    }
                });
            }
            Ok(_) => {}
            Err(_) => {
                samples.episodes_note = Some("(samples unavailable over IPC)".to_string());
            }
        }
    }

    samples
}

/// The "edges / connections" section (issue #2331): provenance + similarity +
/// supersedes edge counts, fact-provenance coverage, and snapshot dedup. When
/// the counts could not be computed for the active tier the section prints the
/// report's `graph_note` instead of zeroed counts.
fn render_edges_section(report: &MemoryReport) -> String {
    let mut out = String::from("\nedges / connections:\n");
    if let Some(note) = &report.graph_note {
        out.push_str(&format!("  {note}\n"));
        return out;
    }
    let g = &report.graph;
    out.push_str(&format!(
        "  DERIVES_FROM            {:>7}     (fact -> episode)\n",
        g.derives_from_edges
    ));
    out.push_str(&format!(
        "  PROCEDURE_DERIVES_FROM  {:>7}     (procedure -> episode)\n",
        g.procedure_derives_from_edges
    ));
    out.push_str(&format!(
        "  SIMILAR_TO              {:>7}     (fact <-> fact)\n",
        g.similar_to_edges
    ));
    out.push_str(&format!(
        "  SUPERSEDES              {:>7}     (deduped snapshot)\n",
        g.supersedes_edges
    ));
    out.push_str(&format!(
        "  facts with provenance:  {} / {}\n",
        g.facts_with_provenance, g.facts_total
    ));
    out.push_str(&format!(
        "  snapshot dedup:         {} distinct caller keys / {} snapshot facts\n",
        g.distinct_snapshot_caller_keys, g.snapshot_facts_total
    ));
    out
}

/// Human-readable counts table (+ samples when `include_samples`).
fn render_human(report: &MemoryReport, include_samples: bool) -> String {
    let c = &report.counts;
    let mut out = String::new();
    out.push_str(&format!(
        "cognitive memory @ {}  (via {})\n\n",
        report.store_path.display(),
        report.access_tier.human(),
    ));
    out.push_str("  TYPE          COUNT\n");
    out.push_str(&format!("  sensory       {:>7}\n", c.sensory_count));
    out.push_str(&format!("  working       {:>7}\n", c.working_count));
    out.push_str(&format!("  episodic      {:>7}\n", c.episodic_count));
    out.push_str(&format!(
        "  semantic      {:>7}     (facts)\n",
        c.semantic_count
    ));
    out.push_str(&format!(
        "  procedural    {:>7}     (procedures)\n",
        c.procedural_count
    ));
    out.push_str(&format!(
        "  prospective   {:>7}     (triggers)\n",
        c.prospective_count
    ));
    out.push_str("  ---------------------\n");
    out.push_str(&format!("  total         {:>7}\n", c.total()));

    out.push_str(&render_edges_section(report));

    if include_samples || has_any_sample(&report.samples) {
        out.push_str("\nsamples (best-effort):\n");
        for row in &report.samples.facts {
            out.push_str(&format!("  facts:        {row}\n"));
        }
        for row in &report.samples.episodes {
            out.push_str(&format!("  episodes:     {row}\n"));
        }
        if let Some(note) = &report.samples.episodes_note {
            out.push_str(&format!("  episodes:     {note}\n"));
        }
        for row in &report.samples.procedures {
            out.push_str(&format!("  procedures:   {row}\n"));
        }
        // Triggers are never row-sampled: the only listing method
        // (`check_triggers`) mutates matches to "triggered" (fire-once) and
        // would consume live goal triggers.
        out.push_str("  triggers:     (count only — see prospective above)\n");
    }

    out
}

fn has_any_sample(s: &MemorySamples) -> bool {
    !s.facts.is_empty()
        || !s.episodes.is_empty()
        || !s.procedures.is_empty()
        || s.episodes_note.is_some()
}

/// JSON rendering with stable scripting keys.
fn render_json(report: &MemoryReport, include_samples: bool) -> String {
    let c = &report.counts;
    let mut value = serde_json::json!({
        "state_root": report.state_root.display().to_string(),
        "store_path": report.store_path.display().to_string(),
        "access_tier": report.access_tier.token(),
        "counts": {
            "sensory": c.sensory_count,
            "working": c.working_count,
            "episodic": c.episodic_count,
            "semantic": c.semantic_count,
            "procedural": c.procedural_count,
            "prospective": c.prospective_count,
            "total": c.total(),
        },
        // Issue #2331: graph-edge / dedup connection counts. Keys are always
        // present (zeroed when not computed) for stable scripting; `edges_note`
        // is set instead when the counts could not be computed over this tier.
        "edges": {
            "derives_from": report.graph.derives_from_edges,
            "procedure_derives_from": report.graph.procedure_derives_from_edges,
            "similar_to": report.graph.similar_to_edges,
            "supersedes": report.graph.supersedes_edges,
        },
        "provenance": {
            "facts_with_provenance": report.graph.facts_with_provenance,
            "facts_total": report.graph.facts_total,
        },
        "snapshot_dedup": {
            "distinct_caller_keys": report.graph.distinct_snapshot_caller_keys,
            "snapshot_facts": report.graph.snapshot_facts_total,
        },
    });

    if let Some(note) = &report.graph_note {
        value["edges_note"] = serde_json::json!(note);
    }

    if include_samples {
        value["samples"] = serde_json::json!({
            "facts": report.samples.facts,
            "episodes": report.samples.episodes,
            "episodes_note": report.samples.episodes_note,
            "procedures": report.samples.procedures,
        });
    }

    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

// TDD (RED) for issue #2679: tests for the agent-facing `simard memory remember`
// write tool (scalar-flag parsing + exit codes + daemon-down). The subcommand,
// `parse_remember_fact_args`, `RememberFactArgs`, and `run_remember_fact` land
// in the implementation step; until then the unresolved paths are the red
// signal. `#[cfg(test)]` so production builds never compile it.
#[cfg(test)]
mod remember_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    /// Seed a temp store with at least one row of every introspectable type so
    /// the report has non-zero counts to assert against.
    fn seeded_store() -> (tempfile::TempDir, LibraryCognitiveMemory) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mem = LibraryCognitiveMemory::open(tmp.path()).expect("open store");
        mem.store_fact(
            "rust",
            "Rust is a systems language",
            0.9,
            &["language".to_string()],
            "test",
        )
        .expect("store_fact");
        mem.store_episode("ran cargo test; 0 failures", "engineer-cycle", None)
            .expect("store_episode");
        mem.store_procedure(
            "ooda:consolidate-memory",
            &["distill episodes".to_string()],
            &[],
        )
        .expect("store_procedure");
        mem.store_prospective("goal:Ship the CLI", "ship the cli", "Pursue goal", 1)
            .expect("store_prospective");
        (tmp, mem)
    }

    #[test]
    fn dispatch_rejects_unknown_subcommand() {
        let args = vec!["frobnicate".to_string()].into_iter();
        let err = dispatch_memory_command(args).unwrap_err().to_string();
        assert!(err.contains("memory frobnicate"), "{err}");
    }

    #[test]
    fn dispatch_help_is_ok() {
        let args = vec!["--help".to_string()].into_iter();
        assert!(dispatch_memory_command(args).is_ok());
    }

    #[test]
    fn dump_type_parse_rejects_unknown() {
        assert!(DumpType::parse("frobs").is_err());
        assert_eq!(DumpType::parse("facts").unwrap(), DumpType::Facts);
    }

    #[test]
    fn dump_type_accepts_count_only_types() {
        assert_eq!(DumpType::parse("working").unwrap(), DumpType::Working);
        assert_eq!(DumpType::parse("sensory").unwrap(), DumpType::Sensory);
    }

    #[test]
    fn dump_type_working_yields_no_sample_rows() {
        let (_tmp, mem) = seeded_store();
        let samples = collect_samples(
            &mem,
            DUMP_SAMPLE_LIMIT,
            DumpType::Working,
            AccessTier::DirectOpen,
            mem.get_statistics().unwrap().episodic_count,
        );
        assert!(samples.facts.is_empty());
        assert!(samples.episodes.is_empty());
        assert!(samples.procedures.is_empty());
        assert!(samples.episodes_note.is_none());
    }

    #[test]
    fn build_report_counts_every_populated_type() {
        let (tmp, mem) = seeded_store();
        let report = build_report(
            &mem,
            tmp.path(),
            AccessTier::DirectOpen,
            DUMP_SAMPLE_LIMIT,
            DumpType::All,
        )
        .expect("build_report");

        assert!(report.counts.semantic_count >= 1, "facts not counted");
        assert!(report.counts.episodic_count >= 1, "episodes not counted");
        assert!(
            report.counts.procedural_count >= 1,
            "procedures not counted"
        );
        assert!(
            report.counts.prospective_count >= 1,
            "prospectives/triggers not counted"
        );
        assert!(report.counts.total() >= 4, "total must sum all types");
    }

    #[test]
    fn render_human_shows_labels_and_counts() {
        let (tmp, mem) = seeded_store();
        let report = build_report(
            &mem,
            tmp.path(),
            AccessTier::DirectOpen,
            STATS_SAMPLE_LIMIT,
            DumpType::All,
        )
        .expect("build_report");
        let text = render_human(&report, false);

        for label in [
            "sensory",
            "working",
            "episodic",
            "semantic",
            "procedural",
            "prospective",
            "total",
            "(facts)",
            "(procedures)",
            "(triggers)",
        ] {
            assert!(
                text.contains(label),
                "human output missing '{label}':\n{text}"
            );
        }
        assert!(
            text.contains("via direct open"),
            "banner must name the access tier:\n{text}"
        );
    }

    #[test]
    fn render_json_has_stable_keys_and_counts() {
        let (tmp, mem) = seeded_store();
        let report = build_report(
            &mem,
            tmp.path(),
            AccessTier::DirectOpen,
            STATS_SAMPLE_LIMIT,
            DumpType::All,
        )
        .expect("build_report");
        let json = render_json(&report, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        let counts = &parsed["counts"];
        for key in [
            "sensory",
            "working",
            "episodic",
            "semantic",
            "procedural",
            "prospective",
            "total",
        ] {
            assert!(
                counts.get(key).is_some(),
                "json counts missing '{key}': {json}"
            );
        }
        assert!(
            counts["semantic"].as_u64().unwrap() >= 1,
            "semantic count must reflect the seeded fact: {json}"
        );
        assert!(
            parsed.get("samples").is_none(),
            "stats json must omit samples"
        );
        assert!(
            parsed.get("state_root").is_some(),
            "json must carry state_root: {json}"
        );
        assert_eq!(
            parsed["access_tier"].as_str(),
            Some("direct-open"),
            "json access_tier must be the stable machine token: {json}"
        );
    }

    #[test]
    fn dump_json_includes_samples() {
        let (tmp, mem) = seeded_store();
        let report = build_report(
            &mem,
            tmp.path(),
            AccessTier::DirectOpen,
            DUMP_SAMPLE_LIMIT,
            DumpType::All,
        )
        .expect("build_report");
        let json = render_json(&report, true);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert!(
            parsed.get("samples").is_some(),
            "dump json must include samples"
        );
    }

    #[test]
    fn samples_are_best_effort_and_present_for_direct_open() {
        let (_tmp, mem) = seeded_store();
        let samples = collect_samples(
            &mem,
            DUMP_SAMPLE_LIMIT,
            DumpType::All,
            AccessTier::DirectOpen,
            mem.get_statistics().unwrap().episodic_count,
        );
        // Direct-open backend exposes the neutral episode enumerator.
        assert!(
            !samples.episodes.is_empty(),
            "direct-open episode samples should be present"
        );
        assert!(
            samples.facts.iter().any(|f| f.contains("rust")),
            "fact sample should surface the seeded concept: {:?}",
            samples.facts
        );
    }

    // ---- Issue #2331: graph-edge / dedup stats section -------------------

    #[test]
    fn render_human_includes_edges_section_labels() {
        let (tmp, mem) = seeded_store();
        let report = build_report(
            &mem,
            tmp.path(),
            AccessTier::DirectOpen,
            STATS_SAMPLE_LIMIT,
            DumpType::All,
        )
        .expect("build_report");
        let text = render_human(&report, false);

        for label in [
            "edges / connections",
            "DERIVES_FROM",
            "PROCEDURE_DERIVES_FROM",
            "SIMILAR_TO",
            "SUPERSEDES",
            "facts with provenance:",
            "snapshot dedup:",
        ] {
            assert!(
                text.contains(label),
                "human edges section missing '{label}':\n{text}"
            );
        }
    }

    #[test]
    fn render_json_includes_edges_objects() {
        let (tmp, mem) = seeded_store();
        let report = build_report(
            &mem,
            tmp.path(),
            AccessTier::DirectOpen,
            STATS_SAMPLE_LIMIT,
            DumpType::All,
        )
        .expect("build_report");
        let json = render_json(&report, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        for key in [
            "derives_from",
            "procedure_derives_from",
            "similar_to",
            "supersedes",
        ] {
            assert!(
                parsed["edges"].get(key).is_some(),
                "json edges missing '{key}': {json}"
            );
        }
        assert!(
            parsed["provenance"].get("facts_total").is_some(),
            "json provenance.facts_total missing: {json}"
        );
        assert!(
            parsed["snapshot_dedup"]
                .get("distinct_caller_keys")
                .is_some(),
            "json snapshot_dedup.distinct_caller_keys missing: {json}"
        );
        assert!(
            parsed.get("edges_note").is_none(),
            "direct-open json must not carry an edges_note: {json}"
        );
    }

    #[test]
    fn build_report_direct_open_counts_provenance_edges() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mem = LibraryCognitiveMemory::open(tmp.path()).expect("open store");
        let ep = mem
            .store_episode("ran cargo test; 0 failures", "engineer-cycle", None)
            .expect("store_episode");
        mem.store_fact_with_provenance(
            "lesson",
            "tests must stay green",
            0.9,
            "distill:cycle",
            None,
            None,
            std::slice::from_ref(&ep),
        )
        .expect("store_fact_with_provenance");

        let report = build_report(
            &mem,
            tmp.path(),
            AccessTier::DirectOpen,
            STATS_SAMPLE_LIMIT,
            DumpType::All,
        )
        .expect("build_report");

        assert!(
            report.graph.derives_from_edges >= 1,
            "DERIVES_FROM edge must be counted after provenance link: {:?}",
            report.graph
        );
        assert!(
            report.graph.facts_with_provenance >= 1,
            "fact with provenance must be counted: {:?}",
            report.graph
        );
        assert!(
            report.graph_note.is_none(),
            "direct open must compute graph"
        );

        let text = render_human(&report, false);
        assert!(
            text.contains("facts with provenance:  1 / "),
            "human output must reflect the provenance coverage:\n{text}"
        );
    }

    #[test]
    fn daemon_socket_tier_notes_edges_unavailable() {
        // Over the daemon IPC socket the graph reader is unavailable, so the
        // report must carry the note and zeroed counts instead of failing.
        let (tmp, mem) = seeded_store();
        let report = build_report(
            &mem,
            tmp.path(),
            AccessTier::DaemonSocket,
            STATS_SAMPLE_LIMIT,
            DumpType::All,
        )
        .expect("build_report");

        assert_eq!(
            report.graph_note.as_deref(),
            Some(DAEMON_GRAPH_NOTE),
            "daemon-socket tier must note that graph stats need a direct open"
        );
        assert_eq!(report.graph, GraphStats::default(), "counts must be zeroed");

        let text = render_human(&report, false);
        assert!(
            text.contains(DAEMON_GRAPH_NOTE),
            "human edges section must print the daemon note:\n{text}"
        );
        assert!(
            !text.contains("DERIVES_FROM"),
            "note path must replace the per-edge rows:\n{text}"
        );

        let json = render_json(&report, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(
            parsed["edges_note"].as_str(),
            Some(DAEMON_GRAPH_NOTE),
            "json must carry edges_note over the daemon socket: {json}"
        );
        assert!(
            parsed["edges"].get("derives_from").is_some(),
            "edges keys stay present (zeroed) for stable scripting: {json}"
        );
    }
}
