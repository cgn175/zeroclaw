# zeroclaw-a2a

Google [A2A (Agent-to-Agent)](https://github.com/google/A2A) protocol implementation for [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw).

Enables secure, authenticated communication between autonomous agents using a task-based HTTP/SSE protocol.

## How It Works

```
┌──────────────┐          POST /tasks           ┌──────────────┐
│   Agent A    │ ──────────────────────────────► │   Agent B    │
│  (zeroclaw)  │                                 │  (zeroclaw)  │
│              │ ◄─── GET /tasks/{id}/stream ─── │              │
│              │      SSE: status, message,      │              │
│              │            artifact             │              │
└──────────────┘                                 └──────────────┘
       │                                                │
       └──── /.well-known/agent.json (discovery) ───────┘
```

Agent A sends a task → Agent B processes it → Agent B streams the response back via SSE → Agent A receives it as a message in its conversation loop. Memory auto-save gives the agent context continuity across the async round-trip.

## Modules

| Module | Description |
|--------|-------------|
| `protocol` | Core A2A types: `AgentCard`, `Task`, `TaskMessage`, `TaskUpdate`, `Artifact`, request/response types |
| `gateway` | Axum HTTP handlers for all A2A endpoints (discovery, create task, get task, stream, cancel) |
| `channel` | `A2AChannel` — implements `Channel` trait for send/listen/health-check with round-trip SSE |
| `pairing` | Secure peer pairing with short-lived one-time codes for bearer token exchange |
| `config` | Configuration types re-exported for convenience |

## Protocol Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/.well-known/agent.json` | GET | Agent discovery — returns `AgentCard` with capabilities, skills, endpoints |
| `/tasks` | POST | Create a new task — dispatches to agent, returns `task_id` |
| `/tasks/{id}` | GET | Get task status and messages |
| `/tasks/{id}/stream` | GET | SSE stream of `TaskUpdate` events (status changes, messages, artifacts) |
| `/tasks/{id}/cancel` | POST | Cancel a running task |

## Key Types

### AgentCard

Discovery document served at `/.well-known/agent.json`:

```json
{
  "name": "My Agent",
  "description": "What this agent does",
  "version": "0.1.0",
  "capabilities": { "streaming": true, "artifacts": true, "push_notifications": false },
  "authentication": { "schemes": ["bearer"] },
  "endpoints": { "tasks": "/tasks", "stream": "/tasks/{id}/stream" },
  "skills": [{ "id": "code-review", "name": "Code Review", "description": "..." }]
}
```

### Task Lifecycle

```
pending → running → completed
                  → failed
                  → cancelled
```

A `Task` contains `messages[]` (user/agent role), `artifacts[]` (file/image/data), and a status.

### TaskUpdate (SSE Events)

```json
{ "task_id": "uuid", "status": "running", "message": { "role": "agent", "content": "..." } }
```

## Configuration (TOML)

```toml
[channels_config.a2a]
enabled = true
listen_port = 3000
discovery_mode = "static"
allowed_peer_ids = ["*"]

[channels_config.a2a.agent_card]
name = "My Agent"
description = "Senior code reviewer"

[[channels_config.a2a.agent_card.skills]]
id = "code-review"
name = "Code Review"
description = "Review code for quality and security"

[[channels_config.a2a.peers]]
id = "agent-alpha"
endpoint = "http://agent-alpha:3000"
bearer_token = "secret-token"
enabled = true

[channels_config.a2a.rate_limit]
requests_per_minute = 60
burst_size = 10
```

## Round-Trip Communication

When an agent sends a message to a peer:

1. `A2AChannel::send()` POSTs a `CreateTaskRequest` to the peer's `/tasks` endpoint
2. The peer's gateway creates a task, dispatches it to the agent loop, and returns the `task_id`
3. `send()` spawns a background SSE subscriber on `/tasks/{task_id}/stream`
4. As the peer agent processes the task, it emits `TaskUpdate` SSE events
5. The subscriber parses these into `ChannelMessage` structs and pushes them into the message bus
6. The originating agent receives the response as a new incoming message
7. Memory auto-save stores the conversation in SQLite for context continuity

## Security

- **Bearer token auth** — every peer-to-peer request requires a valid bearer token
- **Deny-by-default** — unknown peers are rejected; configure `allowed_peer_ids`
- **Constant-time comparison** — prevents timing attacks on token verification
- **Static token provisioning** — the default mode; tokens are pre-configured in each agent's TOML config (e.g., bot-portal auto-generates tokens when creating containers — no manual setup needed)
- **Secure pairing (optional)** — for adding new peers at runtime without pre-shared tokens; uses short-lived 6-digit codes (5 min expiry, single-use) to exchange bearer tokens out-of-band. Not needed when a central orchestrator (like bot-portal) provisions tokens for all agents

## Integration

This crate provides a generic `AgentDispatcher` trait. The main zeroclaw binary implements it to wire A2A tasks into the agent loop:

```rust
use zeroclaw_a2a::{build_a2a_routes, A2AGatewayState, AgentDispatcher};

#[derive(Clone)]
struct MyDispatcher { /* ... */ }

#[async_trait]
impl AgentDispatcher for MyDispatcher {
    async fn dispatch(&self, message: &str) -> anyhow::Result<String> {
        // Process the message through your agent and return the response
        Ok("Done!".to_string())
    }
}

let config = A2AConfig { enabled: true, ..Default::default() };
let state = A2AGatewayState::new(config, MyDispatcher { /* ... */ });
let router = build_a2a_routes(state);
// Merge into your axum app
```

## AGENTS.md — Teaching Agents to Talk

Add inter-agent instructions to your agent's `AGENTS.md` workspace file (injected into the system prompt automatically):

```markdown
## Available Peers

- **senior-reviewer** — Ask for code reviews, architecture advice
- **junior-dev** — Delegate implementation tasks

## How to Communicate

Send a message using the A2A channel with the peer's agent ID as the recipient.
Messages are asynchronous — the response arrives as a new message in your conversation.
Your memory automatically saves context, so you'll remember what you asked for.
Only communicate when you genuinely need another agent's expertise.
```

## License

Apache-2.0
