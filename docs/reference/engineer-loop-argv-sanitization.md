# Engineer Loop argv Sanitization

The keyword-fallback planner in the engineer loop emits `gh` CLI invocations as
**argv-only** command segments — no shell, no string interpolation. To satisfy
the argv validator (which rejects empty or multi-line segments), task-derived
strings flowing into argv are sanitized through a single helper before
emission.

This page documents the sanitization contract, the invariants the validator
enforces, and the rules planners must follow when constructing `gh` argv.

## Sanitization helper (`src/engineer_loop/execution.rs`)

### `collapse_to_single_line(input: &str) -> String`

Module-private helper (currently `fn`, not exported) that:

1. Replaces every `\n` and `\r` byte in `input` with an ASCII space (`' '`).
   All other bytes — including `\t`, `\0`, ANSI escape sequences, and UTF-8
   multi-byte sequences — pass through unchanged.
2. Trims leading and trailing whitespace from the resulting string (this
   includes the spaces produced by collapsing edge `\n`/`\r` characters).

Used to flatten task descriptions (which often contain markdown bullet lists
or paragraph breaks) into a single-line argv segment suitable for
`gh issue create --title <…>`.

Illustrative usage (the helper is module-private, so this snippet is
pseudocode rather than a compiling example — promote to `pub(crate)` if
another planner needs to call it):

```rust
// inside src/engineer_loop/execution.rs
let title = collapse_to_single_line("Fix flaky test\nin module X");
assert_eq!(title, "Fix flaky test in module X");
```

#### Contract

| Property | Guarantee |
|----------|-----------|
| Idempotent | `collapse(collapse(s)) == collapse(s)` |
| Trims surrounding whitespace | Leading/trailing whitespace — including spaces produced by collapsing edge `\n`/`\r` — is removed. |
| Non-empty input may yield empty output | Input containing **no non-whitespace bytes** (e.g. `"  \n  "`) collapses and trims to `""`. Input containing at least one non-whitespace byte yields a non-empty output. |
| Internal whitespace not collapsed | Only edge whitespace is trimmed. Two adjacent newlines (`\n\n`) become two adjacent spaces (`"  "`) inside the result; consumers must not rely on single-space normalization. |
| Pure | No I/O. Allocates only the returned `String`. |

> **Note:** the helper is **not** length-preserving. The trailing `.trim()`
> can shorten the result whenever input has leading or trailing whitespace.

## Issue-creation argv builder (`execution.rs`)

The actual argv construction lives in the `EngineerActionKind::OpenIssue` arm
of `execute_engineer_action` in `src/engineer_loop/execution.rs`
(approximately lines 350–376). Upstream, `src/engineer_loop/selection.rs`
produces the `OpenIssueRequest` (title, body, labels) that this builder
consumes.

The builder emits:

```text
gh issue create --title <SANITIZED_TITLE> [--body <SANITIZED_BODY>] [--label <L> …]
```

#### Rules

1. **Title** is routed through `collapse_to_single_line()` before being
   pushed into argv.
2. **Body** is also routed through `collapse_to_single_line()`.
3. **Empty body** — if the sanitized body is empty (zero bytes), the
   `--body` flag and its value are **omitted entirely**. The builder never
   emits `--body ""`.
4. The builder does **not** add a default placeholder body. An issue with no
   body is created with no `--body` flag at all; `gh` accepts this and
   creates an issue whose body is empty.
5. **Labels** are appended as repeated `--label <value>` pairs, one per
   label in `req.labels`. Labels are **not** routed through
   `collapse_to_single_line()` — they are expected to be short, single-line
   identifiers controlled by the planner, not free-form task text.

#### Why these rules

The argv validator (defense-in-depth, applied to every command before
`Command::new(...).args(&argv).spawn()`) enforces two invariants:

- **P1 (non-empty):** every argv segment contains at least one byte
- **P2 (no line breaks):** no segment contains `\n` or `\r`

Violating either invariant aborts the command with the error:

```
argv-only command segments must be non-empty single-line values
```

The sanitization rules above guarantee P1 and P2 hold for argv segments
derived from task text.

## Validator invariants (defense-in-depth)

The argv validator lives upstream of every command spawn and is **not** a
substitute for sanitization at the source — it is a backstop that fails
loudly if a planner forgets to sanitize.

| Invariant | Enforcement |
|-----------|-------------|
| P1 — non-empty segment | rejects `""` argv values |
| P2 — single line | rejects values containing `\n` or `\r` |
| P3 — no shell layer | argv is passed via `execve`; the validator additionally rejects callers that try to construct `sh -c <script>` with task-derived data |

## Examples

### Multi-line task with no body → safe argv

Input `OpenIssueRequest`:

- `title`: `"Fix the flaky test\nin module X\n\nIt intermittently fails on Windows."`
- `body`: `""`
- `labels`: `[]`

Resulting argv:

```text
["gh", "issue", "create",
 "--title", "Fix the flaky test in module X  It intermittently fails on Windows."]
```

Notes:
- No `--body` flag (body was empty after sanitization).
- The `\n\n` paragraph break in the title becomes `"  "` (two spaces) inside
  the title — internal whitespace is preserved verbatim.
- No embedded newlines, no empty segments.

### Multi-line task with body and labels

Input `OpenIssueRequest`:

- `title`: `"Fix flaky test\nin module X"`
- `body`: `"Repro:\n  cargo test"`
- `labels`: `["bug", "engineer-loop"]`

Resulting argv:

```text
["gh", "issue", "create",
 "--title", "Fix flaky test in module X",
 "--body",  "Repro:   cargo test",
 "--label", "bug",
 "--label", "engineer-loop"]
```

## Testing

The regression test
`select_open_issue_multiline_task_yields_argv_with_no_newlines_or_empties`
in `src/engineer_loop/selection.rs` feeds a multi-line task through the
keyword-fallback planner and asserts:

- no resulting argv segment contains `\n` or `\r`
- no resulting argv segment is empty
- if the body would have been empty, no stray `--body` flag is present in
  the argv (i.e. the omit-empty-optional-flag invariant)

Run with:

```bash
CARGO_TARGET_DIR=/tmp/simard-ws-943 \
  cargo test --package simard --lib \
  engineer_loop::selection::select_open_issue_multiline_task_yields_argv_with_no_newlines_or_empties
```

## Security considerations

- **No shell**: `Command::new("gh").args(&argv)` invokes `execve` directly.
  Sanitization is defense-in-depth, not the sole barrier against injection.
- **Never refactor** these argv builders to `sh -c "<interpolated>"`.
- **Treat all task-derived strings as untrusted (XPIA-style)**: they may
  originate from LLM output and must not be used to construct shell scripts
  or pass through `eval`-style interpreters.
- **Argv visibility**: argv is visible in `/proc/<pid>/cmdline` to other
  processes on the host. Do not place secrets in issue titles or bodies.
- **No new auth surface**: the builder relies on the existing `gh` auth
  (`GH_TOKEN` or `gh auth login`); no additional credential plumbing is
  introduced.

## Adopting the sanitizer in new argv builders

Future argv builders that consume task-derived text must:

1. Call `collapse_to_single_line()` on every segment derived from
   user/LLM/issue content. (If you are outside `execution.rs`, promote the
   helper to `pub(crate)` first — do not duplicate the implementation.)
2. Omit optional flag-value pairs whose value would be empty after
   sanitization (never emit `--flag ""`).
3. Never build `sh -c` strings from task-derived data.
4. Add a regression test asserting all three invariants on a multi-line
   input: P1 (no empty segments), P2 (no `\n`/`\r`), and omission of empty
   optional flag pairs.

Related:

- Issue [#943](https://github.com/rysweet/Simard/issues/943) — original bug
- PR [#970](https://github.com/rysweet/Simard/pull/970) — fix
