//! TDD acceptance / gate suite for the ecosystem runner-hardening batch
//! (recipe-runner PR #131; upstream amplihack-rs #1018 / #1025 / #1024).
//!
//! # Why this suite is downstream-shaped
//!
//! The batch spans **three repositories**. The *code* fixes live upstream and
//! are **not** editable in this checkout:
//!
//!   * **P1** `Repo Guardian` credential-liveness probe + E2BIG child-env
//!     allow-list — upstream `rysweet/amplihack-recipe-runner` (PR #131) and
//!     `rysweet/amplihack-rs`. Root cause is an expired `ANTHROPIC_API_KEY`
//!     (infra 401), so the merge unblock is an **ops secret rotation**, not a
//!     code patch. The upstream regression tests for the probe live in those
//!     repos (see `tests/upstream/` spec artifacts shipped with this batch).
//!   * **P2** publish step-14 version derivation — upstream `amplihack-rs` #1018.
//!   * **P3** graceful reflect/iterate cancellation — upstream `amplihack-rs` #1025.
//!   * **P5** signal-subscriber daemon lifecycle — upstream `amplihack-rs` #1024.
//!   * **P4** #1015 and **P6** Simard backlog are **merge/ops escalations** with
//!     no code brief.
//!
//! Editing `rysweet/Simard` cannot fix upstream code. The **only** native lever
//! here is to *ingest* the landed P2/P3/P5 fixes by bumping the
//! `amplihack-agent-eval` git-rev pin (source repo `amplihack-rs`) to an
//! **audited SHA** and refreshing `Cargo.lock` — exactly the reactive done-gate
//! specified in
//! `docs/howto/ingest-ecosystem-hardening-fixes.md` and worked before by
//! `tests/issue_2626_amplihack_pin_bump.rs`.
//!
//! # What this suite therefore verifies (and what it deliberately defers)
//!
//! Two contracts are checkable in *this* tree today, so they are the acceptance
//! tests the batch's downstream deliverable is verified against:
//!
//!   1. **Documentation contract.** The reference + how-to design pages exist,
//!      are registered in the mkdocs nav, carry the honest
//!      `status: design — not yet implemented` frontmatter (never a premature
//!      `status: implemented`), name every problem's issue/PR, and do **not**
//!      invent the `AMPLIHACK_CHILD_ENV_ALLOWLIST` runtime tunable the design
//!      review struck (the child-env allow-list is a code constant upstream).
//!
//!   2. **Premature-bump gate (fail-closed).** Until the upstream fixes are
//!      **merged and audited**, the pin MUST stay at the pre-batch audited rev
//!      and MUST be an immutable full-SHA on the allow-listed remote. A bump to
//!      an unaudited / moving ref reds this suite. This is the same "audited SHA,
//!      not a moving branch" invariant the how-to's done-gate spells out.
//!
//! The forward **rev-bump acceptance** (pin == the *new* audited SHA, lockfile
//! refreshed, `cargo build && cargo test` green) is **not** hard-coded here: the
//! target SHA does not exist until P2/P3/P5 merge upstream. Fabricating a
//! placeholder SHA would be a false assertion. When the ingest step runs, whoever
//! performs the bump advances `EXPECTED_AGENT_EVAL_REV` below (and the
//! `Cargo.toml` / `Cargo.lock` pins) to the audited SHA in one commit — the
//! RED→GREEN transition, mirroring how #2626 landed.
//!
//! # Why these are file-shaped (std-only, rg/grep-shaped)
//!
//! Like `issue_2626_amplihack_pin_bump.rs` and `docs_integrity.rs`, they read the
//! raw `Cargo.toml` / `Cargo.lock` / docs with std only — no network, no crate
//! import — so an operator running the equivalent `grep` gets the same answer CI
//! does, decoupled from the heavy `simard` build.

use std::fs;
use std::path::PathBuf;

// ── Pin constants (verified against the current tree at authoring time) ──────

/// The audited pre-batch `amplihack-agent-eval` rev (source repo `amplihack-rs`).
/// It **predates** every P2/P3/P5 fix — the reference page records this exact
/// SHA as the pin that "predates all of them". Until the upstream fixes land and
/// are audited, the pin must stay here; the ingest step advances this constant.
const EXPECTED_AGENT_EVAL_REV: &str = "14dc30b10e87764120c6f2bae7f3630522c29e5d";

/// The only git remote `amplihack-agent-eval` may resolve from. A bump must never
/// introduce a new git source (typosquat / allow-list-bypass guard).
const AGENT_EVAL_REMOTE: &str = "https://github.com/rysweet/amplihack-rs.git";

const REFERENCE_DOC: &str = "docs/reference/ecosystem-hardening-pr131.md";
const HOWTO_DOC: &str = "docs/howto/ingest-ecosystem-hardening-fixes.md";

// ── Path / IO helpers ────────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {} ({e})", path.display()))
}

fn cargo_toml() -> String {
    read_repo_file("Cargo.toml")
}

fn cargo_lock() -> String {
    read_repo_file("Cargo.lock")
}

// ── Tiny structural matchers (std-only, comment-aware) ───────────────────────

/// The first non-comment manifest line whose key is exactly `name` (`name = …`).
fn manifest_dep_line(contents: &str, name: &str) -> Option<String> {
    contents
        .lines()
        .map(str::trim)
        .find(|l| {
            if l.starts_with('#') {
                return false;
            }
            match l.strip_prefix(name) {
                Some(rest) => rest.trim_start().starts_with('='),
                None => false,
            }
        })
        .map(str::to_string)
}

/// Extract the value of a `key = "value"` field from a single manifest line.
fn field_value(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key} = \"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn dep_rev(contents: &str, name: &str) -> Option<String> {
    field_value(&manifest_dep_line(contents, name)?, "rev")
}

fn dep_git_remote(contents: &str, name: &str) -> Option<String> {
    field_value(&manifest_dep_line(contents, name)?, "git")
}

/// The `source = "…"` string of the `[[package]]` named `name` in Cargo.lock.
fn locked_source(lockfile: &str, name: &str) -> Option<String> {
    let needle = format!("name = \"{name}\"");
    let mut lines = lockfile.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != needle {
            continue;
        }
        for following in lines.by_ref() {
            let t = following.trim();
            if let Some(v) = t.strip_prefix("source = \"") {
                return v.strip_suffix('"').map(str::to_string);
            }
            if t.starts_with("[[package]]") {
                break;
            }
        }
    }
    None
}

/// True when `rev` is a full 40-char lowercase hex git SHA (not a branch/tag).
fn is_full_sha(rev: &str) -> bool {
    rev.len() == 40
        && rev
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// The YAML frontmatter block (between the first two `---` fences).
fn frontmatter(contents: &str) -> &str {
    let body = contents.strip_prefix("---").unwrap_or(contents);
    match body.find("\n---") {
        Some(end) => &body[..end],
        None => "",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Contract 1 — the batch design documentation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn batch_reference_and_howto_docs_exist() {
    for rel in [REFERENCE_DOC, HOWTO_DOC] {
        let path = repo_root().join(rel);
        assert!(
            path.is_file(),
            "batch design page `{rel}` must exist — it is the specification this \
             suite verifies against."
        );
    }
}

#[test]
fn batch_docs_declare_honest_design_status_not_implemented() {
    // The design review's Issue 1: status must be honest. Nothing has shipped —
    // P2/P3/P5 are unmerged, P1's secret is unrotated, the pin predates all —
    // so claiming `status: implemented` would be a false completion signal.
    for rel in [REFERENCE_DOC, HOWTO_DOC] {
        let fm = read_repo_file(rel);
        let head = frontmatter(&fm);
        assert!(
            head.contains("status: design — not yet implemented"),
            "`{rel}` frontmatter must carry the honest \
             `status: design — not yet implemented` — nothing in this batch has \
             shipped yet."
        );
        assert!(
            !head.contains("status: implemented"),
            "`{rel}` frontmatter must NOT claim `status: implemented`: P2/P3/P5 are \
             unmerged upstream and the P1 credential is unrotated."
        );
    }
}

#[test]
fn batch_reference_names_every_problem_issue() {
    // Cross-cutting constraint: each problem is traceable to its issue/PR.
    let doc = read_repo_file(REFERENCE_DOC);
    for (id, token) in [
        ("P1 (Repo Guardian PR)", "#131"),
        ("P2 (version derivation)", "#1018"),
        ("P3 (graceful reflect stop)", "#1025"),
        ("P5 (subscriber lifecycle)", "#1024"),
        ("P4 (merge escalation)", "#1015"),
    ] {
        assert!(
            doc.contains(token),
            "reference page must name {id} by its tracking ref `{token}` so the \
             batch stays traceable."
        );
    }
}

#[test]
fn batch_docs_do_not_invent_child_env_allowlist_tunable() {
    // Design review Issue 2: the E2BIG child-env allow-list is a *code constant*
    // upstream, NOT a Simard runtime env var. The invented
    // `AMPLIHACK_CHILD_ENV_ALLOWLIST` tunable was struck; it must not reappear.
    for rel in [REFERENCE_DOC, HOWTO_DOC] {
        let doc = read_repo_file(rel);
        assert!(
            !doc.contains("AMPLIHACK_CHILD_ENV_ALLOWLIST"),
            "`{rel}` must not advertise a `AMPLIHACK_CHILD_ENV_ALLOWLIST` runtime \
             tunable — the child-env allow-list is a fixed code constant upstream, \
             not a Simard-settable variable."
        );
    }
}

#[test]
fn batch_docs_are_registered_in_mkdocs_nav() {
    // docs_integrity.rs enforces that every nav entry resolves to a file; this
    // enforces the converse for the batch — the two pages are actually wired into
    // the human-maintained nav so they are discoverable, not orphaned.
    let nav = read_repo_file("mkdocs.yml");
    for rel in [REFERENCE_DOC, HOWTO_DOC] {
        let nav_path = rel.strip_prefix("docs/").unwrap_or(rel);
        assert!(
            nav.contains(nav_path),
            "mkdocs.yml nav must reference `{nav_path}` so the batch page is \
             discoverable (and covered by the docs-integrity gate)."
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Contract 2 — the premature-bump gate (fail-closed until upstream is audited)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn agent_eval_pin_not_prematurely_bumped_before_upstream_lands() {
    // The ingest gate: do NOT bump `amplihack-agent-eval` until P2/P3/P5 have
    // merged upstream AND been audited. Until then the pin stays at the audited
    // pre-batch rev. When the ingest step runs, advance BOTH this constant and
    // `Cargo.toml`/`Cargo.lock` to the new audited SHA in one commit.
    let rev = dep_rev(&cargo_toml(), "amplihack-agent-eval")
        .expect("Cargo.toml must declare a git `amplihack-agent-eval` dependency with a `rev`");
    assert_eq!(
        rev, EXPECTED_AGENT_EVAL_REV,
        "amplihack-agent-eval pin drifted to `{rev}`. Until the upstream P2/P3/P5 \
         fixes land and are audited it must stay at the pre-batch rev \
         {EXPECTED_AGENT_EVAL_REV}. If you are performing the audited ingest, \
         update EXPECTED_AGENT_EVAL_REV in this test in the same commit."
    );
}

#[test]
fn agent_eval_pin_is_immutable_full_sha_on_allowlisted_remote() {
    // A pin must be an immutable 40-char SHA on the known remote — never a
    // `branch`/`tag` (a moving ref could swap the linked code between builds) and
    // never a new git host (typosquat / allow-list bypass).
    let toml = cargo_toml();
    let line = manifest_dep_line(&toml, "amplihack-agent-eval")
        .expect("missing `amplihack-agent-eval` dependency line in Cargo.toml");
    assert!(
        !line.contains("branch =") && !line.contains("tag ="),
        "`amplihack-agent-eval` must be pinned by an immutable `rev` SHA, not a \
         branch/tag: {line}"
    );
    let rev = dep_rev(&toml, "amplihack-agent-eval").expect("no `rev` pin");
    assert!(
        is_full_sha(&rev),
        "`amplihack-agent-eval` rev `{rev}` is not a full 40-char lowercase hex SHA."
    );
    let remote = dep_git_remote(&toml, "amplihack-agent-eval").unwrap_or_default();
    assert_eq!(
        remote, AGENT_EVAL_REMOTE,
        "`amplihack-agent-eval` must stay on {AGENT_EVAL_REMOTE}, found `{remote}`."
    );
}

#[test]
fn cargo_lock_agrees_with_agent_eval_pin() {
    // The lockfile must resolve the same audited rev from the same remote — no
    // drift between manifest intent and the resolved source.
    let source = locked_source(&cargo_lock(), "amplihack-agent-eval")
        .expect("Cargo.lock must contain the amplihack-agent-eval [[package]] source");
    assert!(
        source.contains(EXPECTED_AGENT_EVAL_REV),
        "Cargo.lock amplihack-agent-eval source must record rev \
         {EXPECTED_AGENT_EVAL_REV}; found `{source}`."
    );
    assert!(
        source.contains(AGENT_EVAL_REMOTE.trim_end_matches(".git")),
        "Cargo.lock amplihack-agent-eval source must resolve from {AGENT_EVAL_REMOTE}; \
         found `{source}`."
    );
}

#[test]
fn ingest_howto_encodes_audited_scoped_bump_gate() {
    // The how-to must specify the *scoped* lock refresh and the "audited SHA, not
    // a moving branch" gate — the operational contract the ingest step follows.
    let howto = read_repo_file(HOWTO_DOC);
    assert!(
        howto.contains("cargo update -p amplihack-agent-eval"),
        "the ingest how-to must document the scoped `cargo update -p \
         amplihack-agent-eval` (no unrelated lockfile churn)."
    );
    assert!(
        howto.contains("audited") && howto.contains("moving branch"),
        "the ingest how-to must gate the bump on an *audited* SHA and forbid a \
         *moving branch* ref (supply-chain guard)."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Pure-function pins — matcher logic verified independently of the tree
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn is_full_sha_accepts_only_immutable_revs() {
    assert!(is_full_sha(EXPECTED_AGENT_EVAL_REV));
    assert!(!is_full_sha("main")); // branch ref
    assert!(!is_full_sha("14dc30b")); // short SHA
    assert!(!is_full_sha("14DC30B10E87764120C6F2BAE7F3630522C29E5D")); // uppercase
    assert!(!is_full_sha(&"z".repeat(40))); // non-hex
}

#[test]
fn frontmatter_extracts_only_the_yaml_block() {
    let doc = "---\ntitle: x\nstatus: design — not yet implemented\n---\n\n# Body\nstatus: implemented (in prose, not frontmatter)\n";
    let fm = frontmatter(doc);
    assert!(fm.contains("status: design — not yet implemented"));
    assert!(
        !fm.contains("in prose"),
        "frontmatter() must stop at the closing fence and ignore the body"
    );
}
