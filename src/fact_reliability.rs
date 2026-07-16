//! Shared, pure per-fact reliability scorer for distilled semantic facts
//! (issue #2679, homing the ISAO gate first landed in #2433).
//!
//! ## Why this module exists
//!
//! Post-#2679 the distillation RESULT path no longer parses an agent-authored
//! `{ "facts": [...] }` document. Instead the distiller agentic step writes each
//! fact DIRECTLY through the cognitive-memory write boundary. The reliability
//! gate that used to run in Simard *after the parse* therefore moves to the
//! write boundary and is applied **per fact**. Two seams reach that boundary:
//!
//!   1. the IPC server's `StoreFactGated` dispatch arm (the authoritative,
//!      server-side gate for the real subprocess path), and
//!   2. the in-process `DistillFactSink` used by the deterministic test stubs.
//!
//! To keep those two seams in lock-step — and to *reduce* the
//! `memory_consolidation` fork per the G2 memory-architecture constraint — the
//! scorer lives here as one shared, pure function. Both seams call the SAME
//! [`score_fact_reliability`], so a fact scores identically no matter which
//! boundary writes it.
//!
//! ## Contract vs. the legacy `assess_fact_reliability`
//!
//! The legacy scorer took the whole pass batch so it could compute a
//! **corroboration** term. That batch is unavailable in a per-fact IPC call, so
//! this scorer is a pure function of exactly one fact plus a resolved
//! `grounded: bool`. Grounding is resolved *before* the call (batch-membership
//! for the in-process sink; store-existence for the IPC handler). The
//! corroboration term is deliberately dropped: it was disposition-neutral (it
//! only nudged an already-storable 0.9 → 1.0, never flipped store↔quarantine),
//! so excluding it lets both seams agree on every store/quarantine decision.

/// ISAO reliability gate threshold (issue #2433). A fact whose self-assessed
/// reliability is below this is **quarantined** — not promoted into semantic
/// memory. Tuned so a fact with valid provenance, a known concept label, and
/// non-trivial content clears the bar, while a fact with hallucinated
/// provenance or empty content does not.
pub const RELIABILITY_THRESHOLD: f64 = 0.5;

/// How many same-`concept` priors the identity-dedup step in
/// [`commit_gated_fact`] inspects when deciding whether a new fact merely
/// restates an equal-or-stronger existing fact. `search_facts` returns priors
/// ranked strongest-first and filtered to `>= confidence`, so a genuine
/// equal-or-stronger duplicate surfaces within the first few results; the
/// window is kept intentionally small to bound the per-write query cost on the
/// distillation hot path.
const DEDUP_PRIOR_SCAN_LIMIT: u32 = 5;

/// The closed concept-label set the distillation recipe is constrained to. A
/// fact whose concept does not canonicalize into this set is off-spec and loses
/// the concept-validity component of its reliability score (but is NOT dropped —
/// concept validity is a nudge, not a gate).
pub const KNOWN_CONCEPTS: &[&str] = &["pr-pattern", "bug-pattern", "lesson-learned"];

/// Canonicalize a recipe-emitted concept label to one of [`KNOWN_CONCEPTS`], or
/// `None` if it is genuinely off-spec.
///
/// The distillation recipe's prompt constrains the label to the closed set
/// `{pr-pattern, bug-pattern, lesson-learned}`, but an LLM routinely varies the
/// *surface form* of a label it clearly intends: title/upper case, surrounding
/// whitespace or quotes/sentence punctuation, and space/underscore separators.
/// Canonicalization folds case, trims surrounding whitespace/quote/sentence
/// punctuation, and unifies `_`/space→`-` (collapsing repeated hyphens) before an
/// EXACT match against the three labels. A concept that does not normalize to one
/// of them still returns `None`. The three labels are lexically distinct, so no
/// genuinely different concept can alias onto another.
pub fn canonical_concept(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim().trim_matches(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '.' | ',' | ':' | ';')
    });

    // Fold case, unify separators (`_` and interior spaces behave as `-`) and
    // collapse runs of hyphens in a single pass — folding the lowercase into this
    // loop avoids a second heap allocation for a separately-lowercased string.
    // Trim any leading/trailing hyphens the folding produced afterwards.
    let mut canon = String::with_capacity(trimmed.len());
    let mut prev_hyphen = false;
    for ch in trimmed.chars() {
        let c = if ch == '_' || ch == ' ' {
            '-'
        } else {
            ch.to_ascii_lowercase()
        };
        if c == '-' {
            if !prev_hyphen {
                canon.push('-');
            }
            prev_hyphen = true;
        } else {
            canon.push(c);
            prev_hyphen = false;
        }
    }
    let canon = canon.trim_matches('-');

    match canon {
        "pr-pattern" => Some("pr-pattern"),
        "bug-pattern" => Some("bug-pattern"),
        "lesson-learned" => Some("lesson-learned"),
        _ => None,
    }
}

/// Number of distinct informative words the content-quality signal in
/// [`score_fact_reliability`] needs to distinguish before it awards full credit.
/// The scorer only cares about the `0` / `1–2` / `≥3` bucket, so the informative-
/// word scan stops once this many distinct words are seen (bounding the work on
/// the up-to-64 KiB content body).
const FULL_CONTENT_WORD_BUCKET: usize = 3;

/// Count DISTINCT *informative* words in `content`, stopping once `cap` distinct
/// words have been seen.
///
/// An **informative** word is a whitespace-delimited token bearing at least one
/// alphanumeric character; a token made only of punctuation/symbols (`"..."`,
/// `"-"`, `"—"`) carries no information and is skipped. Each informative token is
/// normalized before the distinctness check — folded to lowercase with every
/// non-alphanumeric character stripped — so `"recall"`, `"Recall"` and
/// `"recall."` collapse to a single distinct word and mere repetition
/// (`"the the the"`) cannot inflate the count past one.
///
/// This is the information proxy the content-quality signal scores against,
/// replacing a raw `split_whitespace` token count that treated punctuation
/// tokens and repeated words as if each carried fresh information. The scan is
/// linear in the (length-capped) content and only runs at fact-commit time, off
/// the recall hot path.
fn distinct_informative_words(content: &str, cap: usize) -> usize {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for token in content.split_whitespace() {
        let normalized: String = token
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        if normalized.is_empty() {
            continue; // punctuation/symbol-only token — carries no information
        }
        seen.insert(normalized);
        if seen.len() >= cap {
            break;
        }
    }
    seen.len()
}

/// Self-assess the reliability of one distilled fact (issue #2433, BGML's
/// *information self-assessment ownership*, §IV). Returns a confidence score in
/// `[0.0, 1.0]` from cheap, locally-available signals — no extra LLM call:
///
/// | Signal | Weight | Rationale |
/// |--------|--------|-----------|
/// | **Provenance grounding** | 0.5 | Resolved by the caller: batch-membership for the in-process sink, store-existence for the IPC handler. A source that cannot be grounded is unverifiable / hallucinated provenance — the strongest unreliability signal. |
/// | **Content quality** | ≤0.3 | Scored over *distinct informative words* (alphanumeric-bearing tokens, case/punctuation-normalized and de-duplicated), not raw whitespace tokens. Content with zero informative words — empty, whitespace-only, or punctuation/symbol-only (`"... ... ..."`) — carries no information and is a HARD gate (score `0.0`); otherwise ≥3 distinct informative words earns the full weight, 1–2 a partial 0.15. Degenerate repetition (`"the the the"`) has one distinct word, so it only earns the partial weight. |
/// | **Concept validity** | 0.1 | Awarded when the concept canonicalizes into [`KNOWN_CONCEPTS`]. |
///
/// A nominal fact (grounded, ≥3 distinct informative words, known concept) scores
/// `0.9`. Because grounding (0.5) is *necessary* to clear
/// [`RELIABILITY_THRESHOLD`] (0.5), an ungrounded fact tops out at `0.4` (content
/// + concept) and a no-information fact scores `0.0`; both are quarantined.
pub fn score_fact_reliability(concept: &str, content: &str, grounded: bool) -> f64 {
    // (0) Hard gate: content that carries no information is quarantined
    // unconditionally, regardless of how trustworthy its provenance looks. This
    // covers empty / whitespace-only content AND content made only of
    // punctuation/symbol tokens (`"... ... ..."`), which carry exactly as much
    // information as an empty string. Distinctness also means degenerate
    // repetition (`"the the the"`) counts as a single word, not three. We only
    // need the 0 / 1–2 / ≥3 bucket, so the scan stops after the third distinct
    // informative word instead of walking the whole (up to 64 KiB) content.
    let words = distinct_informative_words(content, FULL_CONTENT_WORD_BUCKET);
    if words == 0 {
        return 0.0;
    }

    let mut score = 0.0_f64;

    // (1) Provenance grounding — the dominant, *necessary* signal.
    if grounded {
        score += 0.5;
    }

    // (2) Content quality (content has ≥1 informative word here — see the hard
    // gate above).
    if words >= FULL_CONTENT_WORD_BUCKET {
        score += 0.3;
    } else {
        score += 0.15;
    }

    // (3) Concept validity — a nudge, not a gate.
    if canonical_concept(concept).is_some() {
        score += 0.1;
    }

    score.clamp(0.0, 1.0)
}

/// Thin predicate over [`score_fact_reliability`]: a fact clears the promotion
/// gate iff its score is at or above [`RELIABILITY_THRESHOLD`]. Both write-
/// boundary seams share this identical store/quarantine decision.
pub fn fact_passes_gate(concept: &str, content: &str, grounded: bool) -> bool {
    score_fact_reliability(concept, content, grounded) >= RELIABILITY_THRESHOLD
}

/// Normalize fact content into the **identity key** used by [`commit_gated_fact`]'s
/// dedup step: trim, then collapse every run of interior whitespace to a single
/// ASCII space.
///
/// The dedup step asks "does this new fact merely restate an equal-or-stronger
/// existing fact of the same identity?". A restatement that differs only in
/// surrounding or interior whitespace — an LLM re-emitting the same lesson across
/// distillation passes with a stray double space, a tab, or a wrapped newline —
/// carries the identical lesson and must NOT be promoted a second time. Exact
/// `trim()` equality misses that case: `"empty  outcome list"` and
/// `"empty outcome list"` compare unequal and both get stored, inflating semantic
/// memory with a redundant fact and dragging down recall precision. Collapsing
/// interior whitespace folds those trivial variants onto one key.
///
/// This affects the dedup *comparison* only — a fact that survives the gate is
/// still stored **verbatim** via `store_fact_with_provenance`, so no content is
/// rewritten. Case is deliberately preserved: distilled content can carry
/// case-significant tokens (identifiers, error strings), so two facts differing
/// only in case are left as distinct rather than silently merged.
pub fn dedup_content_key(content: &str) -> String {
    // `split_whitespace` already trims leading/trailing whitespace and treats any
    // run of Unicode whitespace as one separator, so joining with a single space
    // yields the canonical single-spaced form in one pass.
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Disposition of one write-boundary gate decision (issue #2679), returned by
/// [`commit_gated_fact`].
#[derive(Debug, Clone, PartialEq)]
pub enum FactGateDecision {
    /// The fact cleared the gate and was persisted with the gate-computed
    /// `confidence`; `node_id` is the new fact node's id.
    Stored { confidence: f64, node_id: String },
    /// The fact was blocked — either below [`RELIABILITY_THRESHOLD`] or an
    /// equal-or-stronger prior of the same identity already exists. Nothing was
    /// written. `confidence` is still the gate-computed score (a caller can
    /// compare it against [`RELIABILITY_THRESHOLD`] to tell a low-reliability
    /// quarantine from a dedup skip).
    Quarantined { confidence: f64 },
}

impl FactGateDecision {
    /// `true` when the fact was persisted.
    pub fn stored(&self) -> bool {
        matches!(self, Self::Stored { .. })
    }

    /// The gate-computed reliability score, available for both dispositions.
    pub fn confidence(&self) -> f64 {
        match self {
            Self::Stored { confidence, .. } | Self::Quarantined { confidence } => *confidence,
        }
    }
}

/// The single shared write-boundary gate (issue #2679): **score → threshold →
/// identity-dedup → persist**, applied to one fact.
///
/// Both seams that reach the write boundary call this so a fact stores or
/// quarantines identically no matter which boundary writes it:
///
///   1. the IPC server's `StoreFactGated` handler (server-side, real subprocess
///      path), and
///   2. the in-process `DistillFactSink` used by the deterministic test stubs.
///
/// `grounded` is resolved by the caller — store-existence for the server,
/// batch-membership for the in-process sink — because the notion of "grounded"
/// differs per seam. Everything downstream of grounding is identical and lives
/// here:
///
///   - Confidence is ALWAYS [`score_fact_reliability`]'s output, never a client
///     hint.
///   - Below [`RELIABILITY_THRESHOLD`] → [`FactGateDecision::Quarantined`].
///   - A weaker-or-equal restatement never clobbers an existing equal-or-stronger
///     fact of the same identity (`concept` + trimmed `content`); such a fact is
///     also quarantined (its score still cleared the threshold, so the caller can
///     distinguish it by `confidence >= RELIABILITY_THRESHOLD`).
///   - Survivors persist via `store_fact_with_provenance` with the gate-computed
///     confidence and the source-episode provenance edges.
pub fn commit_gated_fact(
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    concept: &str,
    content: &str,
    grounded: bool,
    source_id: &str,
    tags: &[String],
    source_episode_ids: &[String],
) -> crate::error::SimardResult<FactGateDecision> {
    let confidence = score_fact_reliability(concept, content, grounded);

    // Threshold quarantine.
    if confidence < RELIABILITY_THRESHOLD {
        return Ok(FactGateDecision::Quarantined { confidence });
    }

    // Identity dedup: do not downgrade/duplicate an equal-or-stronger prior
    // version of the *same* fact (concept + content). `search_facts` is queried
    // with the new confidence as `min_confidence` so it returns only priors
    // strong enough to block; the explicit `>=` is belt-and-suspenders against a
    // backend that ignores the filter. Content is compared on the
    // whitespace-normalized [`dedup_content_key`] so a restatement that differs
    // only in interior/surrounding whitespace is recognized as the same fact
    // (the survivor is still stored verbatim — only this comparison is
    // normalized).
    let new_key = dedup_content_key(content);
    let existing = memory
        .search_facts(concept, DEDUP_PRIOR_SCAN_LIMIT, confidence)
        .unwrap_or_default();
    if existing
        .iter()
        .any(|f| dedup_content_key(&f.content) == new_key && f.confidence >= confidence)
    {
        return Ok(FactGateDecision::Quarantined { confidence });
    }

    // Persist with the gate-computed confidence and provenance edges.
    let node_id = memory.store_fact_with_provenance(
        concept,
        content,
        confidence,
        source_id,
        Some(tags),
        None,
        source_episode_ids,
    )?;
    Ok(FactGateDecision::Stored {
        confidence,
        node_id,
    })
}
