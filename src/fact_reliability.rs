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
    let trimmed = raw
        .trim()
        .trim_matches(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '.' | ',' | ':' | ';')
        })
        .to_ascii_lowercase();

    // Unify separators (`_` and interior spaces behave as `-`) and collapse runs
    // of hyphens, then trim any leading/trailing hyphens the folding produced.
    let mut canon = String::with_capacity(trimmed.len());
    let mut prev_hyphen = false;
    for ch in trimmed.chars() {
        let c = if ch == '_' || ch == ' ' { '-' } else { ch };
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
    // provenance looks.
    let words = content.split_whitespace().count();
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
