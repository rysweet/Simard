# Security Requirements — Issue #2669 (Step 5d)

**Scope decision.** This change has **no network/service boundary, no
authentication surface, and no user-facing endpoint**. It is an in-process Rust
pipeline: memory episodes → LLM agent subprocess (`recipe-runner-rs`) →
per-invocation `facts.json` file → parse/recover → stored facts, plus append-only
`metrics.jsonl` telemetry and file-backed overseer signals (P1).

The security-relevant surface is therefore **not** classic authn/authz. It is:

1. A **trust boundary** — untrusted, LLM-generated JSON (the facts document)
   crossing into the parser via the NEW `strip_json_trailing_commas` repair pass
   (P0.1) and the existing strict `serde_json` arbiter.
2. **Data protection / log hygiene** — untrusted (and potentially sensitive)
   memory content must not leak into `metrics.jsonl` context, `tracing` warns, or
   error strings; the new warn (P0.2) and metric key (P0.3) must stay
   content-free.
3. **Resource / DoS robustness** — a runaway or adversarial agent output must not
   exhaust memory/CPU or panic the parser.

Requirements below are grounded in this worktree's source (line refs current).
Each is tagged **[MUST]** (blocking), **[SHOULD]** (defense-in-depth, strongly
recommended), or **[NOTE]** (pre-existing residual, out-of-scope but recorded).

---

## 0. Threat model (data-flow trust boundaries)

```
episodes (semi-trusted: prior facts/PR/bug text)      ← T0 upstream, pre-existing
   │  serde_json::to_string  → -c episodes={json}
   ▼
recipe-runner-rs  (Command::new, PATH-resolved)        ← T1 spawn boundary
   │  AMPLIHACK_AGENT_BINARY → LLM agent
   ▼
facts.json  in tempfile 0700 dir, unique per call      ← T2 filesystem (LOW risk: 0700, private)
   │  read_to_string(facts_path)                       ← T3 UNBOUNDED READ (DoS surface)
   ▼
trimmed &str
   │  strip_json_trailing_commas(candidate)   [NEW]     ← T4 UNTRUSTED-INPUT PARSE (primary surface)
   │  serde_json::from_str::<RecipeEnvelope>            ← strict arbiter
   ▼
DistillOutput → reliability/grounding/category gates   ← T5 content gates (must remain in force)
   │  build_distill_success_context → record_metric     ← T6 telemetry (leak surface)
   ▼
~/.simard/metrics/metrics.jsonl  (shared $HOME, concurrent appenders)
```

**Primary attacker capability:** shape the LLM's `facts.json` output (directly, or
indirectly via prompt-injected content in upstream episodes — T0, pre-existing).
The P0 fix widens **T4**; the review's job is to ensure T4 is hardened and T5/T6
are not weakened.

---

## 1. Authentication / Authorization

| # | Requirement | Rationale / source |
|---|---|---|
| **AA-1 [MUST]** | Introduce **no new authn/authz surface, no network listener, no IPC endpoint, no new privilege.** The repair pass and metric are pure/local. | No endpoint exists (Step 5b §3); do not create one. |
| **AA-2 [MUST]** | The subprocess (`recipe-runner-rs`) keeps running at the **parent process's existing privilege** — no `sudo`, no setuid, no privilege change. `Command::new` inherits env; do not add secrets/tokens to the spawned env beyond the existing `AMPLIHACK_AGENT_BINARY`. | `distillation.rs:1162-1173`. |
| **AA-3 [SHOULD]** (P1) | Stale-block **self-heal** (P1.2: expire #12, re-evaluate f29bb15c/0c0ada69/7f5afcca) is an *authorization-adjacent* automated action — it clears a goal-block without human review. It **MUST be idempotent and audit-logged** (emit a structured record: which block, why expired, prior state) so a logic bug cannot silently clear a *legitimate* block. | Self-heal is local/automated; the audit trail is the only control. |
| **AA-4 [NOTE]** | `recipe-runner-rs` is spawned by **relative name** (PATH-resolved), so a hostile `PATH` entry could shadow it (binary-hijack). Pre-existing, unchanged by this fix; record for a hardening backlog (resolve to an absolute/trusted path). | `Command::new("recipe-runner-rs")` `distillation.rs:1162`. |

---

## 2. Input validation (the core surface — T4)

`strip_json_trailing_commas` (Brick A) and `scan_cleaned_for_facts` (Brick B)
process **fully untrusted** LLM bytes. Strict `serde_json` remains the sole
validity arbiter; the repair pass must never become a second, weaker parser.

| # | Requirement | Rationale / source |
|---|---|---|
| **IV-1 [MUST] Totality / no panic** | `strip_json_trailing_commas` MUST be **total** over any `&str`: adversarial inputs — unterminated string (`"{\"a\":\"`), lone backslash at EOF, no closer, all-commas, empty, only whitespace, `}`-only — must return a `Cow` and **never panic** (no indexing panic, no slice-on-non-char-boundary). | Interface contract I3 (Step 5b §1). Untrusted input. |
| **IV-2 [MUST] UTF-8 boundary safety** | Scanning MUST be done on **bytes** but only branch on **ASCII** bytes (`,` `}` `]` `"` `\`, and ASCII whitespace ` \t\r\n`). ASCII bytes never occur inside a multi-byte UTF-8 sequence, so byte-indexed decisions are correct; any owned rebuild MUST preserve original byte spans (no `&s[i..j]` on a non-boundary, no lossy re-encode). A comma in emoji-laden content (`{"c":"🎉,"}`) must be untouched. | Mirrors `scan_balanced` byte machine (`extract.rs:233-264`). |
| **IV-3 [MUST] String-literal awareness** | Commas, braces, brackets **inside** JSON string literals MUST NOT be altered, honoring `\"` escapes and `\\`. Adversarial cases MUST be covered by tests: `{"c":"a,}"}`, `{"c":"x,]"}`, escaped quote before comma `{"c":"a\\",}` (the `\\"` is a literal quote, the trailing `,}` is still real), and a `,` immediately after a closing quote. | Fix must not corrupt fact content; mirrors `in_string`/`escaped` (`extract.rs:235-247`). |
| **IV-4 [MUST] Minimality** | Remove **only** a `,` whose next non-whitespace byte is `}` or `]`. No other transformation (no quote-fixing, no json5, no comment stripping, no single→double quote). This bounds the repair's power so it cannot manufacture a valid-looking object from genuinely malformed/hostile bytes. | Interface contract I2 (Step 5b §1). |
| **IV-5 [MUST] Strict-first, repair-on-Err-only** | For the fast path (`distillation.rs:1291`) and **each** span (`:1312`): try strict `serde_json::from_str::<RecipeEnvelope>` **first**; run the stripped-view parse **only** if strict returns `Err`. `serde_json` stays the single arbiter — the repair changes only *whether a candidate parses*, never *what fields mean*. | Interface contract §2 (Step 5b). |
| **IV-6 [MUST] Never a hollow `Ok`** | Genuinely malformed / adversarial input that parses under **neither** view MUST fall through to `None` → `parse_facts_document` returns `Err` → caller defers (retry-safe). The repair MUST NOT convert malformed input into an empty-but-`Ok` result. This is the anti-injection invariant: no unvalidated data enters `DistillOutput`. | `distillation.rs:1264-1270`; contract §5. |
| **IV-7 [MUST] No new field leniency** | Do **not** widen `de_lenient_string` or the `RecipeEnvelope` shape. Field-level coercion is unchanged; the fix is purely byte-level trailing-comma removal. Content still passes the existing gates (IV-8). | `de_lenient_string` `distillation.rs:1364-1380`. |
| **IV-8 [MUST] Downstream content gates remain in force** | After repair+parse, every existing gate MUST still run: grounded-capable tier (non-empty `source_episode_id`, `:1314-1318`), reliability quarantine (`assess_fact_reliability`), empty-`concept` drop and closed-set category canonicalization (`canonical_distill_concept`, `KNOWN_DISTILL_CONCEPTS`). The repair path MUST NOT add any shortcut that trusts content or bypasses a gate. | `:1462`, `canonical_distill_concept` `:1402+`. Prevents T0 prompt-injection from riding the repair path into stored memory. |

---

## 3. Data protection & log hygiene (T2, T6)

The facts document may contain **sensitive memory content** (code snippets, paths,
or secrets accidentally captured in episodes). Telemetry and logs must stay
content-free.

| # | Requirement | Rationale / source |
|---|---|---|
| **DP-1 [MUST] No content in metrics context** | The extended `build_distill_success_context` MUST log only **counts and bounded enum labels** (`input_count`, `fact_count`, `attempt`, `parse_recovery` ∈ {`strict-ok`,`recovered`,`deferred`,`zero-facts`}). It MUST NOT embed any fact `concept`/`content`/`source_episode_id` or a document excerpt. | Current builder logs only counts/labels (`distillation.rs:838-849`); preserve. Metric contract §4 (Step 5b). |
| **DP-2 [MUST] Zero-facts warn is count-only** | The new Brick C warn (P0.2) MUST log only `input_facts`/`kept_facts` integers with the fixed message — **no** fact content, concept strings, or document bytes. | Interface contract §3 (Step 5b). Prevents leaking memory content into logs. |
| **DP-3 [MUST] Bounded, frozen label vocabulary** | `ParseRecovery::as_str` returns one of four fixed ASCII strings; these plus `DistillFailureClass::as_str` are a **frozen vocabulary** (no user/LLM data flows into a metric key/value). No new variant may carry free-form text. | Metric contract §4 versioning (Step 5b). Prevents log/label injection via the metric. |
| **DP-4 [MUST] Preserve private tempfile handling** | Keep the `tempfile` crate's **mode-0700, unique-per-invocation** tempdir and drop-time cleanup; do **not** switch to a predictable `/tmp/…` path or a shared/reused location. This keeps the untrusted facts document off any world-readable path and avoids cross-invocation races/symlink attacks. | `distillation.rs:1139-1147` (0700 via `tempfile`), cleaned on `facts_dir` drop. |
| **DP-5 [SHOULD] Error-excerpt hygiene** | The deferred path still embeds up to 200 chars of the untrusted document into a `SimardError::RpcError` via `truncate` (`:1268-1270`, `:1218`). `truncate` does not strip control/ANSI/newline bytes, so this excerpt can carry **log-forging** characters if the error is later logged verbatim. Pre-existing and not widened by this fix; **recommend** sanitizing control chars in `truncate` (or the log sink) as a small hardening. | `truncate` `distillation.rs:1218-1225`. |
| **DP-6 [MUST] Concurrent-append integrity (record size)** | `metrics.jsonl` is appended concurrently by engineer subprocesses sharing `$HOME`; atomicity relies on the single `write_all` staying **under `PIPE_BUF` (4096 bytes on Linux)**. Adding the `parse_recovery` key keeps records tiny — implementers MUST NOT let the context balloon (e.g. by adding excerpts, per DP-1), or interleaved writes will corrupt telemetry. | `self_metrics/mod.rs:52-64` (single-syscall append rationale). |

---

## 4. Resource / DoS robustness (T3, T4)

| # | Requirement | Rationale / source |
|---|---|---|
| **RD-1 [SHOULD] Cap the facts-file read** | `harvest_facts_file` reads the agent file with `std::fs::read_to_string` and **no size limit** (`distillation.rs:1210`); a runaway/hostile agent could write a multi-GB `facts.json` → OOM. The repair pass **doubles peak memory** for that document (an `Cow::Owned` copy on repair), making the ceiling more consequential. **Recommend** reading at most a bounded prefix (e.g. a few MiB) and treating oversize as a `ParseFailure` (retry-safe, not a panic). Pre-existing gap, now worth closing. | `distillation.rs:1210`; repair allocates Owned only when repairing (contract I3). |
| **RD-2 [MUST] Linear, iterative repair (no recursion)** | `strip_json_trailing_commas` MUST be a **single O(n) pass with an iterative byte cursor** (like `scan_balanced`), never recursive. Pathological deep-nesting input (`{{{{…`) must not blow the stack. | `scan_balanced` uses an `i32` depth counter, iterative (`extract.rs:233-263`). |
| **RD-3 [MUST] Rely on serde's built-in nesting bound** | Do not raise or disable `serde_json`'s default recursion limit (128). Deeply nested adversarial JSON must be **rejected as `Err`** by serde (→ deferred), not accepted. The repair does not change nesting depth, so this bound stays effective. | serde_json default `RECURSION_LIMIT`; repair is depth-neutral. |
| **RD-4 [MUST] Idempotence bounds work** | `f(f(x)) == f(x)` (contract I4) — a single repair pass suffices; do **not** loop repair-then-reparse more than the specified strict→repair→strict twice, so a crafted input cannot induce repeated re-scans. | Interface contract I4 (Step 5b §1). |
| **RD-5 [SHOULD] Bounded span enumeration** | `balanced_objects` already returns spans in one pass; the repair is applied **per candidate**, so total work stays O(n) in document length even for many `{`. Confirm no O(n²) “repair every prefix” pattern is introduced. | `balanced_objects` `extract.rs:276-291`. |

---

## 5. P1 telemetry / hand-off security (lighter surface)

| # | Requirement | Rationale |
|---|---|---|
| **P1-1 [MUST]** | Overseer signals (P1.1) and goal/status files persist **structural data only** (class labels, dedup keys, counts) via `to_string_pretty`/`OpenOptions` — **no secrets, tokens, or raw memory content**. | File-backed, world-visible under `$HOME`; keep payloads structural (Step 5c audit). |
| **P1-2 [MUST]** | Cross-repo hand-offs to `rysweet/agent-kgpacks-rs` (#16/#17, P1.4) are **issue references created via `gh`** — the issue body MUST NOT embed credentials, internal secrets, or full sensitive episode content. | `gh issue create` bodies are public-repo-visible. |
| **P1-3 [SHOULD]** | Gym/quality-gate restoration (P1.3, `dispatch_run_gym_eval`) is a **security-relevant regression to restore** — a silently-skipped quality gate is a control gap. Restoring it (or recording a documented, reviewed skip justification) closes that gap. | `quality:gym_skipped` is a disabled control, per requirements. |
| **P1-4 [MUST]** | Self-heal of stale blocks (P1.2) — see **AA-3**: idempotent + audit-logged. | Automated authorization-adjacent action. |

---

## 6. Security risk register (ranked, with mitigations)

| ID | Risk | Sev | Likelihood | Mitigation (requirement) |
|---|---|---|---|---|
| **S1** | Over-tolerant repair accepts hostile/malformed structure → unvalidated data enters stored memory | High | Low | Strict-first + repair-only-on-Err + never-hollow-`Ok` + minimal (trailing-comma-only) repair (IV-4/5/6); `parse_recovery=recovered` telemetry makes abuse **queryable** (S1 detection). |
| **S2** | String-interior comma/brace corruption alters fact **content** → integrity/misattribution | High | Low | String-aware state machine + escape handling; adversarial string tests (IV-2/IV-3). |
| **S3** | Unbounded facts-file read → memory-exhaustion DoS (worsened 2× by Owned repair copy) | Med | Low | Cap the read + treat oversize as retry-safe `ParseFailure` (RD-1). |
| **S4** | Pathological deep-nesting / huge input → stack/CPU DoS or panic | Med | Low | Iterative O(n) repair, no recursion (RD-2); serde 128-depth bound intact (RD-3); totality (IV-1). |
| **S5** | Prompt-injected episode content (T0) rides the repair path past content gates | Med | Low | Repair is **structural only**; grounding/reliability/category gates unchanged and mandatory (IV-8). No new leniency (IV-7). |
| **S6** | Sensitive memory content leaks into `metrics.jsonl` / logs / error excerpts | Med | Med | Counts+frozen-enum labels only (DP-1/2/3); sanitize error excerpt (DP-5). |
| **S7** | Concurrent metrics append corruption (shared `$HOME`) affecting security telemetry | Low | Med | Keep record < `PIPE_BUF`; single `write_all` (DP-6). |
| **S8** | Silent stale-block self-heal clears a **legitimate** block (P1) | Med | Low | Idempotent + audit-logged self-heal (AA-3/P1-4). |
| **S9** | PATH-hijack of `recipe-runner-rs` | Med | Low | **Pre-existing**, out of scope; backlog: resolve to trusted absolute path (AA-4). |

---

## 7. Implementation security checklist (for the builder / reviewer)

- [ ] `strip_json_trailing_commas` is `pub fn(&str) -> Cow<str>`, iterative,
      single-pass, no recursion, no `unwrap`/`expect`/indexing that can panic
      (IV-1, RD-2).
- [ ] Byte scan branches only on ASCII; owned rebuild reuses original byte spans;
      emoji/multi-byte content preserved (IV-2).
- [ ] `in_string` + `escaped` machine present; string-interior `,`/`}`/`]`
      untouched; `\"` and `\\` handled (IV-3).
- [ ] Removes ONLY a `,` whose next non-ws byte is `}`/`]`; nothing else (IV-4).
- [ ] Strict parse runs first at `:1291` and `:1312`; repair-parse only in the
      `Err` arm (IV-5).
- [ ] Malformed/adversarial input still yields `None` → `Err` → defer
      (never-hollow-`Ok`) — covered by a test (IV-6, S1).
- [ ] No change to `de_lenient_string`, `RecipeEnvelope`, or content gates
      (IV-7, IV-8).
- [ ] `build_distill_success_context` and the zero-facts warn log **counts +
      fixed labels only**, no content, no excerpt (DP-1, DP-2, DP-6 size).
- [ ] `ParseRecovery::as_str` = four fixed ASCII strings, frozen vocabulary
      (DP-3).
- [ ] tempfile 0700 + drop-cleanup unchanged (DP-4).
- [ ] (SHOULD) `read_to_string` size cap added; oversize → `ParseFailure`
      (RD-1, S3).
- [ ] (SHOULD) `truncate` sanitizes control/newline/ANSI, or the error is not
      logged verbatim (DP-5, S6).
- [ ] Tests include adversarial inputs: unterminated string, comma-in-string,
      escaped-quote-then-comma, deep nesting, empty, whitespace-only,
      oversize (if RD-1 adopted).
- [ ] (P1) stale-block self-heal is idempotent + audit-logged (AA-3, S8); gym
      gate restored or skip justified (P1-3); hand-off issues carry no secrets
      (P1-2).

---

## 8. Security posture summary

- **Authn/authz:** N/A by design — no new surface; preserve existing local
  privilege model (§1).
- **Primary control:** treat the LLM facts document as **fully untrusted**; the
  repair pass is a *minimal, structural, string-aware, total* transformation that
  runs **after** strict serde fails and **never** becomes an authority on
  validity or content (§2). All existing grounding/reliability/category gates stay
  mandatory (IV-8).
- **Data protection:** telemetry and logs are **count-and-label only**; the
  private 0700 tempfile handling is preserved (§3).
- **DoS:** iterative O(n) repair, serde depth bound intact, plus a recommended
  read cap to close the pre-existing unbounded-read gap (§4).
- **Net effect:** the fix *narrows* the recurring silent-drop failure without
  widening the attack surface; the one net-new external artifact (`parse_recovery`
  label) is a bounded, frozen enum with no untrusted data — and it doubles as the
  **detection control** for repair abuse/regression (S1).
