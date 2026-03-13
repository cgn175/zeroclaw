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
///
/// Reference: <https://github.com/badlogic/pi-mono/blob/main/packages/ai/src/env-api-keys.ts>
fn provider_to_api_key_env(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "gemini" | "google" => "GEMINI_API_KEY",
        "groq" => "GROQ_API_KEY",
        "cerebras" => "CEREBRAS_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "xai" | "grok" => "XAI_API_KEY",
        "zai" | "z.ai" => "ZAI_API_KEY",
        "minimax" | "minimax-intl" | "minimax-io" | "minimax-global" => "MINIMAX_API_KEY",
        "minimax-cn" | "minimaxi" => "MINIMAX_CN_API_KEY",
        "opencode" | "opencode-zen" => "OPENCODE_API_KEY",
        "kimi-code" | "kimi_coding" | "kimi_for_coding" => "KIMI_API_KEY",
        "copilot" | "github-copilot" => "GITHUB_TOKEN",
        // Providers without a pi.dev-native env var — the key is passed
        // through via models.json custom provider instead.
        "deepseek" => "DEEPSEEK_API_KEY",
        "together" | "together-ai" => "TOGETHER_API_KEY",
        "fireworks" | "fireworks-ai" => "FIREWORKS_API_KEY",
        "perplexity" => "PERPLEXITY_API_KEY",
        "cohere" => "COHERE_API_KEY",
        "nvidia" | "nvidia-nim" => "NVIDIA_API_KEY",
        "hunyuan" | "tencent" => "HUNYUAN_API_KEY",
        "moonshot" => "MOONSHOT_API_KEY",
        "telnyx" => "TELNYX_API_KEY",
        _ => "OPENAI_API_KEY",
    }
}

/// Map a ZeroClaw provider name to the provider identifier that pi.dev
/// recognises in its `defaultProvider` setting.
///
/// Providers that pi.dev knows natively map to their built-in name.
/// All others map to a stable custom-provider key used in `models.json`.
fn provider_to_pi_provider(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        // pi.dev built-in providers
        "anthropic" => "anthropic",
        "openai" => "openai",
        "openrouter" => "openrouter",
        "gemini" | "google" => "google",
        "groq" => "groq",
        "cerebras" => "cerebras",
        "mistral" => "mistral",
        "xai" | "grok" => "xai",
        "zai" | "z.ai" => "zai",
        "minimax" | "minimax-intl" | "minimax-io" | "minimax-global" => "minimax",
        "minimax-cn" | "minimaxi" => "minimax-cn",
        "opencode" | "opencode-zen" => "opencode",
        "kimi-code" | "kimi_coding" | "kimi_for_coding" => "kimi-coding",
        "copilot" | "github-copilot" => "github-copilot",
        "bedrock" | "aws-bedrock" => "amazon-bedrock",
        "ollama" => "ollama",
        // ZeroClaw providers that need custom-provider config in models.json
        "deepseek" => "deepseek",
        "together" | "together-ai" => "together",
        "fireworks" | "fireworks-ai" => "fireworks",
        "perplexity" => "perplexity",
        "cohere" => "cohere",
        "nvidia" | "nvidia-nim" => "nvidia",
        "hunyuan" | "tencent" => "hunyuan",
        "moonshot" => "moonshot",
        "telnyx" => "telnyx",
        "lmstudio" | "lm-studio" => "lmstudio",
        "llamacpp" | "llama.cpp" => "llamacpp",
        "sglang" => "sglang",
        "vllm" => "vllm",
        _ => "openai",
    }
}

/// Return the pi.dev API type string for `models.json` configuration.
///
/// Reference: <https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/models.md>
fn provider_to_pi_api(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "anthropic" => "anthropic-messages",
        "gemini" | "google" => "google-generative-ai",
        _ => "openai-completions",
    }
}

/// Return `true` if pi.dev has this provider built-in (i.e. it doesn't need
/// a custom-provider entry in `models.json` just to be usable).
fn is_pi_native_provider(provider: &str) -> bool {
    matches!(
        provider.to_lowercase().as_str(),
        "anthropic"
            | "openai"
            | "openrouter"
            | "gemini"
            | "google"
            | "groq"
            | "cerebras"
            | "mistral"
            | "xai"
            | "grok"
            | "zai"
            | "z.ai"
            | "minimax"
            | "minimax-cn"
            | "opencode"
            | "opencode-zen"
            | "kimi-code"
            | "kimi_coding"
            | "kimi_for_coding"
            | "copilot"
            | "github-copilot"
            | "bedrock"
            | "aws-bedrock"
            | "ollama"
    )
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

        // Write project-level .pi/settings.json and (when api_url is set)
        // .pi/models.json so pi uses the same provider, model, and endpoint
        // as ZeroClaw.
        //
        // pi resolves custom base URLs via models.json, not environment
        // variables. See:
        //   https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/models.md
        //   https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/providers.md
        let pi_settings_dir = work_dir.join(".pi");
        let pi_settings_path = pi_settings_dir.join("settings.json");
        let pi_models_path = pi_settings_dir.join("models.json");
        let wrote_settings = if !self.provider.is_empty() || !self.model.is_empty() {
            let mut settings = serde_json::Map::new();
            let pi_provider = provider_to_pi_provider(&self.provider);
            if !self.provider.is_empty() {
                settings.insert(
                    "defaultProvider".into(),
                    serde_json::Value::String(pi_provider.into()),
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

        // Write a models.json when:
        //  1. A custom api_url is set (overrides baseUrl for any provider), or
        //  2. The provider is not pi.dev-native and needs a custom-provider
        //     entry so pi knows how to reach it.
        let needs_models_json = !self.provider.is_empty()
            && (self.api_url.as_ref().is_some_and(|u| !u.is_empty())
                || !is_pi_native_provider(&self.provider));
        let wrote_models = if needs_models_json {
            let pi_provider = provider_to_pi_provider(&self.provider);
            let pi_api = provider_to_pi_api(&self.provider);
            let env_key = provider_to_api_key_env(&self.provider);

            let mut provider_obj = serde_json::Map::new();
            if let Some(ref url) = self.api_url {
                if !url.is_empty() {
                    provider_obj
                        .insert("baseUrl".into(), serde_json::Value::String(url.clone()));
                }
            }
            provider_obj.insert(
                "api".into(),
                serde_json::Value::String(pi_api.into()),
            );
            // Tell pi which env var holds the API key for this custom provider.
            provider_obj.insert(
                "apiKey".into(),
                serde_json::Value::String(env_key.into()),
            );
            if !self.model.is_empty() {
                provider_obj.insert(
                    "models".into(),
                    json!([{ "id": self.model }]),
                );
            }

            let models_json = json!({ "providers": { pi_provider: provider_obj } });
            match tokio::fs::create_dir_all(&pi_settings_dir).await {
                Ok(()) => {
                    let json_bytes =
                        serde_json::to_vec_pretty(&models_json).unwrap_or_default();
                    tokio::fs::write(&pi_models_path, json_bytes)
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
            api_url = %self.api_url.as_deref().unwrap_or("<default>"),
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
            }
        }

        // Execute with timeout.
        let result =
            tokio::time::timeout(Duration::from_secs(PI_AGENT_TIMEOUT_SECS), cmd.output()).await;

        // Clean up the project-level settings/models we wrote (best-effort).
        if wrote_models {
            let _ = tokio::fs::remove_file(&pi_models_path).await;
        }
        if wrote_settings {
            let _ = tokio::fs::remove_file(&pi_settings_path).await;
        }
        if wrote_settings || wrote_models {
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
        assert_eq!(provider_to_api_key_env("openrouter"), "OPENROUTER_API_KEY");
        assert_eq!(provider_to_api_key_env("gemini"), "GEMINI_API_KEY");
        assert_eq!(provider_to_api_key_env("groq"), "GROQ_API_KEY");
        assert_eq!(provider_to_api_key_env("cerebras"), "CEREBRAS_API_KEY");
        assert_eq!(provider_to_api_key_env("mistral"), "MISTRAL_API_KEY");
        assert_eq!(provider_to_api_key_env("xai"), "XAI_API_KEY");
        assert_eq!(provider_to_api_key_env("grok"), "XAI_API_KEY");
        assert_eq!(provider_to_api_key_env("zai"), "ZAI_API_KEY");
        assert_eq!(provider_to_api_key_env("minimax"), "MINIMAX_API_KEY");
        assert_eq!(provider_to_api_key_env("minimax-cn"), "MINIMAX_CN_API_KEY");
        assert_eq!(provider_to_api_key_env("opencode"), "OPENCODE_API_KEY");
        assert_eq!(provider_to_api_key_env("kimi-code"), "KIMI_API_KEY");
        assert_eq!(provider_to_api_key_env("copilot"), "GITHUB_TOKEN");
        assert_eq!(provider_to_api_key_env("github-copilot"), "GITHUB_TOKEN");
        assert_eq!(provider_to_api_key_env("deepseek"), "DEEPSEEK_API_KEY");
        assert_eq!(provider_to_api_key_env("together"), "TOGETHER_API_KEY");
        assert_eq!(provider_to_api_key_env("fireworks"), "FIREWORKS_API_KEY");
        assert_eq!(provider_to_api_key_env("perplexity"), "PERPLEXITY_API_KEY");
        assert_eq!(provider_to_api_key_env("cohere"), "COHERE_API_KEY");
        assert_eq!(provider_to_api_key_env("nvidia"), "NVIDIA_API_KEY");
        assert_eq!(provider_to_api_key_env("hunyuan"), "HUNYUAN_API_KEY");
        assert_eq!(provider_to_api_key_env("moonshot"), "MOONSHOT_API_KEY");
        assert_eq!(provider_to_api_key_env("telnyx"), "TELNYX_API_KEY");
        assert_eq!(provider_to_api_key_env("unknown"), "OPENAI_API_KEY");
    }

    #[test]
    fn provider_to_pi_api_mapping() {
        assert_eq!(provider_to_pi_api("anthropic"), "anthropic-messages");
        assert_eq!(provider_to_pi_api("gemini"), "google-generative-ai");
        assert_eq!(provider_to_pi_api("google"), "google-generative-ai");
        assert_eq!(provider_to_pi_api("openai"), "openai-completions");
        assert_eq!(provider_to_pi_api("openrouter"), "openai-completions");
        assert_eq!(provider_to_pi_api("ollama"), "openai-completions");
        assert_eq!(provider_to_pi_api("deepseek"), "openai-completions");
    }

    #[test]
    fn provider_to_pi_provider_mapping() {
        assert_eq!(provider_to_pi_provider("anthropic"), "anthropic");
        assert_eq!(provider_to_pi_provider("openrouter"), "openrouter");
        assert_eq!(provider_to_pi_provider("gemini"), "google");
        assert_eq!(provider_to_pi_provider("ollama"), "ollama");
        assert_eq!(provider_to_pi_provider("copilot"), "github-copilot");
        assert_eq!(provider_to_pi_provider("bedrock"), "amazon-bedrock");
        assert_eq!(provider_to_pi_provider("deepseek"), "deepseek");
        assert_eq!(provider_to_pi_provider("together"), "together");
        assert_eq!(provider_to_pi_provider("fireworks"), "fireworks");
        assert_eq!(provider_to_pi_provider("nvidia"), "nvidia");
        assert_eq!(provider_to_pi_provider("lmstudio"), "lmstudio");
        assert_eq!(provider_to_pi_provider("vllm"), "vllm");
        assert_eq!(provider_to_pi_provider("unknown"), "openai");
    }

    #[test]
    fn native_provider_detection() {
        assert!(is_pi_native_provider("anthropic"));
        assert!(is_pi_native_provider("openai"));
        assert!(is_pi_native_provider("openrouter"));
        assert!(is_pi_native_provider("gemini"));
        assert!(is_pi_native_provider("copilot"));
        assert!(is_pi_native_provider("bedrock"));
        assert!(is_pi_native_provider("ollama"));
        assert!(!is_pi_native_provider("deepseek"));
        assert!(!is_pi_native_provider("together"));
        assert!(!is_pi_native_provider("fireworks"));
        assert!(!is_pi_native_provider("nvidia"));
        assert!(!is_pi_native_provider("telnyx"));
        assert!(!is_pi_native_provider("unknown"));
    }
}
