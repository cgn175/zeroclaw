//! A2A (Agent-to-Agent) Channel
//!
//! This module provides agent-to-agent communication capabilities for ZeroClaw,
//! enabling secure, authenticated messaging between peer agents over HTTP.
//!
//! # Overview
//!
//! The A2A channel implements a simple request/response protocol where agents
//! can exchange messages using bearer token authentication. The protocol
//! supports:
//!
//! - Session-based conversation threading
//! - Reply chains for structured dialogues
//! - Static peer configuration (deny-by-default security model)
//! - Configurable listen port and access controls
//! - Secure peer pairing with short-lived pairing codes
//!
//! # Protocol Types
//!
//! See [`protocol`] for the core message types:
//!
//! - [`A2AMessage`](protocol::A2AMessage) - Message envelope
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
pub use protocol::A2AMessage;
