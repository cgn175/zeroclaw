# A2A (Agent-to-Agent) Protocol Implementation Plan

**Status:** Planning  
**Created:** 2026-02-20  
**Approach:** HTTP/2 + SSE (Server-Sent Events)  
**Estimated Effort:** ~500 LOC, 3-5 days

## Overview

Enable direct agent-to-agent communication without third-party channels (Telegram, Discord, etc.). Uses HTTP/2 for sending messages and SSE for receiving responses, leveraging existing gateway infrastructure.

## Architecture

```
Agent A                          Agent B
   |                                |
   |--POST /a2a/send--------------->| (receive message)
   |                                | (process via agent loop)
   |<--SSE /a2a/stream/:session_id--| (stream response)
   |                                |
```

### Key Design Decisions

1. **Transport:** HTTP/2 + SSE (not WebSocket)
   - Reuses existing gateway HTTP stack
   - No chunking complexity
   - Works through more firewalls
   - Graceful HTTP/1.1 fallback

2. **Discovery:** Static peer list (v1), mDNS later
3. **Security:** Reuse gateway pairing + bearer tokens
4. **Message Format:** JSON (consistent with webhook)
5. **Compression:** Optional gzip for large payloads

## Implementation Tasks

### Phase 1: Core Protocol (MVP)

#### Task 1: A2A Message Types & Protocol
**File:** `src/channels/a2a/protocol.rs`
**Effort:** 2 hours

```rust
// Message envelope
pub struct A2AMessage {
    pub id: String,              // UUID
    pub session_id: String,      // Conversation thread
    pub sender_id: String,       // Peer identity
    pub recipient_id: String,    // Target peer
    pub content: String,         // Message content
    pub timestamp: u64,          // Unix timestamp
    pub reply_to: Option<String>, // Optional parent message ID
}

// Peer configuration
pub struct A2APeer {
    pub id: String,              // Unique peer identifier
    pub endpoint: String,        // https://peer.example.com
    pub bearer_token: String,    // Auth token (encrypted in config)
    pub enabled: bool,
}
```

**Acceptance Criteria:**
- [ ] Message struct with all required fields
- [ ] Peer configuration struct
- [ ] Serialization/deserialization tests
- [ ] Message ID generation (UUID v4)

---

#### Task 2: A2A Config Schema
**File:** `src/config/schema.rs`
**Effort:** 1 hour

```toml
[channels_config.a2a]
enabled = false              # Opt-in
listen_port = 9000          # Dedicated A2A port (or reuse gateway)
discovery_mode = "static"   # "static" only for v1
allowed_peer_ids = ["*"]    # Allowlist (deny-by-default when empty)

[[channels_config.a2a.peers]]
id = "agent-alpha"
endpoint = "https://192.168.1.100:9000"
bearer_token = "encrypted:..."
enabled = true

[[channels_config.a2a.peers]]
id = "agent-beta"
endpoint = "https://agent-beta.local:9000"
bearer_token = "encrypted:..."
enabled = true
```

**Acceptance Criteria:**
- [ ] Config struct with validation
- [ ] Peer array support
- [ ] Bearer token encryption integration
- [ ] Default values (disabled, empty peers)
- [ ] Config roundtrip test

---

#### Task 3: A2A Channel Implementation
**File:** `src/channels/a2a/mod.rs`
**Effort:** 4 hours

```rust
pub struct A2AChannel {
    config: A2AConfig,
    peers: HashMap<String, A2APeer>,
    http_client: reqwest::Client,
    session_streams: Arc<Mutex<HashMap<String, SSEStream>>>,
}

impl Channel for A2AChannel {
    fn name(&self) -> &str { "a2a" }
    
    async fn send(&self, message: &SendMessage) -> Result<()> {
        // 1. Resolve recipient to peer endpoint
        // 2. Build A2AMessage envelope
        // 3. POST to peer's /a2a/send endpoint
        // 4. Handle response (202 Accepted)
    }
    
    async fn listen(&self, tx: Sender<ChannelMessage>) -> Result<()> {
        // 1. Connect SSE streams to all enabled peers
        // 2. Parse incoming A2AMessage events
        // 3. Convert to ChannelMessage
        // 4. Send to tx channel
        // 5. Reconnect on disconnect
    }
    
    async fn health_check(&self) -> bool {
        // Ping all peers, return true if any respond
    }
}
```

**Acceptance Criteria:**
- [ ] Implements full Channel trait
- [ ] Peer resolution logic
- [ ] HTTP client with timeout (30s)
- [ ] SSE stream management
- [ ] Reconnection logic with exponential backoff
- [ ] Unit tests for send/listen

---

#### Task 4: Gateway A2A Endpoints
**File:** `src/gateway/a2a.rs`
**Effort:** 3 hours

```rust
// POST /a2a/send - Receive message from peer
async fn handle_a2a_send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(message): Json<A2AMessage>,
) -> impl IntoResponse {
    // 1. Verify bearer token
    // 2. Check sender in allowlist
    // 3. Validate message structure
    // 4. Queue message for processing
    // 5. Return 202 Accepted with session_id
}

// GET /a2a/stream/:session_id - SSE response stream
async fn handle_a2a_stream(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // 1. Verify bearer token
    // 2. Create SSE stream
    // 3. Stream agent response chunks
    // 4. Close stream on completion
}

// GET /a2a/health - Peer health check
async fn handle_a2a_health() -> impl IntoResponse {
    Json(json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")}))
}
```

**Acceptance Criteria:**
- [ ] Three endpoints registered in gateway router
- [ ] Bearer token authentication
- [ ] Allowlist enforcement
- [ ] SSE streaming with proper headers
- [ ] Integration test with mock peer

---

#### Task 5: A2A Message Processing
**File:** `src/channels/a2a/processor.rs`
**Effort:** 2 hours

```rust
pub async fn process_a2a_message(
    state: Arc<AppState>,
    message: A2AMessage,
) -> Result<String> {
    // 1. Convert A2AMessage to ChannelMessage
    // 2. Load conversation history by session_id
    // 3. Call agent loop (reuse existing logic)
    // 4. Stream response to SSE channel
    // 5. Auto-save conversation memory
    // 6. Return session_id
}
```

**Acceptance Criteria:**
- [ ] Message conversion logic
- [ ] Session-based history management
- [ ] Integration with existing agent loop
- [ ] SSE streaming integration
- [ ] Memory auto-save

---

#### Task 6: Channel Factory Registration
**File:** `src/channels/mod.rs`
**Effort:** 30 minutes

```rust
// Add to channel factory
if let Some(a2a_config) = &config.channels_config.a2a {
    if a2a_config.enabled {
        let channel = A2AChannel::new(a2a_config.clone())?;
        channels.push(Box::new(channel));
    }
}
```

**Acceptance Criteria:**
- [ ] A2A channel registered in factory
- [ ] Conditional on config.enabled
- [ ] Error handling for invalid config

---

### Phase 2: Security & Reliability

#### Task 7: Peer Authentication & Pairing
**File:** `src/channels/a2a/auth.rs`
**Effort:** 2 hours

```rust
// Pairing flow for new peers
pub async fn initiate_peer_pairing(
    peer_endpoint: &str,
) -> Result<(String, String)> {
    // 1. Request pairing code from peer
    // 2. Display code to operator
    // 3. Exchange code for bearer token
    // 4. Store encrypted token in config
    // 5. Return (peer_id, bearer_token)
}
```

**Acceptance Criteria:**
- [ ] Pairing code exchange
- [ ] Token encryption/decryption
- [ ] Config persistence
- [ ] CLI command: `zeroclaw channel pair-a2a <endpoint>`

---

#### Task 8: Rate Limiting & Idempotency
**File:** `src/gateway/a2a.rs` (extend)
**Effort:** 1 hour

- Reuse existing gateway rate limiter
- Add idempotency store for message IDs
- Reject duplicate messages within TTL window

**Acceptance Criteria:**
- [ ] Rate limiting per peer_id
- [ ] Idempotency check on message.id
- [ ] Tests for duplicate rejection

---

#### Task 9: Connection Resilience
**File:** `src/channels/a2a/mod.rs` (extend)
**Effort:** 2 hours

```rust
// Exponential backoff for SSE reconnection
async fn connect_peer_stream_with_retry(
    peer: &A2APeer,
    max_retries: u32,
) -> Result<SSEStream> {
    // Backoff: 2s, 4s, 8s, 16s, 32s, 60s (cap)
}
```

**Acceptance Criteria:**
- [ ] Exponential backoff (2^n seconds, cap at 60s)
- [ ] Max retry limit (10 attempts)
- [ ] Graceful degradation (mark peer offline)
- [ ] Health check recovery

---

### Phase 3: Testing & Documentation

#### Task 10: Integration Tests
**File:** `tests/a2a_integration.rs`
**Effort:** 3 hours

```rust
#[tokio::test]
async fn a2a_send_and_receive_message() {
    // 1. Start two gateway instances (different ports)
    // 2. Configure as peers
    // 3. Send message from A to B
    // 4. Verify B receives and processes
    // 5. Verify A receives response via SSE
}

#[tokio::test]
async fn a2a_unauthorized_peer_rejected() {
    // Test allowlist enforcement
}

#[tokio::test]
async fn a2a_reconnection_after_disconnect() {
    // Test resilience
}
```

**Acceptance Criteria:**
- [ ] End-to-end message flow test
- [ ] Security boundary tests
- [ ] Reconnection tests
- [ ] All tests pass in CI

---

#### Task 11: Documentation
**Files:** `docs/channels-reference.md`, `docs/a2a-setup.md`
**Effort:** 2 hours

**Content:**
- A2A protocol overview
- Configuration guide
- Pairing workflow
- Security considerations
- Troubleshooting guide
- Example multi-agent setup

**Acceptance Criteria:**
- [ ] Updated channels-reference.md
- [ ] New a2a-setup.md guide
- [ ] Example config snippets
- [ ] Troubleshooting section

---

#### Task 12: CLI Commands
**File:** `src/main.rs`, `src/lib.rs`
**Effort:** 1 hour

```bash
# Pair with a new peer
zeroclaw channel pair-a2a https://peer.example.com:9000

# List configured peers
zeroclaw channel list-a2a-peers

# Test peer connectivity
zeroclaw channel test-a2a-peer agent-alpha

# Send test message
zeroclaw channel send-a2a agent-alpha "Hello from agent-beta"
```

**Acceptance Criteria:**
- [ ] Four new subcommands under `channel`
- [ ] Help text for each command
- [ ] Error handling for invalid peers

---

## File Structure

```
src/channels/a2a/
├── mod.rs           # A2AChannel implementation (Channel trait)
├── protocol.rs      # Message types and serialization
├── auth.rs          # Pairing and authentication
└── processor.rs     # Message processing logic

src/gateway/
├── a2a.rs           # A2A endpoints (/a2a/send, /a2a/stream, /a2a/health)
└── mod.rs           # Register A2A routes

tests/
└── a2a_integration.rs  # End-to-end tests

docs/
├── a2a-setup.md        # Setup guide
└── channels-reference.md  # Updated with A2A section
```

## Dependencies

**New crates needed:**
```toml
[dependencies]
# Already in Cargo.toml:
# - reqwest (HTTP client)
# - tokio (async runtime)
# - serde/serde_json (serialization)
# - axum (HTTP server)

# May need to add:
async-stream = "0.3"      # For SSE stream helpers
eventsource-stream = "0.2" # SSE client parsing
```

**Estimated binary size impact:** +50-100KB (minimal, reuses existing deps)

## Testing Strategy

1. **Unit tests:** Each module (protocol, auth, processor)
2. **Integration tests:** Full message flow between two agents
3. **Security tests:** Unauthorized access, allowlist enforcement
4. **Resilience tests:** Network failures, reconnection
5. **Performance tests:** Message throughput, latency

## Rollout Plan

### Phase 1: MVP (Week 1)
- Tasks 1-6: Core protocol working
- Manual testing with two local agents
- Basic documentation

### Phase 2: Hardening (Week 2)
- Tasks 7-9: Security and reliability
- Integration tests
- CLI commands

### Phase 3: Polish (Week 3)
- Tasks 10-12: Testing and docs
- Performance tuning
- Community feedback

## Success Criteria

- [ ] Two ZeroClaw agents can exchange messages without external channels
- [ ] Messages >1MB handled gracefully
- [ ] Reconnection works after network interruption
- [ ] Unauthorized peers blocked
- [ ] Documentation complete
- [ ] All tests pass in CI
- [ ] Zero regressions in existing channels

## Future Enhancements (Post-MVP)

1. **mDNS discovery** - Auto-discover peers on local network
2. **WebSocket upgrade** - For high-frequency messaging
3. **Message compression** - gzip/zstd for large payloads
4. **Mesh routing** - Multi-hop message forwarding
5. **Presence protocol** - Online/offline/busy status
6. **File transfer** - Binary attachment support
7. **Group conversations** - Multi-agent coordination

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| SSE connection drops | Exponential backoff + health checks |
| Large message OOM | Stream processing, no full buffering |
| Peer impersonation | Bearer token + TLS mutual auth (future) |
| Port conflicts | Configurable port, can reuse gateway port |
| NAT traversal | Document tunnel setup (Tailscale/Cloudflare) |

## Open Questions

1. **Port strategy:** Dedicated A2A port (9000) or reuse gateway port (3000)?
   - **Decision:** Start with dedicated port, add gateway reuse option later
   
2. **Message persistence:** Store all A2A messages in memory DB?
   - **Decision:** Yes, treat like any other channel (auto-save enabled)
   
3. **Broadcast support:** One-to-many messaging?
   - **Decision:** Not in MVP, add in Phase 4

4. **TLS requirement:** Enforce HTTPS for all peers?
   - **Decision:** Yes, except localhost (127.0.0.1, ::1)

## Estimated Timeline

- **Phase 1 (MVP):** 12 hours → 2-3 days
- **Phase 2 (Hardening):** 5 hours → 1 day
- **Phase 3 (Polish):** 6 hours → 1 day
- **Total:** ~23 hours → 4-5 days (with testing/debugging buffer)

## Next Steps

1. Review and approve this plan
2. Create task tracking (using beads skill)
3. Start with Task 1 (protocol.rs)
4. Iterate with frequent commits
5. Open draft PR after Phase 1 complete
