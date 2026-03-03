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

use crate::protocol::{A2AConfig, A2APeer, CreateTaskRequest};
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
    /// Sender for pushing received messages (responses from peers) into the message bus.
    /// Set when `listen()` is called.
    response_tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<ChannelMessage>>>>,
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
            response_tx: Arc::new(Mutex::new(None)),
        })
    }

    /// Get the channel name.
    pub fn name(&self) -> &str {
        "a2a"
    }

    /// Send a message to a peer.
    ///
    /// After sending, subscribes to the task's SSE stream to receive the response
    /// back as a `ChannelMessage` through the message bus.
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

        // Parse response to get task_id for SSE subscription
        let resp_body: serde_json::Value = response.json().await?;
        let task_id = resp_body
            .get("task")
            .and_then(|t| t.get("id"))
            .and_then(|id| id.as_str())
            .unwrap_or_default()
            .to_string();

        if task_id.is_empty() {
            tracing::warn!("A2A send to {} succeeded but no task_id in response", message.recipient);
            return Ok(());
        }

        tracing::info!("A2A task {} created on peer {}", task_id, message.recipient);

        // Spawn SSE subscriber if response_tx is available (listen() has been called)
        let tx_guard = self.response_tx.lock().await;
        if let Some(ref tx) = *tx_guard {
            let peer_id = peer.id.clone();
            let peer_endpoint = peer.endpoint.clone();
            let peer_token = peer.bearer_token.clone();
            let http_client = self.http_client.clone();
            let tx = tx.clone();

            tokio::spawn(async move {
                Self::subscribe_to_task_response(
                    peer_id,
                    peer_endpoint,
                    peer_token,
                    task_id,
                    http_client,
                    tx,
                )
                .await;
            });
        }

        Ok(())
    }

    /// Subscribe to a peer's task SSE stream and forward responses as ChannelMessages.
    async fn subscribe_to_task_response(
        peer_id: String,
        peer_endpoint: String,
        peer_token: String,
        task_id: String,
        http_client: reqwest::Client,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) {
        let stream_url = format!("{}/tasks/{}/stream", peer_endpoint, task_id);
        tracing::debug!("Subscribing to A2A task response: {} from peer {}", task_id, peer_id);

        let response = match http_client
            .get(&stream_url)
            .bearer_auth(&peer_token)
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(300)) // 5 min timeout for long tasks
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp,
            Ok(resp) => {
                tracing::warn!(
                    "A2A task stream returned HTTP {} for task {} from peer {}",
                    resp.status(), task_id, peer_id
                );
                return;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to connect to A2A task stream for task {} from peer {}: {}",
                    task_id, peer_id, e
                );
                return;
            }
        };

        let mut buffer = String::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let data = match chunk {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("A2A SSE stream error for task {}: {}", task_id, e);
                    break;
                }
            };

            let text = String::from_utf8_lossy(&data);
            buffer.push_str(&text);

            // Process complete SSE events (separated by double newline)
            while let Some(event_end) = buffer.find("\n\n") {
                let event_block = buffer[..event_end].to_string();
                buffer = buffer[event_end + 2..].to_string();

                for line in event_block.lines() {
                    if let Some(data_str) = line.strip_prefix("data: ") {
                        if let Ok(update) = serde_json::from_str::<crate::protocol::TaskUpdate>(data_str) {
                            // Convert TaskUpdate message to ChannelMessage
                            if let Some(ref msg) = update.message {
                                let channel_msg = ChannelMessage {
                                    id: format!("{}-{}", task_id, current_timestamp_secs()),
                                    sender: peer_id.clone(),
                                    reply_target: peer_id.clone(),
                                    content: msg.content.clone(),
                                    channel: "a2a".to_string(),
                                    timestamp: current_timestamp_secs(),
                                    thread_ts: Some(task_id.clone()),
                                };
                                if tx.send(channel_msg).await.is_err() {
                                    tracing::warn!("A2A response channel closed for task {}", task_id);
                                    return;
                                }
                            }

                            // Stop on terminal states
                            if matches!(
                                update.status,
                                crate::protocol::TaskStatus::Completed
                                    | crate::protocol::TaskStatus::Failed
                                    | crate::protocol::TaskStatus::Cancelled
                            ) {
                                tracing::debug!(
                                    "A2A task {} from peer {} finished: {:?}",
                                    task_id, peer_id, update.status
                                );
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Listen for incoming messages from peers.
    ///
    /// Stores the message sender for use by `send()` response subscribers,
    /// then runs a health-monitoring loop. Actual response messages are received
    /// via per-task SSE subscribers spawned by `send()`.
    pub async fn listen(
        &self,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        // Store tx so send() can spawn response subscribers
        {
            let mut response_tx = self.response_tx.lock().await;
            *response_tx = Some(tx.clone());
        }

        tracing::info!(
            "A2A channel listening with {} peer(s). Responses handled via per-task SSE subscribers.",
            self.peers.len()
        );

        // Keep alive — periodic health checks on peers.
        // The actual message receiving happens via:
        // 1. Gateway (handle_create_task) for incoming tasks FROM peers
        // 2. Per-task SSE subscribers spawned by send() for responses TO our tasks
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;

            if tx.is_closed() {
                tracing::info!("A2A listen channel closed, stopping");
                break;
            }

            // Periodic health check
            for peer in self.peers.values() {
                let url = format!("{}/.well-known/agent.json", peer.endpoint);
                match self.http_client.get(&url).send().await {
                    Ok(response) if response.status().is_success() => {
                        tracing::debug!("A2A peer {} healthy", peer.id);
                    }
                    Ok(response) => {
                        tracing::debug!("A2A peer {} health check: HTTP {}", peer.id, response.status());
                    }
                    Err(e) => {
                        tracing::debug!("A2A peer {} health check error: {}", peer.id, e);
                    }
                }
            }
        }

        Ok(())
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
        _tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        self.listen(_tx).await
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

    #[tokio::test]
    async fn subscribe_parses_task_update_into_channel_message() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ChannelMessage>(10);

        // Simulate what subscribe_to_task_response does: parse SSE data into ChannelMessage
        let sse_data = r#"{"task_id":"task-123","status":"running","message":{"role":"agent","content":"Here is the code review result.","timestamp":"2024-01-01T00:00:00Z"}}"#;
        let update: crate::protocol::TaskUpdate = serde_json::from_str(sse_data).unwrap();

        assert!(update.message.is_some());
        assert_eq!(update.message.as_ref().unwrap().content, "Here is the code review result.");
        assert_eq!(update.status, crate::protocol::TaskStatus::Running);

        if let Some(ref msg) = update.message {
            let channel_msg = ChannelMessage {
                id: "task-123-1234567890".to_string(),
                sender: "peer-reviewer".to_string(),
                reply_target: "peer-reviewer".to_string(),
                content: msg.content.clone(),
                channel: "a2a".to_string(),
                timestamp: 1234567890,
                thread_ts: Some("task-123".to_string()),
            };
            tx.send(channel_msg).await.unwrap();
        }

        let received = rx.recv().await.unwrap();
        assert_eq!(received.content, "Here is the code review result.");
        assert_eq!(received.sender, "peer-reviewer");
        assert_eq!(received.channel, "a2a");
        assert_eq!(received.thread_ts, Some("task-123".to_string()));
    }

    #[test]
    fn task_update_terminal_states_detected() {
        let completed: crate::protocol::TaskUpdate = serde_json::from_str(
            r#"{"task_id":"t1","status":"completed"}"#
        ).unwrap();
        assert!(matches!(completed.status, crate::protocol::TaskStatus::Completed));

        let failed: crate::protocol::TaskUpdate = serde_json::from_str(
            r#"{"task_id":"t2","status":"failed"}"#
        ).unwrap();
        assert!(matches!(failed.status, crate::protocol::TaskStatus::Failed));

        let cancelled: crate::protocol::TaskUpdate = serde_json::from_str(
            r#"{"task_id":"t3","status":"cancelled"}"#
        ).unwrap();
        assert!(matches!(cancelled.status, crate::protocol::TaskStatus::Cancelled));
    }
}
