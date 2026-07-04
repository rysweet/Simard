//! The `rust-expert` knowledge pack (roadmap #2491, child #2493 — Acquisition).
//!
//! This is a small, bounded, in-process `agent-kgpacks-rs`-style knowledge pack
//! covering two competencies Simard exercises constantly in her own codebase:
//! **ownership / the borrow checker** and **error handling**. It is deliberately
//! narrow (a single vertical slice, per #2491's "first experiment") rather than
//! the whole Rust corpus.
//!
//! Every fact and procedure carries [`PackProvenance`] (source + URL + section +
//! version + retrieval date) so that, per the agent-kgpacks guarantee, every
//! learned item traces back to a specific authoritative source. Ingesting the
//! pack into cognitive memory (see [`super::ingest`]) turns these into durable
//! semantic facts and reusable procedures.

use serde::Serialize;

/// Sub-skill tag: move semantics and ownership transfer.
pub const SUBSKILL_OWNERSHIP: &str = "ownership";
/// Sub-skill tag: aliasing / mutable-vs-immutable borrow rules.
pub const SUBSKILL_BORROW_CHECKER: &str = "borrow-checker";
/// Sub-skill tag: lifetimes and elision.
pub const SUBSKILL_LIFETIMES: &str = "lifetimes";
/// Sub-skill tag: `Result` / `?` propagation.
pub const SUBSKILL_ERROR_HANDLING: &str = "error-handling";
/// Sub-skill tag: custom error types (`thiserror` / `anyhow`).
pub const SUBSKILL_ERROR_TYPES: &str = "error-types";

/// The five sub-skills this bounded pack covers, in reporting order.
pub const SUBSKILLS: [&str; 5] = [
    SUBSKILL_OWNERSHIP,
    SUBSKILL_BORROW_CHECKER,
    SUBSKILL_LIFETIMES,
    SUBSKILL_ERROR_HANDLING,
    SUBSKILL_ERROR_TYPES,
];

/// Traceable provenance for a learned item.
///
/// Mirrors the agent-kgpacks lifecycle requirement that "answers trace back to a
/// specific source article": `source` + `url` + `section` identify *where*, and
/// `version` + `retrieved` identify *which revision* and *when*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PackProvenance {
    /// Human-readable source title, e.g. "The Rust Programming Language".
    pub source: &'static str,
    /// Canonical URL for the cited section.
    pub url: &'static str,
    /// Section / chapter anchor within the source.
    pub section: &'static str,
    /// Source revision this fact was drawn from (edition / commit / channel).
    pub version: &'static str,
    /// ISO-8601 retrieval date.
    pub retrieved: &'static str,
}

/// A durable semantic fact in the pack.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PackFact {
    /// Short concept label (becomes the fact `concept`).
    pub concept: &'static str,
    /// The fact statement (becomes the fact `content`).
    pub content: &'static str,
    /// Confidence in the fact, `0.0..=1.0`.
    pub confidence: f64,
    /// Primary sub-skill this fact reinforces (one of [`SUBSKILLS`]).
    pub subskill: &'static str,
    /// Full tag set stored with the fact (always includes `subskill`).
    pub tags: &'static [&'static str],
    /// Where the fact came from.
    pub provenance: PackProvenance,
}

/// A reusable, named procedure in the pack (procedural-memory tier).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PackProcedure {
    /// Stable, semantically-indexed name (becomes the procedure `name`).
    pub name: &'static str,
    /// Ordered steps of the routine.
    pub steps: &'static [&'static str],
    /// Prerequisites (domain conditions) for applying the routine.
    pub prerequisites: &'static [&'static str],
    /// Sub-skill this procedure operationalizes (one of [`SUBSKILLS`]).
    pub subskill: &'static str,
    /// Where the procedure came from.
    pub provenance: PackProvenance,
}

/// A self-contained domain knowledge pack.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct RustExpertPack {
    /// Pack identifier (e.g. `rust-expert`).
    pub name: &'static str,
    /// One-line description of the bounded competency covered.
    pub description: &'static str,
    /// Durable semantic facts.
    pub facts: &'static [PackFact],
    /// Reusable procedures.
    pub procedures: &'static [PackProcedure],
}

impl RustExpertPack {
    /// Number of facts carrying `subskill`.
    pub fn facts_for(&self, subskill: &str) -> usize {
        self.facts.iter().filter(|f| f.subskill == subskill).count()
    }

    /// Number of procedures carrying `subskill`.
    pub fn procedures_for(&self, subskill: &str) -> usize {
        self.procedures
            .iter()
            .filter(|p| p.subskill == subskill)
            .count()
    }
}

// --- Provenance helpers (version-pinned to the 2024-edition Rust corpus) ---

const RETRIEVED: &str = "2026-07-04";

const fn prov(
    source: &'static str,
    section: &'static str,
    url: &'static str,
    version: &'static str,
) -> PackProvenance {
    PackProvenance {
        source,
        url,
        section,
        version,
        retrieved: RETRIEVED,
    }
}

const fn book(section: &'static str, url: &'static str) -> PackProvenance {
    prov(
        "The Rust Programming Language (\"the book\")",
        section,
        url,
        "2024 edition",
    )
}

const fn reference(section: &'static str, url: &'static str) -> PackProvenance {
    prov("The Rust Reference", section, url, "stable 1.95")
}

/// The bounded `rust-expert` pack (ownership/borrow-checker + error-handling).
///
/// 13 durable facts and 5 procedures, ≥2 facts and exactly 1 procedure per
/// sub-skill, so every [`SUBSKILLS`] entry is independently exercisable by the
/// competency gym (see [`super::scenarios`]).
pub static RUST_EXPERT_PACK: RustExpertPack = RustExpertPack {
    name: "rust-expert",
    description: "Bounded Rust competency: ownership / the borrow checker and error handling.",
    facts: &FACTS,
    procedures: &PROCEDURES,
};

static FACTS: [PackFact; 13] = [
    // --- ownership (3) ---
    PackFact {
        concept: "move-semantics",
        content: "Assigning or passing a value of a non-Copy type moves ownership; \
                  the original binding is invalidated and using it afterwards is a \
                  compile error (use-after-move).",
        confidence: 0.98,
        subskill: SUBSKILL_OWNERSHIP,
        tags: &["ownership", "move", "borrow-checker"],
        provenance: book(
            "Ch.4.1 What Is Ownership?",
            "https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html",
        ),
    },
    PackFact {
        concept: "copy-vs-clone",
        content: "Types implementing Copy (integers, bool, char, shared references, \
                  tuples of Copy) are duplicated on assignment instead of moved; for \
                  owned heap data implement or call .clone() explicitly.",
        confidence: 0.97,
        subskill: SUBSKILL_OWNERSHIP,
        tags: &["ownership", "copy", "clone"],
        provenance: book(
            "Ch.4.1 Stack-Only Data: Copy",
            "https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html#stack-only-data-copy",
        ),
    },
    PackFact {
        concept: "ownership-transfer-on-call",
        content: "Passing an owned value to a function transfers ownership into the \
                  callee; to keep using it in the caller, pass a reference (&T / &mut T) \
                  or return the value back.",
        confidence: 0.96,
        subskill: SUBSKILL_OWNERSHIP,
        tags: &["ownership", "functions", "references"],
        provenance: book(
            "Ch.4.1 Ownership and Functions",
            "https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html#ownership-and-functions",
        ),
    },
    // --- borrow-checker (3) ---
    PackFact {
        concept: "aliasing-xor-mutability",
        content: "At any point a value may have either one mutable reference (&mut T) \
                  or any number of shared references (&T), never both; violating this \
                  is the classic 'cannot borrow as mutable because also borrowed as \
                  immutable' error.",
        confidence: 0.98,
        subskill: SUBSKILL_BORROW_CHECKER,
        tags: &["borrow-checker", "aliasing", "mutability"],
        provenance: book(
            "Ch.4.2 The Rules of References",
            "https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html#the-rules-of-references",
        ),
    },
    PackFact {
        concept: "reference-must-not-outlive",
        content: "A reference must never outlive the value it points to; returning a \
                  reference to a local is a dangling-reference error — return the owned \
                  value or take the referent as a parameter instead.",
        confidence: 0.96,
        subskill: SUBSKILL_BORROW_CHECKER,
        tags: &["borrow-checker", "dangling", "lifetimes"],
        provenance: book(
            "Ch.4.2 Dangling References",
            "https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html#dangling-references",
        ),
    },
    PackFact {
        concept: "non-lexical-lifetimes",
        content: "Under non-lexical lifetimes a borrow ends at its last use, not at the \
                  end of the enclosing scope; reordering so the last read of a shared \
                  borrow precedes a later &mut borrow often resolves a conflict without \
                  clones.",
        confidence: 0.9,
        subskill: SUBSKILL_BORROW_CHECKER,
        tags: &["borrow-checker", "nll", "scopes"],
        provenance: book(
            "Ch.4.2 Mutable References (reference scope / NLL)",
            "https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html#mutable-references",
        ),
    },
    // --- lifetimes (2) ---
    PackFact {
        concept: "lifetime-elision-rules",
        content: "Lifetime elision assigns each input reference its own lifetime; if \
                  there is exactly one input lifetime it is given to all outputs, and \
                  for methods the lifetime of &self is given to all outputs — so most \
                  signatures need no explicit annotations.",
        confidence: 0.94,
        subskill: SUBSKILL_LIFETIMES,
        tags: &["lifetimes", "elision", "functions"],
        provenance: reference(
            "Lifetime elision",
            "https://doc.rust-lang.org/reference/lifetime-elision.html",
        ),
    },
    PackFact {
        concept: "struct-holding-reference",
        content: "A struct that stores a reference must declare a lifetime parameter \
                  (struct S<'a> { r: &'a T }) so the compiler can prove the struct never \
                  outlives the borrowed data.",
        confidence: 0.93,
        subskill: SUBSKILL_LIFETIMES,
        tags: &["lifetimes", "structs", "annotations"],
        provenance: book(
            "Ch.10.3 Lifetime Annotations in Struct Definitions",
            "https://doc.rust-lang.org/book/ch10-03-lifetime-syntax.html#lifetime-annotations-in-struct-definitions",
        ),
    },
    // --- error-handling (3) ---
    PackFact {
        concept: "result-vs-panic",
        content: "Use Result<T, E> for recoverable errors the caller should handle and \
                  reserve panic!/unwrap for unrecoverable bugs; returning Result keeps \
                  error handling explicit and composable.",
        confidence: 0.97,
        subskill: SUBSKILL_ERROR_HANDLING,
        tags: &["error-handling", "result", "panic"],
        provenance: book(
            "Ch.9 Error Handling",
            "https://doc.rust-lang.org/book/ch09-00-error-handling.html",
        ),
    },
    PackFact {
        concept: "question-mark-operator",
        content: "The ? operator on a Result returns the error early (after applying \
                  From conversion to the function's error type) and otherwise unwraps the \
                  Ok value, replacing verbose match-and-return error propagation.",
        confidence: 0.98,
        subskill: SUBSKILL_ERROR_HANDLING,
        tags: &["error-handling", "question-mark", "propagation"],
        provenance: book(
            "Ch.9.2 A Shortcut for Propagating Errors: the ? Operator",
            "https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html#a-shortcut-for-propagating-errors-the--operator",
        ),
    },
    PackFact {
        concept: "avoid-unwrap-in-libraries",
        content: "Avoid unwrap()/expect() on Result/Option in library code paths: they \
                  turn recoverable conditions into panics that abort the caller; prefer \
                  ? or explicit matching so callers choose how to handle failure.",
        confidence: 0.95,
        subskill: SUBSKILL_ERROR_HANDLING,
        tags: &["error-handling", "unwrap", "robustness"],
        provenance: prov(
            "Rust API Guidelines",
            "Dependability / error handling",
            "https://rust-lang.github.io/api-guidelines/dependability.html",
            "2024-05",
        ),
    },
    // --- error-types (2) ---
    PackFact {
        concept: "thiserror-for-libraries",
        content: "In library crates derive error enums with thiserror: #[derive(Error)] \
                  plus #[error(\"...\")] messages and #[from] for source conversions \
                  gives ergonomic, typed errors that implement std::error::Error and \
                  compose with ?.",
        confidence: 0.94,
        subskill: SUBSKILL_ERROR_TYPES,
        tags: &["error-types", "thiserror", "libraries"],
        provenance: prov(
            "thiserror crate documentation",
            "derive(Error)",
            "https://docs.rs/thiserror/latest/thiserror/",
            "1.x",
        ),
    },
    PackFact {
        concept: "anyhow-for-applications",
        content: "In application/binary code use anyhow::Result and .context(\"...\") to \
                  add human-readable context while propagating heterogeneous errors, \
                  keeping thiserror for the typed errors libraries expose.",
        confidence: 0.92,
        subskill: SUBSKILL_ERROR_TYPES,
        tags: &["error-types", "anyhow", "context"],
        provenance: prov(
            "anyhow crate documentation",
            "Context",
            "https://docs.rs/anyhow/latest/anyhow/",
            "1.x",
        ),
    },
];

static PROCEDURES: [PackProcedure; 5] = [
    PackProcedure {
        name: "rust-expert:fix-use-after-move",
        steps: &[
            "Read the E0382 'borrow of moved value' / 'value used here after move' span.",
            "Decide whether the callee needs ownership; if not, change the call/binding to borrow (&x or &mut x).",
            "If ownership is required in both places, clone at the move site or restructure so each owner is distinct.",
            "Re-run cargo build to confirm the move error is gone with no new warnings.",
        ],
        prerequisites: &[
            "value is a non-Copy type",
            "the binding is used after being moved",
        ],
        subskill: SUBSKILL_OWNERSHIP,
        provenance: book(
            "Ch.4.1 Ownership and Functions",
            "https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html#ownership-and-functions",
        ),
    },
    PackProcedure {
        name: "rust-expert:resolve-borrow-conflict",
        steps: &[
            "Identify the overlapping borrows in the E0502 message (immutable vs mutable).",
            "Shrink the shared borrow's scope so its last use precedes the &mut borrow (NLL).",
            "If overlap is unavoidable, copy the needed value out of the shared borrow before taking &mut.",
            "Re-run cargo build to confirm the aliasing conflict is resolved.",
        ],
        prerequisites: &["a &mut borrow overlaps an existing & borrow"],
        subskill: SUBSKILL_BORROW_CHECKER,
        provenance: book(
            "Ch.4.2 The Rules of References",
            "https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html#the-rules-of-references",
        ),
    },
    PackProcedure {
        name: "rust-expert:annotate-lifetimes",
        steps: &[
            "When E0106 'missing lifetime specifier' fires, add a named lifetime parameter to the item.",
            "Tie outputs to the correct input lifetime; for structs holding references declare struct S<'a>.",
            "Rely on elision where a single input lifetime or &self already determines the output.",
            "Re-run cargo build to confirm the borrow relationships type-check.",
        ],
        prerequisites: &["a signature or struct stores or returns a reference"],
        subskill: SUBSKILL_LIFETIMES,
        provenance: book(
            "Ch.10.3 Validating References with Lifetimes",
            "https://doc.rust-lang.org/book/ch10-03-lifetime-syntax.html",
        ),
    },
    PackProcedure {
        name: "rust-expert:propagate-with-question-mark",
        steps: &[
            "Change the function to return Result<T, E> (or anyhow::Result<T>).",
            "Replace each .unwrap()/.expect() on a fallible call with the ? operator.",
            "Ensure the function's error type implements From for each propagated error (or use #[from]).",
            "Re-run cargo build and cargo test to confirm no panics remain on the happy path.",
        ],
        prerequisites: &[
            "a function currently unwraps a Result/Option",
            "the caller can handle failure",
        ],
        subskill: SUBSKILL_ERROR_HANDLING,
        provenance: book(
            "Ch.9.2 A Shortcut for Propagating Errors: the ? Operator",
            "https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html#a-shortcut-for-propagating-errors-the--operator",
        ),
    },
    PackProcedure {
        name: "rust-expert:define-thiserror-enum",
        steps: &[
            "Add thiserror and derive #[derive(Debug, thiserror::Error)] on a crate error enum.",
            "Give each variant an #[error(\"...\")] message describing the failure.",
            "Use #[from] on wrapped source errors so ? converts them automatically.",
            "Return Result<T, MyError> from the crate's public API and cargo test the conversions.",
        ],
        prerequisites: &["a library crate needs a typed public error"],
        subskill: SUBSKILL_ERROR_TYPES,
        provenance: prov(
            "thiserror crate documentation",
            "derive(Error)",
            "https://docs.rs/thiserror/latest/thiserror/",
            "1.x",
        ),
    },
];
