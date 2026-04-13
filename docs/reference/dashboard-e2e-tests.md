# Dashboard E2E Test Reference

Reference documentation for the Playwright end-to-end test suite that covers
the Simard operator commands dashboard.

## File structure

```
tests/e2e-dashboard/
├── playwright.config.ts              # Playwright config with two projects
├── fixtures/
│   └── simard-dashboard.ts           # Custom fixtures: loginCode, authenticatedPage, chatPage
├── pages/
│   ├── login.page.ts                 # LoginPage page object
│   └── chat.page.ts                  # ChatPage page object
└── specs/
    ├── auth.spec.ts                  # Authentication flow (7 tests, @structural)
    ├── chat-lifecycle.spec.ts        # Chat UI and WS commands (8 tests, @structural)
    ├── multi-turn.spec.ts            # Context awareness (4 tests, @structural)
    └── meeting-content.spec.ts       # Real LLM validation (3 tests, @smoke)
```

**22 tests total** — 19 structural, 3 smoke.

## Architecture

The test suite follows a three-layer architecture:

```
┌──────────────────────────────────────────────┐
│  Spec files (test scenarios)                 │
├──────────────────────────────────────────────┤
│  Page objects (LoginPage, ChatPage)          │
├──────────────────────────────────────────────┤
│  Fixtures (auth, WS mocking, server start)   │
└──────────────────────────────────────────────┘
```

### Layer 1: Fixtures (`fixtures/simard-dashboard.ts`)

Extends Playwright's `test` with four custom fixtures:

| Fixture | Type | Description |
|---|---|---|
| `loginCode` | `string` | Reads dashkey from `SIMARD_DASHKEY` env or `~/.simard/.dashkey` |
| `authenticatedPage` | `Page` | A Playwright `Page` with a valid `simard_session` cookie pre-set via `POST /api/login` |
| `loginPage` | `LoginPage` | Page object for `/login` on a fresh (unauthenticated) page |
| `chatPage` | `ChatPage` | Page object for the chat tab on an authenticated page |

### Layer 2: Page objects

#### `LoginPage` (`pages/login.page.ts`)

| Method | Returns | Description |
|---|---|---|
| `navigate()` | `void` | Goes to `/login` and waits for the form |
| `submitCode(code)` | `void` | Fills the code input and submits the form |
| `getErrorText()` | `string` | Waits for the error div and returns its text |
| `isErrorVisible()` | `boolean` | Whether the error div is currently visible |
| `waitForRedirect()` | `void` | Waits for navigation to `/` after successful login |

| Locator | Selector | Description |
|---|---|---|
| `form` | `#login-form` | The login form element |
| `codeInput` | `#code` | The dashkey input field |
| `errorDiv` | `#error` | Error message container |

#### `ChatPage` (`pages/chat.page.ts`)

| Method | Returns | Description |
|---|---|---|
| `openChatTab()` | `void` | Clicks the chat tab and waits for messages div |
| `clickReconnect()` | `void` | Clicks the reconnect button in the WS status area |
| `waitForConnected(timeout?)` | `void` | Asserts the WS status shows "Connected" |
| `connectWebSocket()` | `void` | Clicks reconnect and waits for connected state |
| `sendMessage(text)` | `void` | Fills the input and clicks the send button |
| `getMessages()` | `Array<{role, content}>` | Reads all `.chat-msg` elements and parses role/content |
| `getLastMessage()` | `{role, content}` | Returns the most recent message |
| `waitForResponse(timeout?)` | `{role, content}` | Waits for a new message to appear in the DOM |
| `waitForSystemMessage(timeout?)` | `string` | Waits for a new message with `.role.system` class |

| Locator | Selector | Description |
|---|---|---|
| `chatTab` | `.tab[data-tab="chat"]` | Chat tab button |
| `messagesDiv` | `#chat-messages` | Chat messages container |
| `chatInput` | `#chat-input` | Message input field |
| `sendButton` | `#chat-send` | Send button |
| `wsStatus` | `#ws-status` | WebSocket connection status indicator |

### Layer 3: Test specs

#### `auth.spec.ts` — Dashboard Authentication (`@structural`)

| Test | What it validates |
|---|---|
| redirects unauthenticated users to /login | `GET /` returns 303 with `Location: /login` |
| login page renders form with code input | Form visible, `maxlength=8`, `autocomplete=off` |
| invalid code shows error message | "Invalid code" error after wrong dashkey |
| valid code redirects to dashboard | Successful login navigates to `/` |
| API returns 401 without auth | `GET /api/status` without cookie → 401 |
| API returns 200 with valid session cookie | `GET /api/status` with session → 200 |
| dashboard loads after authentication | Dashboard heading and status panel visible |

#### `chat-lifecycle.spec.ts` — Chat Lifecycle (`@structural`)

| Test | What it validates |
|---|---|
| chat tab shows disconnected state initially | "Disconnected" text and reconnect button present |
| chat elements are present | Input, send button, placeholder text |
| sending without connection shows system warning | "Not connected" system message |
| Enter key sends message | Keyboard submit works via mocked WS |
| `/help` returns command list | Response contains /status, /close, /help |
| `/status` returns session info | Response contains Topic:, Messages:, Open: fields |
| `/close` terminates session | Response contains "Meeting closed" and Summary |
| natural conversation gets assistant response | Non-command text gets assistant role response |

#### `multi-turn.spec.ts` — Multi-Turn Context Awareness (`@structural`)

| Test | What it validates |
|---|---|
| second turn references first turn context | Response mentions both messages |
| three-turn conversation maintains thread | Third response still references first topic |
| `/close` summary includes conversation history | Close summary mentions all prior topics |
| message list grows with each turn | DOM element count increases by 2 per exchange |

#### `meeting-content.spec.ts` — Meeting Content Quality (`@smoke`)

| Test | What it validates |
|---|---|
| greeting message appears on connect | System message with "Connected to Simard" |
| LLM responds to a natural language question | Non-empty assistant response > 10 chars |
| `/help` returns recognized commands | Contains /status and /close |

## Test tiers

### Structural (`@structural`)

- **WebSocket**: Mocked via `page.routeWebSocket('**/ws/chat', ...)`
- **Server**: Real dashboard binary (HTML is embedded in Rust)
- **Timeout**: 30 seconds per test
- **Retries**: 0 locally, 1 in CI
- **Use**: CI pipelines, pre-commit validation

### Smoke (`@smoke`)

- **WebSocket**: Real server-side connection to LLM agent
- **Server**: Real dashboard binary with LLM backend configured
- **Timeout**: 120 seconds per test
- **Retries**: 2 (LLM latency variance)
- **Use**: Nightly runs, pre-release validation

## WebSocket mock pattern

Structural tests intercept the WebSocket before it reaches the server:

```typescript
await authenticatedPage.routeWebSocket('**/ws/chat', (ws) => {
  // Send greeting immediately on connect
  ws.send(JSON.stringify({
    role: 'system',
    content: 'Connected to Simard. Speak naturally — /help for commands, /close to end.',
  }));

  ws.onMessage((msg) => {
    const text = typeof msg === 'string' ? msg : msg.toString();
    // Route commands or echo back responses
    if (text.trim() === '/help') {
      ws.send(JSON.stringify({
        role: 'system',
        content: 'Commands: /status, /close, /help. Everything else is natural conversation with Simard.',
      }));
    } else {
      ws.send(JSON.stringify({ role: 'assistant', content: `Response to: ${text}` }));
    }
  });
});
```

This pattern keeps the real DOM rendering pipeline intact while removing
LLM latency and non-determinism from structural tests.

## DOM contract

The tests depend on these selectors from `src/operator_commands_dashboard/routes.rs`:

| Selector | Element | Used by |
|---|---|---|
| `#login-form` | Login form | `LoginPage` |
| `#code` | Dashkey input | `LoginPage` |
| `#error` | Login error display | `LoginPage` |
| `.tab[data-tab="chat"]` | Chat tab button | `ChatPage` |
| `#chat-messages` | Messages container | `ChatPage` |
| `#chat-input` | Message input | `ChatPage` |
| `#chat-send` | Send button | `ChatPage` |
| `#ws-status` | WS connection indicator | `ChatPage` |
| `.chat-msg` | Individual message element | `ChatPage.getMessages()` |
| `.role` | Role label within message | `ChatPage.getMessages()` |
| `.role.system` | System role indicator | `ChatPage.waitForSystemMessage()` |

!!! warning "DOM coupling"
    If the dashboard HTML in `routes.rs` changes these selectors, the page
    objects must be updated to match. The selectors are centralized in the
    page object constructors for single-point-of-change.

## Server protocol

### Authentication

1. `POST /api/login` with `{"code":"<dashkey>"}` → sets `simard_session` cookie
2. All `/api/*` routes return 401 without a valid cookie
3. Non-API routes (including `/ws/chat`) return 303 redirect to `/login`

### WebSocket messages

- **Client → Server**: Plain text (the typed message)
- **Server → Client**: JSON `{"role":"system"|"assistant"|"error","content":"..."}`
- **Commands**: `/help`, `/status`, `/close` — handled server-side before reaching the LLM
- **Greeting**: `"Connected to Simard. Speak naturally — /help for commands, /close to end."` sent on connection
