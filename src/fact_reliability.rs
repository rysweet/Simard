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

/// Self-assess the reliability of one distilled fact (issue #2433, BGML's
/// *information self-assessment ownership*, §IV). Returns a confidence score in
/// `[0.0, 1.0]` from cheap, locally-available signals — no extra LLM call:
///
/// | Signal | Weight | Rationale |
/// |--------|--------|-----------|
/// | **Provenance grounding** | 0.5 | Resolved by the caller: batch-membership for the in-process sink, store-existence for the IPC handler. A source that cannot be grounded is unverifiable / hallucinated provenance — the strongest unreliability signal. |
/// | **Content quality** | ≤0.3 | Empty / whitespace-only content carries no information and is a HARD gate (score `0.0`); otherwise ≥3 words earns the full weight, 1–2 words a partial 0.15. |
/// | **Concept validity** | 0.1 | Awarded when the concept canonicalizes into [`KNOWN_CONCEPTS`]. |
///
/// A nominal fact (grounded, ≥3 words, known concept) scores `0.9`. Because
/// grounding (0.5) is *necessary* to clear [`RELIABILITY_THRESHOLD`] (0.5), an
/// ungrounded fact tops out at `0.4` (content + concept) and an empty fact scores
/// `0.0`; both are quarantined.
pub fn score_fact_reliability(concept: &str, content: &str, grounded: bool) -> f64 {
    // (0) Hard gate: empty / whitespace-only content carries no information and
    // is quarantined unconditionally, regardless of how trustworthy its
    // provenance looks. We only need the 0 / 1–2 / ≥3 word bucket, so stop after
    // the third word instead of scanning the whole (up to 64 KiB) content.
    let words = content.split_whitespace().take(3).count();
    if words == 0 {
        return 0.0;
    }

    let mut score = 0.0_f64;

    // (1) Provenance grounding — the dominant, *necessary* signal.
    if grounded {
        score += 0.5;
    }

    // (2) Content quality (content is non-empty here — see the hard gate above).
    if words >= 3 {
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
///
/// ## Concept-label canonicalization (recall + dedup consistency)
///
/// Before the dedup lookup and the store, the concept is normalized to its
/// canonical [`KNOWN_CONCEPTS`] form via [`canonical_concept`]. An LLM routinely
/// varies the *surface form* of a label it clearly intends (`PR-Pattern`,
/// `bug_pattern`, `Lesson Learned.`); persisting each variant verbatim fragments
/// semantic memory along the concept axis — recall by the canonical label misses
/// variant-stored facts, and two surface-variant restatements of the SAME fact
/// never dedup against each other. Normalizing here — at the single shared write
/// boundary — converges every recognized variant onto one label for BOTH seams.
/// A genuinely off-spec concept does not canonicalize and is stored verbatim,
/// preserving the "concept validity is a nudge, not a gate" contract: no fact is
/// ever dropped or relabeled for an unrecognized concept.
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

    // Canonicalize the concept LABEL to its known-concept form before it is used
    // as the dedup search key and the persisted label, so recognized surface-form
    // variants ("PR-Pattern", "bug_pattern", "Lesson Learned.") converge onto one
    // label instead of fragmenting semantic memory. A genuinely off-spec concept
    // does not canonicalize and is stored verbatim (nudge, not gate).
    let stored_concept: &str = match canonical_concept(concept) {
        Some(canonical) => canonical,
        None => concept,
    };

    // Identity dedup: do not downgrade/duplicate an equal-or-stronger prior
    // version of the *same* fact (concept + content). `search_facts` is queried
    // with the new confidence as `min_confidence` so it returns only priors
    // strong enough to block; the explicit `>=` is belt-and-suspenders against a
    // backend that ignores the filter.
    let new_content = content.trim();
    let existing = memory
        .search_facts(stored_concept, DEDUP_PRIOR_SCAN_LIMIT, confidence)
        .unwrap_or_default();
    if existing
        .iter()
        .any(|f| f.content.trim() == new_content && f.confidence >= confidence)
    {
        return Ok(FactGateDecision::Quarantined { confidence });
    }

    // Persist with the gate-computed confidence and provenance edges.
    let node_id = memory.store_fact_with_provenance(
        stored_concept,
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
