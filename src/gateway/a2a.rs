//! A2A (Agent-to-Agent) Protocol Gateway Endpoints
//!
//! Implements the Google A2A Protocol for agent-to-agent communication.
//!
//! # Endpoints
//!
//! - `GET /.well-known/agent.json` - Agent discovery (AgentCard)
//! - `POST /tasks` - Create a new task (dispatches to agent, returns immediately)
//! - `GET /tasks/{id}` - Get task status and result
//! - `GET /tasks/{id}/stream` - SSE stream for real-time task updates
//! - `POST /tasks/{id}/cancel` - Cancel a running task

use crate::gateway::AppState;
use crate::config::schema::A2AConfig;
use zeroclaw_a2a::protocol::{
    AgentCapabilities, AgentCard, AgentEndpoints, AuthenticationInfo, CancelTaskRequest,
    CreateTaskRequest, CreateTaskResponse, Skill, Task, TaskMessage, TaskStatus, TaskUpdate,
};
use zeroclaw_a2a::TaskEntry as A2ATaskEntry;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json,
    },
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

// =============================================================================
// Task Store
// =============================================================================

/// A stored task entry with broadcast channel for SSE streaming.
pub struct TaskEntry {
    pub task: Task,
    pub update_tx: tokio::sync::broadcast::Sender<TaskUpdate>,
    pub join_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Shared task store for A2A tasks.
#[derive(Clone, Default)]
pub struct TaskStore(Arc<Mutex<HashMap<String, TaskEntry>>>);

impl TaskStore {
    pub fn insert(&self, id: String, entry: TaskEntry) {
        self.0.lock().insert(id, entry);
    }

    pub fn get_task(&self, id: &str) -> Option<Task> {
        self.0.lock().get(id).map(|e| e.task.clone())
    }

    pub fn subscribe(
        &self,
        id: &str,
    ) -> Option<(Task, tokio::sync::broadcast::Receiver<TaskUpdate>)> {
        let store = self.0.lock();
        store.get(id).map(|e| (e.task.clone(), e.update_tx.subscribe()))
    }

    pub fn update_task(&self, id: &str, f: impl FnOnce(&mut Task)) {
        if let Some(entry) = self.0.lock().get_mut(id) {
            f(&mut entry.task);
        }
    }

    pub fn cancel(&self, id: &str) -> Option<TaskUpdate> {
        let mut store = self.0.lock();
        let entry = store.get_mut(id)?;
        match entry.task.status {
            TaskStatus::Pending | TaskStatus::Running => {
                entry.task.set_status(TaskStatus::Cancelled);
                if let Some(handle) = entry.join_handle.take() {
                    handle.abort();
                }
                let update = TaskUpdate::status_update(id, TaskStatus::Cancelled);
                let _ = entry.update_tx.send(update.clone());
                Some(update)
            }
            _ => None, // Already in terminal state
        }
    }
}

// =============================================================================
// Handlers
// =============================================================================

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
                    .map(|s| Skill {
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

/// POST /tasks - Create a new task and dispatch to agent.
pub async fn handle_create_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<
        Json<CreateTaskRequest>,
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
    task.add_message(request.message.clone());

    let (update_tx, _) = tokio::sync::broadcast::channel::<TaskUpdate>(64);

    // Clone what we need for the background task
    let bg_state = state.clone();
    let bg_task_id = task_id.clone();
    let bg_update_tx = update_tx.clone();
    let message_content = request.message.content.clone();

    let join_handle = tokio::spawn(async move {
        // Mark as running
        bg_state.a2a_tasks.update_task(&bg_task_id, |t| {
            t.set_status(TaskStatus::Running);
        });
        let _ = bg_update_tx.send(TaskUpdate::status_update(&bg_task_id, TaskStatus::Running));

        // Run the agent loop
        match crate::gateway::run_gateway_chat_with_tools(&bg_state, &message_content).await {
            Ok(response) => {
                let agent_msg = TaskMessage::agent(&response);
                bg_state.a2a_tasks.update_task(&bg_task_id, |t| {
                    t.add_message(agent_msg.clone());
                    t.set_status(TaskStatus::Completed);
                });
                let _ =
                    bg_update_tx.send(TaskUpdate::message_update(&bg_task_id, agent_msg));
                let _ = bg_update_tx
                    .send(TaskUpdate::status_update(&bg_task_id, TaskStatus::Completed));
            }
            Err(e) => {
                let error_msg = TaskMessage::agent(format!("Error: {e}"));
                bg_state.a2a_tasks.update_task(&bg_task_id, |t| {
                    t.add_message(error_msg.clone());
                    t.set_status(TaskStatus::Failed);
                });
                let _ =
                    bg_update_tx.send(TaskUpdate::message_update(&bg_task_id, error_msg));
                let _ = bg_update_tx
                    .send(TaskUpdate::status_update(&bg_task_id, TaskStatus::Failed));
            }
        }
    });

    let entry = A2ATaskEntry {
        task: task.clone(),
        update_tx,
        join_handle: Some(join_handle),
    };
    state.a2a_tasks.insert(task_id.clone(), entry);

    tracing::info!(
        "A2A task created and dispatched: {} by peer {}",
        task_id,
        peer_id
    );

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

    if let Err(status) = verify_bearer_token(&headers, &a2a_config) {
        return (status, Json(serde_json::json!({"error": "Unauthorized"})));
    }

    match state.a2a_tasks.get_task(&task_id) {
        Some(task) => (StatusCode::OK, Json(serde_json::to_value(task).unwrap())),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Task not found",
                "task_id": task_id
            })),
        ),
    }
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
            )
                .into_response();
        }
    };

    if let Err(status) = verify_bearer_token(&headers, &a2a_config) {
        return (status, Json(serde_json::json!({"error": "Unauthorized"}))).into_response();
    }

    let (current_task, rx) = match state.a2a_tasks.subscribe(&task_id) {
        Some(pair) => pair,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Task not found",
                    "task_id": task_id
                })),
            )
                .into_response();
        }
    };

    // Build SSE stream: first emit current state, then stream updates
    let initial_event = TaskUpdate::status_update(&task_id, current_task.status.clone());
    let initial = tokio_stream::once(Ok::<_, Infallible>(
        Event::default()
            .event("status")
            .data(serde_json::to_string(&initial_event).unwrap()),
    ));

    let is_terminal = matches!(
        current_task.status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
    );

    if is_terminal {
        // Task already done — send current state and close
        return Sse::new(initial)
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    // Stream live updates until the broadcast sender is dropped (background task completes)
    let updates = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(update) => {
            let event_type = if update.message.is_some() {
                "message"
            } else {
                "status"
            };
            Some(Ok::<_, Infallible>(
                Event::default()
                    .event(event_type)
                    .data(serde_json::to_string(&update).unwrap()),
            ))
        }
        Err(_) => None,
    });

    let stream = initial.chain(updates);

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
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

    if let Err(status) = verify_bearer_token(&headers, &a2a_config) {
        return (status, Json(serde_json::json!({"error": "Unauthorized"})));
    }

    match state.a2a_tasks.cancel(&task_id) {
        Some(_) => {
            tracing::info!("A2A task cancelled: {}", task_id);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "task_id": task_id,
                    "status": "cancelled"
                })),
            )
        }
        None => {
            // Check if task exists but is already terminal
            match state.a2a_tasks.get_task(&task_id) {
                Some(task) => (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "Task already in terminal state",
                        "task_id": task_id,
                        "status": task.status
                    })),
                ),
                None => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "Task not found",
                        "task_id": task_id
                    })),
                ),
            }
        }
    }
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
    use zeroclaw_a2a::A2APeer;
    use axum::http::HeaderValue;

    fn create_test_config() -> A2AConfig {
        A2AConfig {
            enabled: true,
            listen_port: 9000,
            discovery_mode: "static".to_string(),
            allowed_peer_ids: vec!["peer-1".to_string(), "peer-2".to_string()],
            peers: vec![
                A2APeer {
                    id: "peer-1".to_string(),
                    endpoint: "https://peer1.example.com".to_string(),
                    bearer_token: "valid-token-1".to_string(),
                    enabled: true,
                },
                A2APeer {
                    id: "peer-2".to_string(),
                    endpoint: "https://peer2.example.com".to_string(),
                    bearer_token: "valid-token-2".to_string(),
                    enabled: true,
                },
                A2APeer {
                    id: "disabled-peer".to_string(),
                    endpoint: "https://disabled.example.com".to_string(),
                    bearer_token: "disabled-token".to_string(),
                    enabled: false,
                },
            ],
            rate_limit: zeroclaw_a2a::A2ARateLimitConfig::default(),
            idempotency: zeroclaw_a2a::A2AIdempotencyConfig::default(),
            reconnect: zeroclaw_a2a::A2AReconnectConfig::default(),
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
