//! ZeroClaw A2A Protocol Crate
//!
//! This crate provides the Google A2A (Agent-to-Agent) protocol implementation
//! for ZeroClaw, enabling secure, authenticated messaging between peer agents.
//!
//! # Modules
//!
//! - [`protocol`] - Core message types and protocol definitions
//! - [`channel`] - Channel implementation for sending/receiving messages
//! - [`pairing`] - Peer authentication and pairing
//! - [`gateway`] - HTTP gateway handlers for A2A endpoints
//! - [`config`] - Configuration types
//!
//! # Abstraction Traits
//!
//! This crate defines several traits that the main crate must implement:
//!
//! - [`AgentDispatcher`] - Dispatches messages to the agent for processing
//! - [`SecretEncryptor`] - Encrypts/decrypts secrets for storage
//! - [`HttpClientBuilder`] - Builds HTTP clients with proxy support

pub mod channel;
pub mod config;
pub mod gateway;
pub mod pairing;
pub mod protocol;

// Re-export commonly used types
pub use channel::A2AChannel;
pub use config::{A2AConfig, A2APeer, A2ARateLimitConfig, A2AIdempotencyConfig, A2AReconnectConfig, AgentCardConfig, SkillConfig};
pub use gateway::{A2AGatewayState, TaskStore, TaskEntry, build_a2a_routes};
pub use protocol::{
    AgentCard, AgentCapabilities, AgentEndpoints, AuthenticationInfo, Artifact, ArtifactType,
    CancelTaskRequest, CancelTaskResponse, CreateTaskRequest, CreateTaskResponse, GetTaskRequest, MessageRole, Skill,
    Task, TaskMessage, TaskStatus, TaskUpdate,
};

use anyhow::Result;
use async_trait::async_trait;

/// Trait for dispatching messages to the agent for processing.
///
/// The main crate implements this trait to wire A2A message handling
/// to the agent loop (e.g., `run_gateway_chat_with_tools`).
#[async_trait]
pub trait AgentDispatcher: Send + Sync + Clone + 'static {
    /// Dispatch a message to the agent and return the response.
    ///
    /// # Arguments
    ///
    /// * `message` - The message content to process
    ///
    /// # Returns
    ///
    /// The agent's response as a string.
    async fn dispatch(&self, message: &str) -> Result<String>;
}

/// Trait for encrypting and decrypting secrets.
///
/// The main crate implements this trait using its SecretStore.
/// This allows the A2A crate to store encrypted bearer tokens.
pub trait SecretEncryptor: Send + Sync + Clone + 'static {
    /// Encrypt a plaintext secret.
    ///
    /// # Arguments
    ///
    /// * `plaintext` - The secret to encrypt
    ///
    /// # Returns
    ///
    /// The encrypted ciphertext or an error.
    fn encrypt(&self, plaintext: &str) -> Result<String>;

    /// Decrypt a ciphertext secret.
    ///
    /// # Arguments
    ///
    /// * `ciphertext` - The encrypted secret
    ///
    /// # Returns
    ///
    /// The decrypted plaintext or an error.
    fn decrypt(&self, ciphertext: &str) -> Result<String>;
}

/// Trait for building HTTP clients with proxy support.
///
/// The main crate implements this to provide proxy-aware HTTP clients.
pub trait HttpClientBuilder: Send + Sync + Clone + 'static {
    /// Build an HTTP client with the given timeout settings.
    ///
    /// # Arguments
    ///
    /// * `connect_timeout_secs` - Timeout for establishing connections
    /// * `request_timeout_secs` - Timeout for complete requests
    ///
    /// # Returns
    ///
    /// A configured `reqwest::Client` or an error.
    fn build_client(&self, connect_timeout_secs: u64, request_timeout_secs: u64) -> Result<reqwest::Client>;
}

/// Core channel trait - implement for any messaging platform.
///
/// This is a simplified version of the channel trait for use within the A2A crate.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Human-readable channel name
    fn name(&self) -> &str;

    /// Send a message through this channel
    async fn send(&self, message: &SendMessage) -> anyhow::Result<()>;

    /// Start listening for incoming messages (long-running)
    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()>;

    /// Check if channel is healthy
    async fn health_check(&self) -> bool {
        true
    }
}

/// A message received from or sent to a channel.
#[derive(Debug, Clone)]
pub struct ChannelMessage {
    pub id: String,
    pub sender: String,
    pub reply_target: String,
    pub content: String,
    pub channel: String,
    pub timestamp: u64,
    /// Platform thread identifier (e.g. Slack `ts`, Discord thread ID).
    pub thread_ts: Option<String>,
}

/// Message to send through a channel.
#[derive(Debug, Clone)]
pub struct SendMessage {
    pub content: String,
    pub recipient: String,
    pub subject: Option<String>,
    pub thread_ts: Option<String>,
}

impl SendMessage {
    /// Create a new message with content and recipient
    pub fn new(content: impl Into<String>, recipient: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
            subject: None,
            thread_ts: None,
        }
    }

    /// Create a new message with content, recipient, and subject
    pub fn with_subject(
        content: impl Into<String>,
        recipient: impl Into<String>,
        subject: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
            subject: Some(subject.into()),
            thread_ts: None,
        }
    }

    /// Set the thread identifier for threaded replies.
    pub fn in_thread(mut self, thread_ts: Option<String>) -> Self {
        self.thread_ts = thread_ts;
        self
    }
}
