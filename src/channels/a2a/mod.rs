//! A2A (Agent-to-Agent) Channel
//!
//! This module provides agent-to-agent communication capabilities for ZeroClaw,
//! enabling secure, authenticated messaging between peer agents using the
//! Google A2A protocol standard.
//!
//! # Overview
//!
//! The A2A channel implements the Google A2A protocol (https://github.com/google/A2A)
//! for interoperable agent-to-agent communication. The protocol supports:
//!
//! - Task-based workflow (create, get, stream, cancel)
//! - Agent Card discovery at /.well-known/agent.json
//! - SSE streaming for real-time task updates
//! - Bearer token authentication
//! - Static peer configuration (deny-by-default security model)
//! - Secure peer pairing with short-lived pairing codes
//!
//! # Protocol Types
//!
//! See [`protocol`] for the core message types:
//!
//! - [`AgentCard`](zeroclaw_a2a::AgentCard) - Agent discovery document
//! - [`Task`](zeroclaw_a2a::Task) - Unit of work
//! - [`TaskMessage`](zeroclaw_a2a::TaskMessage) - Messages within a task
//! - [`A2APeer`](zeroclaw_a2a::A2APeer) - Peer configuration
//! - [`A2AConfig`](zeroclaw_a2a::A2AConfig) - Channel configuration
//!
//! # Pairing
//!
//! See [`pairing`] for peer authentication and pairing:
//!
//! - [`PairingManager`](pairing::PairingManager) - Manages pending pairing codes
//! - [`initiate_peer_pairing`](zeroclaw_a2a::initiate_peer_pairing) - Client pairing flow
//! - [`exchange_pairing_code`](pairing::exchange_pairing_code) - Exchange code for token
//!
//! # Security
//!
//! - Bearer token authentication required for all requests
//! - Peer allowlist controls which agents can communicate
//! - Disabled by default (opt-in activation)
//! - Static peer discovery only (no dynamic registration)
//! - Pairing codes expire after 5 minutes and are single-use
//! - Constant-time comparison prevents timing attacks

// Re-export all A2A types from the zeroclaw-a2a crate
pub use zeroclaw_a2a::{
    channel::{A2AChannel, PeerConnection, PeerState, ReconnectConfig, calculate_backoff_delay},
    pairing::{self, PairingManager},
    protocol::{
        A2AConfig, A2AIdempotencyConfig, A2APeer, A2ARateLimitConfig, A2AReconnectConfig,
        AgentCard, AgentCardConfig, AgentCapabilities, AgentEndpoints, AuthenticationInfo,
        Artifact, ArtifactType, CancelTaskRequest, CancelTaskResponse, CreateTaskRequest,
        CreateTaskResponse, GetTaskRequest, MessageRole, Skill, SkillConfig, Task,
        TaskMessage, TaskStatus, TaskUpdate,
    },
};
