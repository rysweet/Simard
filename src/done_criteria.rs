//! Shared done-criteria detector — the single source of truth for "does this
//! goal describe a checkable finish condition?".
//!
//! # Why this module exists (issue #4930)
//!
//! The OODA no-progress breaker classifies a stalled goal with **no** derivable,
//! machine-checkable done-criteria as `UNCLEAR-CRITERIA` and parks it for human
//! triage. The detector that answers "are the criteria derivable?" originally
//! lived **private** inside [`crate::ooda_loop`]'s no-progress reasoner, while the
//! operator remedy that *repairs* such a goal (a
//! [`crate::goal_board_store::DoneGatePin`]) lived in a different module. Two
//! sides of the same predicate in two places drift — a copy that drops the length
//! cap re-opens an unbounded-scan DoS on untrusted goal text, and a repair that
//! the detector does not recognise silently fails to un-stick the goal.
//!
//! This module hoists the detector to one shared, hardened, pure "stud" that both
//! the classifier and the admission/repair path call. There is exactly one
//! definition of [`CRITERIA_HEADINGS`], one length cap, one checkable-item scan,
//! and one [`has_measurable_criteria`] predicate.
//!
//! # Safety contract
//!
//! Goal `description` text is **untrusted** (arbitrarily long, may carry control
//! chars or `--`-prefixed tokens). Every function here is total and panic-free and
//! bounds its work by [`DERIVE_CRITERIA_MAX_SCAN`]; no regex is used (no ReDoS),
//! and nothing echoes raw goal text — callers surface only a constant heading
//! token plus the goal id.

/// Maximum number of characters of an untrusted goal `description` that the
/// detector scans. The cap bounds the work so no adversarial description can
/// cause a panic or pathological scanning. Bytes past the cap are ignored.
pub const DERIVE_CRITERIA_MAX_SCAN: usize = 8192;

/// Recognised done-criteria section headings. Their presence (with at least one
/// concrete checkable item, see [`has_checkable_item`]) is the positive signal
/// that a goal's done-criteria are *derivable from its own description* — the
/// goal is not criteria-unclear, it spelled its criteria out. Matched
/// case-insensitively as substrings of the length-capped description.
pub const CRITERIA_HEADINGS: &[&str] = &[
    "acceptance criteria",
    "definition of done",
    "success criteria",
    "completion criteria",
    "done criteria",
    "done-criteria",
    "exit criteria",
];

/// Lower-cased form of [`crate::goal_board_store::DONE_WHEN_MARKER`], the sentinel
/// a [`crate::goal_board_store::DoneGatePin`] writes ahead of its operator finish
/// line. A goal carrying this marker has been *repaired* with a measurable anchor
/// (a specific PR/issue the completion gate certifies), so its criteria are
/// machine-checkable even though the finish line is prose rather than a markdown
/// bullet list. [`has_measurable_criteria`] treats the marker as a positive
/// signal so the detector agrees with the repair mechanism (issue #4930).
///
/// A `debug_assert!`-backed cross-module test
/// (`goal_board_store::done_gate_pins::tests::done_when_marker_matches_detector`)
/// keeps this in lock-step with the canonical `DONE_WHEN_MARKER` so the two never
/// drift.
pub(crate) const DONE_WHEN_MARKER_LOWER: &str = "\n\ndone when: ";

/// Produce the single length-capped, lower-cased scan buffer the detector reads.
///
/// One allocation, reused by the heading match and the checkable-item scan:
/// bullets, checkboxes and ordered markers are case-invariant, so a second
/// original-case copy is unnecessary.
#[must_use]
pub fn capped_lowercase_scan(description: &str) -> String {
    description
        .chars()
        .take(DERIVE_CRITERIA_MAX_SCAN)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// True when `text` contains at least one line that reads as a concrete,
/// checkable list item — a markdown bullet (`- `, `* `, `• `), a checkbox
/// (`[ ]` / `[x]`), or an ordered item (`1.` / `2)`). Total and panic-free; used
/// to reject a bare criteria heading with no items (conservative derivation).
#[must_use]
pub fn has_checkable_item(text: &str) -> bool {
    fn starts_with_ordered_item(t: &str) -> bool {
        let mut saw_digit = false;
        for c in t.chars() {
            if c.is_ascii_digit() {
                saw_digit = true;
                continue;
            }
            return saw_digit && (c == '.' || c == ')');
        }
        false
    }
    text.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with("- ")
            || t.starts_with("* ")
            || t.starts_with("• ")
            || t.starts_with("[ ]")
            || t.starts_with("[x]")
            || t.starts_with("[X]")
            || starts_with_ordered_item(t)
    })
}

/// Return the first recognised [`CRITERIA_HEADINGS`] entry present in an
/// already-lower-cased `scan` buffer (see [`capped_lowercase_scan`]), or `None`.
#[must_use]
pub fn matched_criteria_heading(scan: &str) -> Option<&'static str> {
    CRITERIA_HEADINGS.iter().copied().find(|h| scan.contains(h))
}

/// The shared predicate: does `description` carry a checkable, machine-verifiable
/// finish condition?
///
/// True when **either**:
/// * the description states an explicit self-contained criteria section — a
///   recognised [`CRITERIA_HEADINGS`] heading **and** at least one concrete
///   [`has_checkable_item`] item; **or**
/// * the description carries an operator done-gate finish line
///   ([`DONE_WHEN_MARKER_LOWER`]), i.e. it was repaired by a
///   [`crate::goal_board_store::DoneGatePin`] binding a measurable PR/issue anchor.
///
/// The second arm is what keeps this predicate consistent with the repair
/// mechanism (issue #4930): a `goal set-done-gate` repair now genuinely satisfies
/// the detector instead of only passing through an unrelated code path.
///
/// Totality/safety: never panics; length-capped scan; no regex.
#[must_use]
pub fn has_measurable_criteria(description: &str) -> bool {
    let scan = capped_lowercase_scan(description);
    if matched_criteria_heading(&scan).is_some() && has_checkable_item(&scan) {
        return true;
    }
    scan.contains(DONE_WHEN_MARKER_LOWER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_plus_checkable_item_is_measurable() {
        let d = "Do the thing.\n\nAcceptance criteria:\n- ship the roster move";
        assert!(has_measurable_criteria(d));
        assert_eq!(
            matched_criteria_heading(&capped_lowercase_scan(d)),
            Some("acceptance criteria")
        );
    }

    #[test]
    fn bare_heading_without_item_is_not_measurable() {
        let d = "Definition of done: it is done when it feels done";
        assert!(!has_measurable_criteria(d));
    }

    #[test]
    fn ordered_and_checkbox_items_count() {
        assert!(has_measurable_criteria("Success criteria:\n1. merge PR"));
        assert!(has_measurable_criteria(
            "Exit criteria:\n[ ] close the issue"
        ));
    }

    #[test]
    fn done_gate_finish_line_is_measurable() {
        // The finish line a DoneGatePin writes — prose, no markdown bullet — must
        // still be recognised as measurable (issue #4930 A1<->A2/A3 consistency).
        let d = "Move the roster.\n\nDone when: roster is identity-owned \
                 (certified automatically when PR #4440 is merged).";
        assert!(has_measurable_criteria(d));
    }

    #[test]
    fn prose_without_criteria_is_not_measurable() {
        assert!(!has_measurable_criteria(
            "Make the governed repo roster live outside the framework somehow."
        ));
    }

    #[test]
    fn scan_is_length_bounded() {
        // Heading pushed past the cap must not be seen.
        let mut d = "x".repeat(DERIVE_CRITERIA_MAX_SCAN + 500);
        d.push_str("\n\nacceptance criteria:\n- item");
        assert!(!has_measurable_criteria(&d));
    }

    #[test]
    fn adversarial_input_does_not_panic() {
        for pathological in ["", "\0\0\0", "---\n--\n-", "• ", "1.", "[x]"] {
            let _ = has_measurable_criteria(pathological);
        }
    }
}
