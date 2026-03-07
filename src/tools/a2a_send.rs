//! A2A Send Tool — send messages to peer agents via the A2A protocol.
//!
//! This tool is self-contained and works in both gateway and daemon modes.
//! It reads peer configuration from `config.channels_config.a2a` and sends
//! messages directly via HTTP POST to the peer's `/tasks` endpoint, following
//! the Google A2A protocol standard. It reuses protocol types from
//! `zeroclaw_a2a` (CreateTaskRequest, TaskUpdate, TaskStatus) to ensure
//! wire-format compatibility.
//!
//! After sending, it subscribes to the task's SSE stream to collect the
//! peer's response and returns it synchronously to the LLM.

use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use zeroclaw_a2a::{CreateTaskRequest, TaskStatus, TaskUpdate};

/// LLM-callable tool for sending messages to peer agents via A2A.
pub struct A2aSendTool {
    config: Arc<Config>,
}

impl A2aSendTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for A2aSendTool {
    fn name(&self) -> &str {
        "a2a_send"
    }

    fn description(&self) -> &str {
        "Send a message to a peer agent via the A2A (Agent-to-Agent) protocol and wait for their response. \
         The recipient must be a configured peer in your A2A configuration."
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

        // Read A2A config from the main config (works in both gateway and daemon modes)
        let a2a_config = match &self.config.channels_config.a2a {
            Some(cfg) if cfg.enabled => cfg,
            Some(_) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("A2A channel is disabled in configuration".to_string()),
                });
            }
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(
                        "A2A channel is not configured. Add [channels.a2a] section to config.toml"
                            .to_string(),
                    ),
                });
            }
        };

        // Find the peer in the configuration
        let peer = match a2a_config
            .peers
            .iter()
            .find(|p| p.id == to && p.enabled)
        {
            Some(p) => p,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Unknown or disabled peer: '{}'. Available peers: {:?}",
                        to,
                        a2a_config
                            .peers
                            .iter()
                            .filter(|p| p.enabled)
                            .map(|p| p.id.as_str())
                            .collect::<Vec<_>>()
                    )),
                });
            }
        };

        // Build HTTP client
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300)) // 5 min for long-running tasks
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {e}"))?;

        // Create A2A task request using the protocol type from zeroclaw_a2a crate.
        // CreateTaskRequest::new() wraps the content in a TaskMessage{role: User, ...}
        let task_request = CreateTaskRequest::new(message);
        let endpoint_url = format!("{}/tasks", peer.endpoint.trim_end_matches('/'));

        // Send POST request to peer's /tasks endpoint with bearer auth
        let response = http_client
            .post(&endpoint_url)
            .bearer_auth(&peer.bearer_token)
            .json(&task_request)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to send A2A request to {}: {}", endpoint_url, e)
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "A2A request failed with HTTP {}: {}",
                    status, error_body
                )),
            });
        }

        // Parse response to get task_id
        let resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse A2A response: {e}"))?;

        let task_id = resp_body
            .get("task")
            .and_then(|t| t.get("id"))
            .and_then(|id| id.as_str())
            .unwrap_or("unknown")
            .to_string();

        if task_id == "unknown" {
            return Ok(ToolResult {
                success: true,
                output: format!("Message sent to peer '{}' but no task_id returned.", to),
                error: None,
            });
        }

        // Subscribe to SSE stream to collect the peer's response.
        // This mirrors A2AChannel::subscribe_to_task_response() but collects
        // messages synchronously instead of forwarding to a channel bus.
        let stream_url = format!(
            "{}/tasks/{}/stream",
            peer.endpoint.trim_end_matches('/'),
            task_id
        );

        let sse_response = http_client
            .get(&stream_url)
            .bearer_auth(&peer.bearer_token)
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(300))
            .send()
            .await;

        let sse_response = match sse_response {
            Ok(resp) if resp.status().is_success() => resp,
            Ok(resp) => {
                return Ok(ToolResult {
                    success: true,
                    output: format!(
                        "Message sent to peer '{}' (task_id: {}). \
                         Could not subscribe to response stream (HTTP {}).",
                        to,
                        task_id,
                        resp.status()
                    ),
                    error: None,
                });
            }
            Err(e) => {
                return Ok(ToolResult {
                    success: true,
                    output: format!(
                        "Message sent to peer '{}' (task_id: {}). \
                         Could not subscribe to response stream: {}",
                        to, task_id, e
                    ),
                    error: None,
                });
            }
        };

        // Collect response messages from SSE stream using TaskUpdate from zeroclaw_a2a
        let mut collected_messages = Vec::new();
        let mut buffer = String::new();
        let mut stream = sse_response.bytes_stream();
        let mut final_status = TaskStatus::Pending;

        while let Some(chunk) = stream.next().await {
            let data = match chunk {
                Ok(d) => d,
                Err(_) => break,
            };

            let text = String::from_utf8_lossy(&data);
            buffer.push_str(&text);

            // Process complete SSE events (separated by double newline)
            while let Some(event_end) = buffer.find("\n\n") {
                let event_block = buffer[..event_end].to_string();
                buffer = buffer[event_end + 2..].to_string();

                for line in event_block.lines() {
                    if let Some(data_str) = line.strip_prefix("data: ") {
                        // Deserialize using TaskUpdate from zeroclaw_a2a — same type
                        // the gateway serializes, ensuring wire-format compatibility
                        if let Ok(update) = serde_json::from_str::<TaskUpdate>(data_str) {
                            if let Some(ref msg) = update.message {
                                collected_messages.push(msg.content.clone());
                            }
                            final_status = update.status.clone();

                            // Stop on terminal states
                            if matches!(
                                final_status,
                                TaskStatus::Completed
                                    | TaskStatus::Failed
                                    | TaskStatus::Cancelled
                            ) {
                                let response_text = collected_messages.join("\n");
                                return Ok(ToolResult {
                                    success: !matches!(final_status, TaskStatus::Failed),
                                    output: format!(
                                        "Response from peer '{}' (task: {}, status: {:?}):\n\n{}",
                                        to, task_id, final_status, response_text
                                    ),
                                    error: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Stream ended without terminal status
        let response_text = collected_messages.join("\n");
        if response_text.is_empty() {
            Ok(ToolResult {
                success: true,
                output: format!(
                    "Message sent to peer '{}' (task: {}). Stream ended with no response content.",
                    to, task_id
                ),
                error: None,
            })
        } else {
            Ok(ToolResult {
                success: true,
                output: format!(
                    "Response from peer '{}' (task: {}, status: {:?}):\n\n{}",
                    to, task_id, final_status, response_text
                ),
                error: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroclaw_a2a::A2AConfig;

    fn test_config_with_a2a(enabled: bool, peers: Vec<zeroclaw_a2a::A2APeer>) -> Arc<Config> {
        let mut config = Config::default();
        config.channels_config.a2a = Some(A2AConfig {
            enabled,
            listen_port: 9000,
            discovery_mode: "static".to_string(),
            allowed_peer_ids: vec!["*".to_string()],
            peers,
            rate_limit: Default::default(),
            idempotency: Default::default(),
            reconnect: Default::default(),
            agent_card: None,
        });
        Arc::new(config)
    }

    #[test]
    fn tool_metadata() {
        let config = test_config_with_a2a(true, vec![]);
        let tool = A2aSendTool::new(config);
        assert_eq!(tool.name(), "a2a_send");
        assert!(!tool.description().is_empty());
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["to"].is_object());
        assert!(schema["properties"]["message"].is_object());
        assert_eq!(schema["required"], json!(["to", "message"]));
    }

    #[tokio::test]
    async fn missing_to_returns_error() {
        let config = test_config_with_a2a(true, vec![]);
        let tool = A2aSendTool::new(config);
        let result = tool.execute(json!({"message": "hello"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("to"));
    }

    #[tokio::test]
    async fn missing_message_returns_error() {
        let config = test_config_with_a2a(true, vec![]);
        let tool = A2aSendTool::new(config);
        let result = tool.execute(json!({"to": "peer-1"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("message"));
    }

    #[tokio::test]
    async fn no_a2a_config_returns_error() {
        let config = Arc::new(Config::default());
        let tool = A2aSendTool::new(config);
        let result = tool
            .execute(json!({"to": "peer-1", "message": "hello"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn disabled_a2a_returns_error() {
        let config = test_config_with_a2a(false, vec![]);
        let tool = A2aSendTool::new(config);
        let result = tool
            .execute(json!({"to": "peer-1", "message": "hello"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("disabled"));
    }

    #[tokio::test]
    async fn unknown_peer_returns_error() {
        use zeroclaw_a2a::A2APeer;
        let peers = vec![A2APeer {
            id: "peer-2".to_string(),
            endpoint: "http://localhost:8080".to_string(),
            bearer_token: "token".to_string(),
            enabled: true,
        }];
        let config = test_config_with_a2a(true, peers);
        let tool = A2aSendTool::new(config);
        let result = tool
            .execute(json!({"to": "peer-1", "message": "hello"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap()
                .contains("Unknown or disabled peer")
        );
    }
}
