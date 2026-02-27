//! A2A (Agent-to-Agent) Channel Implementation
//!
//! This module provides the channel implementation for agent-to-agent communication,
//! enabling secure, authenticated messaging between peer agents over HTTP.
//!
//! # Architecture (Google A2A Protocol)
//!
//! The A2A channel uses a task-based model:
//! - **Send**: POST to peer's `/tasks` endpoint (creates a Task)
//! - **Listen**: Connect to SSE streams from peers at `/tasks/:id/stream`
//! - **Task**: Contains messages[], artifacts[], and status (pending/running/completed/failed)
//! - **Health Check**: Ping all peers and return true if any respond
//!
//! # Reconnection Strategy
//!
//! SSE connections use exponential backoff for reconnection:
//! - Initial delay: 2s, then 4s, 8s, 16s, 32s, capped at 60s
//! - Maximum retries: 10
//! - HTTP client timeout: 30s

use crate::protocol::{A2AConfig, A2APeer, CreateTaskRequest, TaskMessage};
use crate::{Channel, ChannelMessage, SendMessage};
use async_trait::async_trait;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Reconnect configuration for SSE connections.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Initial retry delay (seconds)
    pub initial_delay_secs: u64,
    /// Maximum retry delay (seconds)
    pub max_delay_secs: u64,
    /// Maximum number of retries
    pub max_retries: u32,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_delay_secs: 2,
            max_delay_secs: 60,
            max_retries: 10,
        }
    }
}

/// Calculate delay for retry attempt (exponential backoff).
///
/// Uses 2^attempt seconds, capped at max_delay_secs.
/// Example delays: attempt 1 = 2s, 2 = 4s, 3 = 8s, 4 = 16s, 5 = 32s, 6+ = 60s
pub fn calculate_backoff_delay(attempt: u32, config: &ReconnectConfig) -> Duration {
    // Use saturating math to prevent overflow
    let delay = config
        .initial_delay_secs
        .saturating_mul(2_u64.saturating_pow(attempt.saturating_sub(1).min(5)));
    Duration::from_secs(delay.min(config.max_delay_secs))
}

/// Peer connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    /// Connected and receiving messages
    Connected,
    /// Currently attempting to connect
    Connecting,
    /// Disconnected but will retry
    Disconnected,
    /// Max retries exceeded, requires health check recovery
    Failed,
}

/// Peer connection tracking with state and retry information.
pub struct PeerConnection {
    /// The peer configuration
    pub peer: A2APeer,
    /// Current connection state
    pub state: PeerState,
    /// Unix timestamp of last successful connection (seconds since epoch)
    pub last_connected: Option<u64>,
    /// Current retry count
    pub retry_count: u32,
}

impl PeerConnection {
    /// Create a new peer connection in Disconnected state.
    pub fn new(peer: A2APeer) -> Self {
        Self {
            peer,
            state: PeerState::Disconnected,
            last_connected: None,
            retry_count: 0,
        }
    }

    /// Mark the peer as successfully connected.
    pub fn mark_connected(&mut self) {
        self.state = PeerState::Connected;
        self.last_connected = Some(current_timestamp_secs());
        self.retry_count = 0;
    }

    /// Mark the peer as disconnected (will retry).
    pub fn mark_disconnected(&mut self) {
        self.state = PeerState::Disconnected;
    }

    /// Mark the peer as failed (max retries exceeded).
    pub fn mark_failed(&mut self) {
        self.state = PeerState::Failed;
    }

    /// Mark the peer as ready for reconnection (after health check recovery).
    pub fn mark_ready_for_reconnect(&mut self) {
        self.state = PeerState::Disconnected;
        self.retry_count = 0;
    }

    /// Increment retry count and return new value.
    pub fn increment_retry(&mut self) -> u32 {
        self.retry_count += 1;
        self.retry_count
    }
}

/// Get current Unix timestamp in seconds.
fn current_timestamp_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A2A Channel for agent-to-agent communication.
///
/// Implements the [`Channel`] trait to enable secure messaging between
/// peer agents using HTTP POST for sending and SSE for receiving messages.
pub struct A2AChannel {
    config: A2AConfig,
    http_client: reqwest::Client,
    peers: HashMap<String, A2APeer>,
    reconnect_config: ReconnectConfig,
}

impl A2AChannel {
    /// Create a new A2A channel with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The A2A configuration
    /// * `http_client` - An optional HTTP client to use. If None, a default client is created.
    pub fn new(config: A2AConfig, http_client: Option<reqwest::Client>) -> anyhow::Result<Self> {
        if !config.enabled {
            anyhow::bail!("A2A channel is disabled in configuration");
        }

        let http_client = http_client.unwrap_or_else(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to build HTTP client")
        });

        let peers: HashMap<String, A2APeer> = config
            .peers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id.clone(), p.clone()))
            .collect();

        let reconnect_config = ReconnectConfig {
            initial_delay_secs: config.reconnect.initial_delay_secs,
            max_delay_secs: config.reconnect.max_delay_secs,
            max_retries: config.reconnect.max_retries,
        };

        Ok(Self {
            config,
            http_client,
            peers,
            reconnect_config,
        })
    }

    /// Get the channel name.
    pub fn name(&self) -> &str {
        "a2a"
    }

    /// Send a message to a peer.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to send
    pub async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let peer = self
            .peers
            .get(&message.recipient)
            .ok_or_else(|| anyhow::anyhow!("Unknown peer: {}", message.recipient))?;

        let request = CreateTaskRequest::new(&message.content);

        let response = self
            .http_client
            .post(format!("{}/tasks", peer.endpoint))
            .bearer_auth(&peer.bearer_token)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to send message: HTTP {} - {}", status, text);
        }

        Ok(())
    }

    /// Listen for incoming messages from peers.
    ///
    /// This connects to SSE streams from all configured peers.
    pub async fn listen(
        &self,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        let peer_connections: Arc<Mutex<HashMap<String, PeerConnection>>> = Arc::new(Mutex::new(
            self.peers
                .values()
                .map(|p| (p.id.clone(), PeerConnection::new(p.clone())))
                .collect(),
        ));

        loop {
            for peer_id in self.peers.keys() {
                let mut connections = peer_connections.lock().await;
                let connection = connections.get_mut(peer_id).unwrap();

                match connection.state {
                    PeerState::Connected | PeerState::Connecting => continue,
                    PeerState::Failed => {
                        // Try recovery after health check interval
                        if let Some(last_connected) = connection.last_connected {
                            let elapsed = current_timestamp_secs() - last_connected;
                            if elapsed < self.config.reconnect.health_check_interval_secs {
                                continue;
                            }
                        }
                        connection.mark_ready_for_reconnect();
                    }
                    PeerState::Disconnected => {
                        connection.state = PeerState::Connecting;
                    }
                }

                let peer = connection.peer.clone();
                let reconnect_config = self.reconnect_config.clone();
                let http_client = self.http_client.clone();
                let tx = tx.clone();
                let connections = peer_connections.clone();

                tokio::spawn(async move {
                    if let Err(e) =
                        Self::connect_to_peer_stream(peer, reconnect_config, http_client, tx, connections).await
                    {
                        tracing::warn!("A2A peer stream error: {}", e);
                    }
                });
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Connect to a peer's SSE stream.
    async fn connect_to_peer_stream(
        peer: A2APeer,
        reconnect_config: ReconnectConfig,
        http_client: reqwest::Client,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        connections: Arc<Mutex<HashMap<String, PeerConnection>>>,
    ) -> anyhow::Result<()> {
        let stream_url = format!("{}/tasks/stream", peer.endpoint);

        loop {
            let mut connection = connections.lock().await;
            let conn = connection.get_mut(&peer.id).unwrap();

            if conn.retry_count >= reconnect_config.max_retries {
                conn.mark_failed();
                anyhow::bail!("Max retries exceeded for peer {}", peer.id);
            }

            let attempt = conn.increment_retry();
            drop(connection);

            let delay = calculate_backoff_delay(attempt, &reconnect_config);
            tokio::time::sleep(delay).await;

            match http_client
                .get(&stream_url)
                .bearer_auth(&peer.bearer_token)
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        let mut connection = connections.lock().await;
                        connection.get_mut(&peer.id).unwrap().mark_connected();
                        drop(connection);

                        let mut stream = response.bytes_stream();
                        while let Some(chunk) = stream.next().await {
                            if let Ok(_data) = chunk {
                                // TODO: Parse SSE events and send ChannelMessage
                                // For now, just keep connection alive
                            }
                        }

                        let mut connection = connections.lock().await;
                        connection.get_mut(&peer.id).unwrap().mark_disconnected();
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "A2A stream connection failed for peer {}: {}",
                        peer.id,
                        e
                    );
                }
            }
        }
    }

    /// Perform a health check on all peers.
    ///
    /// Returns true if at least one peer responds.
    pub async fn health_check(&self) -> bool {
        let mut any_healthy = false;

        for peer in self.peers.values() {
            let url = format!("{}/.well-known/agent.json", peer.endpoint);
            match self.http_client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    any_healthy = true;
                }
                Ok(response) => {
                    tracing::debug!(
                        "A2A peer {} health check failed: HTTP {}",
                        peer.id,
                        response.status()
                    );
                }
                Err(e) => {
                    tracing::debug!("A2A peer {} health check error: {}", peer.id, e);
                }
            }
        }

        any_healthy
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_config_default_values() {
        let config = ReconnectConfig::default();
        assert_eq!(config.initial_delay_secs, 2);
        assert_eq!(config.max_delay_secs, 60);
        assert_eq!(config.max_retries, 10);
    }

    #[test]
    fn calculate_backoff_delay_scales_correctly() {
        let config = ReconnectConfig::default();

        // Attempt 1: 2^0 * 2 = 2s
        assert_eq!(calculate_backoff_delay(1, &config).as_secs(), 2);

        // Attempt 2: 2^1 * 2 = 4s
        assert_eq!(calculate_backoff_delay(2, &config).as_secs(), 4);

        // Attempt 5: 2^4 * 2 = 32s
        assert_eq!(calculate_backoff_delay(5, &config).as_secs(), 32);

        // Attempt 6: capped at max_delay_secs (60s)
        assert_eq!(calculate_backoff_delay(6, &config).as_secs(), 60);

        // Attempt 10: still capped at 60s
        assert_eq!(calculate_backoff_delay(10, &config).as_secs(), 60);
    }

    #[test]
    fn peer_connection_lifecycle() {
        let peer = A2APeer::new("peer-1", "https://example.com", "token");
        let mut conn = PeerConnection::new(peer);

        assert_eq!(conn.state, PeerState::Disconnected);
        assert_eq!(conn.retry_count, 0);

        conn.mark_connected();
        assert_eq!(conn.state, PeerState::Connected);
        assert_eq!(conn.retry_count, 0);
        assert!(conn.last_connected.is_some());

        conn.mark_disconnected();
        assert_eq!(conn.state, PeerState::Disconnected);

        assert_eq!(conn.increment_retry(), 1);
        assert_eq!(conn.increment_retry(), 2);

        conn.mark_failed();
        assert_eq!(conn.state, PeerState::Failed);

        conn.mark_ready_for_reconnect();
        assert_eq!(conn.state, PeerState::Disconnected);
        assert_eq!(conn.retry_count, 0);
    }
}
