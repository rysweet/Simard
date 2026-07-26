//! TDD contract tests for the ecosystem-observe **triage dispatch** stage
//! (issue #4803).
//!
//! The dispatch stage converts an `observed_problems.ctx` handoff into ONE
//! validated JSON array whose elements are either **briefs** (actionable
//! `smart-orchestrator` fix requests) or **escalations** (`recipe: null`
//! problems routed to a human operator). Like the rest of the agentic
//! observe/brief chain, the array is authored by the BRIEF agent — no Rust
//! ever parses the live handoff — so these tests PIN THE CONTRACT the way
//! `tests/ecosystem_observe_assets.rs` does: they assert the documentation and
//! the BRIEF prompt encode the schema/ordering/validation rules, and they
//! provide a self-contained validator + a canonical machine-checkable fixture
//! so the emitted contract is executable, not just prose.
//!
//! These tests are written FIRST (TDD). They fail until the implementation:
//!   1. lands the reconciled `docs/concepts/triage-dispatch.md` doc (linked in
//!      `mkdocs.yml`),
//!   2. reconciles the BRIEF prompt so `is_mechanical_sweep` / `sequence_group`
//!      are documented as OPTIONAL producer hints with defaults, and
//!   3. ships the canonical worked dispatch array as a checked-in fixture at
//!      `tests/fixtures/ecosystem_dispatch/canonical.json`.
//!
//! Rules encoded (from the dispatch contract):
//!   1. Top-level value is a JSON array.
//!   2. Every element is a brief (non-null `recipe`, the four canonical
//!      required fields present, NO `escalate` field) OR an escalation
//!      (`recipe` is `null`, `escalate` is a non-empty string).
//!   3. Every brief `target_repo` is a well-formed `owner/name`.
//!   4. Every brief `success_criteria` is a non-empty array of non-empty strings.
//!   5. Elements are ordered by blast radius, most-critical first.
//!   6. No element corresponds to a `dropped_as_in_flight` input problem.
//!   7. The array carries no credential-shaped secrets (heuristic token-shape
//!      check for shapes like `ghp_`, `AKIA`, PEM blocks, inline bearer, etc.).
//!      PII is producer-trust — it is not, and cannot be, machine-enforced here.
//!
//! Rules 1–4 are self-contained (checkable from the array alone) and are
//! exercised by the in-test `validate_dispatch_array` spec. Rule 7's
//! credential-shape heuristic is likewise self-contained (see `find_secret`),
//! though its PII clause is producer-trust. Rules 5–6 are source-relative;
//! rule 5's *intent* is exercised by `is_blast_radius_ordered`.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Asset helpers
// ---------------------------------------------------------------------------

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Read a required asset, panicking with a helpful message if absent.
fn asset(rel: &str) -> String {
    let path = repo_path(rel);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("asset {} must be readable: {e}", path.display()))
}

/// Read an asset that may not exist yet (used for failing-first existence pins).
fn try_asset(rel: &str) -> Option<String> {
    fs::read_to_string(repo_path(rel)).ok()
}

const DOC: &str = "docs/concepts/triage-dispatch.md";
const BRIEF_PROMPT: &str = "prompt_assets/simard/overseer/problem_to_brief.md";
const CANONICAL_FIXTURE: &str = "tests/fixtures/ecosystem_dispatch/canonical.json";
const MKDOCS: &str = "mkdocs.yml";

const REQUIRED_BRIEF_FIELDS: [&str; 4] = [
    "recipe",
    "task_description",
    "target_repo",
    "success_criteria",
];
const OPTIONAL_HINTS: [&str; 2] = ["is_mechanical_sweep", "sequence_group"];

// ===========================================================================
// GROUP A — Documentation contract pins (the reconciled doc lands in docs/)
// ===========================================================================

/// The reconciled dispatch doc must live under `docs/` so the docs-integrity
/// gate and mkdocs nav can see it — not stranded in a session folder.
#[test]
fn dispatch_doc_exists_under_docs() {
    assert!(
        try_asset(DOC).is_some(),
        "the reconciled triage-dispatch contract must be checked in at {DOC}"
    );
}

/// The doc is wired into the human-maintained mkdocs nav manifest so
/// `tests/docs_integrity.rs` treats it as a first-class, linked concept page.
#[test]
fn dispatch_doc_is_linked_in_mkdocs_nav() {
    let nav = asset(MKDOCS);
    assert!(
        nav.contains("concepts/triage-dispatch.md"),
        "mkdocs.yml nav must link concepts/triage-dispatch.md so the docs gate covers it"
    );
}

/// The doc pins the CANONICAL required brief schema (exactly the four design
/// fields) and explicitly marks the two producer hints OPTIONAL with defaults —
/// this is the reconciliation the schema divergence review demanded.
#[test]
fn dispatch_doc_pins_canonical_brief_schema() {
    let body = asset(DOC);
    for field in REQUIRED_BRIEF_FIELDS {
        assert!(
            body.contains(field),
            "doc must document the canonical required brief field `{field}`"
        );
    }
    // The hints are named AND flagged optional with their documented defaults.
    for hint in OPTIONAL_HINTS {
        assert!(
            body.contains(hint),
            "doc must document the optional producer hint `{hint}`"
        );
    }
    let lc = body.to_lowercase();
    assert!(
        lc.contains("optional") && lc.contains("hint"),
        "doc must describe is_mechanical_sweep / sequence_group as OPTIONAL producer hints"
    );
    assert!(
        lc.contains("default"),
        "doc must state the hints' defaults (is_mechanical_sweep -> false, sequence_group -> null)"
    );
}

/// The doc pins the escalation discriminator: an item is an escalation IFF its
/// `recipe` is `null`; briefs never carry `escalate`.
#[test]
fn dispatch_doc_pins_escalation_discriminator() {
    let body = asset(DOC).to_lowercase();
    assert!(
        body.contains("escalate"),
        "doc must document the `escalate` field for escalation items"
    );
    assert!(
        body.contains("null"),
        "doc must document that a `null` recipe marks an escalation"
    );
    assert!(
        body.contains("iff") || body.contains("if and only if"),
        "doc must state the discriminator as a biconditional (escalation iff recipe is null)"
    );
}

/// The doc pins the blast-radius ordering rule (most-critical first).
#[test]
fn dispatch_doc_pins_blast_radius_ordering() {
    let body = asset(DOC).to_lowercase();
    assert!(
        body.contains("blast radius"),
        "doc must specify blast-radius ordering"
    );
    assert!(
        body.contains("most-critical first") || body.contains("most-important first"),
        "doc must specify most-critical-first ordering"
    );
}

/// The doc pins the self-contained validation rules AND the least-privilege /
/// additive-only constraint applied to every brief.
#[test]
fn dispatch_doc_pins_validation_rules_and_least_privilege() {
    let body = asset(DOC);
    let lc = body.to_lowercase();
    assert!(
        lc.contains("validation rules"),
        "doc must carry a Validation rules section"
    );
    assert!(
        lc.contains("owner/name") || lc.contains("owner/repo"),
        "doc must require a well-formed owner/name target_repo (rule 3)"
    );
    assert!(
        lc.contains("additive") && (lc.contains("non-breaking") || lc.contains("non breaking")),
        "doc must state briefs are additive / non-breaking by default"
    );
    assert!(
        lc.contains("least privilege") || lc.contains("least-privilege"),
        "doc must state the least-privilege constraint for CI/workflow changes"
    );
    assert!(
        body.contains("dropped_as_in_flight"),
        "doc must state the dropped_as_in_flight exclusion (rule 6)"
    );
}

// ===========================================================================
// GROUP B — BRIEF prompt contract pins (the emitter encodes the schema)
// ===========================================================================

/// The BRIEF prompt is the live emitter; it must define the escalation shape
/// `{"recipe": null, "escalate": ...}` for non-actionable problems.
#[test]
fn brief_prompt_defines_escalation_shape() {
    let body = asset(BRIEF_PROMPT);
    assert!(
        body.contains("\"recipe\": null") && body.contains("escalate"),
        "BRIEF prompt must define the escalation shape {{\"recipe\": null, \"escalate\": ...}}"
    );
}

/// The BRIEF prompt lists the four canonical required brief fields.
#[test]
fn brief_prompt_lists_canonical_required_fields() {
    let body = asset(BRIEF_PROMPT);
    for field in REQUIRED_BRIEF_FIELDS {
        assert!(
            body.contains(field),
            "BRIEF prompt must reference the canonical required field `{field}`"
        );
    }
}

/// Reconciliation pin: the BRIEF prompt must present `is_mechanical_sweep` and
/// `sequence_group` as OPTIONAL producer hints (with defaults), matching the
/// design's canonical schema — not as if they were required output fields.
#[test]
fn brief_prompt_marks_hints_optional_with_defaults() {
    let body = asset(BRIEF_PROMPT);
    let lc = body.to_lowercase();
    for hint in OPTIONAL_HINTS {
        assert!(
            body.contains(hint),
            "BRIEF prompt must mention the hint `{hint}`"
        );
    }
    assert!(
        lc.contains("optional"),
        "BRIEF prompt must describe is_mechanical_sweep / sequence_group as OPTIONAL"
    );
    assert!(
        lc.contains("default"),
        "BRIEF prompt must state the hints' defaults when omitted (false / null)"
    );
}

// ===========================================================================
// GROUP C — Self-contained validator (rules 1–4, 7) as an executable spec
// ===========================================================================

/// Validate a dispatch array against the self-contained rules (1–4, 7).
/// Returns `Ok(())` if valid, otherwise `Err(reason)`.
fn validate_dispatch_array(v: &Value) -> Result<(), String> {
    // Rule 1: top-level is an array.
    let items = v.as_array().ok_or("top-level value must be a JSON array")?;
    if items.is_empty() {
        return Err("dispatch array must not be empty".into());
    }

    for (i, item) in items.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("element {i} must be a JSON object"))?;

        let recipe = obj
            .get("recipe")
            .ok_or_else(|| format!("element {i} must carry a `recipe` field"))?;

        if recipe.is_null() {
            // ----- Escalation branch (rule 2) -----
            let esc = obj
                .get("escalate")
                .ok_or_else(|| format!("escalation {i} must carry an `escalate` field"))?;
            let s = esc
                .as_str()
                .ok_or_else(|| format!("escalation {i} `escalate` must be a string"))?;
            if s.trim().is_empty() {
                return Err(format!("escalation {i} `escalate` must be non-empty"));
            }
        } else {
            // ----- Brief branch (rule 2) -----
            if obj.contains_key("escalate") {
                return Err(format!(
                    "brief {i} must NOT carry an `escalate` field (discriminator violation)"
                ));
            }
            let recipe_s = recipe
                .as_str()
                .ok_or_else(|| format!("brief {i} `recipe` must be a string"))?;
            if recipe_s.trim().is_empty() {
                return Err(format!("brief {i} `recipe` must be non-empty"));
            }
            // Canonical required fields present.
            for field in ["task_description", "target_repo", "success_criteria"] {
                if !obj.contains_key(field) {
                    return Err(format!("brief {i} is missing canonical field `{field}`"));
                }
            }
            let td = obj
                .get("task_description")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("brief {i} `task_description` must be a string"))?;
            if td.trim().is_empty() {
                return Err(format!("brief {i} `task_description` must be non-empty"));
            }
            // Rule 3: well-formed owner/name.
            let repo = obj
                .get("target_repo")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("brief {i} `target_repo` must be a string"))?;
            if !is_well_formed_repo(repo) {
                return Err(format!(
                    "brief {i} `target_repo` must be a well-formed owner/name, got {repo:?}"
                ));
            }
            // Rule 4: non-empty array of non-empty strings.
            let crit = obj
                .get("success_criteria")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("brief {i} `success_criteria` must be an array"))?;
            if crit.is_empty() {
                return Err(format!("brief {i} `success_criteria` must be non-empty"));
            }
            for (j, c) in crit.iter().enumerate() {
                let cs = c
                    .as_str()
                    .ok_or_else(|| format!("brief {i} success_criteria[{j}] must be a string"))?;
                if cs.trim().is_empty() {
                    return Err(format!("brief {i} success_criteria[{j}] must be non-empty"));
                }
            }
        }
    }

    // Rule 7: no credential-shaped secrets anywhere in the serialized array
    // (heuristic token-shape check; PII is producer-trust, not enforced here).
    if let Some(hit) = find_secret(&v.to_string()) {
        return Err(format!(
            "dispatch array must contain no secrets (matched {hit})"
        ));
    }

    Ok(())
}

/// A well-formed `owner/name`: exactly one `/`, both sides non-empty and drawn
/// from the GitHub-safe character set, no whitespace.
fn is_well_formed_repo(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        return false;
    }
    let ok = |seg: &str| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    ok(parts[0]) && ok(parts[1])
}

/// Heuristic secret detector for rule 7. Matches common credential shapes.
fn find_secret(text: &str) -> Option<&'static str> {
    const NEEDLES: [&str; 6] = [
        "ghp_",                   // GitHub personal access token
        "github_pat_",            // GitHub fine-grained PAT
        "AKIA",                   // AWS access key id
        "-----BEGIN",             // PEM private key block
        "xoxb-",                  // Slack bot token
        "Authorization: Bearer ", // inline bearer credential
    ];
    NEEDLES.into_iter().find(|n| text.contains(n))
}

/// Rule 5 intent: severity ranks must be non-increasing (most-critical first).
/// Lower rank == more critical (0 = ecosystem-wide blocker).
fn is_blast_radius_ordered(ranks: &[u8]) -> bool {
    ranks.windows(2).all(|w| w[0] <= w[1])
}

// ---- Fixture builders -----------------------------------------------------

fn valid_escalation() -> Value {
    json!({
        "recipe": null,
        "escalate": "Root fs is 100% full: bootstrap deadlock. Manually reclaim stale snapshots first."
    })
}

fn valid_brief() -> Value {
    json!({
        "recipe": "smart-orchestrator",
        "task_description": "In rysweet/amplihack-rs, enable a GitHub merge queue to relieve the merge live-lock. Additive / non-breaking; preserve the PRD.",
        "target_repo": "rysweet/amplihack-rs",
        "success_criteria": [
            "CI green on all required checks",
            "merge queue enabled; live-lock relieved"
        ]
    })
}

// ---- Positive spec tests --------------------------------------------------

#[test]
fn valid_array_of_escalation_then_brief_passes() {
    let arr = json!([valid_escalation(), valid_brief()]);
    assert_eq!(validate_dispatch_array(&arr), Ok(()));
}

#[test]
fn brief_with_optional_hints_present_is_valid() {
    let mut brief = valid_brief();
    brief["is_mechanical_sweep"] = json!(false);
    brief["sequence_group"] = Value::Null;
    let arr = json!([brief]);
    assert_eq!(validate_dispatch_array(&arr), Ok(()));
}

#[test]
fn brief_with_optional_hints_omitted_is_valid() {
    // Rule-2 note: omitted hints must not affect validity.
    let brief = valid_brief();
    assert!(brief.get("is_mechanical_sweep").is_none());
    assert!(brief.get("sequence_group").is_none());
    let arr = json!([brief]);
    assert_eq!(validate_dispatch_array(&arr), Ok(()));
}

// ---- Negative / edge / error tests ---------------------------------------

#[test]
fn top_level_object_is_rejected() {
    let not_array = json!({"recipe": null, "escalate": "x"});
    assert!(validate_dispatch_array(&not_array).is_err());
}

#[test]
fn empty_array_is_rejected() {
    assert!(validate_dispatch_array(&json!([])).is_err());
}

#[test]
fn brief_missing_task_description_is_rejected() {
    let mut brief = valid_brief();
    brief.as_object_mut().unwrap().remove("task_description");
    assert!(validate_dispatch_array(&json!([brief])).is_err());
}

#[test]
fn brief_carrying_escalate_field_is_rejected() {
    let mut brief = valid_brief();
    brief["escalate"] = json!("should not be here");
    assert!(validate_dispatch_array(&json!([brief])).is_err());
}

#[test]
fn escalation_with_empty_escalate_is_rejected() {
    let esc = json!({"recipe": null, "escalate": "   "});
    assert!(validate_dispatch_array(&json!([esc])).is_err());
}

#[test]
fn escalation_missing_escalate_is_rejected() {
    let esc = json!({"recipe": null});
    assert!(validate_dispatch_array(&json!([esc])).is_err());
}

#[test]
fn brief_with_malformed_target_repo_is_rejected() {
    for bad in [
        "no-slash",
        "too/many/slashes",
        "owner/",
        "/name",
        "own er/name",
    ] {
        let mut brief = valid_brief();
        brief["target_repo"] = json!(bad);
        assert!(
            validate_dispatch_array(&json!([brief])).is_err(),
            "target_repo {bad:?} should be rejected"
        );
    }
}

#[test]
fn well_formed_target_repo_is_accepted() {
    for good in [
        "rysweet/Simard",
        "rysweet/amplihack-rs",
        "rysweet/amplihack-recipe-runner",
    ] {
        assert!(is_well_formed_repo(good), "{good} should be well-formed");
    }
}

#[test]
fn brief_with_empty_success_criteria_is_rejected() {
    let mut brief = valid_brief();
    brief["success_criteria"] = json!([]);
    assert!(validate_dispatch_array(&json!([brief])).is_err());
}

#[test]
fn brief_with_blank_success_criterion_is_rejected() {
    let mut brief = valid_brief();
    brief["success_criteria"] = json!(["ok", "  "]);
    assert!(validate_dispatch_array(&json!([brief])).is_err());
}

#[test]
fn brief_with_non_array_success_criteria_is_rejected() {
    let mut brief = valid_brief();
    brief["success_criteria"] = json!("CI green");
    assert!(validate_dispatch_array(&json!([brief])).is_err());
}

#[test]
fn array_containing_a_secret_is_rejected() {
    let mut brief = valid_brief();
    brief["task_description"] = json!("use token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 to auth");
    assert!(
        validate_dispatch_array(&json!([brief])).is_err(),
        "rule 7: an embedded GitHub token must be rejected"
    );
}

#[test]
fn blast_radius_ordering_helper_enforces_non_increasing_criticality() {
    // P1 ecosystem-wide (0) -> P3 cross-cutting (1) -> P2 isolated (2): valid.
    assert!(is_blast_radius_ordered(&[0, 1, 2]));
    assert!(is_blast_radius_ordered(&[0, 0, 1]));
    // Isolated ahead of ecosystem-wide: invalid.
    assert!(!is_blast_radius_ordered(&[2, 0, 1]));
    assert!(!is_blast_radius_ordered(&[1, 0]));
}

// ===========================================================================
// GROUP D — Canonical machine-checkable fixture (the worked end-to-end array)
// ===========================================================================

/// The implementation must ship the canonical worked dispatch array as a
/// checked-in, machine-parseable fixture so the emitted contract is executable
/// (the doc's worked example uses `…` placeholders and cannot be parsed).
#[test]
fn canonical_fixture_exists_and_is_a_valid_dispatch_array() {
    let raw = try_asset(CANONICAL_FIXTURE).unwrap_or_else(|| {
        panic!("canonical dispatch fixture must be checked in at {CANONICAL_FIXTURE}")
    });
    let arr: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{CANONICAL_FIXTURE} must be valid JSON: {e}"));
    validate_dispatch_array(&arr)
        .unwrap_or_else(|e| panic!("{CANONICAL_FIXTURE} must satisfy the dispatch contract: {e}"));
}

/// The canonical fixture encodes the documented P1 -> P3 -> P2 shape: exactly
/// three elements — an escalation first, then the amplihack-rs merge-queue
/// brief, then the amplihack-recipe-runner mdBook brief.
#[test]
fn canonical_fixture_matches_documented_p1_p3_p2_shape() {
    let raw = try_asset(CANONICAL_FIXTURE)
        .unwrap_or_else(|| panic!("missing canonical fixture at {CANONICAL_FIXTURE}"));
    let arr: Value = serde_json::from_str(&raw).expect("fixture must be valid JSON");
    let items = arr.as_array().expect("fixture must be a JSON array");

    assert_eq!(
        items.len(),
        3,
        "canonical dispatch array has exactly 3 elements"
    );

    // Element 0: escalation (recipe null).
    assert!(
        items[0].get("recipe").map(Value::is_null).unwrap_or(false),
        "element 0 must be the P1 escalation (recipe: null)"
    );

    // Element 1: P3 amplihack-rs brief.
    assert_eq!(
        items[1].get("target_repo").and_then(Value::as_str),
        Some("rysweet/amplihack-rs"),
        "element 1 must be the P3 amplihack-rs merge-queue brief"
    );

    // Element 2: P2 amplihack-recipe-runner brief.
    assert_eq!(
        items[2].get("target_repo").and_then(Value::as_str),
        Some("rysweet/amplihack-recipe-runner"),
        "element 2 must be the P2 amplihack-recipe-runner mdBook brief"
    );

    // Both briefs are additive smart-orchestrator runs.
    for i in [1usize, 2] {
        assert_eq!(
            items[i].get("recipe").and_then(Value::as_str),
            Some("smart-orchestrator"),
            "brief {i} must target the smart-orchestrator recipe"
        );
    }
}
