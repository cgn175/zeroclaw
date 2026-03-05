//! A2A Send Tool — send messages to peer agents via the A2A protocol.
//!
//! This tool is a thin wrapper over the live A2A channel registered by
//! `start_channels()`. It calls `get_live_channel("a2a")` and invokes
//! `.send()` on the existing `A2AChannel`, which handles bearer auth,
//! SSE subscription, and response routing automatically.

use super::traits::{Tool, ToolResult};
use crate::channels::{get_live_channel, SendMessage};
use async_trait::async_trait;
use serde_json::json;

/// LLM-callable tool for sending messages to peer agents via A2A.
pub struct A2aSendTool;

impl A2aSendTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for A2aSendTool {
    fn name(&self) -> &str {
        "a2a_send"
    }

    fn description(&self) -> &str {
        "Send a message to a peer agent via the A2A (Agent-to-Agent) protocol. \
         The recipient must be a configured peer in your A2A channel. \
         Messages are delivered asynchronously — responses arrive through your A2A channel."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Peer agent ID to send the message to (must be a configured A2A peer)"
                },
                "message": {
                    "type": "string",
                    "description": "Message content to send to the peer agent"
                }
            },
            "required": ["to", "message"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let to = args.get("to").and_then(|v| v.as_str()).unwrap_or_default();
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        if to.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Missing required parameter: 'to'".to_string()),
            });
        }
        if message.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Missing required parameter: 'message'".to_string()),
            });
        }

        let channel = match get_live_channel("a2a") {
            Some(ch) => ch,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(
                        "A2A channel is not active. Ensure A2A is enabled and peers are configured."
                            .to_string(),
                    ),
                });
            }
        };

        let send_msg = SendMessage::new(message, to);

        match channel.send(&send_msg).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!(
                    "Message sent to peer '{}'. Response will arrive asynchronously via A2A channel.",
                    to
                ),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to send A2A message: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = A2aSendTool::new();
        assert_eq!(tool.name(), "a2a_send");
        assert!(!tool.description().is_empty());
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["to"].is_object());
        assert!(schema["properties"]["message"].is_object());
        assert_eq!(schema["required"], json!(["to", "message"]));
    }

    #[tokio::test]
    async fn missing_to_returns_error() {
        let tool = A2aSendTool::new();
        let result = tool.execute(json!({"message": "hello"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("to"));
    }

    #[tokio::test]
    async fn missing_message_returns_error() {
        let tool = A2aSendTool::new();
        let result = tool.execute(json!({"to": "peer-1"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("message"));
    }

    #[tokio::test]
    async fn no_active_channel_returns_error() {
        let tool = A2aSendTool::new();
        let result = tool
            .execute(json!({"to": "peer-1", "message": "hello"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("not active"));
    }
}
