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
//! See [`zeroclaw_a2a::protocol`] for the core message types:
//!
//! - [`AgentCard`](zeroclaw_a2a::AgentCard) - Agent discovery document
//! - [`Task`](zeroclaw_a2a::Task) - Unit of work
//! - [`TaskMessage`](zeroclaw_a2a::TaskMessage) - Messages within a task
//! - [`A2APeer`](zeroclaw_a2a::A2APeer) - Peer configuration
//! - [`A2AConfig`](zeroclaw_a2a::A2AConfig) - Channel configuration
//!
//! # Pairing
//!
//! See [`zeroclaw_a2a::pairing`] for peer authentication and pairing:
//!
//! - [`PairingManager`](zeroclaw_a2a::PairingManager) - Manages pending pairing codes
//! - [`initiate_peer_pairing`](zeroclaw_a2a::initiate_peer_pairing) - Client pairing flow
//!
//! # Security
//!
//! - Bearer token authentication required for all requests
//! - Peer allowlist controls which agents can communicate
//! - Disabled by default (opt-in activation)
//! - Static peer discovery only (no dynamic registration)
//! - Pairing codes expire after 5 minutes and are single-use
//! - Constant-time comparison prevents timing attacks

// Re-export commonly used A2A types from the zeroclaw-a2a crate
pub use zeroclaw_a2a::protocol::A2AConfig;

use crate::channels::traits::{Channel, ChannelMessage, SendMessage};
use async_trait::async_trait;
use std::sync::Arc;

/// Wrapper around zeroclaw_a2a::A2AChannel that implements the main crate's Channel trait.
pub struct A2AChannel {
    inner: Arc<zeroclaw_a2a::A2AChannel>,
}

impl A2AChannel {
    /// Create a new A2A channel with the given configuration.
    pub fn new(config: A2AConfig, http_client: Option<reqwest::Client>) -> anyhow::Result<Self> {
        let inner = zeroclaw_a2a::A2AChannel::new(config, http_client)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Get the channel name.
    pub fn name(&self) -> &str {
        self.inner.name()
    }

    /// Send a message to a peer.
    pub async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        // Convert SendMessage to zeroclaw_a2a SendMessage
        let a2a_msg = zeroclaw_a2a::SendMessage {
            content: message.content.clone(),
            recipient: message.recipient.clone(),
            subject: message.subject.clone(),
            thread_ts: message.thread_ts.clone(),
        };
        self.inner.send(&a2a_msg).await
    }

    /// Listen for incoming messages from peers.
    pub async fn listen(
        &self,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        // Create a channel to receive messages from the inner channel
        let (a2a_tx, mut a2a_rx) = tokio::sync::mpsc::channel::<zeroclaw_a2a::ChannelMessage>(100);

        // Spawn the inner listen task
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _ = inner.listen(a2a_tx).await;
        });

        // Convert messages and forward
        while let Some(msg) = a2a_rx.recv().await {
            let channel_msg = ChannelMessage {
                id: msg.id,
                sender: msg.sender,
                reply_target: msg.reply_target,
                content: msg.content,
                channel: msg.channel,
                timestamp: msg.timestamp,
                thread_ts: msg.thread_ts,
            };
            if tx.send(channel_msg).await.is_err() {
                break;
            }
        }

        Ok(())
    }

    /// Perform a health check on all peers.
    pub async fn health_check(&self) -> bool {
        self.inner.health_check().await
    }
}

#[async_trait]
impl Channel for A2AChannel {
    fn name(&self) -> &str {
        self.name()
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        self.send(message).await
    }

    async fn listen(
        &self,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        self.listen(tx).await
    }

    async fn health_check(&self) -> bool {
        self.health_check().await
    }
}
