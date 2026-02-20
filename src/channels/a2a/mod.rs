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
//! - [`AgentCard`](protocol::AgentCard) - Agent discovery document
//! - [`Task`](protocol::Task) - Unit of work
//! - [`TaskMessage`](protocol::TaskMessage) - Messages within a task
//! - [`A2APeer`](protocol::A2APeer) - Peer configuration
//! - [`A2AConfig`](protocol::A2AConfig) - Channel configuration
//!
//! # Pairing
//!
//! See [`pairing`] for peer authentication and pairing:
//!
//! - [`PairingManager`](pairing::PairingManager) - Manages pending pairing codes
//! - [`initiate_peer_pairing`](pairing::initiate_peer_pairing) - Client pairing flow
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

pub mod channel;
pub mod pairing;
pub mod processor;
pub mod protocol;

pub use channel::A2AChannel;
