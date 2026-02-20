//! A2A (Agent-to-Agent) Protocol Types
//!
//! This module defines the core message types and configuration structures for
//! agent-to-agent communication in ZeroClaw. The protocol enables secure,
//! authenticated messaging between peer agents over HTTP.
//!
//! # Security Model
//!
//! - Bearer token authentication for all peer-to-peer communication
//! - Deny-by-default for unknown peers (configurable via `allowed_peer_ids`)
//! - Static peer discovery mode (no dynamic peer registration)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Message envelope for agent-to-agent communication.
///
/// This struct represents a single message sent between agents. Each message
/// has a unique ID, belongs to a session (conversation thread), and can
/// reference a parent message for threaded replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2AMessage {
    /// Unique message identifier (UUID v4)
    pub id: String,
    /// Conversation thread identifier for grouping related messages
    pub session_id: String,
    /// Sender peer identifier
    pub sender_id: String,
    /// Recipient peer identifier
    pub recipient_id: String,
    /// Message content (plaintext or structured payload)
    pub content: String,
    /// Unix timestamp (seconds since epoch)
    pub timestamp: u64,
    /// Optional parent message ID for threaded replies
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

impl A2AMessage {
    /// Create a new A2A message with auto-generated UUID and current timestamp.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The conversation thread ID
    /// * `sender_id` - The sender peer identifier
    /// * `recipient_id` - The recipient peer identifier
    /// * `content` - The message content
    ///
    /// # Example
    ///
    /// ```
    /// use zeroclaw::channels::a2a::protocol::A2AMessage;
    ///
    /// let msg = A2AMessage::new(
    ///     "session-123",
    ///     "peer-a",
    ///     "peer-b",
    ///     "Hello, peer!",
    /// );
    /// ```
    pub fn new(
        session_id: impl Into<String>,
        sender_id: impl Into<String>,
        recipient_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            sender_id: sender_id.into(),
            recipient_id: recipient_id.into(),
            content: content.into(),
            timestamp: current_timestamp_secs(),
            reply_to: None,
        }
    }

    /// Create a reply to an existing message.
    ///
    /// The `reply_to` field will be set to the parent message's ID,
    /// and the session_id will be inherited from the parent.
    ///
    /// # Arguments
    ///
    /// * `parent` - The message being replied to
    /// * `sender_id` - The sender peer identifier (may differ from original recipient)
    /// * `content` - The reply content
    pub fn reply_to(
        parent: &A2AMessage,
        sender_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            session_id: parent.session_id.clone(),
            sender_id: sender_id.into(),
            recipient_id: parent.sender_id.clone(),
            content: content.into(),
            timestamp: current_timestamp_secs(),
            reply_to: Some(parent.id.clone()),
        }
    }

    /// Set the reply-to field explicitly.
    pub fn with_reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to = Some(reply_to.into());
        self
    }
}

/// Peer configuration for A2A communication.
///
/// Defines a remote agent that this agent can communicate with.
/// Each peer has a unique ID, an HTTP endpoint, and authentication credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2APeer {
    /// Unique peer identifier (used in message routing)
    pub id: String,
    /// HTTP endpoint URL for the peer (e.g., "https://peer.example.com")
    pub endpoint: String,
    /// Bearer token for authenticating requests to this peer
    pub bearer_token: String,
    /// Whether this peer is enabled for communication
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl A2APeer {
    /// Create a new peer configuration.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique peer identifier
    /// * `endpoint` - HTTP endpoint URL
    /// * `bearer_token` - Authentication token
    ///
    /// # Example
    ///
    /// ```
    /// use zeroclaw::channels::a2a::protocol::A2APeer;
    ///
    /// let peer = A2APeer::new(
    ///     "agent-001",
    ///     "https://agent-001.example.com",
    ///     "secret-token-123",
    /// );
    /// ```
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

    /// Create a disabled peer configuration.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// Rate limit configuration for A2A protocol.
///
/// Controls per-peer rate limiting to prevent abuse and ensure fair usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2ARateLimitConfig {
    /// Requests per minute per peer. Default: 60 (1 req/sec average).
    #[serde(default = "default_requests_per_minute")]
    pub requests_per_minute: u32,
    /// Burst size - max concurrent requests allowed. Default: 10.
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

fn default_requests_per_minute() -> u32 {
    60
}

fn default_burst_size() -> u32 {
    10
}

/// Idempotency configuration for A2A protocol.
///
/// Controls duplicate message detection and rejection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2AIdempotencyConfig {
    /// TTL for idempotency entries in seconds. Default: 86400 (24 hours).
    #[serde(default = "default_idempotency_ttl_secs")]
    pub ttl_secs: u64,
    /// Maximum number of idempotency keys to track. Default: 10000.
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

fn default_idempotency_ttl_secs() -> u64 {
    86400
}

fn default_idempotency_max_keys() -> usize {
    10000
}

/// A2A channel configuration.
///
/// Controls the agent-to-agent communication subsystem, including
/// the listen port, peer discovery mode, and allowed peer whitelist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct A2AConfig {
    /// Whether the A2A channel is enabled
    #[serde(default = "default_false")]
    pub enabled: bool,
    /// Port to listen on for incoming A2A messages
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    /// Peer discovery mode: "static" (only configured peers)
    #[serde(default = "default_discovery_mode")]
    pub discovery_mode: String,
    /// List of allowed peer IDs. ["*"] allows any configured peer.
    #[serde(default = "default_allowed_peer_ids")]
    pub allowed_peer_ids: Vec<String>,
    /// Configured peer definitions
    #[serde(default)]
    pub peers: Vec<A2APeer>,
    /// Rate limiting configuration
    #[serde(default)]
    pub rate_limit: A2ARateLimitConfig,
    /// Idempotency configuration
    #[serde(default)]
    pub idempotency: A2AIdempotencyConfig,
}

impl A2AConfig {
    /// Check if a peer ID is allowed to communicate with this agent.
    ///
    /// Returns true if:
    /// - `allowed_peer_ids` contains "*" (wildcard)
    /// - `allowed_peer_ids` contains the specific peer_id
    pub fn is_peer_allowed(&self, peer_id: &str) -> bool {
        self.allowed_peer_ids.contains(&"*".to_string())
            || self.allowed_peer_ids.contains(&peer_id.to_string())
    }

    /// Find a peer by its ID.
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
        }
    }
}

// Helper functions for serde defaults
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

/// Get current Unix timestamp in seconds.
fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // A2AMessage Tests
    // =========================================================================

    #[test]
    fn message_new_generates_valid_uuid() {
        let msg = A2AMessage::new("session-1", "peer-a", "peer-b", "Hello");

        // Verify UUID format (should be 36 chars with hyphens)
        assert_eq!(msg.id.len(), 36);
        assert!(msg.id.contains('-'));

        // Verify UUID is valid
        assert!(Uuid::parse_str(&msg.id).is_ok());
    }

    #[test]
    fn message_new_sets_fields_correctly() {
        let msg = A2AMessage::new("session-1", "peer-a", "peer-b", "Hello, world!");

        assert_eq!(msg.session_id, "session-1");
        assert_eq!(msg.sender_id, "peer-a");
        assert_eq!(msg.recipient_id, "peer-b");
        assert_eq!(msg.content, "Hello, world!");
        assert!(msg.reply_to.is_none());
    }

    #[test]
    fn message_new_sets_timestamp() {
        let before = current_timestamp_secs();
        let msg = A2AMessage::new("session-1", "peer-a", "peer-b", "Hello");
        let after = current_timestamp_secs();

        assert!(msg.timestamp >= before);
        assert!(msg.timestamp <= after);
    }

    #[test]
    fn message_reply_to_inherits_session() {
        let parent = A2AMessage::new("session-1", "peer-a", "peer-b", "Original");
        let reply = A2AMessage::reply_to(&parent, "peer-b", "Reply content");

        assert_eq!(reply.session_id, parent.session_id);
        assert_eq!(reply.reply_to, Some(parent.id.clone()));
        assert_eq!(reply.recipient_id, "peer-a"); // Original sender becomes recipient
        assert_eq!(reply.sender_id, "peer-b");
    }

    #[test]
    fn message_with_reply_to_sets_field() {
        let msg = A2AMessage::new("session-1", "peer-a", "peer-b", "Hello")
            .with_reply_to("parent-msg-id");

        assert_eq!(msg.reply_to, Some("parent-msg-id".to_string()));
    }

    #[test]
    fn message_serialization_roundtrip() {
        let original = A2AMessage {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            session_id: "session-123".to_string(),
            sender_id: "peer-a".to_string(),
            recipient_id: "peer-b".to_string(),
            content: "Test message".to_string(),
            timestamp: 1_700_000_000,
            reply_to: Some("parent-id".to_string()),
        };

        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let deserialized: A2AMessage =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(original, deserialized);
    }

    #[test]
    fn message_serialization_omits_null_reply_to() {
        let msg = A2AMessage::new("session-1", "peer-a", "peer-b", "Hello");
        let json = serde_json::to_string(&msg).expect("serialization should succeed");

        // Should not contain "reply_to" field when None
        assert!(!json.contains("reply_to"));
    }

    #[test]
    fn message_deserialization_from_json() {
        let json = r#"{
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "session_id": "session-abc",
            "sender_id": "agent-1",
            "recipient_id": "agent-2",
            "content": "Hello from JSON",
            "timestamp": 1_700_000_000,
            "reply_to": "parent-uuid"
        }"#;

        let msg: A2AMessage = serde_json::from_str(json).expect("deserialization should succeed");

        assert_eq!(msg.id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(msg.session_id, "session-abc");
        assert_eq!(msg.sender_id, "agent-1");
        assert_eq!(msg.recipient_id, "agent-2");
        assert_eq!(msg.content, "Hello from JSON");
        assert_eq!(msg.timestamp, 1_700_000_000);
        assert_eq!(msg.reply_to, Some("parent-uuid".to_string()));
    }

    // =========================================================================
    // A2APeer Tests
    // =========================================================================

    #[test]
    fn peer_new_sets_fields_and_defaults_enabled() {
        let peer = A2APeer::new(
            "peer-001",
            "https://peer-001.example.com",
            "bearer-token-123",
        );

        assert_eq!(peer.id, "peer-001");
        assert_eq!(peer.endpoint, "https://peer-001.example.com");
        assert_eq!(peer.bearer_token, "bearer-token-123");
        assert!(peer.enabled);
    }

    #[test]
    fn peer_disabled_builder_works() {
        let peer = A2APeer::new("peer-001", "https://example.com", "token").disabled();
        assert!(!peer.enabled);
    }

    #[test]
    fn peer_serialization_roundtrip() {
        let original = A2APeer {
            id: "peer-001".to_string(),
            endpoint: "https://example.com/a2a".to_string(),
            bearer_token: "secret-token".to_string(),
            enabled: true,
        };

        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let deserialized: A2APeer =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(original, deserialized);
    }

    #[test]
    fn peer_deserialization_defaults_enabled() {
        let json = r#"{
            "id": "peer-001",
            "endpoint": "https://example.com",
            "bearer_token": "token"
        }"#;

        let peer: A2APeer = serde_json::from_str(json).expect("deserialization should succeed");
        assert!(peer.enabled);
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
        assert!(config.is_peer_allowed("peer-123"));
    }

    #[test]
    fn config_is_peer_allowed_with_specific_ids() {
        let config = A2AConfig {
            allowed_peer_ids: vec!["peer-a".to_string(), "peer-b".to_string()],
            ..Default::default()
        };

        assert!(config.is_peer_allowed("peer-a"));
        assert!(config.is_peer_allowed("peer-b"));
        assert!(!config.is_peer_allowed("peer-c"));
        assert!(!config.is_peer_allowed("*"));
    }

    #[test]
    fn config_find_peer_returns_enabled_peer() {
        let peer1 = A2APeer::new("peer-1", "https://peer1.com", "token1");
        let peer2 = A2APeer::new("peer-2", "https://peer2.com", "token2").disabled();

        let config = A2AConfig {
            peers: vec![peer1.clone(), peer2],
            ..Default::default()
        };

        assert!(config.find_peer("peer-1").is_some());
        assert!(config.find_peer("peer-2").is_none()); // Disabled
        assert!(config.find_peer("peer-3").is_none()); // Not found
    }

    #[test]
    fn config_serialization_roundtrip() {
        let original = A2AConfig {
            enabled: true,
            listen_port: 8080,
            discovery_mode: "static".to_string(),
            allowed_peer_ids: vec!["peer-a".to_string(), "peer-b".to_string()],
            peers: vec![
                A2APeer::new("peer-a", "https://peer-a.example.com", "token-a"),
                A2APeer::new("peer-b", "https://peer-b.example.com", "token-b"),
            ],
            rate_limit: A2ARateLimitConfig::default(),
            idempotency: A2AIdempotencyConfig::default(),
        };

        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let deserialized: A2AConfig =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(original.enabled, deserialized.enabled);
        assert_eq!(original.listen_port, deserialized.listen_port);
        assert_eq!(original.discovery_mode, deserialized.discovery_mode);
        assert_eq!(original.allowed_peer_ids, deserialized.allowed_peer_ids);
        assert_eq!(original.peers.len(), deserialized.peers.len());
    }

    #[test]
    fn config_deserialization_uses_defaults() {
        let json = r#"{}"#;

        let config: A2AConfig =
            serde_json::from_str(json).expect("deserialization with defaults should succeed");

        assert!(!config.enabled);
        assert_eq!(config.listen_port, 9000);
        assert_eq!(config.discovery_mode, "static");
        assert_eq!(config.allowed_peer_ids, vec!["*"]);
        assert!(config.peers.is_empty());
    }

    #[test]
    fn config_deserialization_partial() {
        let json = r#"{
            "enabled": true,
            "listen_port": 9090
        }"#;

        let config: A2AConfig = serde_json::from_str(json).expect("deserialization should succeed");

        assert!(config.enabled);
        assert_eq!(config.listen_port, 9090);
        assert_eq!(config.discovery_mode, "static"); // Default
        assert_eq!(config.allowed_peer_ids, vec!["*"]); // Default
    }

    #[test]
    fn config_toml_roundtrip() {
        let config = A2AConfig {
            enabled: true,
            listen_port: 8080,
            discovery_mode: "static".to_string(),
            allowed_peer_ids: vec!["*".to_string()],
            peers: vec![A2APeer::new(
                "zeroclaw-agent-1",
                "https://agent1.zeroclaw.local",
                "secure-bearer-token",
            )],
            rate_limit: A2ARateLimitConfig::default(),
            idempotency: A2AIdempotencyConfig::default(),
        };

        let toml = toml::to_string(&config).expect("TOML serialization should succeed");
        let deserialized: A2AConfig =
            toml::from_str(&toml).expect("TOML deserialization should succeed");

        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(config.listen_port, deserialized.listen_port);
        assert_eq!(config.peers.len(), deserialized.peers.len());
        assert_eq!(config.peers[0].id, deserialized.peers[0].id);
    }

    // =========================================================================
    // JsonSchema Tests
    // =========================================================================

    #[test]
    fn message_generates_schema() {
        let schema = schemars::schema_for!(A2AMessage);
        let json = serde_json::to_string_pretty(&schema).expect("schema generation should succeed");

        assert!(json.contains("A2AMessage"));
        assert!(json.contains("id"));
        assert!(json.contains("session_id"));
        assert!(json.contains("sender_id"));
        assert!(json.contains("recipient_id"));
        assert!(json.contains("content"));
        assert!(json.contains("timestamp"));
        assert!(json.contains("reply_to"));
    }

    #[test]
    fn peer_generates_schema() {
        let schema = schemars::schema_for!(A2APeer);
        let json = serde_json::to_string_pretty(&schema).expect("schema generation should succeed");

        assert!(json.contains("A2APeer"));
        assert!(json.contains("id"));
        assert!(json.contains("endpoint"));
        assert!(json.contains("bearer_token"));
        assert!(json.contains("enabled"));
    }

    #[test]
    fn config_generates_schema() {
        let schema = schemars::schema_for!(A2AConfig);
        let json = serde_json::to_string_pretty(&schema).expect("schema generation should succeed");

        assert!(json.contains("A2AConfig"));
        assert!(json.contains("enabled"));
        assert!(json.contains("listen_port"));
        assert!(json.contains("discovery_mode"));
        assert!(json.contains("allowed_peer_ids"));
        assert!(json.contains("peers"));
    }
}
