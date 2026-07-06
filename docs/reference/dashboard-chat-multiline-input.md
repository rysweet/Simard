---
title: Dashboard Chat — multi-line message input
description: Reference for the multi-line, auto-growing chat composer on the operator dashboard Chat tab — keyboard model (Enter to send, Shift+Enter for a newline), auto-grow sizing, height reset on send, and the CSS/JS coupling that implements it.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./dashboard-chat.md
  - ./dashboard-e2e-tests.md
  - ../dashboard.md
---

# Dashboard Chat — multi-line message input

The **Chat** tab composer accepts **multi-line messages**. The input is a
`<textarea>` that starts at a single-line height, **grows automatically** as you
type additional lines, and caps at a maximum height beyond which it scrolls
internally. `Enter` sends the message; `Shift+Enter` inserts a newline so you
can compose paragraphs, paste multi-line snippets, or lay out a numbered list
before sending.

This reference documents the operator-facing behavior and the small frontend
contract that implements it. For the chat feature as a whole — durable sessions,
the REST session API, and the WebSocket streaming protocol — see the
[Dashboard Chat reference](./dashboard-chat.md).

**Frontend location:**

- `src/operator_commands_dashboard/index_html/part_01.rs` — the `#chat-input`
  `<textarea>` markup and `#chat-send` button.
- `src/operator_commands_dashboard/index_html/part_00.rs` — the `#chat-input`
  CSS (auto-grow sizing constraints).
- `src/operator_commands_dashboard/index_html/part_04.rs` — `sendChat()`, the
  `keydown` handler (send vs. newline), and the `input` auto-grow listener.

---

## Behavior

### Keyboard model

| Key | Action |
|-----|--------|
| `Enter` | **Send** the current message. Leading/trailing whitespace is trimmed; if the result is empty, nothing is sent. |
| `Shift+Enter` | Insert a **newline** into the message without sending. The box grows to accommodate the new line (up to the max height). |
| Typing / paste | The box **auto-grows** to fit the content as lines are added, and **shrinks** back as lines are removed. |

`Enter`-to-send is the default so the common case (one-line messages) stays a
single keystroke. `Shift+Enter` is the escape hatch for deliberate multi-line
composition. The **Send** button remains available and is equivalent to
pressing `Enter`.

### Auto-grow and the height cap

The composer starts at a **single-line minimum height (42px)** so the resting
state is identical to the previous single-line input. As you add lines it grows
to fit the content, up to a **maximum height (150px)**. Past that cap the
textarea stops growing and scrolls internally (`overflow-y: auto`), so a very
long draft never pushes the transcript off-screen or expands the panel without
bound. Deleting lines shrinks the box back down toward the minimum.

The box does **not** expose the native resize grabber (`resize: none`); sizing
is driven entirely by content, not manual dragging.

### Reset after sending

After a message is sent (via `Enter` or the **Send** button) the textarea is
cleared **and reset to its single-line minimum height**. An expanded, multi-line
composer never lingers as an empty tall box after send — the next message starts
fresh at one line.

The reset runs **only on the successful-send path** — after the whitespace and
WebSocket-connection guards, alongside `inp.value=''`. If a send is rejected
(empty text, or no open socket) the draft and its current expanded height are
intentionally preserved so you never lose an in-progress message.

### Busy state

While an assistant reply is streaming, the composer and **Send** button are
**disabled** (`setChatBusy(true)`) and re-enabled when the reply completes
(`done` frame). This is unchanged by the multi-line behavior — you cannot queue
a second message mid-reply.

### Whitespace-only guard

A message consisting only of spaces or newlines is **not sent**. `sendChat()`
trims the value and returns early when the trimmed result is empty, so an
accidental `Enter` on a blank composer is a no-op.

---

## Frontend contract

The multi-line composer is implemented with a CSS sizing envelope plus two small
pieces of JavaScript. All content is still rendered through safe DOM sinks
(`textContent` / `createTextNode`) — see [XSS safety](#xss-safety).

> **Note on snippets:** the code blocks below are normalized for readability. The
> embedded HTML in `index_html/` uses a condensed single-line style. Apply the
> described changes **surgically** (e.g. only append `inp.style.height=''` after
> `inp.value=''`, only swap `height:42px` for the min/max envelope) rather than
> reformatting the existing lines.

### 1. Markup — `part_01.rs`

The composer is a `<textarea>` (not an `<input>`), which is what makes newlines
and vertical growth possible:

```html
<div id="chat-input-row">
  <textarea id="chat-input" placeholder="Type a message… (/close to end session)"></textarea>
  <button id="chat-send" onclick="sendChat()">Send</button>
</div>
```

The `id="chat-input"`, `id="chat-send"`, and placeholder text are a **stable
contract** relied on by the end-to-end tests and must not change.

### 2. Sizing CSS — `part_00.rs`

The `#chat-input` rule constrains the auto-grow between a single-line floor and a
capped ceiling, and scrolls past the cap:

```css
#chat-input{
  flex:1;
  padding:.5rem;
  border:1px solid var(--border);
  border-radius:6px;
  background:var(--card);
  color:var(--fg);
  font-size:.9rem;
  resize:none;
  min-height:42px;      /* single-line resting height (was: height:42px) */
  max-height:150px;     /* growth cap — MUST match the JS clamp below */
  overflow-y:auto;      /* scroll once the cap is reached */
  line-height:1.4;      /* comfortable spacing for multi-line drafts */
}
```

The only CSS change is swapping the fixed `height:42px` for the
`min-height`/`max-height`/`overflow-y` envelope above (plus `line-height` for
readability). Because the fixed height was already interpreted border-box, the
`min-height:42px` resting box is **pixel-identical** to the previous single-line
input.

> **`box-sizing` note:** the dashboard already applies a global
> `*{margin:0;padding:0;box-sizing:border-box}` reset at the top of `part_00.rs`,
> so `#chat-input` is **already** sized border-box — do **not** re-declare
> `box-sizing` on the element. This global reset is what makes auto-grow stable:
> the height the listener assigns is interpreted inclusive of padding, so the box
> settles at the content height instead of gaining padding on every keystroke.

### 3. Auto-grow listener — `part_04.rs`

An `input` listener resizes the textarea to fit its content on every change. It
measures by first collapsing to `auto`, then setting the height to the smaller
of the content's `scrollHeight` and the cap:

```js
const chatInput = document.getElementById('chat-input');
chatInput.addEventListener('input', () => {
  chatInput.style.height = 'auto';                       // reset to measure true content height
  chatInput.style.height = Math.min(chatInput.scrollHeight, 150) + 'px'; // clamp to the cap
});
```

> **Coupling note:** the `150` clamp in the listener and `max-height:150px` in
> the CSS are the same magic number and must be kept in sync. If one changes the
> other must change too, or growth and scrolling will disagree.

The listener computes a **numeric** height only. It never interpolates
user-controlled strings into markup.

### 4. Send + reset — `part_04.rs`

`sendChat()` trims, guards against empty/disconnected states, sends over the
WebSocket, clears the value, **and clears the inline height** so the box
collapses back to the CSS `min-height`:

```js
function sendChat(){
  const inp = document.getElementById('chat-input');
  const txt = inp.value.trim();
  if(!txt) return;                                   // whitespace-only guard
  if(!ws || ws.readyState !== WebSocket.OPEN){
    appendMsg('system','Not connected. Click Reconnect to establish a session.');
    return;
  }
  appendMsg('user', txt);
  ws.send(txt);
  inp.value = '';
  inp.style.height = '';                             // reset to single-line min-height after send
  showTypingIndicator();
  setChatBusy(true);
}
```

### 5. Send-vs-newline keydown — `part_04.rs`

The `keydown` handler distinguishes send from newline. `Enter` alone sends (and
suppresses the default newline); `Enter` with `Shift` falls through to the
browser's default, inserting a newline:

```js
document.getElementById('chat-input').addEventListener('keydown', e => {
  if(e.key === 'Enter' && !e.shiftKey){ e.preventDefault(); sendChat(); }
  // Shift+Enter: no preventDefault → the browser inserts a newline
});
```

---

## XSS safety

Multi-line input does not change the dashboard's rendering model. User-authored
message text — including multi-line and pasted content — is rendered exclusively
through `document.createTextNode` / `textContent`, never `innerHTML`,
`insertAdjacentHTML`, or `eval`. The auto-grow listener manipulates only the
numeric `style.height`. Pasting markup such as
`<img src=x onerror=alert(1)>` renders as **literal text** and is never
executed. See [Dashboard Chat → XSS safety](./dashboard-chat.md#xss-safety).

The backend independently bounds inbound WebSocket frame size (a compile-time
cap in `chat.rs`); a large multi-line paste that exceeds the cap is refused, not
persisted. Client-side sizing is a UX convenience and is never the sole size
control.

---

## Examples

### Compose a multi-line message

1. Open the **Chat** tab and click into the composer.
2. Type the first line, then press **Shift+Enter** to drop to a new line.
   The box grows to two lines.
3. Continue adding lines. Once the content exceeds ~150px tall the box stops
   growing and scrolls internally.
4. Press **Enter** (without Shift) to send. The whole multi-line message is sent
   as one turn, and the composer resets to a single line.

### Paste a code snippet, then send

Paste a multi-line snippet into the composer — the box auto-grows to fit (up to
the cap, then scrolls). Review it, edit if needed, and press **Enter** to send
the entire block as a single message.

---

## Testing

End-to-end coverage lives in
[`tests/e2e-dashboard/specs/chat-lifecycle.spec.ts`](./dashboard-e2e-tests.md).
That spec already asserts the composer placeholder, the not-connected warning,
and `Enter`-to-send. The multi-line rows below **extend** it and must land
together with the feature — they are the intended coverage, not all present
today:

| Scenario | Expectation |
|----------|-------------|
| `Enter` on a filled composer | Message is sent; transcript shows the `user` turn; composer clears and returns to single-line height. |
| `Shift+Enter` | A newline is inserted; the message is **not** sent; the box grows. |
| Repeated `Shift+Enter` past the cap | The box stops growing at the max height and scrolls internally. |
| `Enter` on a whitespace-only composer | No message is sent (no-op). |
| Busy state | Composer and **Send** are disabled while a reply streams and re-enabled on `done`. |
| Multi-line paste containing markup | Rendered as literal text (no script execution). |

The selectors `#chat-input`, `#chat-send`, and the composer placeholder are part
of the test contract; keep them stable.

---

## Related reading

- [Dashboard Chat — sessions, storage, and streaming protocol](./dashboard-chat.md)
- [Dashboard guide → Chat tab](../dashboard.md#chat-tab-durable-resumable-sessions)
- [Dashboard end-to-end tests](./dashboard-e2e-tests.md)
