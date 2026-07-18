//! Goal-identity dedup primitives for idempotent done-gate PR emission
//! (Problem 4, issues [#4166]/[#4189]).
//!
//! Total, no-panic, no-I/O bricks — mirrors [`crate::stewardship::dedup`]:
//!
//! - [`goal_dedup_key`] — a stable, one-way, 16-lowercase-hex key derived ONLY
//!   from durable goal identity (`goal_id` + `repo`), NEVER from the goal title.
//! - [`parse_goal_key_trailer`] — the total/no-panic parser for the
//!   `Simard-Goal-Key:` PR-body trailer stamped by the emitting engineer.
//! - [`find_open_pr_for_goal`] — matches a goal-key against a list of open PRs
//!   by trailer (primary) then head-branch convention (fallback).
//!
//! These are the pure surface consulted by the third `dispatch_spawn_engineer`
//! guard and the advisory `gh` reconciliation. See
//! `docs/concepts/idempotent-done-gate-pr-emission.md` and
//! `docs/reference/goal-pr-emission-ledger-api.md`.
//!
//! [#4166]: https://github.com/rysweet/Simard/issues/4166
//! [#4189]: https://github.com/rysweet/Simard/issues/4189
//!
//! # Status
//! The bricks below are implemented and GREEN under the accompanying
//! `#[cfg(test)]` contract suite. They are the pure surface the advisory `gh`
//! reconciliation and the third `dispatch_spawn_engineer` guard consult; until
//! that guard is wired in a follow-up, only the inline tests exercise them, so
//! the module carries `#![allow(dead_code)]`.

// These bricks are consumed by the third `dispatch_spawn_engineer` guard, wired
// in a follow-up; until then only the inline tests exercise them.
#![allow(dead_code)]

#[allow(unused_imports)]
pub use crate::stewardship::merge_authority::GoalPrRef;

/// Line-anchored, case-sensitive prefix of the goal-key PR-body trailer.
pub const GOAL_KEY_TRAILER_PREFIX: &str = "Simard-Goal-Key: ";

/// Head-branch prefix an engineer names its branch with: `engineer/{key}-<slug>`.
pub const GOAL_BRANCH_PREFIX: &str = "engineer/";

/// Only the first `MAX_TRAILER_SCAN_BYTES` of a (attacker-controllable) PR body
/// are scanned for the trailer, bounding worst-case parse cost.
pub const MAX_TRAILER_SCAN_BYTES: usize = 64 * 1024;

/// Stable, one-way goal-identity key: the first 16 lowercase-hex chars of
/// `sha256(frame(goal_id) + "\n" + frame(repo))`, where `frame` backslash-escapes
/// any literal `\` and newline. Never derived from the goal title.
///
/// The escaping is an identity no-op for ordinary (backslash-free, newline-free)
/// identities — so the preimage reduces to the plain `goal_id + "\n" + repo` —
/// while keeping the encoding injective, so a newline embedded in one field can
/// never be confused with the field boundary (e.g. `(id="a", repo="b\nc")` and
/// `(id="a\nb", repo="c")` yield distinct keys).
///
/// Total and deterministic: identical `(goal_id, repo)` always yields the same
/// key across daemon restarts, engineer churn, and re-planning.
pub fn goal_dedup_key(goal_id: &str, repo: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(frame_identity_field(goal_id).as_bytes());
    hasher.update(b"\n");
    hasher.update(frame_identity_field(repo).as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for byte in &digest[..8] {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Injective, boundary-safe framing of one identity field: backslash-escape any
/// literal `\` (`\` -> `\\`) and newline (`\n` -> `\` + `n`). An identity no-op
/// when the field contains neither byte, so ordinary identities hash the plain
/// `goal_id + "\n" + repo` preimage.
fn frame_identity_field(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for ch in field.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// Parse the `Simard-Goal-Key:` body trailer, returning the 16-hex key iff a
/// single unambiguous valid trailer is present. Total/no-panic on arbitrary
/// input. Contract:
///
/// | Rule | Behaviour |
/// | --- | --- |
/// | Prefix | Exact, line-anchored `^Simard-Goal-Key: ` (case-sensitive). |
/// | Value | Must match `^[0-9a-f]{16}$`; anything else is ignored. |
/// | Cap | Only the first [`MAX_TRAILER_SCAN_BYTES`] of the body are scanned. |
/// | Multiple | Two or more *distinct* valid trailers ⇒ `None` (ambiguous). |
/// | No panic | Control chars / truncation handled without panicking. |
pub fn parse_goal_key_trailer(body: &str) -> Option<String> {
    // Bound worst-case parse cost against an attacker-controllable body: scan
    // only the first MAX_TRAILER_SCAN_BYTES, truncated to a char boundary so the
    // slice never panics on multi-byte input.
    let mut end = MAX_TRAILER_SCAN_BYTES.min(body.len());
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    let scanned = &body[..end];

    let mut found: Option<String> = None;
    for line in scanned.lines() {
        let Some(value) = line.strip_prefix(GOAL_KEY_TRAILER_PREFIX) else {
            continue;
        };
        if !is_goal_key(value) {
            continue;
        }
        match &found {
            // First valid trailer.
            None => found = Some(value.to_string()),
            // A second, DISTINCT valid trailer is ambiguous ⇒ ignore all.
            Some(existing) if existing != value => return None,
            // Identical duplicate ⇒ still unambiguous.
            Some(_) => {}
        }
    }
    found
}

/// A goal key is exactly 16 lowercase-hex characters (`^[0-9a-f]{16}$`).
fn is_goal_key(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Given the goal-key and a list of open PRs, return the matching PR by
/// precedence: (1) `Simard-Goal-Key:` body trailer, then (2) the head-branch
/// convention `engineer/{goal-key}-` with a `-` boundary guard. `None` if none
/// match.
pub fn find_open_pr_for_goal<'a>(
    goal_key: &str,
    open_prs: &'a [GoalPrRef],
) -> Option<&'a GoalPrRef> {
    // Precedence (1): a PR whose body carries the goal-key trailer.
    if let Some(pr) = open_prs
        .iter()
        .find(|pr| parse_goal_key_trailer(&pr.body).as_deref() == Some(goal_key))
    {
        return Some(pr);
    }
    // Precedence (2): the head-branch convention `engineer/{goal-key}-`, with a
    // trailing `-` boundary so a longer key is never matched by a shorter prefix.
    let branch_prefix = format!("{GOAL_BRANCH_PREFIX}{goal_key}-");
    open_prs
        .iter()
        .find(|pr| pr.head_ref_name.starts_with(&branch_prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    // ── goal_dedup_key ─────────────────────────────────────────────────────

    /// Independently recompute the specified algorithm: first 16 lowercase-hex
    /// chars of sha256(goal_id + "\n" + repo).
    fn expected_key(goal_id: &str, repo: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(goal_id.as_bytes());
        hasher.update(b"\n");
        hasher.update(repo.as_bytes());
        let digest = hasher.finalize();
        let mut out = String::with_capacity(16);
        for b in &digest[..8] {
            out.push_str(&format!("{b:02x}"));
        }
        out
    }

    #[test]
    fn key_matches_sha256_of_id_newline_repo_truncated_16_hex() {
        let key = goal_dedup_key("coin-benchmark", "rysweet/Simard");
        assert_eq!(key, expected_key("coin-benchmark", "rysweet/Simard"));
    }

    #[test]
    fn key_is_deterministic() {
        assert_eq!(
            goal_dedup_key("kgpacks-parity", "rysweet/Simard"),
            goal_dedup_key("kgpacks-parity", "rysweet/Simard"),
        );
    }

    #[test]
    fn key_is_16_lowercase_hex() {
        let key = goal_dedup_key("some-goal", "owner/repo");
        assert_eq!(key.len(), 16, "key must be exactly 16 chars");
        assert!(
            key.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "key must be lowercase hex only, got {key:?}",
        );
    }

    #[test]
    fn key_changes_with_goal_id() {
        assert_ne!(
            goal_dedup_key("goal-a", "rysweet/Simard"),
            goal_dedup_key("goal-b", "rysweet/Simard"),
        );
    }

    #[test]
    fn key_changes_with_repo() {
        assert_ne!(
            goal_dedup_key("same-goal", "rysweet/Simard"),
            goal_dedup_key("same-goal", "rysweet/other"),
        );
    }

    #[test]
    fn key_id_repo_boundary_is_not_ambiguous() {
        // The "\n" separator must prevent (id="a", repo="b\nc") from colliding
        // with (id="a\nb", repo="c"). Both flatten to the same byte stream only
        // if the separator is ignored.
        let a = goal_dedup_key("a", "b\nc");
        let b = goal_dedup_key("a\nb", "c");
        assert_ne!(a, b, "id/repo boundary must be unambiguous");
    }

    // ── parse_goal_key_trailer ─────────────────────────────────────────────

    const VALID_KEY: &str = "4f2a9c1e7b3d0a58";
    const OTHER_KEY: &str = "00112233445566ff";

    #[test]
    fn trailer_accepts_valid_line_anchored() {
        let body = format!("Some PR description.\n\nSimard-Goal-Key: {VALID_KEY}\n");
        assert_eq!(parse_goal_key_trailer(&body).as_deref(), Some(VALID_KEY));
    }

    #[test]
    fn trailer_accepts_when_first_line() {
        let body = format!("Simard-Goal-Key: {VALID_KEY}");
        assert_eq!(parse_goal_key_trailer(&body).as_deref(), Some(VALID_KEY));
    }

    #[test]
    fn trailer_requires_line_anchor() {
        // Mid-line occurrence (not at the start of a line) must NOT match.
        let body = format!("see Simard-Goal-Key: {VALID_KEY} inline");
        assert_eq!(parse_goal_key_trailer(&body), None);
    }

    #[test]
    fn trailer_prefix_is_case_sensitive() {
        let body = format!("simard-goal-key: {VALID_KEY}\n");
        assert_eq!(parse_goal_key_trailer(&body), None);
    }

    #[test]
    fn trailer_rejects_uppercase_hex_value() {
        let body = "Simard-Goal-Key: 4F2A9C1E7B3D0A58\n".to_string();
        assert_eq!(parse_goal_key_trailer(&body), None);
    }

    #[test]
    fn trailer_rejects_wrong_length_value() {
        let too_short = "Simard-Goal-Key: 4f2a9c1e\n".to_string();
        let too_long = format!("Simard-Goal-Key: {VALID_KEY}ff\n");
        assert_eq!(parse_goal_key_trailer(&too_short), None);
        assert_eq!(parse_goal_key_trailer(&too_long), None);
    }

    #[test]
    fn trailer_rejects_non_hex_value() {
        let body = "Simard-Goal-Key: zzzzzzzzzzzzzzzz\n".to_string();
        assert_eq!(parse_goal_key_trailer(&body), None);
    }

    #[test]
    fn trailer_identical_duplicates_still_match() {
        // Two IDENTICAL valid trailers are not "distinct" — the single value
        // is unambiguous.
        let body = format!("Simard-Goal-Key: {VALID_KEY}\nSimard-Goal-Key: {VALID_KEY}\n");
        assert_eq!(parse_goal_key_trailer(&body).as_deref(), Some(VALID_KEY));
    }

    #[test]
    fn trailer_multiple_distinct_valid_ignores_all() {
        let body = format!("Simard-Goal-Key: {VALID_KEY}\nSimard-Goal-Key: {OTHER_KEY}\n");
        assert_eq!(
            parse_goal_key_trailer(&body),
            None,
            "two distinct valid trailers are ambiguous ⇒ ignore all",
        );
    }

    #[test]
    fn trailer_beyond_scan_cap_is_ignored() {
        // Push the trailer past MAX_TRAILER_SCAN_BYTES with non-trailer padding.
        let mut body = "x".repeat(MAX_TRAILER_SCAN_BYTES + 16);
        body.push('\n');
        body.push_str(&format!("Simard-Goal-Key: {VALID_KEY}\n"));
        assert_eq!(parse_goal_key_trailer(&body), None);
    }

    #[test]
    fn trailer_within_scan_cap_is_found() {
        let mut body = "padding line\n".repeat(8);
        body.push_str(&format!("Simard-Goal-Key: {VALID_KEY}\n"));
        assert!(body.len() < MAX_TRAILER_SCAN_BYTES);
        assert_eq!(parse_goal_key_trailer(&body).as_deref(), Some(VALID_KEY));
    }

    #[test]
    fn trailer_is_total_on_control_chars_without_panicking() {
        // Must not panic on odd control characters / CRLF around the trailer.
        let body = format!("\u{0}\r\n\tSimard-Goal-Key: {VALID_KEY}\r\n\u{7}");
        // Whatever the match decision, the call must return without panicking.
        let _ = parse_goal_key_trailer(&body);
    }

    #[test]
    fn trailer_absent_returns_none() {
        assert_eq!(parse_goal_key_trailer("no trailer here\n"), None);
        assert_eq!(parse_goal_key_trailer(""), None);
    }

    // ── find_open_pr_for_goal ──────────────────────────────────────────────

    fn pr(number: u32, head: &str, body: &str) -> GoalPrRef {
        GoalPrRef {
            number,
            url: format!("https://github.com/rysweet/Simard/pull/{number}"),
            head_ref_name: head.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn matches_by_trailer() {
        let prs = vec![
            pr(10, "some-branch", "unrelated body"),
            pr(
                11,
                "another",
                &format!("body\nSimard-Goal-Key: {VALID_KEY}\n"),
            ),
        ];
        let found = find_open_pr_for_goal(VALID_KEY, &prs).expect("should match by trailer");
        assert_eq!(found.number, 11);
    }

    #[test]
    fn matches_by_branch_convention() {
        let prs = vec![pr(
            12,
            &format!("engineer/{VALID_KEY}-fix-thing"),
            "no trailer",
        )];
        let found =
            find_open_pr_for_goal(VALID_KEY, &prs).expect("should match by branch convention");
        assert_eq!(found.number, 12);
    }

    #[test]
    fn trailer_takes_precedence_over_branch() {
        // PR #20 carries the trailer; PR #21 only the branch. Trailer wins.
        let prs = vec![
            pr(21, &format!("engineer/{VALID_KEY}-x"), "no trailer"),
            pr(
                20,
                "unrelated-branch",
                &format!("Simard-Goal-Key: {VALID_KEY}\n"),
            ),
        ];
        let found = find_open_pr_for_goal(VALID_KEY, &prs).expect("match");
        assert_eq!(found.number, 20, "body trailer must take precedence");
    }

    #[test]
    fn branch_boundary_guard_rejects_key_prefix_collision() {
        // `engineer/{key}ff-...` must NOT match key {key} — the `-` boundary
        // guard prevents a longer key from being matched by a shorter prefix.
        let prs = vec![pr(
            30,
            &format!("engineer/{VALID_KEY}ff-more"),
            "no trailer",
        )];
        assert!(
            find_open_pr_for_goal(VALID_KEY, &prs).is_none(),
            "branch prefix without `-` boundary must not match",
        );
    }

    #[test]
    fn returns_none_when_no_pr_matches() {
        let prs = vec![
            pr(40, "random-branch", "nothing here"),
            pr(
                41,
                &format!("engineer/{OTHER_KEY}-x"),
                &format!("Simard-Goal-Key: {OTHER_KEY}\n"),
            ),
        ];
        assert!(find_open_pr_for_goal(VALID_KEY, &prs).is_none());
    }

    #[test]
    fn distinct_goal_pr_is_not_matched() {
        // A PR that belongs to a DIFFERENT goal must never be returned for this
        // goal's key — the property that keeps distinct-goal PRs unaffected.
        let prs = vec![pr(
            50,
            &format!("engineer/{OTHER_KEY}-y"),
            &format!("Simard-Goal-Key: {OTHER_KEY}\n"),
        )];
        assert!(find_open_pr_for_goal(VALID_KEY, &prs).is_none());
    }

    #[test]
    fn empty_pr_list_returns_none() {
        assert!(find_open_pr_for_goal(VALID_KEY, &[]).is_none());
    }
}
