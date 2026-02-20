//! A2A (Agent-to-Agent) Protocol Gateway Endpoints
//!
//! This module implements the Google A2A Protocol standard for agent-to-agent communication.
//!
//! # Google A2A Protocol Endpoints
//!
//! - `GET /.well-known/agent.json` - Agent discovery (AgentCard)
//! - `POST /tasks` - Create a new task
//! - `GET /tasks/{id}` - Get task status and result
//! - `GET /tasks/{id}/stream` - SSE stream for task updates
//! - `POST /tasks/{id}/cancel` - Cancel a running task
//!
//! # Security Model
//!
//! - Bearer token authentication for all task endpoints
//! - Peer allowlist controls which agents can communicate
//! - Deny-by-default for unknown peers
//! - TLS recommended for production deployments

use crate::channels::a2a::protocol::{
    AgentCapabilities, AgentCard, AgentEndpoints, AuthenticationInfo, CancelTaskRequest,
    CreateTaskResponse, Task,
};
use crate::config::schema::A2AConfig;
use crate::gateway::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};

/// GET /.well-known/agent.json - Agent discovery endpoint.
pub async fn handle_agent_card(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.lock();
    let a2a_config = config.channels_config.a2a.clone();

    let card = if let Some(a2a) = a2a_config {
        if let Some(agent_card_config) = a2a.agent_card {
            AgentCard {
                name: agent_card_config.name,
                description: agent_card_config.description,
                version: env!("CARGO_PKG_VERSION").to_string(),
                capabilities: AgentCapabilities::default(),
                authentication: AuthenticationInfo::default(),
                endpoints: AgentEndpoints::default(),
                skills: agent_card_config
                    .skills
                    .into_iter()
                    .map(|s| crate::channels::a2a::protocol::Skill {
                        id: s.id,
                        name: s.name,
                        description: s.description,
                        input_schema: None,
                        output_schema: None,
                    })
                    .collect(),
            }
        } else {
            default_agent_card()
        }
    } else {
        default_agent_card()
    };

    tracing::debug!("Agent card requested: {}", card.name);
    Json(card)
}

/// POST /tasks - Create a new task.
pub async fn handle_create_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<
        Json<crate::channels::a2a::protocol::CreateTaskRequest>,
        axum::extract::rejection::JsonRejection,
    >,
) -> impl IntoResponse {
    let a2a_config = match &state.config.lock().channels_config.a2a {
        Some(config) if config.enabled => config.clone(),
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "A2A channel not enabled"})),
            );
        }
    };

    let peer_id = match verify_bearer_token(&headers, &a2a_config) {
        Ok(id) => id,
        Err(status) => {
            return (status, Json(serde_json::json!({"error": "Unauthorized"})));
        }
    };

    let Json(request) = match body {
        Ok(req) => req,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "Invalid JSON body",
                    "details": e.to_string()
                })),
            );
        }
    };

    let task_id = uuid::Uuid::new_v4().to_string();
    let mut task = Task::new(&task_id);
    task.add_message(request.message);

    tracing::info!("A2A task created: {} by peer {}", task_id, peer_id);

    let response = CreateTaskResponse { task };

    (
        StatusCode::CREATED,
        Json(serde_json::to_value(response).unwrap()),
    )
}

/// GET /tasks/{id} - Get task status and result.
pub async fn handle_get_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    let a2a_config = match &state.config.lock().channels_config.a2a {
        Some(config) if config.enabled => config.clone(),
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "A2A channel not enabled"})),
            );
        }
    };

    match verify_bearer_token(&headers, &a2a_config) {
        Ok(_) => {}
        Err(status) => {
            return (status, Json(serde_json::json!({"error": "Unauthorized"})));
        }
    };

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "Task not found",
            "task_id": task_id
        })),
    )
}

/// POST /tasks/{id}/cancel - Cancel a task.
pub async fn handle_cancel_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    _body: Result<Json<CancelTaskRequest>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let a2a_config = match &state.config.lock().channels_config.a2a {
        Some(config) if config.enabled => config.clone(),
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "A2A channel not enabled"})),
            );
        }
    };

    match verify_bearer_token(&headers, &a2a_config) {
        Ok(_) => {}
        Err(status) => {
            return (status, Json(serde_json::json!({"error": "Unauthorized"})));
        }
    };

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "Task not found",
            "task_id": task_id
        })),
    )
}

/// GET /tasks/{id}/stream - SSE stream for task updates.
pub async fn handle_task_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    let a2a_config = match &state.config.lock().channels_config.a2a {
        Some(config) if config.enabled => config.clone(),
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "A2A channel not enabled"})),
            );
        }
    };

    match verify_bearer_token(&headers, &a2a_config) {
        Ok(_) => {}
        Err(status) => {
            return (status, Json(serde_json::json!({"error": "Unauthorized"})));
        }
    };

    if task_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Task ID cannot be empty"})),
        );
    }

    tracing::debug!("A2A task stream requested for task: {}", task_id);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "task_id": task_id,
            "status": "pending",
            "message": "Streaming not yet implemented"
        })),
    )
}

// =============================================================================
// Helper Functions
// =============================================================================

fn default_agent_card() -> AgentCard {
    AgentCard {
        name: "ZeroClaw Agent".to_string(),
        description: "ZeroClaw autonomous agent".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: AgentCapabilities::default(),
        authentication: AuthenticationInfo::default(),
        endpoints: AgentEndpoints::default(),
        skills: vec![],
    }
}

fn is_peer_allowed(config: &A2AConfig, peer_id: &str) -> bool {
    config.allowed_peer_ids.contains(&"*".to_string())
        || config.allowed_peer_ids.contains(&peer_id.to_string())
}

fn verify_bearer_token(headers: &HeaderMap, config: &A2AConfig) -> Result<String, StatusCode> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    for peer in &config.peers {
        if peer.enabled && constant_time_eq(token, &peer.bearer_token) {
            if is_peer_allowed(config, &peer.id) {
                return Ok(peer.id.clone());
            }
            return Err(StatusCode::FORBIDDEN);
        }
    }

    Err(StatusCode::UNAUTHORIZED)
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};

    let hash_a = Sha256::digest(a.as_bytes());
    let hash_b = Sha256::digest(b.as_bytes());

    let mut result = 0u8;
    for (x, y) in hash_a.iter().zip(hash_b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::A2APeerConfig;
    use axum::http::HeaderValue;

    fn create_test_config() -> A2AConfig {
        A2AConfig {
            enabled: true,
            listen_port: 9000,
            discovery_mode: "static".to_string(),
            allowed_peer_ids: vec!["peer-1".to_string(), "peer-2".to_string()],
            peers: vec![
                A2APeerConfig {
                    id: "peer-1".to_string(),
                    endpoint: "https://peer1.example.com".to_string(),
                    bearer_token: "valid-token-1".to_string(),
                    enabled: true,
                },
                A2APeerConfig {
                    id: "peer-2".to_string(),
                    endpoint: "https://peer2.example.com".to_string(),
                    bearer_token: "valid-token-2".to_string(),
                    enabled: true,
                },
                A2APeerConfig {
                    id: "disabled-peer".to_string(),
                    endpoint: "https://disabled.example.com".to_string(),
                    bearer_token: "disabled-token".to_string(),
                    enabled: false,
                },
            ],
            rate_limit: crate::config::schema::A2ARateLimitConfig::default(),
            idempotency: crate::config::schema::A2AIdempotencyConfig::default(),
            reconnect: crate::config::schema::A2AReconnectConfig::default(),
            agent_card: None,
        }
    }

    #[test]
    fn verify_bearer_token_valid() {
        let config = create_test_config();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer valid-token-1"),
        );

        let result = verify_bearer_token(&headers, &config);
        assert_eq!(result.unwrap(), "peer-1");
    }

    #[test]
    fn verify_bearer_token_invalid() {
        let config = create_test_config();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer invalid-token"),
        );

        let result = verify_bearer_token(&headers, &config);
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn verify_bearer_token_missing() {
        let config = create_test_config();
        let headers = HeaderMap::new();

        let result = verify_bearer_token(&headers, &config);
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn verify_bearer_token_disabled_peer() {
        let config = create_test_config();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer disabled-token"),
        );

        let result = verify_bearer_token(&headers, &config);
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn is_peer_allowed_with_wildcard() {
        let mut config = create_test_config();
        config.allowed_peer_ids = vec!["*".to_string()];

        assert!(is_peer_allowed(&config, "any-peer"));
        assert!(is_peer_allowed(&config, "peer-1"));
    }

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq("token-123", "token-123"));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq("token-123", "token-456"));
    }
}
