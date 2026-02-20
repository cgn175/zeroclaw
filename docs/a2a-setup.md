# A2A (Agent-to-Agent) Protocol Setup

## Overview

ZeroClaw implements the **Google A2A Protocol** standard for agent-to-agent communication. This enables direct, interoperable communication between AI agents without third-party channels.

**Protocol:** [Google A2A Standard](https://github.com/google/A2A)  
**Version:** 1.0  
**Transport:** HTTP/HTTPS with Server-Sent Events (SSE)

## Architecture

The Google A2A protocol uses a **task-based model**:

```
Agent A                          Agent B
   |                                |
   |--GET /.well-known/agent.json->| (discover capabilities)
   |<--AgentCard-------------------| 
   |                                |
   |--POST /tasks------------------>| (create task with message)
   |<--Task (pending)---------------| 
   |                                |
   |--GET /tasks/{id}/stream------->| (subscribe to updates)
   |<--SSE: TaskUpdate (running)----| 
   |<--SSE: TaskUpdate (completed)--| 
   |                                |
```

### Key Concepts

- **AgentCard**: Describes agent capabilities, skills, and endpoints
- **Task**: Unit of work containing messages, artifacts, and status
- **TaskMessage**: Individual message within a task
- **TaskUpdate**: Real-time status/result updates via SSE
- **Bearer Token**: Static authentication per peer

## Quick Start

### 1. Enable A2A Channel

Add to `~/.zeroclaw/config.toml`:

```toml
[channels_config.a2a]
enabled = true
listen_port = 9000
discovery_mode = "static"
allowed_peer_ids = ["*"]  # Or specific peer IDs

# Optional: Customize your AgentCard
[channels_config.a2a.agent_card]
name = "My ZeroClaw Agent"
description = "Autonomous AI assistant"

[[channels_config.a2a.agent_card.skills]]
id = "code-review"
name = "Code Review"
description = "Review code for quality and security"

[[channels_config.a2a.peers]]
id = "peer-agent-1"
endpoint = "https://peer.example.com:9000"
bearer_token = "your-bearer-token-here"
enabled = true
```

### 2. Discover a Peer Agent

```bash
# Fetch the peer's AgentCard
curl https://peer.example.com:9000/.well-known/agent.json

# Response:
# {
#   "name": "Peer Agent",
#   "description": "...",
#   "version": "0.1.0",
#   "capabilities": {...},
#   "skills": [...]
# }
```

### 3. Create a Task

```bash
# Send a task to a peer
curl -X POST https://peer.example.com:9000/tasks \
  -H "Authorization: Bearer your-token" \
  -H "Content-Type: application/json" \
  -d '{
    "message": {
      "role": "user",
      "content": "Review this code: fn main() { println!(\"Hello\"); }"
    }
  }'

# Response:
# {
#   "task": {
#     "id": "task-123",
#     "status": "pending",
#     "messages": [...],
#     "created_at": "2026-02-20T16:00:00Z"
#   }
# }
```

### 4. Stream Task Updates

```bash
# Subscribe to task updates via SSE
curl -N https://peer.example.com:9000/tasks/task-123/stream \
  -H "Authorization: Bearer your-token"

# SSE stream:
# data: {"status":"running","message":"Processing..."}
# data: {"status":"completed","result":"Code looks good!"}
```

### 5. Start the Daemon

```bash
zeroclaw daemon
```

The A2A channel will:
- Serve your AgentCard at `/.well-known/agent.json`
- Accept task creation at `/tasks`
- Stream task updates at `/tasks/{id}/stream`

## Configuration Reference

### A2AConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable A2A channel |
| `listen_port` | u16 | `9000` | Port to listen on |
| `discovery_mode` | string | `"static"` | Peer discovery mode |
| `allowed_peer_ids` | [string] | `[]` | Allowed peer IDs (`"*"` = all, `[]` = deny all) |

### AgentCard Configuration

```toml
[channels_config.a2a.agent_card]
name = "My Agent"
description = "What this agent does"

[[channels_config.a2a.agent_card.skills]]
id = "skill-id"
name = "Skill Name"
description = "What this skill does"
```

### Peer Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique peer identifier |
| `endpoint` | string | yes | Peer base URL (e.g., `https://peer.example.com:9000`) |
| `bearer_token` | string | yes | Static authentication token |
| `enabled` | bool | yes | Whether to connect to this peer |

### Rate Limiting (Optional)

```toml
[channels_config.a2a.rate_limit]
requests_per_minute = 60
burst_size = 10
```

### Connection Resilience (Optional)

```toml
[channels_config.a2a.reconnect]
initial_delay_secs = 2
max_delay_secs = 60
max_retries = 10
```

## Google A2A Endpoints

ZeroClaw implements these standard endpoints:

| Endpoint | Method | Auth | Description |
|----------|--------|------|-------------|
| `/.well-known/agent.json` | GET | None | Agent discovery (AgentCard) |
| `/tasks` | POST | Bearer | Create a new task |
| `/tasks/{id}` | GET | Bearer | Get task status and result |
| `/tasks/{id}/stream` | GET | Bearer | Subscribe to task updates (SSE) |
| `/tasks/{id}/cancel` | POST | Bearer | Cancel a running task |

## Security Considerations

- **TLS Recommended**: Use HTTPS for production deployments
- **Bearer Tokens**: Static tokens stored in config (consider encryption)
- **Allowlist**: Empty `allowed_peer_ids` denies all inbound connections
- **Rate Limiting**: Prevents abuse and DoS attacks
- **No Dynamic Pairing**: Tokens are configured statically (no pairing flow)

## Multi-Agent Setup Example

Configure three agents in a mesh network:

**Agent Alpha** (`~/.zeroclaw/config.toml`):
```toml
[channels_config.a2a]
enabled = true
listen_port = 9000
allowed_peer_ids = ["agent-beta", "agent-gamma"]

[channels_config.a2a.agent_card]
name = "Agent Alpha"
description = "Primary coordinator agent"

[[channels_config.a2a.peers]]
id = "agent-beta"
endpoint = "https://192.168.1.101:9000"
bearer_token = "beta-token-123"
enabled = true

[[channels_config.a2a.peers]]
id = "agent-gamma"
endpoint = "https://192.168.1.102:9000"
bearer_token = "gamma-token-456"
enabled = true
```

**Agent Beta** and **Agent Gamma**: Similar configuration with appropriate peer endpoints.

## Troubleshooting

### Connection refused

```bash
# Test if peer is reachable
curl https://peer.example.com:9000/.well-known/agent.json

# Check firewall rules
sudo ufw status
```

### Authentication failed (401/403)

- Verify bearer token matches peer's configuration
- Check that your peer ID is in the peer's `allowed_peer_ids`
- Ensure `Authorization: Bearer <token>` header is present

### Task not found (404)

- Task IDs are ephemeral (not persisted across restarts)
- Verify task was created successfully (check response)
- Check task ID is correct in the URL

### SSE stream disconnects

- Network instability between agents
- Increase `max_retries` in reconnect config
- Check for firewall/proxy timeouts on long-lived connections

## Current Limitations

⚠️ **Note:** The following features are not yet fully implemented:

1. **Task Streaming**: The `listen()` method is stubbed and does not actively stream task updates
2. **Task Persistence**: Tasks are not stored; they exist only in memory
3. **Task Processing**: Task creation returns a stub response; no actual agent processing yet

These will be implemented in future updates. See `docs/a2a-cleanup-progress.md` for status.

## Log Keywords

Use these keywords to filter A2A-related log events:

| Signal Type | Log Keyword |
|-------------|-------------|
| Startup | `A2A channel listening on port` |
| AgentCard request | `Agent card requested` |
| Task created | `A2A task created` |
| Authorization failure | `Unauthorized` / `Forbidden` |
| Task stream | `A2A task stream requested` |

## See Also

- [Google A2A Protocol Specification](https://github.com/google/A2A)
- [Channels Reference](./channels-reference.md) — Complete channel configuration
- [Config Reference](./config-reference.md) — General configuration options
- [Troubleshooting](./troubleshooting.md) — General troubleshooting guide
- [A2A Cleanup Progress](./a2a-cleanup-progress.md) — Implementation status
