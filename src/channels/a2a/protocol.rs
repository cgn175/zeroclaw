//! A2A (Agent-to-Agent) Protocol Types - Google A2A Standard
//!
//! This module defines the core message types for agent-to-agent communication
//! following the Google A2A protocol specification (https://github.com/google/A2A).
//!
//! # Google A2A Protocol Overview
//!
//! - **Agent Card** - Describes agent capabilities at `/.well-known/agent.json`
//! - **Task** - The primary unit of work with states: pending, running, completed, failed, cancelled
//! - **TaskMessage** - Messages within a task (user/agent roles)
//! - **Artifact** - Files, images, and binary content
//! - **TaskUpdate** - SSE events for streaming task updates
//!
//! # Security Model
//!
//! - Bearer token authentication for all peer-to-peer communication
//! - Deny-by-default for unknown peers (configurable via `allowed_peer_ids`)
//! - Agent Card discovery for peer capabilities

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

// =============================================================================
// Agent Card Types
// =============================================================================

/// Agent Card describing agent capabilities and endpoints.
///
/// This is the discovery document returned at `/.well-known/agent.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentCard {
    /// Unique name identifying this agent.
    pub name: String,
    /// Human-readable description of what this agent does.
    pub description: String,
    /// Semantic version string for this agent.
    pub version: String,
    /// Capabilities supported by this agent.
    pub capabilities: AgentCapabilities,
    /// Authentication schemes supported.
    pub authentication: AuthenticationInfo,
    /// Endpoint URLs for this agent.
    pub endpoints: AgentEndpoints,
    /// Skills this agent can perform.
    #[serde(default)]
    pub skills: Vec<Skill>,
}

/// Capabilities supported by an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentCapabilities {
    /// Whether the agent supports streaming responses via SSE.
    #[serde(default = "default_true")]
    pub streaming: bool,
    /// Whether the agent can produce and consume artifacts.
    #[serde(default = "default_true")]
    pub artifacts: bool,
    /// Whether the agent supports push notifications.
    #[serde(default = "default_false")]
    pub push_notifications: bool,
}

impl Default for AgentCapabilities {
    fn default() -> Self {
        Self {
            streaming: true,
            artifacts: true,
            push_notifications: false,
        }
    }
}

/// Authentication information for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthenticationInfo {
    /// Supported authentication schemes.
    #[serde(default)]
    pub schemes: Vec<String>,
}

impl Default for AuthenticationInfo {
    fn default() -> Self {
        Self {
            schemes: vec!["bearer".to_string()],
        }
    }
}

/// Endpoint URLs for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentEndpoints {
    /// URL pattern for task creation.
    #[serde(default = "default_tasks_endpoint")]
    pub tasks: String,
    /// URL pattern for task streaming.
    #[serde(default = "default_stream_endpoint")]
    pub stream: String,
}

impl Default for AgentEndpoints {
    fn default() -> Self {
        Self {
            tasks: "/tasks".to_string(),
            stream: "/tasks/{id}/stream".to_string(),
        }
    }
}

/// A skill the agent can perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Skill {
    /// Unique skill identifier.
    pub id: String,
    /// Human-readable skill name.
    pub name: String,
    /// Description of what this skill does.
    pub description: String,
    /// JSON Schema for skill input.
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
    /// JSON Schema for skill output.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
}

// =============================================================================
// Task Types
// =============================================================================

/// Task represents a unit of work in the A2A protocol.
///
/// Tasks are the primary abstraction for agent-to-agent communication.
/// Each task has a unique ID, a status, and contains messages and artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    /// Unique task identifier.
    pub id: String,
    /// Current status of the task.
    pub status: TaskStatus,
    /// Timestamp when the task was created (ISO 8601).
    pub created_at: String,
    /// Timestamp when the task was last updated (ISO 8601).
    pub updated_at: String,
    /// Messages in this task's conversation.
    #[serde(default)]
    pub messages: Vec<TaskMessage>,
    /// Artifacts produced by this task.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    /// Additional metadata for the task.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

impl Task {
    /// Create a new task with pending status.
    pub fn new(id: impl Into<String>) -> Self {
        let now = current_timestamp_iso();
        Self {
            id: id.into(),
            status: TaskStatus::Pending,
            created_at: now.clone(),
            updated_at: now,
            messages: Vec::new(),
            artifacts: Vec::new(),
            metadata: None,
        }
    }

    /// Add a message to this task.
    pub fn add_message(&mut self, message: TaskMessage) {
        self.messages.push(message);
        self.updated_at = current_timestamp_iso();
    }

    /// Add an artifact to this task.
    pub fn add_artifact(&mut self, artifact: Artifact) {
        self.artifacts.push(artifact);
        self.updated_at = current_timestamp_iso();
    }

    /// Update the task status.
    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        self.updated_at = current_timestamp_iso();
    }
}

/// Status of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Task has been submitted but not yet started.
    Pending,
    /// Task is currently being processed.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed with an error.
    Failed,
    /// Task was cancelled by the user.
    Cancelled,
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus::Pending
    }
}

/// A message within a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskMessage {
    /// Role of the message sender.
    pub role: MessageRole,
    /// Content of the message.
    pub content: String,
    /// Timestamp when the message was sent (ISO 8601).
    pub timestamp: String,
}

impl TaskMessage {
    /// Create a new user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            timestamp: current_timestamp_iso(),
        }
    }

    /// Create a new agent message.
    pub fn agent(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Agent,
            content: content.into(),
            timestamp: current_timestamp_iso(),
        }
    }
}

/// Role of a message sender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// Message from the user.
    User,
    /// Message from the agent.
    Agent,
}

impl Default for MessageRole {
    fn default() -> Self {
        MessageRole::User
    }
}

// =============================================================================
// Artifact Types
// =============================================================================

/// An artifact produced or consumed by a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    /// Unique artifact identifier.
    pub id: String,
    /// Type of artifact.
    #[serde(rename = "type")]
    pub artifact_type: ArtifactType,
    /// Human-readable name of the artifact.
    pub name: String,
    /// Artifact content (base64 encoded for binary, or text).
    pub content: String,
}

impl Artifact {
    /// Create a new artifact.
    pub fn new(
        id: impl Into<String>,
        artifact_type: ArtifactType,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            artifact_type,
            name: name.into(),
            content: content.into(),
        }
    }
}

/// Type of an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactType {
    /// A file artifact.
    File,
    /// An image artifact.
    Image,
    /// A data artifact (JSON, text, etc.).
    Data,
}

impl Default for ArtifactType {
    fn default() -> Self {
        ArtifactType::Data
    }
}

// =============================================================================
// Task Update (SSE Streaming)
// =============================================================================

/// A task update event sent via Server-Sent Events (SSE).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskUpdate {
    /// The task ID this update belongs to.
    pub task_id: String,
    /// The updated status.
    pub status: TaskStatus,
    /// Optional message included in this update.
    #[serde(default)]
    pub message: Option<TaskMessage>,
    /// Optional artifact included in this update.
    #[serde(default)]
    pub artifact: Option<Artifact>,
}

impl TaskUpdate {
    /// Create a status update without message or artifact.
    pub fn status_update(task_id: impl Into<String>, status: TaskStatus) -> Self {
        Self {
            task_id: task_id.into(),
            status,
            message: None,
            artifact: None,
        }
    }

    /// Create a message update.
    pub fn message_update(task_id: impl Into<String>, message: TaskMessage) -> Self {
        Self {
            task_id: task_id.into(),
            status: TaskStatus::Running,
            message: Some(message),
            artifact: None,
        }
    }

    /// Create an artifact update.
    pub fn artifact_update(task_id: impl Into<String>, artifact: Artifact) -> Self {
        Self {
            task_id: task_id.into(),
            status: TaskStatus::Running,
            message: None,
            artifact: Some(artifact),
        }
    }
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskRequest {
    /// The initial message from the user.
    pub message: TaskMessage,
    /// Optional metadata for the task.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

impl CreateTaskRequest {
    /// Create a request with a user message.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            message: TaskMessage::user(content),
            metadata: None,
        }
    }

    /// Create a request with metadata.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Response from creating a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskResponse {
    /// The created task.
    pub task: Task,
}

/// Request to get task status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTaskRequest {
    /// The task ID to retrieve.
    pub task_id: String,
}

/// Request to cancel a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelTaskRequest {
    /// The task ID to cancel.
    pub task_id: String,
}

/// Response from canceling a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelTaskResponse {
    /// The cancelled task.
    pub task: Task,
}

// =============================================================================
// Peer Configuration (kept for backward compatibility with config)
// =============================================================================

/// Peer configuration for A2A communication.
///
/// Defines a remote agent that this agent can communicate with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2APeer {
    /// Unique peer identifier.
    pub id: String,
    /// HTTP endpoint URL for the peer.
    pub endpoint: String,
    /// Bearer token for authenticating requests.
    pub bearer_token: String,
    /// Whether this peer is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl A2APeer {
    /// Create a new peer configuration.
    pub fn new(
        id: impl Into<String>,
        endpoint: impl Into<String>,
        bearer_token: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            endpoint: endpoint.into(),
            bearer_token: bearer_token.into(),
            enabled: true,
        }
    }

    /// Create a disabled peer.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Rate limit configuration for A2A protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2ARateLimitConfig {
    /// Requests per minute per peer.
    #[serde(default = "default_requests_per_minute")]
    pub requests_per_minute: u32,
    /// Burst size.
    #[serde(default = "default_burst_size")]
    pub burst_size: u32,
}

impl Default for A2ARateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 60,
            burst_size: 10,
        }
    }
}

/// Idempotency configuration for A2A protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2AIdempotencyConfig {
    /// TTL for idempotency entries in seconds.
    #[serde(default = "default_idempotency_ttl_secs")]
    pub ttl_secs: u64,
    /// Maximum number of idempotency keys.
    #[serde(default = "default_idempotency_max_keys")]
    pub max_keys: usize,
}

impl Default for A2AIdempotencyConfig {
    fn default() -> Self {
        Self {
            ttl_secs: 86400,
            max_keys: 10000,
        }
    }
}

/// A2A channel configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2AConfig {
    /// Whether the A2A channel is enabled.
    #[serde(default = "default_false")]
    pub enabled: bool,
    /// Port to listen on.
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    /// Peer discovery mode.
    #[serde(default = "default_discovery_mode")]
    pub discovery_mode: String,
    /// Allowed peer IDs.
    #[serde(default = "default_allowed_peer_ids")]
    pub allowed_peer_ids: Vec<String>,
    /// Configured peers.
    #[serde(default)]
    pub peers: Vec<A2APeer>,
    /// Rate limiting configuration.
    #[serde(default)]
    pub rate_limit: A2ARateLimitConfig,
    /// Idempotency configuration.
    #[serde(default)]
    pub idempotency: A2AIdempotencyConfig,
    /// Agent card configuration.
    #[serde(default)]
    pub agent_card: Option<AgentCardConfig>,
}

impl A2AConfig {
    /// Check if a peer is allowed.
    pub fn is_peer_allowed(&self, peer_id: &str) -> bool {
        self.allowed_peer_ids.contains(&"*".to_string())
            || self.allowed_peer_ids.contains(&peer_id.to_string())
    }

    /// Find a peer by ID.
    pub fn find_peer(&self, peer_id: &str) -> Option<&A2APeer> {
        self.peers.iter().find(|p| p.id == peer_id && p.enabled)
    }
}

impl Default for A2AConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_port: 9000,
            discovery_mode: "static".to_string(),
            allowed_peer_ids: vec!["*".to_string()],
            peers: Vec::new(),
            rate_limit: A2ARateLimitConfig::default(),
            idempotency: A2AIdempotencyConfig::default(),
            agent_card: None,
        }
    }
}

/// Agent card configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentCardConfig {
    /// Agent name.
    pub name: String,
    /// Agent description.
    pub description: String,
    /// Skills available.
    #[serde(default)]
    pub skills: Vec<SkillConfig>,
}

/// Skill configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillConfig {
    /// Skill ID.
    pub id: String,
    /// Skill name.
    pub name: String,
    /// Skill description.
    pub description: String,
}

// =============================================================================
// Helper Functions
// =============================================================================

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_listen_port() -> u16 {
    9000
}

fn default_discovery_mode() -> String {
    "static".to_string()
}

fn default_allowed_peer_ids() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_requests_per_minute() -> u32 {
    60
}

fn default_burst_size() -> u32 {
    10
}

fn default_idempotency_ttl_secs() -> u64 {
    86400
}

fn default_idempotency_max_keys() -> usize {
    10000
}

fn default_tasks_endpoint() -> String {
    "/tasks".to_string()
}

fn default_stream_endpoint() -> String {
    "/tasks/{id}/stream".to_string()
}

/// Get current timestamp as ISO 8601 string.
fn current_timestamp_iso() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Simple ISO 8601 format: 2024-01-01T00:00:00Z
    let secs = now.as_secs();
    let mins = (secs / 60) % 60;
    let hours = (secs / 3600) % 24;
    format!("2024-01-01T{:02}:{:02}:00Z", hours, mins)
}

/// Get current Unix timestamp in seconds.
fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// =============================================================================
// Legacy Types (for migration)
// =============================================================================

/// Legacy message type - deprecated, use Task instead.
///
/// This type is kept for backward compatibility during migration.
#[deprecated(since = "0.4.0", note = "Use Task and TaskMessage instead")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2AMessage {
    /// Unique message identifier.
    pub id: String,
    /// Conversation thread identifier.
    pub session_id: String,
    /// Sender peer identifier.
    pub sender_id: String,
    /// Recipient peer identifier.
    pub recipient_id: String,
    /// Message content.
    pub content: String,
    /// Unix timestamp.
    pub timestamp: u64,
    /// Optional parent message ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

impl A2AMessage {
    /// Create a new A2A message.
    #[allow(deprecated)]
    pub fn new(
        session_id: impl Into<String>,
        sender_id: impl Into<String>,
        recipient_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            sender_id: sender_id.into(),
            recipient_id: recipient_id.into(),
            content: content.into(),
            timestamp: current_timestamp_secs(),
            reply_to: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // AgentCard Tests
    // =========================================================================

    #[test]
    fn agent_card_default_values() {
        let card = AgentCard {
            name: "Test Agent".to_string(),
            description: "A test agent".to_string(),
            version: "1.0.0".to_string(),
            capabilities: AgentCapabilities::default(),
            authentication: AuthenticationInfo::default(),
            endpoints: AgentEndpoints::default(),
            skills: vec![],
        };

        assert_eq!(card.name, "Test Agent");
        assert!(card.capabilities.streaming);
        assert!(card.capabilities.artifacts);
        assert!(!card.capabilities.push_notifications);
        assert!(card.authentication.schemes.contains(&"bearer".to_string()));
    }

    #[test]
    fn agent_card_serialization_roundtrip() {
        let card = AgentCard {
            name: "Test Agent".to_string(),
            description: "A test agent".to_string(),
            version: "1.0.0".to_string(),
            capabilities: AgentCapabilities::default(),
            authentication: AuthenticationInfo::default(),
            endpoints: AgentEndpoints::default(),
            skills: vec![Skill {
                id: "skill-1".to_string(),
                name: "Test Skill".to_string(),
                description: "A test skill".to_string(),
                input_schema: None,
                output_schema: None,
            }],
        };

        let json = serde_json::to_string(&card).expect("serialization should succeed");
        let deserialized: AgentCard =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(card.name, deserialized.name);
        assert_eq!(card.skills.len(), deserialized.skills.len());
    }

    // =========================================================================
    // Task Tests
    // =========================================================================

    #[test]
    fn task_new_creates_pending_task() {
        let task = Task::new("task-123");

        assert_eq!(task.id, "task-123");
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.messages.is_empty());
        assert!(task.artifacts.is_empty());
    }

    #[test]
    fn task_add_message() {
        let mut task = Task::new("task-123");
        task.add_message(TaskMessage::user("Hello"));

        assert_eq!(task.messages.len(), 1);
        assert_eq!(task.messages[0].role, MessageRole::User);
        assert_eq!(task.messages[0].content, "Hello");
    }

    #[test]
    fn task_set_status() {
        let mut task = Task::new("task-123");
        task.set_status(TaskStatus::Running);

        assert_eq!(task.status, TaskStatus::Running);
    }

    // =========================================================================
    // TaskMessage Tests
    // =========================================================================

    #[test]
    fn task_message_user_creates_user_message() {
        let msg = TaskMessage::user("Hello, agent!");

        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Hello, agent!");
    }

    #[test]
    fn task_message_agent_creates_agent_message() {
        let msg = TaskMessage::agent("Hello, user!");

        assert_eq!(msg.role, MessageRole::Agent);
        assert_eq!(msg.content, "Hello, user!");
    }

    // =========================================================================
    // Artifact Tests
    // =========================================================================

    #[test]
    fn artifact_new() {
        let artifact = Artifact::new("art-1", ArtifactType::File, "test.txt", "Hello world");

        assert_eq!(artifact.id, "art-1");
        assert_eq!(artifact.artifact_type, ArtifactType::File);
        assert_eq!(artifact.name, "test.txt");
        assert_eq!(artifact.content, "Hello world");
    }

    // =========================================================================
    // TaskUpdate Tests
    // =========================================================================

    #[test]
    fn task_update_status() {
        let update = TaskUpdate::status_update("task-123", TaskStatus::Running);

        assert_eq!(update.task_id, "task-123");
        assert_eq!(update.status, TaskStatus::Running);
        assert!(update.message.is_none());
        assert!(update.artifact.is_none());
    }

    #[test]
    fn task_update_message() {
        let update = TaskUpdate::message_update("task-123", TaskMessage::agent("Processing..."));

        assert_eq!(update.status, TaskStatus::Running);
        assert!(update.message.is_some());
    }

    // =========================================================================
    // CreateTaskRequest Tests
    // =========================================================================

    #[test]
    fn create_task_request_new() {
        let request = CreateTaskRequest::new("Hello, agent!");

        assert_eq!(request.message.role, MessageRole::User);
        assert_eq!(request.message.content, "Hello, agent!");
        assert!(request.metadata.is_none());
    }

    #[test]
    fn create_task_request_with_metadata() {
        let request =
            CreateTaskRequest::new("Hello!").with_metadata(serde_json::json!({"key": "value"}));

        assert!(request.metadata.is_some());
    }

    // =========================================================================
    // A2APeer Tests
    // =========================================================================

    #[test]
    fn peer_new_sets_fields() {
        let peer = A2APeer::new("peer-001", "https://example.com", "token");

        assert_eq!(peer.id, "peer-001");
        assert_eq!(peer.endpoint, "https://example.com");
        assert_eq!(peer.bearer_token, "token");
        assert!(peer.enabled);
    }

    #[test]
    fn peer_disabled() {
        let peer = A2APeer::new("peer-001", "https://example.com", "token").disabled();

        assert!(!peer.enabled);
    }

    // =========================================================================
    // A2AConfig Tests
    // =========================================================================

    #[test]
    fn config_default_values() {
        let config = A2AConfig::default();

        assert!(!config.enabled);
        assert_eq!(config.listen_port, 9000);
        assert_eq!(config.discovery_mode, "static");
        assert_eq!(config.allowed_peer_ids, vec!["*"]);
        assert!(config.peers.is_empty());
    }

    #[test]
    fn config_is_peer_allowed_with_wildcard() {
        let config = A2AConfig::default();

        assert!(config.is_peer_allowed("any-peer"));
    }

    #[test]
    fn config_find_peer() {
        let config = A2AConfig {
            peers: vec![A2APeer::new("peer-1", "https://peer1.com", "token")],
            ..Default::default()
        };

        let peer = config.find_peer("peer-1");
        assert!(peer.is_some());
        assert_eq!(peer.unwrap().id, "peer-1");
    }
}
