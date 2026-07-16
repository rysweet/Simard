# Primary — Signature Emission Pipeline Trace + 2× Bug-vs-Honest Verdict + Drift Recheck

**Role:** PRIMARY investigator.
**Focus:** signature emission/assembly pipeline trace · 2× "bug-vs-honest" verdict · drift
recheck of every load-bearing citation against the **current** HEAD.
**Branch/HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `b47b6413`.
**Prior primary HEADs:** `7293de99` (emission trace), `dea65df8` (reconciliation ledger).
**Doctrine:** validate-don't-re-derive. This pass re-reads live source at `b47b6413`, measures
drift since the prior primaries, and re-runs the load-bearing tests instead of trusting the docs.

---

## 0. Verdict (three-part, all confirmed at HEAD `b47b6413`)

1. **Emission trace holds, byte-for-byte.** Every token in the recurring
   `overseer-obs:…|goal:blocked:…|workstream-gap|…` blob is produced by exactly one emitter, and
   all citations re-verify against live `src/overseer/` with only ±small line drift (see §2).
2. **The "2×" is HONEST, not a bug.** It is the `Signal::RecurringSignature.occurrences` counter
   (floor `RECURRING_SIGNATURE_THRESHOLD = 2`) rendered by the summary at `mod.rs:1361`. It is a
   faithful re-observation count of a static, unresolved problem set — **not** a dedup/storage/
   replay artifact and **not** a miscount. Now confirmed *empirically* by a net-new test (§4).
3. **Zero code drift since the prior primaries.** Between `7293de99`/`dea65df8` and `b47b6413`
   the **only** source change under `src/` is `tests_root_cause.rs` (+99 lines, tests only). Every
   other commit is docs-only investigation consolidation. **The prior root-cause analysis is live
   and un-invalidated** (§3).

---

## 1. The string's single source (re-grepped at HEAD)

`grep -rn "recurring signature seen\|in cognitive memory" src/overseer/` returns exactly one
functional hit: **`mod.rs:1361`**

```rust
"recurring signature seen {occurrences}× in cognitive memory ({signature})"
```

(The only other match, `mod.rs:1177`, is a doc-comment on `PriorOccurrence`.) The quoted
investigation string is therefore a `Signal::RecurringSignature` summary with `occurrences = 2`
and `signature = <the giant nested composite>`. Unchanged from the prior finding.

---

## 2. Emission pipeline — token-by-token provenance (re-verified @ b47b6413)

| Token in blob | Emitter (file:line @ HEAD) | Construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1068-1073` | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` — **line 1072** is the sole `\|` source |
| `goal:blocked:<slug>-<8hex>` | `classify_signal` GoalBlocked arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")` |
| nested `overseer-obs:…` fragment | `classify_signal` RecurringSignature arm — `mod.rs:1353-1363` | `sanitize_recalled(signature)` at `1359` admits the WHOLE recalled composite as ONE opaque `dedup_key` |
| `workstream-gap` (bare constant) | `classify_signal` WorkstreamGap arm — `mod.rs:1368-1372` | literal `"workstream-gap"` at `1371` — one consolidated key per Observe pass, per-gap identity erased |

**Constants re-verified (all unchanged):**
- `RECURRING_SIGNATURE_THRESHOLD = 2` — `signal.rs:362`; emitted at `signal.rs:463` (`>= 2`).
- `RECURRENCE_ESCALATION_THRESHOLD = 3` — `root_cause.rs:33`; gate at `mod.rs:1613` (`>= 3`).
- `RECALLED_TEXT_MAX_LEN = 8192` — `capabilities.rs:455`; UTF-8-boundary cap at `capabilities.rs:472`.

**Recall/count seam re-verified:** `signal.rs:455-469` counts recalled `episodes` by
`failure_signature` into a `BTreeMap`, emitting `RecurringSignature{signature, occurrences}` when
`occurrences >= RECURRING_SIGNATURE_THRESHOLD`. This is the only producer of the "2×".

**Concatenation semantics unchanged (load-bearing):** `keys.join("|")` (`mod.rs:1072`) is the ONLY
`|` introduced. Every `|` in the flat blob is *either* a fresh outer key boundary *or* a frozen
boundary inside a previously-nested composite — indistinguishable in the flat string. That
delimiter overload is the whole "duplication" illusion; flat-string inspection **overcounts**.

---

## 3. Drift measurement (the core of this pass)

```
$ git log --oneline 7293de99..HEAD
b47b6413 docs(investigation): consolidate sixteenth-wave … §23
a68296c6 docs(investigation): consolidate fifteenth-wave … §22
9fd1ea0a docs(investigation): primary signature emission/assembly … @ 7293de99

$ git diff --stat dea65df8..HEAD -- src/
 src/overseer/tests_root_cause.rs | 99 +++++++++++++++++++++++  (1 file, +99, tests only)
```

**Interpretation:** no production code under `src/overseer/` (or anywhere in `src/`) changed since
the prior primaries. The emission functions, classify arms, thresholds, and the `sanitize_recalled`
cap are **byte-identical** to what `7293de99`/`dea65df8` verified. Therefore:

- **D1 (self-ingestion nesting)** — recall-derived `overseer-obs:` tokens re-nested via
  `mod.rs:1359` → `orient` → `observation_signature` — **still live and unguarded.**
- **D1b (truncation breaks idempotency)** — the 8192-byte cap can slice a `…-<hash>` mid-token,
  breaking the `mod.rs:1064-1067` invariant for large blobs — **still live and unguarded.**
- **D2 / D3** (blocked-goal escalation dead-zone + ratchet; `WorkstreamCoverage` routing hole) —
  out of emission sub-scope; **also unchanged**, still tracked by the cross-investigation synthesis.

No citation went stale. No remedy became obsolete. The investigation should still **extend, not
restart** — the delta this window is purely additional test coverage.

---

## 4. What the +99 lines added — and why it upgrades the 2× verdict to *empirical*

`tests_root_cause.rs` gained two net-new "two-lane decoupling" tests (H1). They convert the "the
count is honest; audit the loop not the counter" claim from *reasoned* to *test-locked*:

- **`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`** (`:490`) — a
  `RecurringSignature{occurrences = 2+3+5 = 10}` (loud, far above BOTH floors) with an **empty**
  Lane-B recall leaves `why.recurrence == 0` and `decide` returns `UnblockGoal` (self-heal). Proves
  a loud Lane-A "N×" **cannot** trip Lane-B's `>=3` escalation — the two lanes share no counter.
- **`lane_b_escalates_without_any_lane_a_signal`** (`:536`) — Lane B escalates on its own
  `>=3` `PriorOccurrence` recall with Lane A entirely silent. Proves the converse independence.

**Re-run at HEAD (targeted):** `cargo test -p simard --lib overseer::tests_root_cause`
→ **21 passed; 0 failed.** Both new tests green.

This directly closes the "bug-vs-honest" question: the "2×" is a real re-observation of a static
problem set on **Lane A (episodic recall)**; escalation lives on the decoupled **Lane B (root-cause
occurrences)**. A stuck "2× forever" therefore indicts a **missing convergence rung** (D2/D3), not
the counter. Bug-vs-honest → **honest count, unhealthy loop.**

---

## 5. Coverage gap — re-checked, still open

Consistent with the prior primary: there is **no** test asserting that a recall-derived
`RecurringSignature` is kept *out of* the next `observation_signature` (anti-nesting for D1), and
**no** test that a composite exceeding 8192 bytes stays byte-stable across generations (D1b). The
new +99 lines lock **lane decoupling**, not **emission hygiene** — D1/D1b remain unguarded. Any fix
must still add an anti-nesting test and a large-blob idempotency test.

---

## 6. Emission-scoped remediation candidates (unchanged, still valid at HEAD)

1. **Break self-ingestion at the write boundary (D1):** in/just-before `observation_signature`,
   exclude `overseer-obs:`-prefixed (recall-derived) keys from the composite; guard with an
   anti-nesting test. Must preserve the legitimate priority-raise in `orient` for genuine
   *non-self* recurrences.
2. **Make nesting idempotent if kept (D1b):** replace raw truncation of the recalled signature with
   a stable fixed-width hash (e.g. `overseer-obs-recall:<sha8>`), restoring the `mod.rs:1064-1067`
   invariant and the write-back gate's dedup for large blobs.
3. **Preserve per-gap identity (independent):** key `WorkstreamGap` on `GapItem.signature` instead
   of the bare `"workstream-gap"` constant (`mod.rs:1371`) so `dedup()` can collapse repeats.

(1)/(2) are alternatives to the same seam; (3) is independent. All are emission-hygiene only; the
D2 convergence rung and D3 closing edge remain the cross-investigation synthesis's scope.

---

## 7. Remaining unknowns (emission scope, unchanged)

- Exact write-back generation count for this specific blob (repeat multiplicity ≈5–6 ⇒ ≈5–6
  generations, not recoverable from the string alone).
- Whether the 8192-byte truncation (D1b) already fired for this blob — needs the raw stored
  `[sig:…]` byte length from the live episode.

---

## 8. One-line reconciliation

**Emission trace verified byte-for-byte at `b47b6413`; "2×" is an honest, now test-locked Lane-A
re-observation count decoupled from Lane-B escalation; the only drift since the prior primaries is
+99 lines of decoupling tests, so every prior defect (D1/D1b/D2/D3) is still live — extend, don't
restart.**
