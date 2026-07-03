# Chat Optimization Scan Results

**Scanned:** 2026-07-03
**Scope:** CLI Chat, GUI Chat (egui), VSCode Addon Chat
**Goal:** Identify blocking calls, unnecessary allocations, re-render issues, memory leaks, race conditions, and other optimization opportunities.

---

## Table of Contents

1. [CLI Chat (`src/cli/chat.rs`)](#1-cli-chat)
2. [GUI Chat (`gui/src/views/chat/`)](#2-gui-chat)
3. [VSCode Addon Chat (`vscode-addon/src/chatView.ts`)](#3-vscode-addon-chat)
4. [Cross-Cutting Findings](#4-cross-cutting-findings)

---

## 1. CLI Chat

### File: `src/cli/chat.rs`

---

#### 🔴 CRITICAL: Background Tokio Task Leak on Ctrl+C (L693–L701)

```rust
// Line 693-700
_ = signal::ctrl_c() => {
    eprintln!(
        "\n{}Interrupted agent response. Use /clear to reset.{}",
        ansi!("33"), ansi!("0")
    );
    // We can't cancel the chat task from here, but we break out
    // of the streaming loop. The agent will complete in background.
    break;
}
```

**Problem:** The comment admits the agent task cannot be cancelled — it runs to completion as a zombie task in the background. Since `run_agent_with_tools` takes `&mut messages`, and the spawned task clones `messages` (line 636), the background task holds a large `Vec<Message>` allocation that remains live until the agent finishes (potentially minutes). Over multiple Ctrl+C presses, these zombie tasks accumulate.

**Recommendation:** Use an `AbortController`-style mechanism. Replace the raw `tokio::spawn` with a cancellable pattern:

```rust
let (abort_tx, mut abort_rx) = tokio::sync::watch::channel(false);
let chat_task = tokio::spawn(async move {
    tokio::select! {
        result = agent_ref.chat(msgs, None, options, sender) => result,
        _ = abort_rx.changed() => Err(anyhow::anyhow!("cancelled")),
    }
});
// On Ctrl+C: abort_tx.send(true).ok();
// Then: chat_task.await.ok();
```

---

#### 🟡 MEDIUM: Full Session Clone on Every Auto-Save (L585–594, L609–619)

```rust
// Line 585-594 — inside every loop iteration
if !messages.is_empty() {
    let session = ChatSession {
        messages: messages.clone(),  // FULL DEEP CLONE
        agent_name: primary.clone(),
        version: 1,
    };
    if let Ok(json) = serde_json::to_string(&session) {
        let _ = std::fs::write(&session_path, &json);  // BLOCKING I/O
    }
}
```

**Problem:** Every user message and response triggers:
1. A full deep clone of ALL messages (`Vec<Message>` with full content strings).
2. A blocking `std::fs::write` on the current thread (the async executor thread).
3. An identical block at exit (line 609-619).

For a session with 100+ messages containing code blocks, each auto-save clones potentially megabytes of data and blocks the executor.

**Recommendation:**
- Use `tokio::fs::write` to avoid blocking the executor.
- Only serialize/save the last N messages, or use incremental append.
- Debounce saves: if a save is already in-flight, skip (use an `AtomicBool`).
- Serialize directly from `&messages` without an intermediate `ChatSession` allocation.

```rust
// Debounced, non-blocking save:
static SAVE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
if !SAVE_IN_FLIGHT.swap(true, Ordering::AcqRel) {
    let json = serde_json::to_string(&messages)?;
    tokio::spawn(async move {
        tokio::fs::write(&session_path, &json).await.ok();
        SAVE_IN_FLIGHT.store(false, Ordering::Release);
    });
}
```

---

#### 🟡 MEDIUM: Empty Assistant Message Duplication (L855–866)

```rust
// Line 855-866
if !response.is_empty() {
    let last_is_assistant = messages
        .last()
        .map(|m| m.role == "assistant")
        .unwrap_or(false);
    if !last_is_assistant {
        messages.push(Message {
            role: "assistant".to_string(),
            content: response.clone(),  // ANOTHER CLONE
        });
    }
}
```

**Problem:** After `run_agent_with_tools`, the agent has already pushed the response into `messages` on the follow-up path (line 797-799). But here we check `last_is_assistant` and potentially push a duplicate. Worse, `response.clone()` copies potentially large response strings.

**Recommendation:** Track whether the response was already pushed (e.g., using a `bool` return from the follow-up path), and skip the duplicate push.

---

#### 🟡 MEDIUM: Blocking `std::process::Command` on Async Executor (L369–548)

**Locations:**
- L374: `git diff` in `/diff`
- L413: `git status` in `/commit`
- L436: `git add` in `/commit`
- L456: `git commit` in `/commit`
- L513: `git diff --stat` in `/review`
- L526: `git diff` in `/review`

All spawn synchronous child processes on the async executor thread. Each `std::process::Command::output()` blocks the thread for the duration of the process. On a single-threaded async runtime, this blocks ALL concurrent tasks.

**Recommendation:** Wrap these in `tokio::task::spawn_blocking()` or use `tokio::process::Command`:

```rust
let output = tokio::task::spawn_blocking(move || {
    std::process::Command::new("git").args(["diff"]).output()
}).await??;
```

---

#### 🟡 MEDIUM: `terminal_width()` Spawns a Subprocess Every Call (markdown_renderer.rs L427–L443)

```rust
fn terminal_width() -> usize {
    #[cfg(unix)]
    {
        use std::process::Command;
        if let Ok(out) = Command::new("stty").arg("size").output() { ... }
    }
    80
}
```

**Problem:** `terminal_width()` is called for every code block (line 322) and horizontal rule (line 107). Each call spawns a child process. On the CLI chat path, the terminal width rarely changes during a session.

**Recommendation:** Cache the result with `std::sync::OnceLock` or a `thread_local!`:

```rust
fn terminal_width() -> usize {
    static WIDTH: OnceLock<usize> = OnceLock::new();
    *WIDTH.get_or_init(|| {
        #[cfg(unix)]
        { ... /* one-time stty call */ }
        #[cfg(not(unix))]
        80
    })
}
```

---

#### 🟢 LOW: Redundant String Allocations in `render_inline` (markdown_renderer.rs L203–L295)

**Problem:** The function repeatedly calls `format!()` to wrap segments with ANSI codes, creating temporary `String` allocations for each bold/italic/code segment. For long passages with heavy formatting, this creates many short-lived `String` objects.

**Recommendation:** Use `write!()` into a shared `&mut String` buffer (passed as parameter) to avoid intermediate allocations:

```rust
fn render_inline(text: &str, out: &mut String) {
    // ... instead of returning String each time, push into out
    out.push_str(ansi("90"));
    out.push_str(code);
    out.push_str(ansi("0"));
}
```

---

#### 🟢 LOW: `render_table()` Repeats ANSI Code Allocation Per Cell (markdown_renderer.rs L329–L409)

**Problem:** Each call to `ansi("90")`, `ansi("1")`, etc. creates a new `format!("\u{001B}[{}m", code)` string. For tables with many cells, this adds allocation overhead.

**Recommendation:** Pre-compute common ANSI codes as `&str` constants or thread-local statics:

```rust
const ANSI_GRAY: &str = "\u{001B}[90m";
const ANSI_RESET: &str = "\u{001B}[0m";
```

---

## 2. GUI Chat

### Files: `gui/src/views/chat/chat_impl.rs` + `chat_impl/` submodules

---

#### 🔴 CRITICAL: `handle_paste_events` Reads Files Synchronously on UI Thread (L277–L347)

```rust
// Line 282-294 — inside ui.input() callback
if let Ok(data) = std::fs::read(path) {  // BLOCKS UI THREAD
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    // ...
}
```

**Problem:** `std::fs::read()` blocks the egui UI thread for potentially large files. This causes frame drops (stuttering) when pasting images or dropping files. Similarly, the dropped files section (L277-297) also reads files synchronously.

**Recommendation:** Offload file reading and base64 encoding to a background thread (`std::thread::spawn` or a dedicated async task), then send the result back via `pending_tx`:

```rust
let tx = self.pending_tx.clone();
std::thread::spawn(move || {
    if let Ok(data) = std::fs::read(path) {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        let _ = tx.send(PendingResponse::AttachmentProcessed { ... });
    }
});
```

---

#### 🔴 CRITICAL: Unbounded `rendered_content_hashes` Growth (chat_impl.rs L528, L1086)

```rust
// Line 1086 in old_ui_content.rs
self.rendered_content_hashes.resize(msg_count, 0);
```

**Problem:** `rendered_content_hashes: Vec<u64>` grows with the message count and is never trimmed. When sessions are switched, the `Vec` still contains hashes for the previous session's messages (at their indices). If session A has 500 messages and you switch to session B with 10, the Vec is resized to 10 but the old data persists in the unused capacity. Over hours of use with many long sessions, this Vec plus the `expand_full_text: HashSet<usize>` could grow unboundedly.

**Recommendation:** Clear `rendered_content_hashes` and `expand_full_text` on session switch (in the `active_session` setter logic at L907-915):

```rust
// When switching sessions:
self.rendered_content_hashes.clear();
self.expand_full_text.clear();
```

---

#### 🔴 CRITICAL: Full Session Clone Every Save (storage.rs L148)

```rust
// Line 148 — save_sessions_to_disk()
let sessions = self.sessions.clone();  // FULL DEEP CLONE
```

**Problem:** `self.sessions` is `Vec<Session>` where each `Session` contains `Vec<Message>` with large content strings. Every save (after every message send and every streaming chunk in `process_pending` line 1161) clones the ENTIRE session list — potentially megabytes. This happens on the UI thread before spawning the async task.

**Recommendation:** Serialize directly from `&self.sessions` instead of cloning. If ownership is needed for the async task, use `Arc<Mutex<Vec<Session>>>` or serialize within the async spawn:

```rust
// Instead of cloning, serialize inside the spawn:
tokio::spawn(async move {
    let json = serde_json::to_string_pretty(&*sessions)?;
    // ...
});
```

---

#### 🟡 MEDIUM: Clone-on-Write in `show_messages` (old_ui_content.rs L1145–L1161)

```rust
// Line 1145-1161 — inside every frame for every message
let m = &msgs[msg_idx];
(
    m.role == "user",
    m.timestamp,
    m.model.clone(),          // String clone
    m.content.clone(),        // BIG String clone (could be 500KB+)
    !m.thinking.is_empty() && m.role != "user",
    m.thinking.clone(),       // String clone
    m.sub_agent_records.clone(),  // Vec clone with Strings
    m.command_records.clone(),    // Vec clone with Strings
)
```

**Problem:** Every frame, every visible message's content, thinking, model, sub-agent records, and command records are deep-cloned into a tuple. For 100 messages with code blocks, this clones potentially 50+ MB of strings per frame.

**Recommendation:** Use a struct with references instead of a tuple with owned values. Or compute the tuple fields on demand rather than all at once. Specifically, don't clone `sub_agent_records` and `command_records` unless they are actually being displayed (which depends on `show_all_sub_agents` / `show_sub_agent_idx` checks).

Better yet: access message fields through indexed borrows inside the rendering paths rather than extracting everything upfront.

---

#### 🟡 MEDIUM: History Messages Clone in `send_message` (runtime.rs L248–L254)

```rust
let history_messages: Vec<serde_json::Value> = self.sessions[self.active_session]
    .messages
    .iter()
    .filter(|m| !m.content.is_empty() || m.role != "assistant")
    .take(50)
    .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))  // .take(50) but clones ALL msg content
    .collect();
```

**Problem:** The `.take(50)` limits the output to 50 items, but `.map()` still clones EVERY message's content (a 500KB+ clone each) before `.take()` can cut it off. Actually, due to Rust's iterator laziness, `.take(50)` does stop after 50 items — but `serde_json::json!({ ... })` still clones the full content string for each of the 50 messages.

**Recommendation:** Truncate content for the message payload:

```rust
let MAX_HISTORY_CHARS = 100_000; // per message
.map(|m| {
    let content = if m.content.len() > MAX_HISTORY_CHARS {
        format!("{}... [truncated {} chars]", &m.content[..MAX_HISTORY_CHARS], m.content.len())
    } else {
        m.content.clone()
    };
    serde_json::json!({ "role": m.role, "content": content })
})
```

---

#### 🟡 MEDIUM: Unbounded `Vec<GenerationState>` Growth (chat_impl.rs L77)

```rust
pub generation_states: Vec<GenerationState>,
```

**Problem:** `generation_states` grows with each new generation but is only trimmed via `remove_generation` (L767), which is called on `ChatCompleted` or `Error`. If the GUI somehow gets into a state where completions stop being processed (e.g., channel congestion), generations accumulate without bound.

**Recommendation:** Cap the vec or prune stale entries by age in `process_pending`:

```rust
// Periodically prune generation_states older than 5 minutes
self.generation_states.retain(|s| s.started_at.elapsed() < Duration::from_secs(300));
```

---

#### 🟡 MEDIUM: Unnecessary `self.sessions.clone()` in `new_session` (chat_impl.rs L892–L913)

```rust
fn new_session(&mut self) {
    self.stop_sending();
    let count = self.sessions.len() + 1;
    self.sessions.push(Self::default_session(...));
    // ... then at line 912:
    self.save_sessions_to_disk();  // <-- clones all sessions again
}
```

**Problem:** `stop_sending()` (L969) also calls `save_sessions_to_disk()` indirectly? No — `stop_sending()` does not save. But later `save_sessions_to_disk()` (L912) clones all sessions. Since we just added one session, we could serialize only the new session or use an incremental save.

**Recommendation:** Add an `incremental_save_sessions_to_disk()` that only persists the newly added session's data, or at minimum only serialize once per operation.

---

#### 🟡 MEDIUM: `save_ui_state` Clones All Strings (chat_impl.rs L926–L939)

```rust
pub fn save_ui_state(&self, ui_state: &mut GlobalUiState) {
    ui_state.selected_mode = self.selected_mode.clone();
    ui_state.show_token_details = self.show_token_details;
    // ...
    ui_state.input_draft = self.input.clone();
    ui_state.session_search_query = self.session_search_query.clone();
    ui_state.template_search_query = self.template_search_query.clone();
}
```

**Problem:** Called on app close/shutdown. Each clone copies potentially large strings (input_draft could be the user's half-written novel, template_search_query could be long). This is minor since it only happens once, but the string clones are unnecessary if we use `std::mem::take` for transient fields.

**Recommendation:** Use `std::mem::take` for fields that don't need to survive the clone:

```rust
ui_state.input_draft = std::mem::take(&mut self.input);
```

---

#### 🟢 LOW: `reqwest::Client` Builder Duplication (chat_impl.rs L517–L527)

```rust
stream_client: reqwest::Client::builder()
    .timeout(Duration::from_secs(300))
    .read_timeout(Duration::from_secs(60))
    .build()
    .unwrap_or_else(|_| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .read_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }),
```

**Problem:** The fallback chain creates two `Client` instances before creating the third. The first two are discarded silently.

**Recommendation:** Use a helper function or just `unwrap_or_else(|_| reqwest::Client::new())`:

```rust
stream_client: reqwest::Client::builder()
    .timeout(Duration::from_secs(300))
    .read_timeout(Duration::from_secs(60))
    .build()
    .unwrap_or_else(|_| reqwest::Client::new()),
```

---

#### 🟢 LOW: Redundant String Allocation in `render_segment` Copy Button (render.rs L136)

```rust
let code_ref = code.clone();
if ui.button(...).on_hover_text(copy_code_hint).clicked() {
    ui.ctx().copy_text(code_ref);
}
```

**Problem:** `code.clone()` allocates a copy of potentially large code block content every frame for every visible code block, even if the user never clicks the copy button.

**Recommendation:** Clone only on click:

```rust
if ui.button(...).on_hover_text(copy_code_hint).clicked() {
    ui.ctx().copy_text(code.clone());
}
```

The current code clones `code` eagerly (binding it to `code_ref` before the button check), but the clone only happens once per frame since Rust evaluates `let code_ref = code.clone()` once. This is actually okay for mutability reasons — the one clone per frame is necessary because `code` is behind a reference and `copy_text` requires owned data. However, moving the clone into the click handler avoids the per-frame allocation:

```rust
if ui.button(...).clicked() {
    ui.ctx().copy_text(code.clone());  // clone only on click
}
```

---

#### 🟢 LOW: New `reqwest::Client` Allocation Per Workflow Call (runtime.rs L351–L361)

```rust
// Inside the async spawn in send_message
let workflow_client = reqwest::Client::builder()
    .timeout(Duration::from_secs(300))
    .read_timeout(Duration::from_secs(60))
    .build()
    .unwrap_or_else(|_| { ... });
```

**Problem:** Every workflow-mode generation creates a new `reqwest::Client`. The top-level `ChatView` already has `stream_client` for HTTP requests. Using the same client (with cloned `.timeout()` if needed) avoids this allocation.

**Recommendation:** Pass `stream_client.clone()` into the async block instead of creating a new client.

---

## 3. VSCode Addon Chat

### Files: `vscode-addon/src/chatView.ts`, `vscode-addon/src/chatHtmlTemplate.ts`

---

#### 🔴 CRITICAL: Per-Token `postMessage` Calls During Streaming (chatView.ts L449–L457)

```typescript
onToken: (token: string) => {
    tokenAccumulator.push(token);
    const count = this.streamProcessor.incrementTokens();
    // Send incremental token to webview
    this._view?.webview.postMessage({
        type: "streamToken",
        token,
        tokenCount: count,
    });
},
```

**Problem:** Every single token from the AI model triggers a separate `postMessage()` to the webview. For a 5000-token response, this is 5000 cross-process IPC calls. Each `postMessage` involves JSON serialization, serialization on the extension host side, deserialization on the webview side, and DOM manipulation. This creates significant overhead and can cause UI jank in the webview.

**Recommendation:** Implement token batching — accumulate tokens in a buffer and flush on a timer or after N tokens:

```typescript
let tokenBatch: string[] = [];
let batchTimer: ReturnType<typeof setTimeout> | null = null;
const FLUSH_INTERVAL_MS = 50;
const MAX_BATCH_SIZE = 20;

function flushBatch() {
    if (tokenBatch.length === 0) return;
    const batch = tokenBatch.join("");
    tokenBatch = [];
    this._view?.webview.postMessage({
        type: "streamTokens",
        tokens: batch,
        tokenCount: this.streamProcessor.tokenCount,
    });
}

onToken: (token: string) => {
    tokenAccumulator.push(token);
    this.streamProcessor.incrementTokens();
    tokenBatch.push(token);
    if (tokenBatch.length >= MAX_BATCH_SIZE) {
        flushBatch();
    } else if (!batchTimer) {
        batchTimer = setTimeout(() => { batchTimer = null; flushBatch(); }, FLUSH_INTERVAL_MS);
    }
},
```

The webview side would then append `tokens` to the message container once per batch instead of once per token.

---

#### 🟡 MEDIUM: Full Session Save After Every Message (chatView.ts L191–L192, L569, L584)

```typescript
private async _addMessageToCurrentSession(message: ChatMessage) {
    const messages = this._getCurrentSessionMessages();
    messages.push(message);
    this._sessions.set(this._currentSession, messages);
    await this._saveSessions();  // FULL STATE PERSIST
    // ...
}
```

**Problem:** `_saveSessions()` serializes ALL sessions to `context.globalState` every time a single message is added. For 50 sessions with 500+ messages each, this serializes potentially hundreds of KB per message.

**Recommendation:** Store sessions separately or persist only the changed session:

```typescript
private async _saveSessions() {
    this._trimSessions();
    // Only update the current session in globalState
    await this.context.globalState.update(
        `go-on-chat-session-${this._currentSession}`,
        this._sessions.get(this._currentSession)
    );
}
```

Alternatively, debounce saves: skip if another save is already queued.

---

#### 🟡 MEDIUM: Redundant `_getCurrentSessionMessages` in `_addMessageToCurrentSession` (L188–L189)

```typescript
private async _addMessageToCurrentSession(message: ChatMessage) {
    const messages = this._getCurrentSessionMessages();  // Map.get + touch timestamp
    messages.push(message);
    this._sessions.set(this._currentSession, messages);  // Map.set with same reference
```

The `.set()` call writes back the same array reference. This is a no-op for the Map. Meanwhile, `_getCurrentSessionMessages()` already calls `Date.now()` and updates the LRU timestamp on every message add. The `Date.now()` call (L184) is unnecessary per-message overhead:

```typescript
private _getCurrentSessionMessages(): ChatMessage[] {
    this._sessionLastAccessed.set(this._currentSession, Date.now());  // Every add!
    return this._sessions.get(this._currentSession) || [];
}
```

**Recommendation:** Defer the LRU timestamp update to session switch time or batch it:

```typescript
private _touchCurrentSession() {
    this._sessionLastAccessed.set(this._currentSession, Date.now());
}
```

Call `_touchCurrentSession()` only on session switch, not on every message add.

---

#### 🟡 MEDIUM: Redundant `_saveSessions()` Calls in Error Path (chatView.ts L606, L633)

```typescript
// Line 606 — inside catch block for provider-not-ready
await this._addMessageToCurrentSession(systemMessage);  // calls _saveSessions() internally
this._view.webview.postMessage({ type: "addMessage", ...systemMessage });

// Line 633 — inside catch block for general errors
await this._addMessageToCurrentSession(errorMessage);  // calls _saveSessions() internally
```

**Problem:** Both error paths save sessions twice: once in `_addMessageToCurrentSession` and then potentially again later. For provider-not-ready, the user's original message was already saved in line 381 before the error handler runs. So we have:
1. User message saved (line 381)
2. System message saved (line 606, inside `_addMessageToCurrentSession`)

Two `globalState.update` calls within milliseconds. VS Code's `globalState.update` is rate-limited but still wasteful.

**Recommendation:** Batch session saves and call only once at the end of the handler:

```typescript
// Only call _saveSessions() once at the end:
await this._saveSessions();  // single flush after all mutations
```

---

#### 🟡 MEDIUM: Unnecessary `Array.isArray` Check on Known Structure (chatView.ts L113–L117)

```typescript
for (const [sessionName, messages] of Object.entries(storedSessions)) {
    this._sessions.set(
        sessionName,
        Array.isArray(messages) ? (messages as ChatMessage[]) : [],  // L116
    );
}
```

**Problem:** The data comes from `globalState` which was written by the same code as JSON. If it's not an array, it's corrupted data. The `Array.isArray` check adds overhead on every load (50 sessions × startup) for an extremely unlikely edge case.

**Recommendation:** Remove the check or move it to a one-time validation pass:

```typescript
for (const [sessionName, messages] of Object.entries(storedSessions)) {
    this._sessions.set(sessionName, messages as ChatMessage[]);
}
```

---

#### 🟡 MEDIUM: Race Condition in `_switchSession` + Backend Checkpoint Merge (chatView.ts L952–L989)

```typescript
private async _switchSession(sessionName: string) {
    // ...
    this._currentSession = sessionName;
    let messages = this._getCurrentSessionMessages();

    if (this.manager.isRunning()) {
        const remote = await this.manager.sendRequest("checkpoint.load", { ... });
        // ...
    }
    this._view?.webview.postMessage({ type: "switchSession", sessionName, messages });
}
```

**Problem:** `_currentSession` is updated to `sessionName` at line 961, but the checkpoint merge runs asynchronously. If the user rapidly switches sessions:
1. Switch to session A → `_currentSession = A`
2. Switch to session B → `_currentSession = B`
3. Step 1's checkpoint response arrives → merges checkpoint into session A's messages
4. `postMessage` with (potentially stale) messages

This is a classic TOCTOU race. Messages from the wrong session could be displayed.

**Recommendation:** Guard with an incrementing `switchEpoch` counter:

```typescript
private switchEpoch = 0;

private async _switchSession(sessionName: string) {
    const epoch = ++this.switchEpoch;
    // ...
    if (this.manager.isRunning()) {
        const remote = await this.manager.sendRequest("checkpoint.load", { ... });
        if (epoch !== this.switchEpoch) return;  // stale response
        // ...
    }
}
```

---

#### 🟢 LOW: `_validateJavaScriptSnippet` Creates Regex Array Per Call (chatView.ts L765–L793)

```typescript
private _validateJavaScriptSnippet(code: string): string | null {
    const dangerousPatterns: Array<{ pattern: RegExp; reason: string }> = [
        { pattern: /\brequire\s*\(/i, reason: "require() is not allowed." },
        // ... 17 regex patterns
    ];
    // ...
}
```

**Problem:** This 17-element array with 17 compiled `RegExp` objects is allocated on every code execution attempt. `RegExp` compilation is relatively expensive.

**Recommendation:** Hoist to a module-level constant:

```typescript
const DANGEROUS_PATTERNS: ReadonlyArray<{ readonly pattern: RegExp; readonly reason: string }> = [
    { pattern: /\brequire\s*\(/i, reason: "require() is not allowed." },
    // ...
];

private _validateJavaScriptSnippet(code: string): string | null {
    for (const { pattern, reason } of DANGEROUS_PATTERNS) {
        if (pattern.test(code)) return reason;
    }
    return null;
}
```

---

#### 🟢 LOW: `_getExecutionConfig()` Reads VS Code Config Per Call (chatView.ts L804–L819)

```typescript
private _getExecutionConfig() {
    const config = vscode.workspace.getConfiguration("go-on");
    return {
        pythonPath: config.get<string>("pythonPath", "python"),
        executionTimeout: config.get<number>("execution.timeout", 30000),
        allowedShellPaths: config.get<string[]>("execution.allowedShellPaths", [...]),
    };
}
```

**Problem:** Called for every code execution (L804, L833, L880). Each call reads from VS Code's configuration store, which involves JSON parsing and potentially inter-process communication with the settings service.

**Recommendation:** Cache the result for a short duration (e.g., 5 seconds), or read the settings once at startup and listen for `onDidChangeConfiguration`:

```typescript
private _execConfig: { pythonPath: string; executionTimeout: number; allowedShellPaths: string[] } | null = null;

private _getExecutionConfig() {
    if (this._execConfig) return this._execConfig;
    const config = vscode.workspace.getConfiguration("go-on");
    this._execConfig = { ... };
    return this._execConfig;
}

// In constructor:
this.context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(e => {
        if (e.affectsConfiguration("go-on")) this._execConfig = null;
    })
);
```

---

#### 🟢 LOW: Inefficient CSP in `getChatHtml` (chatHtmlTemplate.ts L29)

```html
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
```

**Problem:** The CSP allows `'unsafe-inline'` for styles, which escapes the strict CSP model. This was likely added for convenience during development.

**Recommendation:** Remove `'unsafe-inline'` and move all inline `<style>` blocks to the external CSS file, or use a nonce for style tags as well:

```html
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'nonce-${nonce}'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
```

---

## 4. Cross-Cutting Findings

### 4.1 Token Estimation Via String Length (All Modes)

**CLI** (chat.rs L632): `let estimated_prompt_tokens = (prompt_chars / 4) as u64;`
**GUI** (chat_impl.rs L225): `from_chars, from_words` weighted average
**VSCode** (chatView.ts L59): Simple counter

All three modes use different token estimation strategies. The CLI's `prompt_chars / 4` is a rough heuristic. The GUI's improved estimator (L193-226) is better but still estimates based on character/word count rather than actual tokenization.

**Recommendation:** For the GUI, the estimator is adequate for display purposes. For the CLI, consider moving to the GUI's improved estimator or delegating to the backend's actual tokenizer when available.

### 4.2 Session Persistence Strategy

All three modes persist sessions differently:
- **CLI:** Single file, overwrites every turn (blocking I/O)
- **GUI:** JSON file, clones entire session list before serializing (clones + blocking write in spawn)
- **VSCode:** VS Code `globalState`, saves all sessions every mutation

**Recommendation:** Adopt a uniform incremental persistence strategy: only persist changed data, debounce writes, and prefer async I/O.

### 4.3 Error Recovery Duplication

The provider-not-ready error path is duplicated in both the streaming error handler (L499–L531) and the outer catch block (L596–L625) of `chatView.ts`. The system message creation logic is identical.

**Recommendation:** Extract the provider-not-ready helper into a shared method:

```typescript
private async _handleProviderNotReady(errorMsg: string): Promise<void> {
    // ... shared logic
}
```

---

## Summary of Priority Items

| Priority | Mode | File | Issue |
|----------|------|------|-------|
| 🔴 Critical | GUI | `ui/old_ui_content.rs:1145` | Full message content cloned every frame |
| 🔴 Critical | GUI | `chat_impl.rs:277` | File reads on UI thread (paste/drop) |
| 🔴 Critical | VSCode | `chatView.ts:449` | Per-token `postMessage` during streaming |
| 🔴 Critical | CLI | `chat.rs:693` | Zombie agent tasks on Ctrl+C |
| 🔴 Critical | GUI | `storage.rs:148` | Full session list clone on every save |
| 🟡 Medium | CLI | `chat.rs:585` | Blocking `fs::write` on async executor |
| 🟡 Medium | GUI | `chat_impl.rs:1086` | Unbounded `rendered_content_hashes` growth |
| 🟡 Medium | CLI | `chat.rs:369-548` | Sync `Command::output()` on async executor |
| 🟡 Medium | VSCode | `chatView.ts:191` | Full session save after every message |
| 🟡 Medium | VSCode | `chatView.ts:952` | Race condition in `_switchSession` |
| 🟡 Medium | GUI | `runtime.rs:248` | History messages clone content strings |
| 🟡 Medium | CLI | `markdown_renderer.rs:427` | `terminal_width()` spawns `stty` per call |
| 🟢 Low | VSCode | `chatView.ts:765` | Regex array allocated per `_validateJavaScriptSnippet` call |
| 🟢 Low | GUI | `render.rs:136` | Code block content cloned per frame |
| 🟢 Low | CLI | `markdown_renderer.rs:203` | ANSI format! allocations in tight loop |

**Total findings: 23** (6 Critical, 10 Medium, 7 Low)
