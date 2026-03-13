use super::shell::collect_allowed_shell_env_vars;
use super::traits::{Tool, ToolResult};
use crate::security::SecurityPolicy;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

/// Maximum execution time for pi agent (5 mins).
const PI_AGENT_TIMEOUT_SECS: u64 = 300;
/// Maximum output size in bytes (1MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Map a ZeroClaw provider name to the environment variable name that pi.dev
/// expects for the corresponding API key.
fn provider_to_api_key_env(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" | "openrouter" => "OPENAI_API_KEY",
        "gemini" | "google" => "GEMINI_API_KEY",
        "groq" => "GROQ_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "xai" => "XAI_API_KEY",
        _ => "OPENAI_API_KEY",
    }
}

/// Map a ZeroClaw provider name to the provider identifier that pi.dev
/// recognises in its `defaultProvider` setting.
fn provider_to_pi_provider(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "anthropic" => "anthropic",
        "openai" => "openai",
        "openrouter" => "openrouter",
        "gemini" | "google" => "google",
        "groq" => "groq",
        "mistral" => "mistral",
        "ollama" => "ollama",
        "deepseek" => "deepseek",
        "xai" => "xai",
        _ => "openai",
    }
}

/// Tool to invoke the pi.dev terminal-based coding agent.
///
/// Spawns the `pi` CLI in print mode (`pi -p "<task>"`) and captures stdout.
/// Forwards ZeroClaw's configured provider, model, and API key so pi uses
/// the same LLM backend. Requires `pi` to be installed and on `$PATH`.
/// Gated behind `[pi_agent] enabled = true` in `config.toml`.
pub struct PiAgentTool {
    security: Arc<SecurityPolicy>,
    /// ZeroClaw provider name (e.g. "anthropic", "openrouter").
    provider: String,
    /// Model identifier routed through the provider.
    model: String,
    /// API key for the provider (if configured).
    api_key: Option<String>,
    /// Optional base URL override (e.g. for Ollama or custom endpoints).
    api_url: Option<String>,
}

impl PiAgentTool {
    pub fn new(
        security: Arc<SecurityPolicy>,
        provider: String,
        model: String,
        api_key: Option<String>,
        api_url: Option<String>,
    ) -> Self {
        Self {
            security,
            provider,
            model,
            api_key,
            api_url,
        }
    }
}

#[async_trait]
impl Tool for PiAgentTool {
    fn name(&self) -> &str {
        "pi_coding_agent"
    }

    fn description(&self) -> &str {
        "Delegate a complex coding or development task to the pi.dev terminal-based coding agent. \
         Uses the same LLM provider and model as the main ZeroClaw agent."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The complex coding task description or prompt to send to the pi agent."
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory relative to the workspace root. If omitted, uses the workspace root."
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if task.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Missing or empty 'task' parameter".into()),
            });
        }

        // Autonomy gate: block in ReadOnly mode (pi can write files).
        if matches!(
            self.security.autonomy,
            crate::security::AutonomyLevel::ReadOnly
        ) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("pi_coding_agent is not allowed in read-only autonomy mode".into()),
            });
        }

        // Resolve and sandbox the working directory.
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let workspace_canon = self
            .security
            .workspace_dir
            .canonicalize()
            .unwrap_or_else(|_| self.security.workspace_dir.clone());
        let work_dir = if cwd.is_empty() {
            workspace_canon.clone()
        } else {
            let combined = workspace_canon.join(cwd);
            match combined.canonicalize() {
                Ok(canon) if canon.starts_with(&workspace_canon) => canon,
                Ok(_) => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("Requested directory escapes the workspace".into()),
                    });
                }
                Err(_) => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Invalid or non-existent directory: {cwd}")),
                    });
                }
            }
        };

        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        // Write a project-level .pi/settings.json so pi uses the same
        // provider and model as ZeroClaw.
        let pi_settings_dir = work_dir.join(".pi");
        let pi_settings_path = pi_settings_dir.join("settings.json");
        let wrote_settings = if !self.provider.is_empty() || !self.model.is_empty() {
            let mut settings = serde_json::Map::new();
            if !self.provider.is_empty() {
                settings.insert(
                    "defaultProvider".into(),
                    serde_json::Value::String(provider_to_pi_provider(&self.provider).into()),
                );
            }
            if !self.model.is_empty() {
                settings.insert(
                    "defaultModel".into(),
                    serde_json::Value::String(self.model.clone()),
                );
            }
            match tokio::fs::create_dir_all(&pi_settings_dir).await {
                Ok(()) => {
                    let json_bytes =
                        serde_json::to_vec_pretty(&serde_json::Value::Object(settings))
                            .unwrap_or_default();
                    tokio::fs::write(&pi_settings_path, json_bytes)
                        .await
                        .is_ok()
                }
                Err(_) => false,
            }
        } else {
            false
        };

        // Build command. Use print mode for clean text output.
        let mut cmd = Command::new("pi");
        cmd.arg("-p").arg(task).current_dir(&work_dir);
        
        tracing::info!(
            task = %task,
            provider = %self.provider,
            model = %self.model,
            cwd = %work_dir.display(),
            "Invoking pi agent"
        );

        // Clear the environment to prevent leaking API keys and secrets
        // (CWE-200), then re-add only safe, functional variables.
        cmd.env_clear();
        for var in collect_allowed_shell_env_vars(&self.security) {
            if let Ok(val) = std::env::var(&var) {
                cmd.env(&var, val);
            }
        }

        // Inject the API key as the provider-specific env var that pi expects.
        if let Some(ref key) = self.api_key {
            if !key.is_empty() {
                let env_name = provider_to_api_key_env(&self.provider);
                cmd.env(env_name, key);

                // For OpenRouter, pi expects traffic through the OpenAI-compatible
                // endpoint, so also set the base URL override.
                if self.provider.eq_ignore_ascii_case("openrouter") {
                    cmd.env(
                        "OPENAI_BASE_URL",
                        self.api_url
                            .as_deref()
                            .unwrap_or("https://openrouter.ai/api/v1"),
                    );
                } else if let Some(ref url) = self.api_url {
                    // For Ollama or custom endpoints, pass the base URL.
                    if self.provider.eq_ignore_ascii_case("ollama") {
                        cmd.env("OLLAMA_HOST", url);
                    }
                }
            }
        }

        // Execute with timeout.
        let result =
            tokio::time::timeout(Duration::from_secs(PI_AGENT_TIMEOUT_SECS), cmd.output()).await;

        // Clean up the project-level settings we wrote (best-effort).
        if wrote_settings {
            let _ = tokio::fs::remove_file(&pi_settings_path).await;
            // Remove the .pi dir only if we created it and it's now empty.
            let _ = tokio::fs::remove_dir(&pi_settings_dir).await;
        }

        match result {
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

                tracing::info!(
                    exit_code = %output.status,
                    stdout_len = stdout.len(),
                    stderr_len = stderr.len(),
                    "Pi agent completed"
                );

                if !stdout.is_empty() {
                    tracing::debug!(stdout = %stdout, "Pi agent stdout");
                }
                if !stderr.is_empty() {
                    tracing::debug!(stderr = %stderr, "Pi agent stderr");
                }

                if stdout.len() > MAX_OUTPUT_BYTES {
                    stdout.truncate(crate::util::floor_utf8_char_boundary(
                        &stdout,
                        MAX_OUTPUT_BYTES,
                    ));
                    stdout.push_str("\n... [output truncated at 1MB]");
                }
                if stderr.len() > MAX_OUTPUT_BYTES {
                    stderr.truncate(crate::util::floor_utf8_char_boundary(
                        &stderr,
                        MAX_OUTPUT_BYTES,
                    ));
                    stderr.push_str("\n... [stderr truncated at 1MB]");
                }

                Ok(ToolResult {
                    success: output.status.success(),
                    output: stdout,
                    error: if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr)
                    },
                })
            }
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to execute pi command: {e}")),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "pi agent timed out after {PI_AGENT_TIMEOUT_SECS}s and was killed"
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{AutonomyLevel, SecurityPolicy};

    fn test_security(autonomy: AutonomyLevel) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }

    fn make_tool(autonomy: AutonomyLevel) -> PiAgentTool {
        PiAgentTool::new(
            test_security(autonomy),
            "anthropic".into(),
            "claude-sonnet-4-20250514".into(),
            Some("sk-test-key".into()),
            None,
        )
    }

    #[test]
    fn pi_agent_tool_name() {
        let tool = make_tool(AutonomyLevel::Supervised);
        assert_eq!(tool.name(), "pi_coding_agent");
    }

    #[test]
    fn pi_agent_tool_description_not_empty() {
        let tool = make_tool(AutonomyLevel::Supervised);
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn pi_agent_tool_schema_has_required_task() {
        let tool = make_tool(AutonomyLevel::Supervised);
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["task"].is_object());
        assert!(schema["properties"]["cwd"].is_object());
        let required = schema["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("task")));
    }

    #[tokio::test]
    async fn pi_agent_rejects_empty_task() {
        let tool = make_tool(AutonomyLevel::Full);
        let result = tool.execute(json!({"task": ""})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("Missing"));
    }

    #[tokio::test]
    async fn pi_agent_blocks_readonly_autonomy() {
        let tool = make_tool(AutonomyLevel::ReadOnly);
        let result = tool
            .execute(json!({"task": "write hello world"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("read-only"));
    }

    #[tokio::test]
    async fn pi_agent_blocks_rate_limited() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            max_actions_per_hour: 0,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });
        let tool = PiAgentTool::new(
            security,
            "anthropic".into(),
            "claude-sonnet-4-20250514".into(),
            None,
            None,
        );
        let result = tool
            .execute(json!({"task": "build something"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Rate limit"));
    }

    #[test]
    fn provider_to_env_key_mapping() {
        assert_eq!(provider_to_api_key_env("anthropic"), "ANTHROPIC_API_KEY");
        assert_eq!(provider_to_api_key_env("openai"), "OPENAI_API_KEY");
        assert_eq!(provider_to_api_key_env("openrouter"), "OPENAI_API_KEY");
        assert_eq!(provider_to_api_key_env("gemini"), "GEMINI_API_KEY");
        assert_eq!(provider_to_api_key_env("groq"), "GROQ_API_KEY");
        assert_eq!(provider_to_api_key_env("unknown"), "OPENAI_API_KEY");
    }

    #[test]
    fn provider_to_pi_provider_mapping() {
        assert_eq!(provider_to_pi_provider("anthropic"), "anthropic");
        assert_eq!(provider_to_pi_provider("openrouter"), "openrouter");
        assert_eq!(provider_to_pi_provider("gemini"), "google");
        assert_eq!(provider_to_pi_provider("ollama"), "ollama");
        assert_eq!(provider_to_pi_provider("unknown"), "openai");
    }
}
