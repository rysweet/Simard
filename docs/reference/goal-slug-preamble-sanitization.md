---
title: Goal-slug launcher-preamble sanitization
description: Reference for the launcher-preamble sanitizer that runs as the first step of goal_slug() in src/goals/types.rs. Titles carrying the Copilot CLI launch-log preamble (the "ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: <path>" line and similar non-goal startup lines) are stripped before kebab-casing, so derived goal slugs and engineer/<slug> branch names never leak host paths, config paths, or the NODE_OPTIONS value into git refs. The change is additive: clean single-line titles remain byte-identical to the pre-fix output, preserving stable goal IDs.
last_updated: 2026-07-21
owner: simard
doc_type: reference
status: reference
related:
  - ./goal-target-repo-routing.md
  - ./goal-board-api.md
  - ./typed-ooda-goal-session-rails.md
  - ./engineer-worktree-isolation.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ../reference/text-parsing-wire-formats.md
---

# Goal-slug launcher-preamble sanitization

> **Issue [#4376](https://github.com/rysweet/Simard/issues/4376).** The Copilot
> CLI launch-log preamble that the agent binary prepends to its stdout was
> leaking into derived **goal slugs** and **`engineer/<slug>` branch names**,
> risking corrupted automation refs and disclosing host/config paths inside git
> refs. `goal_slug()` now strips the launcher preamble as its first step, before
> any kebab-casing, so the derived slug and branch are always clean.

`goal_slug()` (in `src/goals/types.rs`) turns a goal **title** into a stable,
filesystem- and git-safe slug. That slug is the identity used for the goal ID
and for the engineer worktree branch (`engineer/<slug>-<suffix>`). When a title
is captured from an agent's stdout, it may carry the Copilot CLI's
**launch-log preamble** — non-goal startup lines the binary emits before its
real answer. Slugifying a preamble-polluted title produced a slug that embedded
fragments of a host path, a config path, or the `NODE_OPTIONS` value.

This reference documents the sanitizer, its API contract, the anchored
launcher shapes it recognises, and the invariants the regression suite pins.

For the sibling fix that strips the same preamble from **brain decision output**
at the `recipe_output` chokepoint, see
[Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md).
That fix protects the *decision parser*; this fix protects *slug/branch
derivation*. They operate on different surfaces and are both required.

---

## The problem

The Copilot CLI prepends lines like the following to stdout before the agent's
answer:

```text
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/azureuser/.amplihack/config
```

When such a line is captured as (or prepended to) a goal title and passed to the
pre-fix `goal_slug()`, kebab-casing turned the path separators and metacharacters
into dashes, yielding a slug such as:

```text
node-options-max-old-space-size-32768-saved-preference-to-change-home-azureuser-amplihack-config
```

Two failure modes follow:

1. **Information disclosure.** The host username (`azureuser`), the amplihack
   config path (`/home/azureuser/.amplihack/config`), and the `NODE_OPTIONS`
   tuning value are baked into the goal ID, the `engineer/<slug>` branch name,
   and therefore any pushed git ref and PR head — all durable, world-readable
   automation artifacts.
2. **Corrupted refs.** Metacharacters in a preamble (`/`, `..`, `~`, leading
   `-`) that survive into a branch name produce invalid or dangerous git refs
   (argv-injection via a leading `-`, path traversal via `..`, nested ref paths
   via `/`).

---

## The fix: strip via the shared launcher recognizer

`goal_slug()` strips the launcher preamble as its **first step** — before the
lowercase/kebab-case loop — by **reusing the existing launcher-line recognizer**
rather than re-implementing it. It reuses the narrow, **launcher-only**
predicate (`is_copilot_launcher_line`), *not* the broader `strip_recipe_noise`
(which also drops recipe-runner banner lines like `Recipe:` / `Steps:` /
`[completed]` and ISO-timestamped tracing lines — all of which are plausible
*legitimate* goal-title text and must not be eaten on the slug surface):

```rust
// Promote the launcher-only predicate to `pub(crate)` in
// src/recipe_output/extract.rs and reuse it — do NOT fork the shape list.
use crate::recipe_output::extract::is_copilot_launcher_line;

/// Drop only Copilot CLI launcher-preamble lines from a captured title, reusing
/// the single shared launcher recognizer so the shape list lives in exactly one
/// place. Deliberately narrower than `strip_recipe_noise`: it does NOT strip
/// recipe-runner banners or tracing lines, which are valid title text here.
fn strip_launcher_preamble(title: &str) -> Cow<'_, str> {
    if title.lines().any(is_copilot_launcher_line) {
        // rebuild without launcher lines, trim, return Owned
    } else {
        Cow::Borrowed(title) // zero-copy for clean titles
    }
}

pub fn goal_slug(title: &str) -> String {
    let title = strip_launcher_preamble(&title);
    // …existing lowercase / kebab-case / truncate-with-hash logic, unchanged…
}
```

> **Single recognizer, two surfaces — do not duplicate.** The anchored
> launcher-shape logic already lives once in
> [`src/recipe_output/extract.rs`](../concepts/copilot-launcher-preamble-stripping.md)
> as `is_copilot_launcher_line` (today private, backing `strip_recipe_noise`).
> `goal_slug()`'s wrapper must call that shared predicate — promote it to
> `pub(crate)` — not carry its own copy of the shape list. This preserves the
> sibling fix's "one chokepoint" design: the next time the Copilot CLI reshapes
> its banner, exactly **one** recognizer changes and every consumer — decision
> parsing *and* slug/branch derivation — re-hardens at once. The only new code
> on the slug side is the thin per-line filter wrapper (needed because the slug
> surface must keep runner-banner/tracing lines that `strip_recipe_noise`
> discards); the *shape knowledge* is not duplicated.

### What it strips

The wrapper drops only whole lines for which the shared
`is_copilot_launcher_line` predicate returns `true`, then returns the surviving
goal text. It therefore recognises the **same launcher family** the
`recipe_output` chokepoint does (and nothing more):

- The **`NODE_OPTIONS` saved-preference** info line — anchored on the joint
  signature: the line **starts with the `ℹ` info marker** (U+2139) **and**
  contains `NODE_OPTIONS=` **and** contains `(saved preference)`. All three
  conditions must hold; a title that merely mentions `NODE_OPTIONS` in prose
  (no leading `ℹ`, no `(saved preference)` marker) is **not** stripped.
- `Run 'copilot update' …` update-nag lines.
- `launching copilot binary=… version="GitHub Copilot CLI …"` launcher lines.
- Leading `INFO`/`WARN`-prefixed launcher lines that carry **no** ISO-8601
  timestamp (the shape that distinguishes a launcher banner from a real log
  line).

After the launcher lines are removed, the remaining lines are rejoined and the
result is trimmed. If nothing matched, the input is returned as
`Cow::Borrowed` (zero-copy) so clean titles pay no allocation cost.

### Anchoring, not substring matching

The cardinal risk is **over-stripping** a legitimate title — e.g. a goal that
literally reads *"Document the NODE_OPTIONS tuning we use"*. The sanitizer never
matches a bare substring; it matches only the **full preamble signature**:

| Guard | Effect |
|-------|--------|
| Joint-marker anchor (leading `ℹ` info marker **and** `NODE_OPTIONS=` **and** `(saved preference)`) | A title that mentions `NODE_OPTIONS` alone — with no leading `ℹ` and no `(saved preference)` marker — is preserved verbatim. |
| Whole-line match only | A launcher shape embedded mid-sentence in a real title does not trigger a strip; only a standalone launcher line is dropped. |
| Strip **before** normalization | Metacharacters (`/`, `..`, `~`, leading `-`) in a launcher line are removed *before* kebab-casing runs, so they can never survive into a branch name. |

---

## API contract

`goal_slug(title: &str) -> String` — unchanged public signature. New behavior:

| Input | Output |
|-------|--------|
| Clean single-line title within the length cap | **Byte-identical** to the pre-fix slug (stable IDs preserved). |
| Clean title over the cap | Truncated slug + 8-hex-char SHA-256 suffix, exactly as before. The hash is computed over the **stripped** title. |
| Preamble line + real title (multi-line) | Preamble line dropped; slug derives only from the real goal text. |
| Preamble line **only** (no goal text) | Empty-after-strip input yields the same empty-slug result the pre-fix code produced for whitespace-only titles; callers already treat an empty slug as an invalid goal record. |

### Invariants (regression-pinned)

The regression suite in `src/goals/types.rs` (alongside the existing
stability tests) asserts:

1. **Byte-identical stability.** Every case in
   `goal_slug_short_titles_are_byte_identical_to_legacy_behaviour` still passes
   unchanged — the fix is additive and does not alter clean-title output.
2. **Preamble → clean slug.** A title prefixed with the
   `ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: …`
   line produces a slug derived **only** from the goal text, with no
   `node-options`, `azureuser`, `amplihack`, or `config` fragments.
3. **Branch safety (negative property).** The derived `engineer/<slug>` branch
   contains no `/` (beyond the `engineer/` prefix), no `..`, and no leading `-`.
4. **No over-strip.** A legitimate title mentioning `NODE_OPTIONS` in prose is
   returned unmodified and slugifies normally.

---

## Examples

```rust
use simard::goals::types::goal_slug;

// Clean title — unchanged, byte-identical to legacy output.
assert_eq!(goal_slug("Fix broken features"), "fix-broken-features");

// Preamble-polluted title — the launcher line is stripped first.
let polluted = "ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). \
    To change: /home/azureuser/.amplihack/config\n\
    Redact websocket token from WSS connect logs";
assert_eq!(
    goal_slug(polluted),
    "redact-websocket-token-from-wss-connect-logs"
);

// Legitimate mention of NODE_OPTIONS — NOT stripped (no full signature).
assert_eq!(
    goal_slug("Document our NODE_OPTIONS tuning"),
    "document-our-node-options-tuning"
);
```

The engineer worktree that Simard branches for the second goal is
`engineer/redact-websocket-token-from-wss-connect-logs-<suffix>` — a clean,
disclosure-free ref, instead of a branch embedding
`/home/azureuser/.amplihack/config`.

---

## Security considerations

- **Information-disclosure fix.** The title is treated as **untrusted input**.
  Sanitization runs at the single derivation surface (`goal_slug`), so every
  slug and branch is protected regardless of which capture path produced the
  title.
- **Validate-after-normalize ordering.** Stripping runs *before* kebab-case
  normalization, guaranteeing that launcher metacharacters cannot be laundered
  into an otherwise-valid-looking branch name.
- **No new field visibility.** No struct field visibility is widened. The only
  visibility change permitted is promoting the **shared launcher predicate**
  (`is_copilot_launcher_line`) to `pub(crate)` so the slug wrapper can call the
  single canonical copy — never a forked duplicate of the shape list.
  Structured `tracing` + OTel only — the sanitizer emits no `print!`/`println!`
  and never logs the raw preamble it removed.

---

## Why the slug surface (not the capture point)

Stripping at `goal_slug()` is the **smallest surface that deterministically
protects both the slug and the branch**, because both derive from the same
function. Sanitizing further upstream at each capture point
(goal-curation / ooda-brain) was considered and left **out of scope**: it would
touch multiple call sites, risk drift, and could not guarantee protection for a
future capture path. If a specific capture path is later found to inject
non-launcher noise, that can be hardened additively without changing this
contract.

---

## Related reading

- [Concept: Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md)
  — the sibling fix that strips the same preamble from brain **decision output**
  at the `recipe_output` chokepoint.
- [Goal target-repo routing API reference](./goal-target-repo-routing.md)
  — how a goal's slug and target repo drive engineer worktree branching.
- [Engineer worktree isolation](./engineer-worktree-isolation.md)
  — how `engineer/<slug>` branches are allocated off the target repo.
- [Typed OODA goal/session rails](./typed-ooda-goal-session-rails.md)
  — the goal → session → branch identity chain the slug anchors.
