# A2A Round-Trip Communication Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable full round-trip A2A communication so when Agent A sends a task to Agent B, Agent A receives the response back as a `ChannelMessage` through the existing message bus — and memory auto-save gives the agent context continuity.

**Architecture:** Fix `send()` to capture the returned `task_id` and spawn an SSE subscriber for `/tasks/{id}/stream`. Fix `listen()` to be a notification-based hub (not polling a non-existent global stream). Parse SSE events into `ChannelMessage` structs that flow through zeroclaw's existing message dispatch loop. The agent's SQLite memory auto-save naturally preserves cross-turn context.

**Tech Stack:** Rust, zeroclaw-a2a crate, SSE (reqwest streaming), tokio mpsc channels, serde_json

---

## Current State & Gap Analysis

### What works:
- `A2AChannel::send()` — POSTs `CreateTaskRequest` to peer's `/tasks` endpoint ✅
- `A2AChannel::listen()` — spawns per-peer connection loops with reconnect ✅
- Gateway `handle_create_task` — creates task, dispatches to agent, streams updates via SSE ✅
- Gateway `/tasks/{id}/stream` — SSE endpoint emitting `TaskUpdate` events (status, message, artifact) ✅
- Message bus — `spawn_supervised_listener` → `run_message_dispatch_loop` processes `ChannelMessage` ✅
- Memory auto-save — conversations stored in SQLite for recall on subsequent turns ✅

### What's broken:
1. **`send()` is fire-and-forget** — returns `Ok(())` without capturing the `task_id` from the response
2. **`listen()` connects to `/tasks/stream`** — a global stream URL that doesn't exist (gateway only serves `/tasks/{id}/stream`)
3. **SSE parsing is `TODO`** at `channel.rs:319-321` — `if let Ok(_data) = chunk { // TODO }`
4. **No bridge between send→response** — no mechanism to subscribe to a task's SSE after sending it

### How memory helps:
When the response arrives as a `ChannelMessage`, the existing message dispatch loop:
1. Auto-saves incoming message to SQLite memory (`MemoryCategory::Conversation`)
2. Loads memory context before the agent processes the message
3. The agent sees its prior conversation and remembers what it asked for

---

## Task 1: Add response subscriber channel to A2AChannel

**Files:**
- Modify: `crates/zeroclaw-a2a/src/channel.rs` (A2AChannel struct)
- Modify: `crates/zeroclaw-a2a/src/lib.rs` (ChannelMessage)

**Purpose:** Add a shared `mpsc::Sender<ChannelMessage>` to `A2AChannel` so `send()` can spawn SSE subscribers that push responses back into the message bus.

**Step 1: Add a response sender field to A2AChannel**

In `crates/zeroclaw-a2a/src/channel.rs`, add a field to hold the sender for incoming responses:

```rust
pub struct A2AChannel {
    config: A2AConfig,
    http_client: reqwest::Client,
    peers: HashMap<String, A2APeer>,
    reconnect_config: ReconnectConfig,
    /// Sender for pushing received messages (responses from peers) into the message bus.
    /// Set when `listen()` is called.
    response_tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<ChannelMessage>>>>,
}
```

Update `new()` to initialize it:

```rust
Ok(Self {
    config,
    http_client,
    peers,
    reconnect_config,
    response_tx: Arc::new(Mutex::new(None)),
})
```

**Step 2: Store the tx in listen()**

At the start of `listen()`, save the sender:

```rust
pub async fn listen(
    &self,
    tx: tokio::sync::mpsc::Sender<ChannelMessage>,
) -> anyhow::Result<()> {
    // Store tx so send() can spawn response subscribers
    {
        let mut response_tx = self.response_tx.lock().await;
        *response_tx = Some(tx.clone());
    }
    // ... rest of existing listen logic
```

**Step 3: Commit**

```bash
cd ~/projects/zeroclaw
git add crates/zeroclaw-a2a/src/channel.rs
git commit -m "feat(a2a): add response_tx field for round-trip communication"
```

---

## Task 2: Make send() capture task_id and spawn SSE subscriber

**Files:**
- Modify: `crates/zeroclaw-a2a/src/channel.rs` (send method)

**Purpose:** After POSTing to `/tasks`, parse the `CreateTaskResponse` to get the `task_id`, then spawn a background task that subscribes to `/tasks/{task_id}/stream` and forwards `TaskUpdate` messages as `ChannelMessage`.

**Step 1: Write the SSE response parser function**

Add a new function `subscribe_to_task_response` to `A2AChannel`:

```rust
/// Subscribe to a peer's task SSE stream and forward responses as ChannelMessages.
async fn subscribe_to_task_response(
    peer_id: String,
    peer_endpoint: String,
    peer_token: String,
    task_id: String,
    http_client: reqwest::Client,
    tx: tokio::sync::mpsc::Sender<ChannelMessage>,
) {
    let stream_url = format!("{}/tasks/{}/stream", peer_endpoint, task_id);
    tracing::debug!("Subscribing to A2A task response: {} from peer {}", task_id, peer_id);

    let response = match http_client
        .get(&stream_url)
        .bearer_auth(&peer_token)
        .header("Accept", "text/event-stream")
        .timeout(std::time::Duration::from_secs(300)) // 5 min timeout for long tasks
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            tracing::warn!(
                "A2A task stream returned HTTP {} for task {} from peer {}",
                resp.status(), task_id, peer_id
            );
            return;
        }
        Err(e) => {
            tracing::warn!(
                "Failed to connect to A2A task stream for task {} from peer {}: {}",
                task_id, peer_id, e
            );
            return;
        }
    };

    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let data = match chunk {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("A2A SSE stream error for task {}: {}", task_id, e);
                break;
            }
        };

        let text = String::from_utf8_lossy(&data);
        buffer.push_str(&text);

        // Process complete SSE events (separated by double newline)
        while let Some(event_end) = buffer.find("\n\n") {
            let event_block = buffer[..event_end].to_string();
            buffer = buffer[event_end + 2..].to_string();

            for line in event_block.lines() {
                if let Some(data_str) = line.strip_prefix("data: ") {
                    if let Ok(update) = serde_json::from_str::<crate::protocol::TaskUpdate>(data_str) {
                        // Convert TaskUpdate message to ChannelMessage
                        if let Some(ref msg) = update.message {
                            let channel_msg = ChannelMessage {
                                id: format!("{}-{}", task_id, current_timestamp_secs()),
                                sender: peer_id.clone(),
                                reply_target: peer_id.clone(),
                                content: msg.content.clone(),
                                channel: "a2a".to_string(),
                                timestamp: current_timestamp_secs(),
                                thread_ts: Some(task_id.clone()),
                            };
                            if tx.send(channel_msg).await.is_err() {
                                tracing::warn!("A2A response channel closed for task {}", task_id);
                                return;
                            }
                        }

                        // Stop on terminal states
                        if matches!(
                            update.status,
                            crate::protocol::TaskStatus::Completed
                                | crate::protocol::TaskStatus::Failed
                                | crate::protocol::TaskStatus::Cancelled
                        ) {
                            tracing::debug!(
                                "A2A task {} from peer {} finished: {:?}",
                                task_id, peer_id, update.status
                            );
                            return;
                        }
                    }
                }
            }
        }
    }
}
```

**Step 2: Update send() to capture task_id and spawn subscriber**

Replace the existing `send()` method:

```rust
pub async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
    let peer = self
        .peers
        .get(&message.recipient)
        .ok_or_else(|| anyhow::anyhow!("Unknown peer: {}", message.recipient))?;

    let request = CreateTaskRequest::new(&message.content);

    let response = self
        .http_client
        .post(format!("{}/tasks", peer.endpoint))
        .bearer_auth(&peer.bearer_token)
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to send message: HTTP {} - {}", status, text);
    }

    // Parse response to get task_id
    let resp_body: serde_json::Value = response.json().await?;
    let task_id = resp_body
        .get("task")
        .and_then(|t| t.get("id"))
        .and_then(|id| id.as_str())
        .unwrap_or_default()
        .to_string();

    if task_id.is_empty() {
        tracing::warn!("A2A send to {} succeeded but no task_id in response", message.recipient);
        return Ok(());
    }

    tracing::info!("A2A task {} created on peer {}", task_id, message.recipient);

    // Spawn SSE subscriber if response_tx is available (listen() has been called)
    let tx_guard = self.response_tx.lock().await;
    if let Some(ref tx) = *tx_guard {
        let peer_id = peer.id.clone();
        let peer_endpoint = peer.endpoint.clone();
        let peer_token = peer.bearer_token.clone();
        let http_client = self.http_client.clone();
        let tx = tx.clone();

        tokio::spawn(async move {
            Self::subscribe_to_task_response(
                peer_id,
                peer_endpoint,
                peer_token,
                task_id,
                http_client,
                tx,
            )
            .await;
        });
    }

    Ok(())
}
```

**Step 3: Commit**

```bash
cd ~/projects/zeroclaw
git add crates/zeroclaw-a2a/src/channel.rs
git commit -m "feat(a2a): send() captures task_id and subscribes to response SSE stream"
```

---

## Task 3: Fix listen() to remove broken global stream polling

**Files:**
- Modify: `crates/zeroclaw-a2a/src/channel.rs` (listen method, connect_to_peer_stream)

**Purpose:** The current `listen()` tries to connect to a non-existent `/tasks/stream` global endpoint. Since responses are now handled by `send()` spawning per-task subscribers, simplify `listen()` to just keep peer health monitoring alive and handle any unsolicited incoming tasks.

**Step 1: Simplify listen() to health-monitoring loop**

Replace the `listen()` method body (keep the `response_tx` store from Task 1):

```rust
pub async fn listen(
    &self,
    tx: tokio::sync::mpsc::Sender<ChannelMessage>,
) -> anyhow::Result<()> {
    // Store tx so send() can spawn response subscribers
    {
        let mut response_tx = self.response_tx.lock().await;
        *response_tx = Some(tx.clone());
    }

    tracing::info!(
        "A2A channel listening with {} peer(s). Responses handled via per-task SSE subscribers.",
        self.peers.len()
    );

    // Keep alive — periodic health checks on peers
    // The actual message receiving happens via:
    // 1. Gateway (handle_create_task) for incoming tasks FROM peers
    // 2. Per-task SSE subscribers spawned by send() for responses TO our tasks
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;

        if tx.is_closed() {
            tracing::info!("A2A listen channel closed, stopping");
            break;
        }

        // Periodic health check
        for peer in self.peers.values() {
            let url = format!("{}/.well-known/agent.json", peer.endpoint);
            match self.http_client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    tracing::debug!("A2A peer {} healthy", peer.id);
                }
                Ok(response) => {
                    tracing::debug!("A2A peer {} health check: HTTP {}", peer.id, response.status());
                }
                Err(e) => {
                    tracing::debug!("A2A peer {} health check error: {}", peer.id, e);
                }
            }
        }
    }

    Ok(())
}
```

**Step 2: Remove `connect_to_peer_stream` method (no longer needed)**

Delete the `connect_to_peer_stream` method entirely. Its functionality is replaced by `subscribe_to_task_response` (per-task, not per-peer).

**Step 3: Clean up unused fields**

Remove the `PeerConnection`, `PeerState`, and related helper types if they are only used by the old `connect_to_peer_stream`. Keep them if used elsewhere (e.g., tests).

Actually — keep `PeerConnection` and `PeerState` since they have tests and could be useful later. Just remove the `connect_to_peer_stream` usage from `listen()`.

**Step 4: Commit**

```bash
cd ~/projects/zeroclaw
git add crates/zeroclaw-a2a/src/channel.rs
git commit -m "feat(a2a): simplify listen() to health monitoring, responses via per-task SSE"
```

---

## Task 4: Add tests for SSE parsing and round-trip

**Files:**
- Modify: `crates/zeroclaw-a2a/src/channel.rs` (tests module)

**Step 1: Add test for SSE data parsing logic**

```rust
#[tokio::test]
async fn subscribe_to_task_response_parses_message_update() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ChannelMessage>(10);

    // Simulate what subscribe_to_task_response does internally
    let sse_data = r#"{"task_id":"task-123","status":"running","message":{"role":"agent","content":"Here is the code review result.","timestamp":"2024-01-01T00:00:00Z"}}"#;
    let update: crate::protocol::TaskUpdate = serde_json::from_str(sse_data).unwrap();

    assert!(update.message.is_some());
    assert_eq!(update.message.as_ref().unwrap().content, "Here is the code review result.");
    assert_eq!(update.status, crate::protocol::TaskStatus::Running);

    // Simulate pushing it as a ChannelMessage
    if let Some(ref msg) = update.message {
        let channel_msg = ChannelMessage {
            id: format!("task-123-{}", 1234567890u64),
            sender: "peer-reviewer".to_string(),
            reply_target: "peer-reviewer".to_string(),
            content: msg.content.clone(),
            channel: "a2a".to_string(),
            timestamp: 1234567890,
            thread_ts: Some("task-123".to_string()),
        };
        tx.send(channel_msg).await.unwrap();
    }

    let received = rx.recv().await.unwrap();
    assert_eq!(received.content, "Here is the code review result.");
    assert_eq!(received.sender, "peer-reviewer");
    assert_eq!(received.channel, "a2a");
    assert_eq!(received.thread_ts, Some("task-123".to_string()));
}

#[tokio::test]
async fn subscribe_stops_on_completed_status() {
    let sse_data = r#"{"task_id":"task-456","status":"completed","message":{"role":"agent","content":"Done.","timestamp":"2024-01-01T00:00:00Z"}}"#;
    let update: crate::protocol::TaskUpdate = serde_json::from_str(sse_data).unwrap();

    assert_eq!(update.status, crate::protocol::TaskStatus::Completed);
}
```

**Step 2: Run tests**

```bash
cd ~/projects/zeroclaw
cargo test -p zeroclaw-a2a -- --nocapture
```

Expected: All tests pass, including new ones.

**Step 3: Commit**

```bash
git add crates/zeroclaw-a2a/src/channel.rs
git commit -m "test(a2a): add round-trip SSE parsing tests"
```

---

## Task 5: Verify compilation and existing tests

**Step 1: Build the crate**

```bash
cd ~/projects/zeroclaw
cargo build -p zeroclaw-a2a
```

**Step 2: Build the main binary (integration check)**

```bash
cargo build
```

**Step 3: Run all a2a tests**

```bash
cargo test -p zeroclaw-a2a
```

**Step 4: Commit if any fixups needed**

```bash
git add -A
git commit -m "fix(a2a): compilation fixups for round-trip communication"
```

---

## AGENTS.md Guidance

After the code changes are working, here's how to instruct agents via their `AGENTS.md` file to use A2A communication:

```markdown
# Inter-Agent Communication

You can communicate with other agents through the A2A (Agent-to-Agent) protocol.
Messages you send will be delivered as tasks, and the response will come back
to you as a new message in your conversation.

## Available Peers

- **senior-reviewer** — Senior code reviewer. Ask for code reviews, architecture advice.
- **junior-dev** — Junior developer. Delegate implementation tasks.

## How to Communicate

To send a message to another agent, use the `send_message` tool with:
- `channel`: "a2a"
- `recipient`: the peer's agent ID (e.g., "senior-reviewer")
- `content`: your message

Example: "Please review this function for security issues: [paste code]"

## Important Notes

- Messages are **asynchronous**. After you send a request, continue your current work.
  The response will arrive as a new message from that agent.
- Your **memory** automatically saves conversations, so you'll have context when the
  response arrives. If you need to remember what you asked, note it in your response
  to the user.
- Use `thread_ts` to group related A2A exchanges (the task_id is used automatically).
- Only communicate when you genuinely need another agent's expertise. Don't delegate
  tasks you can handle yourself.
```

This works because:
1. **`AGENTS.md`** is injected into the system prompt (line 3862 of `channels/mod.rs`)
2. **Memory auto-save** stores both the outgoing context and incoming response in SQLite
3. **`memory.recall()`** retrieves relevant prior A2A conversations on subsequent turns
4. **`thread_ts`** = `task_id` groups the conversation so the agent sees context continuity
