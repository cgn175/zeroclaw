//! Google A2A Protocol Unit Tests
//!
//! Tests for the Google A2A standard implementation types and structures.

use zeroclaw::channels::a2a::protocol::{
    AgentCard, AgentCapabilities, AuthenticationInfo, CreateTaskRequest, MessageRole, Task,
    TaskMessage, TaskStatus, TaskUpdate,
};

#[test]
fn test_agent_card_creation() {
    let card = AgentCard {
        name: "Test Agent".to_string(),
        description: "A test agent".to_string(),
        version: "1.0.0".to_string(),
        capabilities: AgentCapabilities::default(),
        authentication: AuthenticationInfo::default(),
        endpoints: Default::default(),
        skills: vec![],
    };

    assert_eq!(card.name, "Test Agent");
    assert_eq!(card.version, "1.0.0");
}

#[test]
fn test_task_creation() {
    let task = Task::new("task-123");

    assert_eq!(task.id, "task-123");
    assert_eq!(task.status, TaskStatus::Pending);
    assert!(task.messages.is_empty());
}

#[test]
fn test_task_add_message() {
    let mut task = Task::new("task-123");
    let message = TaskMessage::user("Hello");

    task.add_message(message.clone());

    assert_eq!(task.messages.len(), 1);
    assert_eq!(task.messages[0].content, "Hello");
    assert_eq!(task.messages[0].role, MessageRole::User);
}

#[test]
fn test_create_task_request() {
    let request = CreateTaskRequest::new("Test message");

    assert_eq!(request.message.content, "Test message");
    assert_eq!(request.message.role, MessageRole::User);
    assert!(request.metadata.is_none());
}

#[test]
fn test_create_task_request_with_metadata() {
    let request = CreateTaskRequest::new("Test")
        .with_metadata(serde_json::json!({"key": "value"}));

    assert!(request.metadata.is_some());
    assert_eq!(request.metadata.unwrap()["key"], "value");
}

#[test]
fn test_task_update_status() {
    let update = TaskUpdate::status_update("task-123", TaskStatus::Running);

    assert_eq!(update.task_id, "task-123");
    assert_eq!(update.status, TaskStatus::Running);
    assert!(update.message.is_none());
    assert!(update.artifact.is_none());
}

#[test]
fn test_task_update_message() {
    let message = TaskMessage::agent("Processing...");
    let update = TaskUpdate::message_update("task-123", message);

    assert_eq!(update.task_id, "task-123");
    assert_eq!(update.status, TaskStatus::Running);
    assert!(update.message.is_some());
    assert_eq!(update.message.unwrap().content, "Processing...");
}

#[test]
fn test_task_status_serialization() {
    let status = TaskStatus::Completed;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"completed\"");

    let deserialized: TaskStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, TaskStatus::Completed);
}

#[test]
fn test_message_role_serialization() {
    let role = MessageRole::User;
    let json = serde_json::to_string(&role).unwrap();
    assert_eq!(json, "\"user\"");

    let deserialized: MessageRole = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, MessageRole::User);
}
