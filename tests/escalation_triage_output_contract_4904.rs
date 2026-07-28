//! Escalation-triage OUTPUT-CONTRACT + marker-scrub gate — issue #4904.
//!
//! Step 7 (TDD) acceptance tests for "triage & course-correct a blocked goal
//! before escalating to a human". They lock the *observable contract* the
//! escalation-triage brain (`prompt_assets/simard/overseer/escalation_triage.md`)
//! must satisfy for the #4904 case — the case where a goal to lift Simard's test
//! coverage above 70% was recorded blocked, never escalated for 24h, yet had in
//! fact already been delivered by merged work.
//!
//! The reasoning itself lives in the agentic recipe (guideline G3), so these are
//! CONTRACT tests, not unit tests of imperative reasoning code. They assert:
//!
//!   1. A reusable **marker-scrub gate** (`marker_scrub_violations`) that scans
//!      any operator-facing string for the forbidden internal markers and
//!      returns every hit. This is the zero-leak hard gate the recipe/reference
//!      doc promise; the tests prove it has teeth (flags every forbidden token
//!      AND the raw #4904 diagnostic inputs) and does not false-positive on
//!      clean plain English.
//!   2. The **six-key output contract** for the #4904 worked example published in
//!      `docs/reference/escalation-triage-api.md`: exactly the six keys, a valid
//!      `decision` enum value, the `escalate` null-rule, and — critically — that
//!      every operator-facing string passes the marker-scrub gate.
//!   3. The reference doc documents the full scrub list, the decision enum, and
//!      the two deterministic rails (`simard goal complete` / `handle_complete`
//!      and `notify()`), including idempotency + the three completion outcomes.
//!
//! These are shaped like the checks a human reviewer would run by hand, and they
//! red if the deliverable regresses (wrong enum, non-null escalate on a
//! non-question decision, a leaked marker token, an extra/missing key, or a
//! reference doc that stops documenting the gate/enum/rails).

use std::path::PathBuf;

// ════════════════════════════════════════════════════════════════════════════
// Marker-scrub gate — the forbidden internal tokens the operator must NEVER see.
// ════════════════════════════════════════════════════════════════════════════

/// Every internal marker / lock token that is forbidden in ANY operator-facing
/// string (the six-key JSON output and every Signal/email body). Superset of the
/// recipe's translate-never-forward list plus the #4904-specific raw diagnostic
/// fragments (the typed blocked-terminal outcome UUID and the raw reason marker).
///
/// The 🔒 lock glyph is written as its escape so this source file itself never
/// contains the raw marker.
const FORBIDDEN_MARKERS: &[&str] = &[
    "OODA-SAFEGUARD",
    "UNCLEAR-CRITERIA",
    "GENUINELY-STUCK",
    "health-review:blocked-terminal",
    "blocked-terminal outcome",
    "why=",
    "evidence=[",
    "\u{1F512}", // 🔒 lock token
];

/// The #4904 raw diagnostic inputs (translate-only, never surfaced). Used to
/// prove the gate catches the exact strings this task was handed.
const RAW_INTERNAL_WHY_4904: &str = "typed blocked-terminal outcome \
019f6c08-d053-7d93-89bf-f1f86aee408c on goal 4d27c91a; never escalated \
(escalations=0 across all overseer ticks in the last 24h)";
const RAW_REASON_MARKER_4904: &str = "health-review:blocked-terminal";

/// The marker-scrub GATE: return every forbidden marker present in `text`.
/// Empty result == clean == safe to show a human.
fn marker_scrub_violations(text: &str) -> Vec<&'static str> {
    FORBIDDEN_MARKERS
        .iter()
        .copied()
        .filter(|m| text.contains(m))
        .collect()
}

// ════════════════════════════════════════════════════════════════════════════
// Doc/asset access helpers.
// ════════════════════════════════════════════════════════════════════════════

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("expected {rel} to exist and be readable: {e}"))
}

/// Extract the first ```json fenced block that appears AFTER `heading` in a
/// Markdown document. Returns the raw JSON text (fences stripped).
fn json_block_after_heading(markdown: &str, heading: &str) -> String {
    let after = markdown
        .split_once(heading)
        .unwrap_or_else(|| panic!("doc is missing the {heading:?} heading"))
        .1;
    let start = after
        .find("```json")
        .unwrap_or_else(|| panic!("no ```json block after the {heading:?} heading"));
    let body = &after[start + "```json".len()..];
    let end = body
        .find("```")
        .unwrap_or_else(|| panic!("unterminated ```json block after {heading:?}"));
    body[..end].trim().to_string()
}

const REFERENCE_DOC: &str = "docs/reference/escalation-triage-api.md";

/// Parse the published #4904 worked-output JSON from the reference doc.
fn worked_output_4904() -> serde_json::Map<String, serde_json::Value> {
    let doc = read_repo_file(REFERENCE_DOC);
    let raw = json_block_after_heading(&doc, "### Worked output (#4904)");
    let value: serde_json::Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("the #4904 worked-output block is not valid JSON: {e}\n{raw}"));
    value
        .as_object()
        .expect("the #4904 worked output must be a JSON object")
        .clone()
}

const EXPECTED_KEYS: &[&str] = &[
    "problem",
    "next_step",
    "root_cause",
    "decision",
    "action_taken",
    "escalate",
];
const DECISION_ENUM: &[&str] = &[
    "rewrite-done-gate",
    "complete-delivered-goal",
    "ask-operator-one-question",
];

// ════════════════════════════════════════════════════════════════════════════
// Section A — the marker-scrub gate has teeth (and no false positives).
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn gate_flags_every_forbidden_marker_individually() {
    for marker in FORBIDDEN_MARKERS {
        let text = format!("operator update: everything is fine {marker} trailing words");
        let hits = marker_scrub_violations(&text);
        assert!(
            hits.contains(marker),
            "the marker-scrub gate must flag the forbidden token {marker:?}, got {hits:?}"
        );
    }
}

#[test]
fn gate_passes_clean_plain_english() {
    let clean = "Simard's work to lift automated test coverage above 70% was recorded as stuck \
                 and then left alone, so it made no further progress and nobody was told. The \
                 coverage work has already been delivered by merged changes, so the goal can be \
                 closed and nothing is needed from you.";
    let hits = marker_scrub_violations(clean);
    assert!(
        hits.is_empty(),
        "clean plain-English operator text must pass the gate, but tripped on {hits:?}"
    );
}

/// The gate must catch the exact raw diagnostic inputs this task was handed —
/// proving translation (not passthrough) is required before a human sees them.
#[test]
fn gate_catches_the_raw_4904_diagnostic_inputs() {
    let why_hits = marker_scrub_violations(RAW_INTERNAL_WHY_4904);
    assert!(
        why_hits.contains(&"blocked-terminal outcome"),
        "the raw internal_why must trip the gate on 'blocked-terminal outcome', got {why_hits:?}"
    );

    let marker_hits = marker_scrub_violations(RAW_REASON_MARKER_4904);
    assert!(
        marker_hits.contains(&"health-review:blocked-terminal"),
        "the raw reason_marker must trip the gate, got {marker_hits:?}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section B — the #4904 six-key output contract.
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn worked_output_has_exactly_the_six_contract_keys() {
    let obj = worked_output_4904();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected: Vec<&str> = EXPECTED_KEYS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        keys, expected,
        "the #4904 output must have EXACTLY the six contract keys — no more, no fewer"
    );
}

#[test]
fn worked_output_decision_is_complete_delivered_goal() {
    let obj = worked_output_4904();
    let decision = obj
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .expect("decision must be a string");
    assert!(
        DECISION_ENUM.contains(&decision),
        "decision {decision:?} must be one of the enum values {DECISION_ENUM:?}"
    );
    assert_eq!(
        decision, "complete-delivered-goal",
        "for #4904 the evidence (merged coverage work) forces the complete-delivered-goal decision"
    );
}

/// `escalate` is non-null ONLY when `decision == ask-operator-one-question`.
/// For #4904 (`complete-delivered-goal`) it must be JSON null.
#[test]
fn worked_output_escalate_honours_the_null_rule() {
    let obj = worked_output_4904();
    let decision = obj
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .unwrap();
    let escalate = obj.get("escalate").expect("escalate key must be present");

    if decision == "ask-operator-one-question" {
        assert!(
            !escalate.is_null(),
            "ask-operator-one-question must carry a non-null escalate reason"
        );
    } else {
        assert!(
            escalate.is_null(),
            "decision {decision:?} must have escalate == null, got {escalate:?}"
        );
    }
}

/// Every operator-facing string value in the six-key output must pass the gate.
#[test]
fn worked_output_is_entirely_marker_free() {
    let obj = worked_output_4904();
    for key in EXPECTED_KEYS {
        if let Some(s) = obj.get(*key).and_then(serde_json::Value::as_str) {
            let hits = marker_scrub_violations(s);
            assert!(
                hits.is_empty(),
                "output field {key:?} leaks forbidden marker(s) {hits:?}: {s:?}"
            );
        }
    }
}

/// The action taken must reflect the goal-completion rail (mark complete /
/// removed from the board / recorded so it cannot be reopened) rather than a
/// promise to escalate — this is course-correction, not a bare notify-and-count.
#[test]
fn worked_output_action_reflects_goal_completion() {
    let obj = worked_output_4904();
    let action = obj
        .get("action_taken")
        .and_then(serde_json::Value::as_str)
        .expect("action_taken must be a string")
        .to_lowercase();
    assert!(
        action.contains("complete") || action.contains("closed") || action.contains("done"),
        "action_taken must describe completing/closing the delivered goal: {action:?}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section C — the reference doc documents the gate, the enum, and the rails.
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn reference_doc_documents_the_full_scrub_list() {
    let doc = read_repo_file(REFERENCE_DOC);
    for marker in FORBIDDEN_MARKERS {
        assert!(
            doc.contains(marker),
            "the reference doc's marker-scrub section must list the forbidden token {marker:?}"
        );
    }
    let lower = doc.to_lowercase();
    assert!(
        lower.contains("zero-leak") || lower.contains("blocks the emit"),
        "the reference doc must describe the scrub as a hard zero-leak gate, not best-effort"
    );
}

#[test]
fn reference_doc_documents_the_decision_enum() {
    let doc = read_repo_file(REFERENCE_DOC);
    for value in DECISION_ENUM {
        assert!(
            doc.contains(value),
            "the reference doc must document the decision enum value {value:?}"
        );
    }
}

#[test]
fn reference_doc_documents_both_deterministic_rails() {
    let doc = read_repo_file(REFERENCE_DOC);
    let lower = doc.to_lowercase();

    // Rail 1: goal-completion seam + idempotency + the three outcomes.
    assert!(
        doc.contains("simard goal complete") && lower.contains("handle_complete"),
        "the reference doc must document the `simard goal complete` / handle_complete rail"
    );
    assert!(
        lower.contains("idempotent") && lower.contains("tombstone"),
        "the reference doc must document idempotency and the durable tombstone"
    );
    for outcome in ["reopened", "completed", "absent"] {
        assert!(
            lower.contains(outcome),
            "the reference doc must document the {outcome:?} completion outcome"
        );
    }

    // Rail 2: the dual-channel operator notifier with the goal-blocked kind.
    assert!(
        lower.contains("notify") && doc.contains("goal-blocked"),
        "the reference doc must document the notify() rail and the goal-blocked kind"
    );
    assert!(
        lower.contains("three")
            && (lower.contains("update") || lower.contains("message") || lower.contains("signal")),
        "the reference doc must document the three required plain-English operator updates"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section D — the JSON-block extractor helper behaves (guards the tests above).
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn json_block_extractor_reads_the_first_block_after_a_heading() {
    let md = "# Doc\n\n### Target\n\n```json\n{\"a\": 1}\n```\n\ntrailing\n";
    let block = json_block_after_heading(md, "### Target");
    assert_eq!(block, "{\"a\": 1}");
}
