---
title: Overview action-detail humanization
description: Reference for humanizeActionDetail — the client-side plain-English humanizer that strips brain/launcher jargon from the Overview tab's raw cycle action detail strings before they are escaped and rendered.
last_updated: 2026-06-28
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ./subagent-tmux-tracking.md
  - ./ooda-brain-decision-protocol.md
---

# Overview action-detail humanization

Reference documentation for **`humanizeActionDetail`** — the client-side helper
that turns the raw `outcome.detail` strings emitted by the OODA daemon into
plain English before they are shown on the dashboard **Overview** tab. It is the
render-layer fix for the last deferred clarity item in the dashboard usability
pass ([#2358](https://github.com/rysweet/Simard/issues/2358), P2 item 3: *raw
brain-action-detail on the Overview tab*).

## Why this exists

The daemon records, for every action it takes in a cycle, an `outcome.detail`
string that is written for machines, not operators. A typical detail reads:

```
advance-goal: brain: continue_skipping (brain-error fallback: no decision keyword
found in model output, defaulting to continue) agent='engineer-2026-06-27-...'
```

Before this fix the Overview tab printed that string verbatim in two places —
the **Last Cycle Actions** list and the **Recent actions** stream — so an
operator saw `brain:`, `continue_skipping`, `no decision keyword found … defaulting
to …` and other insider shorthand. The Goals tab
([`goals_status.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/goals_status.rs)) and the
Brain Failures card
([`brain_failures.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/brain_failures.rs))
already humanized their own detail strings server-side; the Overview tab was the
one remaining surface that leaked the raw form.

`humanizeActionDetail` closes that gap **at the render layer only**. The
canonical strings produced by `brain` / `ooda_brain` are unchanged, so logs,
the cost ledger, and the backend protocol tests stay byte-for-byte identical —
only what the operator *reads* on the Overview tab changes.

> **Scope boundary.** This is a pure presentation transform. It does not touch
> any canonical `brain` / `ooda_brain` string, any backend logic, or any HTTP
> response shape. It lives entirely in
> [`src/operator_commands_dashboard/index_html/part_01.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/index_html/part_01.rs)
> as inline dashboard JavaScript.

## Overview

`humanizeActionDetail(detail)` is a `string → string` transform that:

1. **Coerces and guards** its input (`null`/`undefined` → `''`; everything else
   is `String(detail)`).
2. **Strips machine prefixes** anchored at the start of the string —
   `brain:`, `advance-goal:`, `no-action:`, and the generic `<x>-brain:` family
   (e.g. `deterministic-brain:`, `fallback-brain:`).
3. **Maps known decision tokens** to plain phrases via a small allowlist
   (e.g. `continue_skipping` → `continued without acting`).
4. **Drops brain-fallback boilerplate** such as
   `no decision keyword found … defaulting to …`.
5. **Applies the shared `BANNED_JARGON` strip** (the same blocklist enforced on
   the static tab ledes) so any banned token that leaks into a detail string is
   removed.
6. **Collapses whitespace** and trims.
7. **Preserves any `agent='engineer-…'` substring verbatim**, so the inline
   **Attach →** button contract in
   [`renderActionDetail`](#relationship-to-renderactiondetail) still matches.

It always returns **plain text** — never HTML or markup. The text is escaped by
the caller (`esc(...)`) as the final step before it reaches the DOM. See
[Security model](#security-model).

### Transform examples

| Raw `outcome.detail` | Rendered on Overview |
|----------------------|----------------------|
| `brain: continue_skipping` | `continued without acting` |
| `advance-goal: brain: continue_skipping (brain-error fallback: no decision keyword found in model output, defaulting to continue)` | `continued without acting` |
| `no-action: brain: nothing actionable this cycle` | `nothing actionable this cycle` |
| `spawn_engineer dispatched: agent='engineer-2026-06-27-abc'` | `spawn_engineer dispatched: agent='engineer-2026-06-27-abc'` *(plus an Attach → button in the Recent actions list when the session is live)* |
| `deterministic-brain: prefix-routed` | `chosen by built-in routing rules` |
| `` (empty / `null`) | `` (empty — the surrounding `<span>` is not rendered) |

Strings that are already plain prose pass through essentially unchanged: the
helper is conservative and only rewrites the anchored prefixes, the allowlisted
tokens, the known boilerplate, and banned jargon. Unknown content is preserved
so the helper never destroys information it does not recognize.

## JavaScript API

The helper is defined alongside the other `#2358` humanizers (`humanizeActionKind`,
`humanizeGoalId`, `humanizeCycleSummary`, `humanizeDuration`) in the inline
dashboard script in `part_01.rs`.

```js
/**
 * Humanize a raw cycle action `detail` string for operator display.
 * Render-layer only — the canonical brain/ooda_brain strings are unchanged.
 *
 * @param {string|null|undefined} detail  Raw outcome.detail from the daemon.
 * @returns {string}  Plain text (never HTML). Caller escapes with esc().
 */
function humanizeActionDetail(detail) { /* … */ }
```

### Contract

| Property | Guarantee |
|----------|-----------|
| Output type | Always a `string`. `null`/`undefined`/non-string inputs are coerced; never throws. |
| Output format | Plain text only — never HTML, never markup. Must always flow through `esc()` before reaching the DOM. |
| Idempotence | `humanizeActionDetail(humanizeActionDetail(x))` ≡ `humanizeActionDetail(x)` for all inputs (a humanized string has no remaining prefixes/tokens to rewrite). |
| `agent='…'` preservation | Any `agent='engineer-<id>'` substring is preserved verbatim, byte-for-byte, including the single quotes. |
| ReDoS safety | All regexes are anchored, linear, and non-backtracking — no nested quantifiers, no `RegExp(userInput)`. |
| Empty result | Empty / whitespace-only input returns `''`; the caller omits the surrounding `<span>` entirely. |

### Allowlist tokens

Decision-token humanization is **allowlist-based**. The JS allowlist mirrors the
*semantics* of the server-side humanizers so a given token reads consistently
across the Overview, Goals, and Brain Failures surfaces. The exact wording may
differ — the Overview phrasing is chosen for that surface and its truncated inline
span — but it never contradicts the server-side meaning:

| Token (canonical) | Plain phrase (Overview / JS) | Server-side semantic mirror |
|-------------------|------------------------------|-----------------------------|
| `continue_skipping` | `continued without acting` | Goals tab chips this as **Skipped** ([`goals_status.rs::classify`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/goals_status.rs)); the Brain Failures decision humanizer renders the bare token as *continue skipping* ([`brain_failures.rs::humanize_decision`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/brain_failures.rs)). |
| `deterministic-brain: prefix-routed`, `fallback-brain: prefix-routed` | `chosen by built-in routing rules` | [`brain_failures.rs::humanize_rationale`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/brain_failures.rs) renders the full sentence *“Chosen by Simard's built-in routing rules (no language model was needed).”* |
| *(generic)* `<x>-brain:` prefix | stripped (prefix removed, prose kept) | `brain_failures.rs::humanize_rationale` generic branch (`split_once("-brain:")`) |

Tokens not on the allowlist are left intact (after prefix/jargon stripping) —
the helper never invents a translation for a token it does not recognize.

> **Note.** `continued without acting` is a deliberate Overview-surface
> presentation choice; it is *semantically* consistent with the Goals **Skipped**
> chip but is not byte-identical to any server string (the server emits either
> the `Skipped` chip or the bare *continue skipping*). The `*-brain:` /
> `prefix-routed` markers originate in the Brain Failures **rationale** field;
> handling them here is defensive, in case such a marker ever leaks into an
> Overview `outcome.detail`.

## Render-site wiring

`humanizeActionDetail` is wired into the **two** Overview render sites that
display raw cycle detail. Both follow the **escape-last** invariant: humanize
the raw string, truncate the humanized (still un-escaped) text, then escape
exactly once.

### Site 1 — Last Cycle Actions (`part_01.rs`, ~L396)

The "Last Cycle Actions" list renders each outcome's detail in a truncated,
ellipsized `<span>`:

```js
// before:
${o.detail ? '<span …>' + esc(o.detail.substring(0,120)) + '</span>' : ''}

// after — humanize once, then gate the <span> on the humanized result so a
// detail that humanizes to '' (all prefix/jargon) is omitted, not rendered empty:
${(() => { const hd = humanizeActionDetail(o.detail);
   return hd ? '<span …>' + esc(hd.substring(0,120)) + '</span>' : ''; })()}
```

Order of operations: **humanize → truncate (120 chars) → `esc()`**. Truncating
the humanized (pre-escape) text guarantees a multi-byte HTML entity can never be
split across the 120-char boundary. The `<span>` is gated on the *humanized*
value rather than the raw `o.detail`, so a detail consisting only of stripped
prefixes/jargon collapses to no span at all instead of an empty grey ellipsis.

> **Design note.** This refines the original wiring in the design spec (which
> kept the raw `o.detail ?` guard and called `humanizeActionDetail(o.detail)`
> inline). Gating on the humanized value both makes the *empty-result* contract
> below true and calls the humanizer exactly once.

### Site 2 — Recent actions (`part_01.rs`, ~L431)

The "Recent actions" stream passes its detail through the shared
[`renderActionDetail`](#relationship-to-renderactiondetail) helper, which builds
the inline **Attach →** button and performs its own single `esc()` internally.
To avoid double-escaping, Site 2 humanizes the **raw** string *before* handing it
to `renderActionDetail`:

```js
// the recent-actions IIFE humanizes the raw detail first, then truncates the
// humanized text to 200 chars before handing it to renderActionDetail:
renderActionDetail((function(){
  const h = humanizeActionDetail(a.detail);
  const arr = Array.from(h);                       // code-point safe
  const d = arr.length > 200 ? arr.slice(0,200).join('') + '…' : arr.join('');
  return d || a.action_description || '';
})())
```

Order of operations: **humanize → truncate (200 chars) → `renderActionDetail`
(which escapes once)**. Humanizing *before* truncation shortens the leading
machine prefixes, so an `agent='engineer-…'` reference is more likely to fall
inside the 200-char budget; when it does, `renderActionDetail`'s
`agent='(engineer-[A-Za-z0-9_-]+)'` match still fires and the Attach button
renders for live sessions. (If the reference lies beyond 200 chars even after
humanizing, truncation drops it and no button is shown — the same limit that
already applied to the raw string.)

### Relationship to `renderActionDetail`

[`renderActionDetail`](./subagent-tmux-tracking.md#recent-actions-inline-attach-links)
is **unchanged** by this feature. It remains the single owner of the escape step
for the Recent actions stream (`const safe = esc(detail || '')`) and the single
owner of Attach-button construction. `humanizeActionDetail` runs strictly
*upstream* of it and feeds it raw (un-escaped) plain text. The two helpers
compose without either one having to know the other's internals:

```
raw detail ──► humanizeActionDetail ──► renderActionDetail ──► DOM
              (plain text, agent='…'      (escapes once,
               preserved)                  adds Attach button)
```

> **Implementation note — fix the stale `renderActionDetail` comment.**
> `renderActionDetail`'s current doc-comment in `part_01.rs` reads *"Returns an
> HTML string (caller already escaped the detail)."* That comment is **stale and
> misleading**: the function escapes its own input (`const safe = esc(detail || '')`)
> and Site 2 hands it **raw** (humanized but un-escaped) text. When implementing
> this feature, correct that comment so it states that `renderActionDetail` is the
> single owner of the `esc()` step. Leaving it as-is invites a future edit to
> "helpfully" pre-escape the detail at the call site and double-escape it — and it
> makes the [SR-D1](#security-model) escape-last contract read as if escaping were
> the caller's responsibility, which it is not.

## Security model

`humanizeActionDetail` handles attacker-influenceable text (an action detail can
echo content a sub-agent produced), so it is held to the dashboard's render-layer
security invariants:

| ID | Invariant | How it is met |
|----|-----------|---------------|
| **SR-D1** | **Escape-last (CRITICAL).** `esc()` is the terminal operation on every humanized value at both render sites. | Site 1 escapes after humanize+truncate; Site 2 delegates the single `esc()` to `renderActionDetail`. |
| **SR-D2** | **Text-only output.** The helper returns plain text — never HTML — and is only ever placed into a text context via `esc()`. | No `innerHTML` / `insertAdjacentHTML` sink; the value never becomes an attribute value. |
| **SR-V1** | **Null / type safety.** | `if (detail == null) return ''` then `String(detail)` before any regex. |
| **SR-V2** | **ReDoS prevention.** | Anchored, linear, non-backtracking regexes (`^brain:\s*`, fixed-token alternations); no nested quantifiers; input is already length-bounded by the caller's truncation. |
| **SR-V3** | **Allowlist mapping.** | Decision-token humanization is allowlist-based; no `eval`, no `new RegExp(userInput)`. |
| **SR-D5** | **Attach-button integrity.** | `agent='engineer-…'` is preserved verbatim so the button contract is neither broken nor newly forged. `esc()` leaves single quotes intact, so the post-escape `agentId` match still works, and the button stays gated on a `subagentSessionsCache` hit. |
| **SR-A1** | **No surface widening.** | No new endpoints, globals, network calls, DOM sinks, or auth changes — a pure render-layer transform. |

### XSS regression guarantee

A `<script>` or `<img onerror=…>` payload embedded in a detail string survives
*only* as escaped HTML entities (`&lt;script&gt;`, `&lt;img onerror=…&gt;`) at
**both** render sites — the escape-last invariant ensures the humanizer never
introduces an un-escaped sink. This is asserted by the test suite (see
[Tests](#tests)).

## Configuration

There are no new CLI flags or environment variables. The only configurable input
is the shared **`BANNED_JARGON`** blocklist, defined once in Rust and injected
into the client script:

- Source of truth:
  [`tab_meta::BANNED_JARGON`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/index_html/tab_meta.rs)
  (`OODA`, `Observe-Orient-Decide-Act`, `spawn_engineer`, `LadybugDB`,
  `cognitive memory`, `synergize`, `leverage`, `ideate`).
- It is serialized to a JS array literal via the `{{BANNED_JARGON_JS}}` marker
  and assigned to `const BANNED_JARGON` in `part_01.rs`, so the *same* ban that
  governs the static tab ledes also governs dynamically rendered action details.

Adding a term to `BANNED_JARGON` automatically extends the strip to
`humanizeActionDetail` (and to `humanizeCycleSummary`) with no further wiring.

## Tests

| Test | Location | Asserts |
|------|----------|---------|
| `rendered_html_humanizes_overview_action_detail` | [`index_html/tests_tab_meta.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/index_html/tests_tab_meta.rs) | `INDEX_HTML` contains `function humanizeActionDetail(`; both render sites are wired (`humanizeActionDetail(o.detail` at Site 1, and `humanizeActionDetail(` inside the recent-actions IIFE at Site 2); the raw path is gone (`!contains("esc(o.detail.substring(0,120))")`). |
| XSS regression assertion | `index_html/tests_tab_meta.rs` | An `<img onerror=…>` / `<script>` payload survives only as escaped entities — proving the escape-last invariant holds at both sites. |
| Server-side humanizer mirror | [`brain_failures.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/brain_failures.rs) tests, [`goals_status.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/goals_status.rs) tests | The canonical decision/rationale semantics that the JS allowlist mirrors (e.g. `continue_skipping` → `Skipped`, `*-brain:` prefix stripped). |

Run the dashboard tests:

```bash
# the dashboard lives in the `simard` crate; filter by module path:
cargo test -p simard operator_commands_dashboard
```

The Playwright jargon-clarity smoke suite (`tests/e2e-dashboard/`) additionally
exercises the running Overview tab end-to-end and asserts no banned jargon
appears in the rendered detail strings. See
[Dashboard E2E tests](./dashboard-e2e-tests.md).

## Invariants

```
INV1: humanizeActionDetail(x) is always a string (null/undefined/non-string → '').
INV2: esc() is the terminal op on every humanized value at both Overview render sites.
INV3: any agent='engineer-<id>' substring in x is preserved verbatim in the output.
INV4: truncation is applied to raw (pre-esc) text only — entities are never split.
INV5: humanizeActionDetail is idempotent.
INV6: no canonical brain / ooda_brain string is modified — presentation layer only.
INV7: a <script>/<img onerror> payload in x renders only as escaped entities.
```

## See also

- [Dashboard](../dashboard.md) — the operator dashboard overview, tabs, and the
  Tab Identity Contract.
- [Subagent tmux tracking](./subagent-tmux-tracking.md) — `renderActionDetail`
  and the inline **Attach →** button contract that this helper composes with.
- [OODA brain decision protocol](./ooda-brain-decision-protocol.md) — the
  canonical decision/rationale tokens that the allowlist humanizes.
- [`brain_failures.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/brain_failures.rs)
  / [`goals_status.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/goals_status.rs) —
  the server-side humanizers whose semantics this client helper mirrors.
