//! Operator subcommand `simard memory <stats|dump>` — read-only introspection
//! of the six-type cognitive-memory store on the library backend (issue #2308
//! follow-up).
//!
//! Both commands open the store through [`open_reader_bridge`], the canonical
//! read-only consumer entry point: when the OODA daemon is up they route
//! through its memory socket (no lock contention); when it is down they fall
//! back to a direct open of the on-disk store. Neither command mutates memory.
//!
//! See `docs/reference/simard-memory-cli.md` for the operator reference.

use std::path::{Path, PathBuf};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::memory_cognitive::CognitiveStatistics;
use crate::memory_ipc::{open_reader_bridge, socket_path_for};

pub(super) const MEMORY_HELP: &str = "\
Simard memory subcommand — cognitive-memory introspection (read-only)

Usage:
  simard memory stats [state-root] [--json]
  simard memory dump  [state-root] [--type=TYPE] [--limit=N] [--json]

stats  Print per-type counts (sensory, working, episodic, semantic/facts,
       procedural/procedures, prospective/triggers) plus a few sample rows.
dump   Print counts plus a larger set of sample rows per type for eyeballing
       content. --type restricts to one of: facts, episodes, procedures.

Both commands are read-only and safe to run while the OODA daemon holds the
store: they route through the daemon socket when it is up and fall back to a
direct open when it is down. With no [state-root] they resolve
$SIMARD_STATE_ROOT, then $HOME/.simard.
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
}

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
        other => Err(format!("unsupported command 'memory {other}'").into()),
    }
}

/// Resolve the state root: explicit argument wins, else the shared resolver
/// the daemon uses (`$SIMARD_STATE_ROOT`, then `$HOME/.simard`).
fn resolve_state_root(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(crate::state_root::simard_state_root)
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

    let reader = open_reader_bridge(&state_root)?;
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

    let reader = open_reader_bridge(&state_root)?;
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
    Ok(MemoryReport {
        state_root: state_root.to_path_buf(),
        store_path: state_root.join("cognitive"),
        access_tier,
        counts,
        samples,
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
    });

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
}
