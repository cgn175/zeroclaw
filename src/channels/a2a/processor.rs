//! A2A (Agent-to-Agent) Message Processor
//!
//! This module handles incoming A2A message processing, integrating with the
//! agent loop, memory system, and SSE streaming for responses.
//!
//! # Architecture
//!
//! The processor converts A2A messages to ChannelMessages, loads conversation
//! history, invokes the agent loop, streams responses via SSE, and auto-saves
//! to memory.

use super::protocol::A2AMessage;
use crate::channels::traits::ChannelMessage;
use crate::gateway::AppState;
use crate::memory::{Memory, MemoryCategory, MemoryEntry};
// Provider trait imported when needed for provider operations
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Minimum characters per chunk when streaming response to SSE.
const STREAM_CHUNK_MIN_CHARS: usize = 80;

/// Convert an A2A message to a ChannelMessage.
///
/// # Mapping
/// - `a2a.id` -> used as part of channel message ID (prefixed with "a2a_")
/// - `a2a.sender_id` -> sender and reply_target
/// - `a2a.content` -> content
/// - `a2a.session_id` -> thread_ts (for conversation threading)
/// - `a2a.timestamp` -> timestamp
pub fn a2a_to_channel_message(msg: &A2AMessage) -> ChannelMessage {
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

/// Load conversation history for a given session ID.
///
/// Queries the memory system for messages with matching session_id tag,
/// returning them as ChatMessage objects for the agent loop context.
async fn load_conversation_history(memory: &dyn Memory, session_id: &str) -> Vec<ChatMessage> {
    let mut history = Vec::new();

    // Query memory for entries with this session_id
    // We use the list method with session_id filter to get all conversation entries
    match memory
        .list(Some(&MemoryCategory::Conversation), Some(session_id))
        .await
    {
        Ok(entries) => {
            // Sort by timestamp to maintain conversation order
            let mut sorted_entries: Vec<MemoryEntry> = entries;
            sorted_entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

            for entry in sorted_entries {
                // Determine role based on key prefix or content
                // Keys starting with "a2a_in_" are incoming (user), "a2a_out_" are outgoing (assistant)
                let role = if entry.key.starts_with("a2a_in_") {
                    "user"
                } else if entry.key.starts_with("a2a_out_") {
                    "assistant"
                } else {
                    // Default to user for unknown entries
                    "user"
                };

                history.push(ChatMessage {
                    role: role.to_string(),
                    content: entry.content,
                });
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to load conversation history for session {}: {}",
                session_id,
                e
            );
        }
    }

    history
}

/// Save a message to memory with session tagging.
async fn save_message_to_memory(
    memory: &dyn Memory,
    session_id: &str,
    content: &str,
    is_incoming: bool,
) -> Result<()> {
    let key_prefix = if is_incoming { "a2a_in" } else { "a2a_out" };
    let key = format!("{}_{}_{}", key_prefix, session_id, Uuid::new_v4());

    memory
        .store(
            &key,
            content,
            MemoryCategory::Conversation,
            Some(session_id),
        )
        .await
        .context("Failed to store message in memory")?;

    Ok(())
}

/// Stream response chunks to an SSE sender.
///
/// Chunks are accumulated until they reach STREAM_CHUNK_MIN_CHARS
/// for efficient streaming.
async fn stream_response_to_sse(response: &str, tx: &mpsc::Sender<String>) -> Result<()> {
    let mut chunk = String::new();

    for word in response.split_inclusive(char::is_whitespace) {
        chunk.push_str(word);

        if chunk.len() >= STREAM_CHUNK_MIN_CHARS {
            tx.send(std::mem::take(&mut chunk))
                .await
                .context("SSE channel closed")?;
        }
    }

    // Send any remaining content
    if !chunk.is_empty() {
        tx.send(chunk).await.context("SSE channel closed")?;
    }

    Ok(())
}

/// Process an incoming A2A message through the agent loop.
///
/// This function:
/// 1. Converts A2AMessage to ChannelMessage
/// 2. Loads conversation history by session_id
/// 3. Calls the agent loop with the message and history
/// 4. Streams response chunks to SSE channel
/// 5. Auto-saves conversation memory
///
/// # Arguments
///
/// * `state` - The application state containing config, memory, provider
/// * `message` - The incoming A2A message
///
/// # Returns
///
/// The session_id of the processed message, or an error if processing failed.
pub async fn process_a2a_message(state: Arc<AppState>, message: A2AMessage) -> Result<String> {
    let session_id = message.session_id.clone();
    let sender_id = message.sender_id.clone();

    tracing::info!(
        "Processing A2A message from {}: session={}",
        sender_id,
        session_id
    );

    // Step 1: Convert A2AMessage to ChannelMessage
    let channel_msg = a2a_to_channel_message(&message);

    // Step 2: Load conversation history by session_id
    let mut history = load_conversation_history(state.mem.as_ref(), &session_id).await;

    // Step 3: Save incoming message to memory (if auto_save enabled)
    if state.auto_save {
        if let Err(e) = save_message_to_memory(
            state.mem.as_ref(),
            &session_id,
            &message.content,
            true, // incoming
        )
        .await
        {
            tracing::warn!("Failed to save incoming A2A message to memory: {}", e);
            // Continue processing - don't fail on memory errors
        }
    }

    // Step 4: Build system prompt and prepare messages for agent
    let config_guard = state.config.lock();
    let _provider_name = config_guard
        .default_provider
        .as_deref()
        .unwrap_or("openrouter");
    // _provider_name is kept for future use when provider selection is needed
    let multimodal_config = config_guard.multimodal.clone();
    let max_tool_iterations = config_guard.agent.max_tool_iterations;
    let workspace_dir = config_guard.workspace_dir.clone();

    // Build tool descriptions for system prompt
    let tool_descs: Vec<(&str, &str)> = vec![
        ("shell", "Execute terminal commands."),
        ("file_read", "Read file contents."),
        ("file_write", "Write file contents."),
        ("memory_store", "Save to memory."),
        ("memory_recall", "Search memory."),
        ("memory_forget", "Delete a memory entry."),
    ];

    let system_prompt = crate::channels::build_system_prompt(
        &workspace_dir,
        &state.model,
        &tool_descs,
        &[], // skills
        None,
        None,
    );

    drop(config_guard);

    // Add system prompt if history is empty
    if history.is_empty() {
        history.push(ChatMessage::system(system_prompt));
    }

    // Add the current user message
    history.push(ChatMessage::user(channel_msg.content.clone()));

    // Step 5: Create channel for SSE streaming
    let (sse_tx, mut sse_rx) = mpsc::channel::<String>(100);

    // Clone values needed for the spawned task
    let session_id_for_task = session_id.clone();
    let mem = state.mem.clone();
    let auto_save = state.auto_save;

    // Spawn SSE streaming task
    let sse_handle = tokio::spawn(async move {
        let mut full_response = String::new();

        while let Some(chunk) = sse_rx.recv().await {
            full_response.push_str(&chunk);
            // In a real implementation, this would send to the SSE broadcaster
            // For now, we just accumulate the response
            tracing::debug!(
                "A2A SSE chunk for session {}: {} chars",
                session_id_for_task,
                chunk.len()
            );
        }

        // Save outgoing response to memory
        if auto_save && !full_response.is_empty() {
            if let Err(e) = save_message_to_memory(
                mem.as_ref(),
                &session_id_for_task,
                &full_response,
                false, // outgoing
            )
            .await
            {
                tracing::warn!("Failed to save outgoing A2A response to memory: {}", e);
            }
        }

        full_response
    });

    // Step 6: Call agent loop
    // For now, we use a simplified approach - directly query the provider
    // In a full implementation, this would use the complete agent loop with tools
    let response = match call_agent_loop(
        state,
        &history,
        &multimodal_config,
        max_tool_iterations,
        Some(sse_tx.clone()),
    )
    .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Agent loop failed for A2A message: {}", e);
            let error_msg = format!("Error processing message: {}", e);
            // Send error through SSE channel
            let _ = sse_tx.send(error_msg.clone()).await;
            error_msg
        }
    };

    // Close the SSE channel to signal completion
    drop(sse_tx);

    // Wait for the SSE handler to complete (including memory save)
    match tokio::time::timeout(std::time::Duration::from_secs(30), sse_handle).await {
        Ok(Ok(_)) => {
            tracing::debug!("A2A SSE handler completed for session {}", session_id);
        }
        Ok(Err(e)) => {
            tracing::warn!("A2A SSE handler panicked: {}", e);
        }
        Err(_) => {
            tracing::warn!("A2A SSE handler timed out for session {}", session_id);
        }
    }

    tracing::info!(
        "A2A message processed for session {}: response length = {} chars",
        session_id,
        response.len()
    );

    Ok(session_id)
}

/// Call the agent loop with the given history and configuration.
///
/// This is a simplified implementation that queries the provider directly.
/// In a full implementation, this would invoke the complete agent loop
/// with tool support from `crate::agent::loop_`.
async fn call_agent_loop(
    state: Arc<AppState>,
    history: &[ChatMessage],
    multimodal_config: &crate::config::MultimodalConfig,
    _max_tool_iterations: usize,
    on_delta: Option<mpsc::Sender<String>>,
) -> Result<String> {
    // Prepare messages for the provider
    let prepared =
        crate::multimodal::prepare_messages_for_provider(history, multimodal_config).await?;

    // Query the provider
    let response = state
        .provider
        .chat_with_history(&prepared.messages, &state.model, state.temperature)
        .await?;

    // Stream response if channel provided
    if let Some(tx) = on_delta {
        if let Err(e) = stream_response_to_sse(&response, &tx).await {
            tracing::warn!("Failed to stream A2A response: {}", e);
        }
    }

    Ok(response)
}

/// Process an A2A message with full tool support.
///
/// This is an advanced version that uses the complete agent loop with
/// tool execution capabilities. Requires additional setup for tools
/// and runtime adapters.
#[allow(dead_code)]
pub async fn process_a2a_message_with_tools(
    _state: Arc<AppState>,
    message: A2AMessage,
) -> Result<String> {
    // This would be implemented to use the full agent loop from crate::agent::loop_
    // including tool execution, approval management, etc.
    //
    // For now, delegate to the basic processor
    tracing::debug!(
        "Full tool support not yet implemented for A2A message: {}",
        message.id
    );

    Ok(message.session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, MemoryCategory, MemoryEntry};
    use async_trait::async_trait;
    use parking_lot::Mutex;

    /// Mock memory backend for testing
    struct MockMemory {
        entries: Mutex<Vec<MemoryEntry>>,
    }

    impl MockMemory {
        fn new() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
            }
        }

        fn with_entries(entries: Vec<MemoryEntry>) -> Self {
            Self {
                entries: Mutex::new(entries),
            }
        }
    }

    #[async_trait]
    impl Memory for MockMemory {
        fn name(&self) -> &str {
            "mock"
        }

        async fn store(
            &self,
            key: &str,
            content: &str,
            category: MemoryCategory,
            session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            let entry = MemoryEntry {
                id: Uuid::new_v4().to_string(),
                key: key.to_string(),
                content: content.to_string(),
                category,
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: session_id.map(String::from),
                score: None,
            };
            self.entries.lock().push(entry);
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            category: Option<&MemoryCategory>,
            session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            let entries = self.entries.lock();
            let filtered: Vec<MemoryEntry> = entries
                .iter()
                .filter(|e| {
                    let category_match = category.map(|c| e.category == *c).unwrap_or(true);
                    let session_match = session_id
                        .map(|s| e.session_id.as_deref() == Some(s))
                        .unwrap_or(true);
                    category_match && session_match
                })
                .cloned()
                .collect();
            Ok(filtered)
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(self.entries.lock().len())
        }

        async fn health_check(&self) -> bool {
            true
        }
    }

    #[test]
    fn a2a_to_channel_message_conversion() {
        let a2a_msg = A2AMessage::new(
            "session-123",
            "peer-a",
            "peer-b",
            "Hello, this is a test message",
        );

        let channel_msg = a2a_to_channel_message(&a2a_msg);

        assert_eq!(channel_msg.id, format!("a2a_{}", a2a_msg.id));
        assert_eq!(channel_msg.sender, "peer-a");
        assert_eq!(channel_msg.reply_target, "peer-a");
        assert_eq!(channel_msg.content, "Hello, this is a test message");
        assert_eq!(channel_msg.channel, "a2a");
        assert_eq!(channel_msg.timestamp, a2a_msg.timestamp);
        assert_eq!(channel_msg.thread_ts, Some("session-123".to_string()));
    }

    #[test]
    fn a2a_to_channel_message_preserves_session() {
        let parent = A2AMessage::new("session-456", "peer-x", "peer-y", "Original");
        let reply = A2AMessage::reply_to(&parent, "peer-y", "Reply content");

        let channel_msg = a2a_to_channel_message(&reply);

        assert_eq!(channel_msg.sender, "peer-y");
        assert_eq!(channel_msg.reply_target, "peer-y");
        assert_eq!(channel_msg.thread_ts, Some("session-456".to_string()));
    }

    #[tokio::test]
    async fn load_conversation_history_empty() {
        let memory = MockMemory::new();
        let history = load_conversation_history(&memory, "session-123").await;

        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn load_conversation_history_with_entries() {
        let entries = vec![
            MemoryEntry {
                id: "1".to_string(),
                key: "a2a_in_session-123_msg1".to_string(),
                content: "Hello".to_string(),
                category: MemoryCategory::Conversation,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                session_id: Some("session-123".to_string()),
                score: None,
            },
            MemoryEntry {
                id: "2".to_string(),
                key: "a2a_out_session-123_msg2".to_string(),
                content: "Hi there".to_string(),
                category: MemoryCategory::Conversation,
                timestamp: "2026-01-01T00:00:01Z".to_string(),
                session_id: Some("session-123".to_string()),
                score: None,
            },
        ];

        let memory = MockMemory::with_entries(entries);
        let history = load_conversation_history(&memory, "session-123").await;

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[1].content, "Hi there");
    }

    #[tokio::test]
    async fn save_message_to_memory_incoming() {
        let memory = MockMemory::new();

        save_message_to_memory(&memory, "session-123", "Test message", true)
            .await
            .unwrap();

        let entries = memory.entries.lock();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].key.starts_with("a2a_in_session-123_"));
        assert_eq!(entries[0].content, "Test message");
        assert_eq!(entries[0].category, MemoryCategory::Conversation);
        assert_eq!(entries[0].session_id, Some("session-123".to_string()));
    }

    #[tokio::test]
    async fn save_message_to_memory_outgoing() {
        let memory = MockMemory::new();

        save_message_to_memory(&memory, "session-123", "Test response", false)
            .await
            .unwrap();

        let entries = memory.entries.lock();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].key.starts_with("a2a_out_session-123_"));
        assert_eq!(entries[0].content, "Test response");
    }

    #[tokio::test]
    async fn stream_response_to_sse_chunks() {
        let (tx, mut rx) = mpsc::channel::<String>(10);
        let response = "This is a test response with multiple words to verify chunking behavior";

        stream_response_to_sse(response, &tx).await.unwrap();
        drop(tx); // Close channel

        let mut chunks = Vec::new();
        while let Some(chunk) = rx.recv().await {
            chunks.push(chunk);
        }

        // Should have at least one chunk
        assert!(!chunks.is_empty());

        // Verify all content is preserved
        let combined: String = chunks.concat();
        assert_eq!(combined, response);
    }

    #[tokio::test]
    async fn stream_response_to_sse_empty() {
        let (tx, mut rx) = mpsc::channel::<String>(10);

        stream_response_to_sse("", &tx).await.unwrap();
        drop(tx);

        let mut chunks = Vec::new();
        while let Some(chunk) = rx.recv().await {
            chunks.push(chunk);
        }

        assert!(chunks.is_empty());
    }

    #[test]
    fn stream_chunk_min_chars_constant() {
        // Verify the chunk size constant is reasonable
        assert_eq!(STREAM_CHUNK_MIN_CHARS, 80);
        assert!(STREAM_CHUNK_MIN_CHARS > 0);
    }
}
