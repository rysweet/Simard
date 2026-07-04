//! Pack -> cognitive-memory ingest (roadmap #2491 Pillar 2a; child #2493).
//!
//! Ingesting a knowledge pack must *populate cognitive memory* — durable
//! semantic facts and reusable procedures — rather than leaving the knowledge in
//! an external retrieval index. This is the seam that lets learned Rust
//! expertise persist, consolidate, and be recalled at the moment of need.
//!
//! Facts are written with [`CognitiveMemoryOps::store_fact_with_provenance`] so
//! their source URL/section/version travels into memory as the fact `source_id`
//! plus provenance tags. Procedures are written with
//! [`CognitiveMemoryOps::store_procedure`], with a `competency:<subskill>`
//! marker plus `pack:`/`source:` provenance breadcrumbs prepended to their
//! prerequisites — so the competency gym can recall them by sub-skill and a
//! recalled procedure stays traceable to its source.

use std::collections::HashMap;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;

use super::pack::{PackFact, PackProcedure, RustExpertPack};

/// Prefix marking a procedure prerequisite as a competency tag.
pub const COMPETENCY_PREFIX: &str = "competency:";

/// Which subset of a pack to ingest.
///
/// [`IngestScope::All`] ingests the full pack (the "healthy" acquisition path).
/// [`IngestScope::OnlySubskills`] ingests only items whose `subskill` is in the
/// list — used to build the deliberately-degraded knowledge state that the
/// calibration guard (issue #1241 discipline) requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestScope {
    /// Ingest every fact and procedure.
    All,
    /// Ingest only facts/procedures whose sub-skill appears in the list.
    OnlySubskills(Vec<String>),
}

impl IngestScope {
    fn includes(&self, subskill: &str) -> bool {
        match self {
            IngestScope::All => true,
            IngestScope::OnlySubskills(keep) => keep.iter().any(|s| s == subskill),
        }
    }
}

/// Outcome of ingesting a pack: the fact/procedure yield into memory.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct IngestReport {
    /// Pack that was ingested.
    pub pack_name: String,
    /// Semantic facts successfully written to memory.
    pub facts_ingested: usize,
    /// Facts that failed to write (best-effort ingestion).
    pub facts_failed: usize,
    /// Procedures successfully written to memory.
    pub procedures_ingested: usize,
    /// Procedures that failed to write.
    pub procedures_failed: usize,
    /// Node ids of the ingested facts (for downstream provenance checks).
    pub fact_ids: Vec<String>,
    /// Node ids of the ingested procedures.
    pub procedure_ids: Vec<String>,
}

impl IngestReport {
    /// Total durable items (facts + procedures) written to memory.
    pub fn total_yield(&self) -> usize {
        self.facts_ingested + self.procedures_ingested
    }
}

/// Build the `source_id` recorded on a fact so it traces back to its source.
fn fact_source_id(pack: &RustExpertPack, fact: &PackFact) -> String {
    format!(
        "kgpack:{}:{}#{}",
        pack.name, fact.provenance.url, fact.provenance.section
    )
}

/// Build the tag set stored with a fact: the pack's own tags plus a
/// provenance-source tag so the origin survives in memory.
fn fact_tags(pack: &RustExpertPack, fact: &PackFact) -> Vec<String> {
    let mut tags: Vec<String> = fact.tags.iter().map(|t| (*t).to_string()).collect();
    tags.push(format!("pack:{}", pack.name));
    tags.push(format!("source:{}", fact.provenance.url));
    tags
}

/// Build the provenance metadata map stored alongside a fact.
fn fact_metadata(fact: &PackFact) -> HashMap<String, serde_json::Value> {
    let mut meta = HashMap::new();
    meta.insert(
        "source".to_string(),
        serde_json::Value::String(fact.provenance.source.to_string()),
    );
    meta.insert(
        "url".to_string(),
        serde_json::Value::String(fact.provenance.url.to_string()),
    );
    meta.insert(
        "section".to_string(),
        serde_json::Value::String(fact.provenance.section.to_string()),
    );
    meta.insert(
        "version".to_string(),
        serde_json::Value::String(fact.provenance.version.to_string()),
    );
    meta.insert(
        "retrieved".to_string(),
        serde_json::Value::String(fact.provenance.retrieved.to_string()),
    );
    meta
}

/// Prerequisites written for a procedure: a `competency:<subskill>` marker and
/// `pack:` / `source:` provenance breadcrumbs, followed by the pack's domain
/// prerequisites. The breadcrumbs persist the procedure's provenance into memory
/// (procedural storage has no dedicated provenance fields), so a recalled
/// procedure remains traceable to its source.
fn procedure_prerequisites(pack: &RustExpertPack, proc: &PackProcedure) -> Vec<String> {
    let mut prereqs = Vec::with_capacity(proc.prerequisites.len() + 3);
    prereqs.push(format!("{COMPETENCY_PREFIX}{}", proc.subskill));
    prereqs.push(format!("pack:{}", pack.name));
    prereqs.push(format!("source:{}", proc.provenance.url));
    prereqs.extend(proc.prerequisites.iter().map(|p| (*p).to_string()));
    prereqs
}

/// Ingest the whole pack into cognitive memory (the healthy acquisition path).
pub fn ingest_pack_into_memory(
    pack: &RustExpertPack,
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<IngestReport> {
    ingest_pack_scoped(pack, memory, &IngestScope::All)
}

/// Ingest a `scope`-filtered view of the pack into cognitive memory.
///
/// Best-effort: an individual fact/procedure that fails to store is counted in
/// `*_failed` and logged, but does not abort the ingest — the report always
/// reflects the true yield.
pub fn ingest_pack_scoped(
    pack: &RustExpertPack,
    memory: &dyn CognitiveMemoryOps,
    scope: &IngestScope,
) -> SimardResult<IngestReport> {
    let mut report = IngestReport {
        pack_name: pack.name.to_string(),
        ..IngestReport::default()
    };

    for fact in pack.facts.iter().filter(|f| scope.includes(f.subskill)) {
        let tags = fact_tags(pack, fact);
        let metadata = fact_metadata(fact);
        match memory.store_fact_with_provenance(
            fact.concept,
            fact.content,
            fact.confidence,
            &fact_source_id(pack, fact),
            Some(&tags),
            Some(&metadata),
            &[],
        ) {
            Ok(id) => {
                report.facts_ingested += 1;
                report.fact_ids.push(id);
            }
            Err(e) => {
                report.facts_failed += 1;
                tracing::warn!(
                    target: "simard::rust_expertise",
                    concept = fact.concept,
                    error = %e,
                    "rust-expert pack: failed to ingest fact (non-fatal)"
                );
            }
        }
    }

    for proc in pack
        .procedures
        .iter()
        .filter(|p| scope.includes(p.subskill))
    {
        let steps: Vec<String> = proc.steps.iter().map(|s| (*s).to_string()).collect();
        let prerequisites = procedure_prerequisites(pack, proc);
        match memory.store_procedure(proc.name, &steps, &prerequisites) {
            Ok(id) => {
                report.procedures_ingested += 1;
                report.procedure_ids.push(id);
            }
            Err(e) => {
                report.procedures_failed += 1;
                tracing::warn!(
                    target: "simard::rust_expertise",
                    procedure = proc.name,
                    error = %e,
                    "rust-expert pack: failed to ingest procedure (non-fatal)"
                );
            }
        }
    }

    Ok(report)
}
