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

use super::protocol::{A2AConfig, A2AMessage, A2APeer, CreateTaskRequest};
use crate::channels::traits::{Channel, ChannelMessage, SendMessage};
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
        .saturating_mul(2_u64.saturating_pow(attempt.min(5)));
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
    /// * `config` - The A2A channel configuration containing peer definitions
    ///
    /// # Example
    ///
    /// ```
    /// use zeroclaw::channels::a2a::{A2AChannel, A2AConfig, A2APeer};
    ///
    /// let config = A2AConfig {
    ///     enabled: true,
    ///     listen_port: 9000,
    ///     discovery_mode: "static".to_string(),
    ///     allowed_peer_ids: vec!["*".to_string()],
    ///     peers: vec![
    ///         A2APeer::new("peer-1", "https://peer1.example.com", "token123"),
    ///     ],
    /// };
    ///
    /// let channel = A2AChannel::new(config);
    /// ```
    pub fn new(config: A2AConfig) -> Self {
        let http_client = crate::config::build_runtime_proxy_client_with_timeouts(
            "channel.a2a",
            30, // 30s timeout
            10, // 10s connect timeout
        );

        let peers: HashMap<String, A2APeer> = config
            .peers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id.clone(), p.clone()))
            .collect();

        Self {
            config,
            http_client,
            peers,
            reconnect_config: ReconnectConfig::default(),
        }
    }

    /// Create a new A2A channel with custom reconnect configuration.
    pub fn with_reconnect_config(config: A2AConfig, reconnect_config: ReconnectConfig) -> Self {
        let http_client = crate::config::build_runtime_proxy_client_with_timeouts(
            "channel.a2a",
            30, // 30s timeout
            10, // 10s connect timeout
        );

        let peers: HashMap<String, A2APeer> = config
            .peers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id.clone(), p.clone()))
            .collect();

        Self {
            config,
            http_client,
            peers,
            reconnect_config,
        }
    }

    /// Resolve a recipient identifier to a peer endpoint.
    ///
    /// Returns `Some(peer)` if the recipient matches a configured and enabled peer
    /// that is also in the allowlist.
    fn resolve_peer(&self, recipient: &str) -> Option<&A2APeer> {
        let peer = self.peers.get(recipient)?;
        if !self.config.is_peer_allowed(&peer.id) {
            return None;
        }
        Some(peer)
    }

    /// Convert an A2A message to a channel message.
    fn a2a_to_channel_message(msg: &A2AMessage) -> ChannelMessage {
        ChannelMessage {
            id: format!("a2a_{}", msg.id),
            sender: msg.sender_id.clone(),
            reply_target: msg.sender_id.clone(),
            content: msg.content.clone(),
            channel: "a2a".to_string(),
            timestamp: msg.timestamp,
            thread_ts: Some(msg.session_id.clone()),
        }
    }

    /// Build the full URL for a peer's tasks endpoint (Google A2A).
    fn peer_tasks_url(peer: &A2APeer) -> String {
        let base = peer.endpoint.trim_end_matches('/');
        format!("{}/tasks", base)
    }

    /// Build the full URL for a peer's task stream endpoint (Google A2A).
    fn peer_task_stream_url(peer: &A2APeer, task_id: &str) -> String {
        let base = peer.endpoint.trim_end_matches('/');
        format!("{}/tasks/{}/stream", base, task_id)
    }

    /// Legacy: Build the full URL for a peer's send endpoint.
    #[deprecated(note = "Use peer_tasks_url instead")]
    fn peer_send_url(peer: &A2APeer) -> String {
        Self::peer_tasks_url(peer)
    }

    /// Legacy: Build the full URL for a peer's SSE endpoint.
    #[deprecated(note = "Use peer_task_stream_url instead")]
    fn peer_sse_url(peer: &A2APeer) -> String {
        let base = peer.endpoint.trim_end_matches('/');
        format!("{}/a2a/events", base)
    }

    /// Calculate exponential backoff delay for reconnection.
    ///
    /// Returns delay in milliseconds: 2s, 4s, 8s, 16s, 32s, capped at 60s.
    fn backoff_delay_ms(retry_count: u32) -> u64 {
        let delay = if retry_count >= 6 {
            60_000 // Cap at 60s
        } else {
            2u64.saturating_pow(retry_count + 1).saturating_mul(1000)
        };
        delay
    }

    /// Perform health check on a single peer.
    async fn check_peer_health(peer: &A2APeer, client: &reqwest::Client) -> bool {
        let url = format!("{}/health", peer.endpoint.trim_end_matches('/'));
        match client
            .get(&url)
            .header("Authorization", format!("Bearer {}", peer.bearer_token))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// Background task to recover failed peers via periodic health checks.
    async fn health_check_recovery(
        peer_connections: Arc<Mutex<Vec<PeerConnection>>>,
        http_client: reqwest::Client,
        interval: Duration,
    ) {
        loop {
            tokio::time::sleep(interval).await;

            let mut connections = peer_connections.lock().await;
            for conn in connections.iter_mut() {
                if conn.state == PeerState::Failed {
                    tracing::debug!(
                        "A2A: Attempting health check recovery for peer {}",
                        conn.peer.id
                    );
                    // Try a health check
                    if Self::check_peer_health(&conn.peer, &http_client).await {
                        tracing::info!(
                            "A2A: Peer {} recovered via health check, ready for reconnect",
                            conn.peer.id
                        );
                        conn.mark_ready_for_reconnect();
                    }
                }
            }
            // Mutex guard dropped here
        }
    }
}

#[async_trait]
impl Channel for A2AChannel {
    fn name(&self) -> &str {
        "a2a"
    }

    /// Send a message to a peer agent.
    ///
    /// # Process
    ///
    /// 1. Resolve recipient to peer endpoint from config
    /// 2. Build TaskMessage (Google A2A protocol)
    /// 3. POST to peer's `/tasks` endpoint
    /// 4. Handle response
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The recipient is not a configured/allowed peer
    /// - The HTTP request fails
    /// - The peer returns a non-success status
    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        // Step 1: Resolve recipient to peer endpoint
        let peer = self
            .resolve_peer(&message.recipient)
            .ok_or_else(|| anyhow::anyhow!("Unknown or disallowed peer: {}", message.recipient))?;

        // Step 2: Create task request with user message (Google A2A protocol)
        let task_request = CreateTaskRequest::new(&message.content);

        // Step 3: POST to peer's /tasks endpoint
        let url = Self::peer_tasks_url(peer);
        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", peer.bearer_token))
            .header("Content-Type", "application/json")
            .json(&task_request)
            .send()
            .await?;

        // Step 4: Handle response
        let status = response.status();
        if status == reqwest::StatusCode::ACCEPTED || status.is_success() {
            tracing::debug!("A2A task accepted by peer: {}", peer.id);
            Ok(())
        } else {
            let body = response
                .text()
                .await
                .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));
            anyhow::bail!("A2A task send failed ({}): {}", status, body);
        }
    }

    /// Listen for incoming messages via SSE from all enabled peers.
    ///
    /// # Process
    ///
    /// 1. Connect SSE streams to all enabled peers
    /// 2. Parse incoming A2AMessage events
    /// 3. Convert to ChannelMessage
    /// 4. Send to tx channel
    ///
    /// # Reconnection
    ///
    /// Uses exponential backoff (2s, 4s, 8s, 16s, 32s, capped at 60s)
    /// with a maximum of 10 retries per peer connection. Failed peers
    /// are periodically recovered via health check background task.
    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        let peers: Vec<A2APeer> = self
            .peers
            .values()
            .filter(|p| p.enabled && self.config.is_peer_allowed(&p.id))
            .cloned()
            .collect();

        if peers.is_empty() {
            tracing::warn!("A2A: No enabled peers to listen to");
            // Sleep indefinitely since there's nothing to listen to
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }

        tracing::info!("A2A: Starting SSE listeners for {} peer(s)", peers.len());

        // Create shared peer connection state
        let peer_connections: Arc<Mutex<Vec<PeerConnection>>> = Arc::new(Mutex::new(
            peers
                .iter()
                .map(|p| PeerConnection::new(p.clone()))
                .collect(),
        ));

        // Spawn health check recovery background task
        let health_check_client = self.http_client.clone();
        let health_check_connections = Arc::clone(&peer_connections);
        let health_check_handle = tokio::spawn(async move {
            Self::health_check_recovery(
                health_check_connections,
                health_check_client,
                Duration::from_secs(30), // Check every 30 seconds
            )
            .await;
        });

        // Spawn a task for each peer to listen to its SSE stream
        let mut handles = Vec::new();
        let reconnect_config = self.reconnect_config.clone();

        // Clone peers to avoid lifetime issues with tokio::spawn
        let peers_owned = peers.clone();

        for (index, peer) in peers_owned.into_iter().enumerate() {
            let http_client = self.http_client.clone();
            let tx = tx.clone();
            let peer_id = peer.id.clone();
            let peer_connections = Arc::clone(&peer_connections);
            let config = reconnect_config.clone();

            let handle = tokio::spawn(async move {
                loop {
                    // Check if peer is in Failed state (wait for health check recovery)
                    {
                        let connections = peer_connections.lock().await;
                        if connections[index].state == PeerState::Failed {
                            tracing::debug!(
                                "A2A: Peer {} is in Failed state, waiting for health check recovery",
                                peer_id
                            );
                            drop(connections); // Release lock before sleeping
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                    }

                    // Mark as connecting
                    {
                        let mut connections = peer_connections.lock().await;
                        connections[index].state = PeerState::Connecting;
                    }

                    let url = Self::peer_sse_url(&peer);
                    tracing::debug!("A2A: Connecting to peer {} SSE stream at {}", peer_id, url);

                    match http_client
                        .get(&url)
                        .header("Authorization", format!("Bearer {}", peer.bearer_token))
                        .header("Accept", "text/event-stream")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            if !response.status().is_success() {
                                tracing::warn!(
                                    "A2A: Peer {} returned status {} for SSE connection",
                                    peer_id,
                                    response.status()
                                );

                                // Increment retry count
                                let mut connections = peer_connections.lock().await;
                                let retry_count = connections[index].increment_retry();

                                if retry_count > config.max_retries {
                                    tracing::error!(
                                        "A2A: Max retries exceeded for peer {}, marking as Failed",
                                        peer_id
                                    );
                                    connections[index].mark_failed();
                                    continue; // Continue to wait for health check recovery
                                }

                                let delay = calculate_backoff_delay(retry_count, &config);
                                drop(connections); // Release lock before sleeping
                                tracing::debug!(
                                    "A2A: Reconnecting to peer {} in {:?} (retry {})",
                                    peer_id,
                                    delay,
                                    retry_count
                                );
                                tokio::time::sleep(delay).await;
                                continue;
                            }

                            // Connection successful - mark as connected and reset retry count
                            {
                                let mut connections = peer_connections.lock().await;
                                connections[index].mark_connected();
                                tracing::info!("A2A: Successfully connected to peer {}", peer_id);
                            }

                            // Process the SSE stream
                            let mut stream = response.bytes_stream();
                            let mut buffer = String::new();
                            let mut stream_ended_gracefully = true;

                            while let Some(chunk_result) = stream.next().await {
                                match chunk_result {
                                    Ok(chunk) => {
                                        // Convert bytes to string and process
                                        match std::str::from_utf8(&chunk) {
                                            Ok(text) => {
                                                buffer.push_str(text);

                                                // Process complete SSE events (separated by double newline)
                                                while let Some(pos) = buffer.find("\n\n") {
                                                    let event = buffer[..pos].to_string();
                                                    buffer = buffer[pos + 2..].to_string();

                                                    // Parse the SSE event
                                                    if let Some(data_line) = event
                                                        .lines()
                                                        .find(|l| l.starts_with("data:"))
                                                    {
                                                        let data = &data_line[5..].trim();

                                                        // Try to parse as A2AMessage
                                                        match serde_json::from_str::<A2AMessage>(
                                                            data,
                                                        ) {
                                                            Ok(a2a_msg) => {
                                                                // Validate peer is allowed
                                                                let channel_msg =
                                                                    Self::a2a_to_channel_message(
                                                                        &a2a_msg,
                                                                    );
                                                                if tx
                                                                    .send(channel_msg)
                                                                    .await
                                                                    .is_err()
                                                                {
                                                                    tracing::debug!("A2A: Channel closed, stopping listener for peer {}", peer_id);
                                                                    return;
                                                                }
                                                            }
                                                            Err(e) => {
                                                                tracing::warn!("A2A: Failed to parse message from peer {}: {}", peer_id, e);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "A2A: Invalid UTF-8 from peer {}: {}",
                                                    peer_id,
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "A2A: Stream error from peer {}: {}",
                                            peer_id,
                                            e
                                        );
                                        stream_ended_gracefully = false;
                                        break; // Break to trigger reconnection
                                    }
                                }
                            }

                            if stream_ended_gracefully {
                                tracing::info!(
                                    "A2A: SSE stream ended gracefully for peer {}, reconnecting...",
                                    peer_id
                                );
                            } else {
                                tracing::warn!(
                                    "A2A: SSE stream error for peer {}, reconnecting...",
                                    peer_id
                                );
                            }

                            // Mark as disconnected before retry
                            {
                                let mut connections = peer_connections.lock().await;
                                connections[index].mark_disconnected();
                            }
                        }
                        Err(e) => {
                            tracing::warn!("A2A: Failed to connect to peer {}: {}", peer_id, e);

                            // Increment retry count
                            let mut connections = peer_connections.lock().await;
                            let retry_count = connections[index].increment_retry();

                            if retry_count > config.max_retries {
                                tracing::error!(
                                    "A2A: Max retries exceeded for peer {}, marking as Failed",
                                    peer_id
                                );
                                connections[index].mark_failed();
                                continue; // Continue to wait for health check recovery
                            }

                            let delay = calculate_backoff_delay(retry_count, &config);
                            drop(connections); // Release lock before sleeping
                            tracing::debug!(
                                "A2A: Reconnecting to peer {} in {:?} (retry {})",
                                peer_id,
                                delay,
                                retry_count
                            );
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            });

            handles.push(handle);
        }

        // Wait for all listeners (they run indefinitely unless they hit max retries)
        // The health check recovery task runs indefinitely
        tokio::select! {
            _ = async {
                for handle in handles {
                    let _ = handle.await;
                }
            } => {},
            _ = health_check_handle => {},
        }

        Ok(())
    }

    /// Check if the channel is healthy by pinging all peers.
    ///
    /// Returns `true` if at least one peer responds successfully.
    /// This follows a "best effort" approach where partial connectivity
    /// is considered healthy.
    async fn health_check(&self) -> bool {
        let peers: Vec<&A2APeer> = self.peers.values().filter(|p| p.enabled).collect();

        if peers.is_empty() {
            // No peers configured - consider healthy (nothing to do)
            return true;
        }

        let mut any_healthy = false;

        for peer in peers {
            let url = format!("{}/health", peer.endpoint.trim_end_matches('/'));
            match self
                .http_client
                .get(&url)
                .header("Authorization", format!("Bearer {}", peer.bearer_token))
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        any_healthy = true;
                        tracing::debug!("A2A: Peer {} is healthy", peer.id);
                    } else {
                        tracing::debug!(
                            "A2A: Peer {} returned status {}",
                            peer.id,
                            response.status()
                        );
                    }
                }
                Err(e) => {
                    tracing::debug!("A2A: Peer {} health check failed: {}", peer.id, e);
                }
            }
        }

        any_healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Peer Resolution Tests
    // =========================================================================

    #[test]
    fn a2a_channel_name() {
        let config = A2AConfig::default();
        let channel = A2AChannel::new(config);
        assert_eq!(channel.name(), "a2a");
    }

    #[test]
    fn resolve_peer_returns_configured_peer() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let config = A2AConfig {
            enabled: true,
            peers: vec![peer.clone()],
            ..Default::default()
        };
        let channel = A2AChannel::new(config);

        let resolved = channel.resolve_peer("peer-1");
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().id, "peer-1");
    }

    #[test]
    fn resolve_peer_returns_none_for_unknown_peer() {
        let config = A2AConfig::default();
        let channel = A2AChannel::new(config);

        assert!(channel.resolve_peer("unknown-peer").is_none());
    }

    #[test]
    fn resolve_peer_returns_none_for_disabled_peer() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123").disabled();
        let config = A2AConfig {
            peers: vec![peer],
            ..Default::default()
        };
        let channel = A2AChannel::new(config);

        assert!(channel.resolve_peer("peer-1").is_none());
    }

    #[test]
    fn resolve_peer_respects_allowlist() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let config = A2AConfig {
            peers: vec![peer],
            allowed_peer_ids: vec!["peer-2".to_string()], // peer-1 not allowed
            ..Default::default()
        };
        let channel = A2AChannel::new(config);

        assert!(channel.resolve_peer("peer-1").is_none());
    }

    #[test]
    fn resolve_peer_accepts_wildcard_allowlist() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let config = A2AConfig {
            peers: vec![peer],
            allowed_peer_ids: vec!["*".to_string()],
            ..Default::default()
        };
        let channel = A2AChannel::new(config);

        assert!(channel.resolve_peer("peer-1").is_some());
    }

    // =========================================================================
    // Message Conversion Tests
    // =========================================================================

    #[test]
    fn a2a_to_channel_message_conversion() {
        let a2a_msg = A2AMessage::new(
            "session-123",
            "peer-a",
            "peer-b",
            "Hello, this is a test message",
        );

        let channel_msg = A2AChannel::a2a_to_channel_message(&a2a_msg);

        assert_eq!(channel_msg.id, format!("a2a_{}", a2a_msg.id));
        assert_eq!(channel_msg.sender, "peer-a");
        assert_eq!(channel_msg.reply_target, "peer-a");
        assert_eq!(channel_msg.content, "Hello, this is a test message");
        assert_eq!(channel_msg.channel, "a2a");
        assert_eq!(channel_msg.timestamp, a2a_msg.timestamp);
        assert_eq!(channel_msg.thread_ts, Some("session-123".to_string()));
    }

    #[test]
    fn a2a_to_channel_message_preserves_reply_chain() {
        let parent = A2AMessage::new("session-456", "peer-x", "peer-y", "Original");
        let reply = A2AMessage::reply_to(&parent, "peer-y", "Reply content");

        let channel_msg = A2AChannel::a2a_to_channel_message(&reply);

        assert_eq!(channel_msg.sender, "peer-y");
        assert_eq!(channel_msg.reply_target, "peer-y");
        assert_eq!(channel_msg.thread_ts, Some("session-456".to_string()));
    }

    // =========================================================================
    // URL Construction Tests
    // =========================================================================

    #[test]
    fn peer_send_url_construction() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        assert_eq!(
            A2AChannel::peer_send_url(&peer),
            "https://peer1.example.com/a2a/send"
        );

        // Test with trailing slash
        let peer_with_slash = A2APeer::new("peer-2", "https://peer2.example.com/", "token123");
        assert_eq!(
            A2AChannel::peer_send_url(&peer_with_slash),
            "https://peer2.example.com/a2a/send"
        );
    }

    #[test]
    fn peer_sse_url_construction() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        assert_eq!(
            A2AChannel::peer_sse_url(&peer),
            "https://peer1.example.com/a2a/events"
        );
    }

    // =========================================================================
    // Backoff Delay Tests
    // =========================================================================

    #[test]
    fn backoff_delay_exponential_pattern() {
        // Exponential backoff: 2s, 4s, 8s, 16s, 32s
        assert_eq!(A2AChannel::backoff_delay_ms(0), 2_000);
        assert_eq!(A2AChannel::backoff_delay_ms(1), 4_000);
        assert_eq!(A2AChannel::backoff_delay_ms(2), 8_000);
        assert_eq!(A2AChannel::backoff_delay_ms(3), 16_000);
        assert_eq!(A2AChannel::backoff_delay_ms(4), 32_000);
    }

    #[test]
    fn backoff_delay_capped_at_60s() {
        // After 32s, should cap at 60s
        assert_eq!(A2AChannel::backoff_delay_ms(5), 60_000);
        assert_eq!(A2AChannel::backoff_delay_ms(6), 60_000);
        assert_eq!(A2AChannel::backoff_delay_ms(10), 60_000);
        assert_eq!(A2AChannel::backoff_delay_ms(100), 60_000);
    }

    // =========================================================================
    // Channel Construction Tests
    // =========================================================================

    #[test]
    fn new_channel_filters_disabled_peers() {
        let peer1 = A2APeer::new("peer-1", "https://peer1.example.com", "token1");
        let peer2 = A2APeer::new("peer-2", "https://peer2.example.com", "token2").disabled();

        let config = A2AConfig {
            peers: vec![peer1, peer2],
            ..Default::default()
        };
        let channel = A2AChannel::new(config);

        assert!(channel.peers.contains_key("peer-1"));
        assert!(!channel.peers.contains_key("peer-2"));
    }

    #[test]
    fn new_channel_with_empty_config() {
        let config = A2AConfig::default();
        let channel = A2AChannel::new(config);

        assert!(channel.peers.is_empty());
        assert!(!channel.config.enabled);
    }

    // =========================================================================
    // ReconnectConfig Tests
    // =========================================================================

    #[test]
    fn reconnect_config_default_values() {
        let config = ReconnectConfig::default();
        assert_eq!(config.initial_delay_secs, 2);
        assert_eq!(config.max_delay_secs, 60);
        assert_eq!(config.max_retries, 10);
    }

    #[test]
    fn reconnect_config_custom_values() {
        let config = ReconnectConfig {
            initial_delay_secs: 5,
            max_delay_secs: 120,
            max_retries: 20,
        };
        assert_eq!(config.initial_delay_secs, 5);
        assert_eq!(config.max_delay_secs, 120);
        assert_eq!(config.max_retries, 20);
    }

    // =========================================================================
    // Exponential Backoff Tests
    // =========================================================================

    #[test]
    fn calculate_backoff_delay_exponential_pattern() {
        let config = ReconnectConfig::default();

        // Exponential backoff: 2s, 4s, 8s, 16s, 32s
        assert_eq!(calculate_backoff_delay(1, &config), Duration::from_secs(2));
        assert_eq!(calculate_backoff_delay(2, &config), Duration::from_secs(4));
        assert_eq!(calculate_backoff_delay(3, &config), Duration::from_secs(8));
        assert_eq!(calculate_backoff_delay(4, &config), Duration::from_secs(16));
        assert_eq!(calculate_backoff_delay(5, &config), Duration::from_secs(32));
    }

    #[test]
    fn calculate_backoff_delay_capped_at_max() {
        let config = ReconnectConfig::default();

        // After 32s, should cap at 60s (max_delay_secs)
        assert_eq!(calculate_backoff_delay(6, &config), Duration::from_secs(60));
        assert_eq!(
            calculate_backoff_delay(10, &config),
            Duration::from_secs(60)
        );
        assert_eq!(
            calculate_backoff_delay(100, &config),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn calculate_backoff_delay_with_custom_config() {
        let config = ReconnectConfig {
            initial_delay_secs: 5,
            max_delay_secs: 100,
            max_retries: 10,
        };

        // 5s, 10s, 20s, 40s, 80s, capped at 100s
        assert_eq!(calculate_backoff_delay(1, &config), Duration::from_secs(5));
        assert_eq!(calculate_backoff_delay(2, &config), Duration::from_secs(10));
        assert_eq!(calculate_backoff_delay(3, &config), Duration::from_secs(20));
        assert_eq!(calculate_backoff_delay(4, &config), Duration::from_secs(40));
        assert_eq!(calculate_backoff_delay(5, &config), Duration::from_secs(80));
        assert_eq!(
            calculate_backoff_delay(6, &config),
            Duration::from_secs(100)
        );
    }

    #[test]
    fn calculate_backoff_delay_attempt_zero() {
        let config = ReconnectConfig::default();
        // Attempt 0 should use initial delay (2^0 = 1, so 2 * 1 = 2s)
        assert_eq!(calculate_backoff_delay(0, &config), Duration::from_secs(2));
    }

    // =========================================================================
    // PeerState Tests
    // =========================================================================

    #[test]
    fn peer_state_variants() {
        // Test that all states can be created and compared
        assert_eq!(PeerState::Connected, PeerState::Connected);
        assert_eq!(PeerState::Connecting, PeerState::Connecting);
        assert_eq!(PeerState::Disconnected, PeerState::Disconnected);
        assert_eq!(PeerState::Failed, PeerState::Failed);

        assert_ne!(PeerState::Connected, PeerState::Disconnected);
        assert_ne!(PeerState::Failed, PeerState::Connecting);
    }

    // =========================================================================
    // PeerConnection Tests
    // =========================================================================

    #[test]
    fn peer_connection_new() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let conn = PeerConnection::new(peer.clone());

        assert_eq!(conn.peer.id, "peer-1");
        assert_eq!(conn.state, PeerState::Disconnected);
        assert!(conn.last_connected.is_none());
        assert_eq!(conn.retry_count, 0);
    }

    #[test]
    fn peer_connection_mark_connected() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let mut conn = PeerConnection::new(peer);

        conn.mark_connected();

        assert_eq!(conn.state, PeerState::Connected);
        assert!(conn.last_connected.is_some());
        assert_eq!(conn.retry_count, 0);
    }

    #[test]
    fn peer_connection_mark_disconnected() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let mut conn = PeerConnection::new(peer);

        conn.mark_connected();
        conn.mark_disconnected();

        assert_eq!(conn.state, PeerState::Disconnected);
        // last_connected should be preserved from connected state
        assert!(conn.last_connected.is_some());
    }

    #[test]
    fn peer_connection_mark_failed() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let mut conn = PeerConnection::new(peer);

        conn.mark_failed();

        assert_eq!(conn.state, PeerState::Failed);
    }

    #[test]
    fn peer_connection_mark_ready_for_reconnect() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let mut conn = PeerConnection::new(peer);

        conn.retry_count = 15;
        conn.mark_failed();
        conn.mark_ready_for_reconnect();

        assert_eq!(conn.state, PeerState::Disconnected);
        assert_eq!(conn.retry_count, 0);
    }

    #[test]
    fn peer_connection_increment_retry() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let mut conn = PeerConnection::new(peer);

        assert_eq!(conn.increment_retry(), 1);
        assert_eq!(conn.increment_retry(), 2);
        assert_eq!(conn.increment_retry(), 3);
        assert_eq!(conn.retry_count, 3);
    }

    #[test]
    fn peer_connection_full_lifecycle() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let mut conn = PeerConnection::new(peer);

        // Initial state
        assert_eq!(conn.state, PeerState::Disconnected);
        assert_eq!(conn.retry_count, 0);

        // Connecting
        conn.state = PeerState::Connecting;
        assert_eq!(conn.state, PeerState::Connecting);

        // Connected
        conn.mark_connected();
        assert_eq!(conn.state, PeerState::Connected);
        assert_eq!(conn.retry_count, 0);

        // Disconnected (connection lost)
        conn.mark_disconnected();
        assert_eq!(conn.state, PeerState::Disconnected);

        // Retry attempts
        conn.increment_retry();
        conn.increment_retry();
        assert_eq!(conn.retry_count, 2);

        // Failed (max retries exceeded)
        conn.mark_failed();
        assert_eq!(conn.state, PeerState::Failed);

        // Recovery
        conn.mark_ready_for_reconnect();
        assert_eq!(conn.state, PeerState::Disconnected);
        assert_eq!(conn.retry_count, 0);

        // Reconnecting
        conn.state = PeerState::Connecting;
        conn.mark_connected();
        assert_eq!(conn.state, PeerState::Connected);
    }

    // =========================================================================
    // Channel with Custom ReconnectConfig Tests
    // =========================================================================

    #[test]
    fn channel_with_custom_reconnect_config() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let config = A2AConfig {
            peers: vec![peer],
            ..Default::default()
        };

        let custom_reconnect = ReconnectConfig {
            initial_delay_secs: 5,
            max_delay_secs: 120,
            max_retries: 20,
        };

        let channel = A2AChannel::with_reconnect_config(config, custom_reconnect);

        assert_eq!(channel.reconnect_config.initial_delay_secs, 5);
        assert_eq!(channel.reconnect_config.max_delay_secs, 120);
        assert_eq!(channel.reconnect_config.max_retries, 20);
    }

    #[test]
    fn channel_default_reconnect_config() {
        let config = A2AConfig::default();
        let channel = A2AChannel::new(config);

        assert_eq!(channel.reconnect_config.initial_delay_secs, 2);
        assert_eq!(channel.reconnect_config.max_delay_secs, 60);
        assert_eq!(channel.reconnect_config.max_retries, 10);
    }

    // =========================================================================
    // Graceful Degradation Tests
    // =========================================================================

    #[test]
    fn multiple_peers_independent_states() {
        let peer1 = A2APeer::new("peer-1", "https://peer1.example.com", "token1");
        let peer2 = A2APeer::new("peer-2", "https://peer2.example.com", "token2");
        let peer3 = A2APeer::new("peer-3", "https://peer3.example.com", "token3");

        let mut conn1 = PeerConnection::new(peer1);
        let mut conn2 = PeerConnection::new(peer2);
        let conn3 = PeerConnection::new(peer3);

        // Simulate different states
        conn1.mark_connected(); // peer-1 is connected
        conn2.mark_failed(); // peer-2 has failed
                             // peer-3 is disconnected (will retry)

        assert_eq!(conn1.state, PeerState::Connected);
        assert_eq!(conn2.state, PeerState::Failed);
        assert_eq!(conn3.state, PeerState::Disconnected);

        // Verify graceful degradation: one peer failing doesn't affect others
        assert_eq!(conn1.retry_count, 0);
        assert_eq!(conn2.retry_count, 0);
        assert_eq!(conn3.retry_count, 0);
    }

    // =========================================================================
    // Max Retry Limit Tests
    // =========================================================================

    #[test]
    fn max_retry_limit_enforcement() {
        let config = ReconnectConfig::default();
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let mut conn = PeerConnection::new(peer);

        // Simulate retry attempts up to max
        for _ in 0..config.max_retries {
            conn.increment_retry();
        }

        assert_eq!(conn.retry_count, config.max_retries);

        // One more retry exceeds the limit
        let next_retry = conn.increment_retry();
        assert_eq!(next_retry, config.max_retries + 1);

        // Mark as failed when max retries exceeded
        conn.mark_failed();
        assert_eq!(conn.state, PeerState::Failed);
    }

    #[test]
    fn retry_count_reset_on_success() {
        let peer = A2APeer::new("peer-1", "https://peer1.example.com", "token123");
        let mut conn = PeerConnection::new(peer);

        // Simulate some retries
        conn.increment_retry();
        conn.increment_retry();
        conn.increment_retry();
        assert_eq!(conn.retry_count, 3);

        // Successful connection resets counter
        conn.mark_connected();
        assert_eq!(conn.retry_count, 0);
    }

    // =========================================================================
    // Backoff Overflow Protection Tests
    // =========================================================================

    #[test]
    fn backoff_delay_no_overflow() {
        let config = ReconnectConfig {
            initial_delay_secs: u64::MAX,
            max_delay_secs: 60,
            max_retries: 10,
        };

        // Should not panic or overflow
        let delay = calculate_backoff_delay(1, &config);
        assert_eq!(delay, Duration::from_secs(60)); // Capped at max_delay_secs
    }

    #[test]
    fn backoff_delay_with_high_attempt_number() {
        let config = ReconnectConfig::default();

        // Very high attempt number should still work
        let delay = calculate_backoff_delay(1000, &config);
        assert_eq!(delay, Duration::from_secs(60)); // Capped at max_delay_secs
    }
}
