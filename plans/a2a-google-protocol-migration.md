# A2A Protocol Migration to Google A2A Standard

**Status:** Planning  
**Created:** 2026-02-20  
**Target:** Migrate from custom A2A protocol to Google's A2A open standard  
**Scope:** Core protocol types and endpoints only (no backward compatibility)

## Overview

This plan outlines the migration from ZeroClaw's custom A2A (Agent-to-Agent) protocol to Google's official A2A protocol standard (https://github.com/google/A2A). The Google A2A protocol is an open standard for agent-to-agent communication that enables interoperability between different agent frameworks.

**Important:** This plan focuses on implementing the core Google A2A specification. Legacy endpoints will be replaced, not maintained in parallel.

## Current Implementation Summary

ZeroClaw currently has a custom A2A implementation with:

- **Transport:** HTTP/2 + SSE (Server-Sent Events)
- **Endpoints:**
  - `POST /a2a/send` - Send messages to peer
  - `GET /a2a/stream/:session_id` - SSE stream for responses
  - `GET /a2a/health` - Health check
  - `POST /a2a/pair/request` - Request pairing code
  - `POST /a2a/pair/confirm` - Confirm pairing
- **Message Format:** Custom JSON structure with `id`, `session_id`, `sender_id`, `recipient_id`, `content`, `timestamp`, `reply_to`
- **Authentication:** Bearer tokens with pairing flow
- **Discovery:** Static peer configuration

## Google A2A Protocol Specification

### Core Concepts

1. **Agent Card** - A JSON document describing an agent's capabilities, endpoints, and authentication requirements
2. **Tasks** - The primary unit of work in A2A, with states: `pending`, `running`, `completed`, `failed`, `cancelled`
3. **Messages** - User messages and agent responses within a task
4. **Artifacts** - Files, images, and other binary content produced or consumed by agents
5. **Streaming** - SSE for real-time task updates

### Key Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/.well-known/agent.json` | GET | Agent discovery - returns Agent Card |
| `/tasks` | POST | Create a new task |
| `/tasks/{id}` | GET | Get task status and result |
| `/tasks/{id}/cancel` | POST | Cancel a running task |
| `/tasks/{id}/stream` | GET | SSE stream for task updates |

### Protocol Types

#### AgentCard

```json
{
  "name": "string",
  "description": "string",
  "version": "string",
  "capabilities": {
    "streaming": true,
    "artifacts": true,
    "push_notifications": false
  },
  "authentication": {
    "schemes": ["bearer", "api_key"]
  },
  "endpoints": {
    "tasks": "/tasks",
    "stream": "/tasks/{id}/stream"
  },
  "skills": [
    {
      "id": "string",
      "name": "string",
      "description": "string",
      "input_schema": {},
      "output_schema": {}
    }
  ]
}
```

#### Task

```json
{
  "id": "string",
  "status": "pending|running|completed|failed|cancelled",
  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z",
  "messages": [
    {
      "role": "user|agent",
      "content": "string",
      "timestamp": "2024-01-01T00:00:00Z"
    }
  ],
  "artifacts": [
    {
      "id": "string",
      "type": "file|image|data",
      "name": "string",
      "content": "base64|string"
    }
  ],
  "metadata": {}
}
```

#### TaskUpdate (SSE Event)

```json
{
  "task_id": "string",
  "status": "running",
  "message": {
    "role": "agent",
    "content": "string",
    "timestamp": "2024-01-01T00:00:00Z"
  },
  "artifact": null
}
```

## Gap Analysis (Simplified - No Backward Compatibility)

### Protocol Types

| Current | Google A2A | Migration Action |
|---------|------------|------------------|
| `A2AMessage` | `Task` + `Message` | Replace with task-based model |
| `A2APeer` | `AgentCard` | Replace with Agent Card discovery |
| Custom pairing | Authentication schemes | Adopt standard auth schemes |
| `session_id` | `task.id` | Use task IDs |

### Endpoints

| Current | Google A2A | Migration Action |
|---------|------------|------------------|
| `POST /a2a/send` | `POST /tasks` | Replace endpoint |
| `GET /a2a/stream/:session_id` | `GET /tasks/{id}/stream` | Replace endpoint |
| `GET /a2a/health` | `GET /.well-known/agent.json` | Replace with Agent Card |
| `POST /a2a/pair/*` | Authentication headers | Remove - use standard auth |

### Authentication

| Current | Google A2A | Migration Action |
|---------|------------|------------------|
| Custom pairing flow | Bearer token | Use standard Bearer auth |
| 6-digit pairing codes | Removed | Not needed |

## Migration Strategy (Direct Replacement)

### Phase 1: Replace Protocol Types

1. Replace `A2AMessage` with `Task` and `TaskMessage`
2. Replace `A2APeer` with `AgentCard` discovery
3. Update configuration schema
4. Remove old pairing code

### Phase 2: Replace Endpoints

1. Replace `POST /a2a/send` → `POST /tasks`
2. Replace `GET /a2a/stream/:session_id` → `GET /tasks/:id/stream`
3. Replace `GET /a2a/health` → `GET /.well-known/agent.json`
4. Remove `/a2a/pair/*` endpoints

### Phase 3: Update Channel Implementation

1. Update A2AChannel to use new protocol
2. Add Agent Card discovery client
3. Update send/listen implementation

### Phase 4: Cleanup

1. Remove legacy code
2. Update configuration schema
3. Update documentation

## Implementation Tasks

### Task 1: Add Protocol Types

**File:** `src/channels/a2a/protocol.rs`

Add new types:

```rust
/// Agent Card describing agent capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub version: String,
    pub capabilities: AgentCapabilities,
    pub authentication: AuthenticationInfo,
    pub endpoints: EndpointInfo,
    pub skills: Vec<Skill>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentCapabilities {
    pub streaming: bool,
    pub artifacts: bool,
    pub push_notifications: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuthenticationInfo {
    pub schemes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EndpointInfo {
    pub tasks: String,
    pub stream: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
}

/// Task represents a unit of work in A2A protocol.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    pub id: String,
    pub status: TaskStatus,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<TaskMessage>,
    pub artifacts: Vec<Artifact>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    pub id: String,
    #[serde(rename = "type")]
    pub artifact_type: ArtifactType,
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum ArtifactType {
    File,
    Image,
    Data,
}

/// Task update event for SSE streaming.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskUpdate {
    pub task_id: String,
    pub status: TaskStatus,
    pub message: Option<TaskMessage>,
    pub artifact: Option<Artifact>,
}
```

### Task 2: Add Agent Card Endpoint

**File:** `src/gateway/a2a.rs`

Add endpoint:

```rust
/// GET /.well-known/agent.json - Agent discovery.
pub async fn handle_agent_card(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let card = AgentCard {
        name: "ZeroClaw Agent".to_string(),
        description: "ZeroClaw autonomous agent".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: AgentCapabilities {
            streaming: true,
            artifacts: true,
            push_notifications: false,
        },
        authentication: AuthenticationInfo {
            schemes: vec!["bearer".to_string()],
        },
        endpoints: EndpointInfo {
            tasks: "/a2a/v1/tasks".to_string(),
            stream: "/a2a/v1/tasks/{id}/stream".to_string(),
        },
        skills: vec![],
    };
    
    Json(card)
}
```

### Task 3: Add Task Endpoints

**File:** `src/gateway/a2a.rs`

Add new endpoints:

```rust
/// POST /a2a/v1/tasks - Create a new task.
pub async fn handle_create_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Json<CreateTaskRequest>,
) -> impl IntoResponse {
    // 1. Verify authentication
    // 2. Create task with pending status
    // 3. Queue for processing
    // 4. Return task with 201 Created
}

/// GET /a2a/v1/tasks/{id} - Get task status.
pub async fn handle_get_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    // 1. Verify authentication
    // 2. Look up task
    // 3. Return task or 404
}

/// GET /a2a/v1/tasks/{id}/stream - SSE stream for task updates.
pub async fn handle_task_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    // 1. Verify authentication
    // 2. Create SSE stream
    // 3. Stream task updates
}

/// POST /a2a/v1/tasks/{id}/cancel - Cancel a task.
pub async fn handle_cancel_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    // 1. Verify authentication
    // 2. Cancel task if running
    // 3. Return updated task
}
```

### Task 4: Update Channel Implementation

**File:** `src/channels/a2a/channel.rs`

Add Google A2A client methods:

```rust
impl A2AChannel {
    /// Discover peer agent via Agent Card.
    pub async fn discover_peer(&self, endpoint: &str) -> Result<AgentCard> {
        let url = format!("{}/.well-known/agent.json", endpoint.trim_end_matches('/'));
        let response = self.http_client
            .get(&url)
            .send()
            .await?;
        
        let card: AgentCard = response.json().await?;
        Ok(card)
    }
    
    /// Create a task on a peer agent.
    pub async fn create_task(&self, peer: &A2APeer, content: &str) -> Result<Task> {
        let url = format!("{}/tasks", peer.endpoint.trim_end_matches('/'));
        let request = CreateTaskRequest {
            message: content.to_string(),
            metadata: None,
        };
        
        let response = self.http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", peer.bearer_token))
            .json(&request)
            .send()
            .await?;
        
        let task: Task = response.json().await?;
        Ok(task)
    }
    
    /// Stream task updates from a peer.
    pub async fn stream_task(&self, peer: &A2APeer, task_id: &str) -> Result<impl Stream<Item = TaskUpdate>> {
        let url = format!("{}/tasks/{}/stream", peer.endpoint.trim_end_matches('/'), task_id);
        
        // SSE stream parsing
        // ...
    }
}
```

### Task 5: Update Configuration Schema

**File:** `src/config/schema.rs`

Update A2A configuration:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct A2AConfig {
    /// Whether the A2A channel is enabled
    pub enabled: bool,
    
    /// Port to listen on for incoming A2A connections
    pub listen_port: u16,
    
    /// Discovery mode: "static" or "auto"
    pub discovery_mode: String,
    
    /// Allowed peer IDs
    pub allowed_peer_ids: Vec<String>,
    
    /// Configured peers
    pub peers: Vec<A2APeer>,
    
    /// Agent card configuration
    #[serde(default)]
    pub agent_card: Option<AgentCardConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentCardConfig {
    pub name: String,
    pub description: String,
    pub skills: Vec<SkillConfig>,
}
```

### Task 6: Update Router

**File:** `src/gateway/mod.rs`

Register new routes:

```rust
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Google A2A endpoints (replacing legacy)
        .route("/.well-known/agent.json", get(handle_agent_card))
        .route("/tasks", post(handle_create_task))
        .route("/tasks/:id", get(handle_get_task))
        .route("/tasks/:id/stream", get(handle_task_stream))
        .route("/tasks/:id/cancel", post(handle_cancel_task))
        .with_state(state)
}
```

### Task 7: Add Task Store

**File:** `src/channels/a2a/store.rs` (new file)

```rust
/// In-memory task store with TTL cleanup.
pub struct TaskStore {
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    ttl: Duration,
    max_tasks: usize,
}

struct TaskEntry {
    task: Task,
    created_at: Instant,
}

impl TaskStore {
    pub fn new(ttl: Duration, max_tasks: usize) -> Self { ... }
    
    pub async fn create(&self, task: Task) -> Result<()> { ... }
    pub async fn get(&self, id: &str) -> Option<Task> { ... }
    pub async fn update(&self, task: Task) -> Result<()> { ... }
    pub async fn delete(&self, id: &str) -> Result<()> { ... }
}
```

### Task 8: Update Documentation

**Files:** 
- `docs/a2a-setup.md`
- `docs/channels-reference.md`

Update documentation to reflect:
- New Google A2A endpoints
- Agent Card discovery
- Task-based workflow

## File Structure

```
src/channels/a2a/
├── mod.rs           # Module exports
├── channel.rs       # Channel implementation (replace)
├── protocol.rs      # Protocol types (replace with Google A2A)
├── processor.rs     # Message processing (update)
└── store.rs         # Task store (new)

src/gateway/
├── a2a.rs           # A2A endpoints (replace with Google A2A)
└── mod.rs           # Router registration (update)

docs/
├── a2a-setup.md     # Updated setup guide
└── channels-reference.md  # Updated reference
```

## Testing Strategy

1. **Unit Tests:** Protocol types and serialization
2. **Integration Tests:** Task creation, retrieval, streaming
3. **Interoperability:** Test with other Google A2A implementations

## Success Criteria

- [ ] Agent Card endpoint returns valid Google A2A format
- [ ] Task creation and retrieval work correctly
- [ ] SSE streaming delivers task updates
- [ ] Documentation is complete
- [ ] All tests pass

## References

- Google A2A Specification: https://github.com/google/A2A
- Google A2A Documentation: https://google.github.io/A2A/
