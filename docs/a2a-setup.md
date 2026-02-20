# A2A (Agent-to-Agent) Protocol Setup

## Overview

ZeroClaw's A2A protocol enables direct agent-to-agent communication without third-party channels (Telegram, Discord, etc.). Agents communicate over HTTP/2 with Server-Sent Events (SSE) for bidirectional streaming.

## Architecture

- **Transport**: HTTP/2 + SSE for bidirectional communication
- **Authentication**: Bearer token authentication per peer
- **Discovery**: Static peer discovery via configuration
- **Message Format**: JSON with UUID-based message tracking
- **Default Port**: 9000 (dedicated A2A port)

```
Agent A                          Agent B
   |                                |
   |--POST /a2a/send--------------->| (send message)
   |                                | (process via agent loop)
   |<--SSE /a2a/stream/:session_id--| (stream response)
   |                                |
```

## Quick Start

### 1. Enable A2A Channel

Add to `~/.zeroclaw/config.toml`:

```toml
[channels_config.a2a]
enabled = true
listen_port = 9000
discovery_mode = "static"
allowed_peer_ids = ["*"]

[[channels_config.a2a.peers]]
id = "agent-alpha"
endpoint = "https://192.168.1.100:9000"
bearer_token = "encrypted:..."
enabled = true
```

### 2. Pair with a Peer

```bash
# Initiate pairing with a remote agent
zeroclaw channel pair-a2a https://peer.example.com:9000

# Enter the 6-digit pairing code when prompted
# The bearer token will be automatically encrypted and stored
```

### 3. Test Connectivity

```bash
# Test a specific peer
zeroclaw channel test-a2a-peer agent-alpha

# Send a test message
zeroclaw channel send-a2a agent-alpha "Hello from my agent!"

# List all configured peers
zeroclaw channel list-a2a-peers
```

### 4. Start the Daemon

```bash
zeroclaw daemon
```

The A2A channel will listen on the configured port and establish SSE connections to all enabled peers.

## Configuration Reference

### A2AConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable A2A channel |
| `listen_port` | u16 | `9000` | Port to listen on for incoming A2A connections |
| `discovery_mode` | string | `"static"` | Peer discovery mode (only `"static"` supported in v1) |
| `allowed_peer_ids` | [string] | `["*"]` | Allowed peer IDs (`"*"` allows all, empty denies all) |

### Peer Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique peer identifier |
| `endpoint` | string | yes | Peer URL (e.g., `https://192.168.1.100:9000`) |
| `bearer_token` | string | yes | Authentication token (stored encrypted) |
| `enabled` | bool | yes | Whether to connect to this peer |

### Rate Limiting

```toml
[channels_config.a2a.rate_limit]
requests_per_minute = 60
burst_size = 10
```

### Connection Resilience

```toml
[channels_config.a2a.reconnect]
initial_delay_secs = 2
max_delay_secs = 60
max_retries = 10
```

Reconnection uses exponential backoff: 2s, 4s, 8s, 16s, 32s, then caps at 60s.

## Security Considerations

- **TLS Required**: Always use HTTPS for production endpoints (enforced except for localhost)
- **Bearer Tokens**: Encrypted at rest using the configured secrets provider
- **Pairing Codes**: Expire after 5 minutes; single-use only
- **Allowlist**: Empty `allowed_peer_ids` denies all inbound connections (deny-by-default)
- **Rate Limiting**: Prevents abuse and retry-loop storms

## Multi-Agent Setup Example

Configure three agents that can all communicate with each other:

**Agent Alpha** (`~/.zeroclaw/config.toml`):
```toml
[channels_config.a2a]
enabled = true
listen_port = 9000
allowed_peer_ids = ["agent-beta", "agent-gamma"]

[[channels_config.a2a.peers]]
id = "agent-beta"
endpoint = "https://192.168.1.101:9000"
bearer_token = "encrypted:..."
enabled = true

[[channels_config.a2a.peers]]
id = "agent-gamma"
endpoint = "https://192.168.1.102:9000"
bearer_token = "encrypted:..."
enabled = true
```

**Agent Beta** (`~/.zeroclaw/config.toml`):
```toml
[channels_config.a2a]
enabled = true
listen_port = 9000
allowed_peer_ids = ["agent-alpha", "agent-gamma"]

[[channels_config.a2a.peers]]
id = "agent-alpha"
endpoint = "https://192.168.1.100:9000"
bearer_token = "encrypted:..."
enabled = true

[[channels_config.a2a.peers]]
id = "agent-gamma"
endpoint = "https://192.168.1.102:9000"
bearer_token = "encrypted:..."
enabled = true
```

## Troubleshooting

### Connection refused

- Check firewall rules allow traffic on `listen_port`
- Verify the peer's endpoint URL is correct and reachable
- Confirm the peer agent is running and A2A is enabled

```bash
# Test connectivity manually
curl -k https://<peer-endpoint>/a2a/health
```

### Authentication failed

- Verify the bearer token is correct and not expired
- Re-pair if the token was regenerated on the peer
- Check that `allowed_peer_ids` includes the sender's ID

```bash
# Re-pair with a peer
zeroclaw channel pair-a2a https://peer.example.com:9000
```

### Rate limit exceeded

- Adjust `rate_limit` configuration if legitimate traffic is being throttled
- Check for retry loops in client code that may be causing excessive requests
- Review logs for repeated message IDs (indicating duplicate submissions)

### SSE stream disconnects

- Check network stability between agents
- Verify reconnection configuration in logs
- Increase `max_retries` if network is intermittently unstable

### Message not received

- Confirm sender is in the recipient's `allowed_peer_ids`
- Check that the recipient's `listen_port` is not blocked by firewall
- Verify the session ID is consistent for conversation threading

## Log Keywords

Use these keywords to filter A2A-related log events:

| Signal Type | Log Keyword |
|-------------|-------------|
| Startup / healthy | `A2A channel listening on port` |
| Peer connected | `A2A: connected to peer` |
| Authorization failure | `A2A: rejected message from unauthorized peer` |
| Transport failure | `A2A: connection error` / `A2A: reconnecting to peer` |
| Rate limited | `A2A: rate limit exceeded for peer` |

## See Also

- [Channels Reference](./channels-reference.md) — Complete channel configuration reference
- [Config Reference](./config-reference.md) — General configuration options
- [Troubleshooting](./troubleshooting.md) — General troubleshooting guide
- [A2A Implementation Plan](./a2a-implementation-plan.md) — Technical implementation details
