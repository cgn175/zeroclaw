//! A2A Configuration Types
//!
//! This module re-exports configuration types from [`crate::protocol`] for
//! convenient access. All types are defined in the protocol module to keep
//! protocol and config serialization in one place.
//!
//! # Types
//!
//! - [`A2AConfig`] - Main A2A channel configuration
//! - [`A2APeer`] - Peer configuration
//! - [`A2ARateLimitConfig`] - Rate limiting settings
//! - [`A2AIdempotencyConfig`] - Idempotency settings
//! - [`A2AReconnectConfig`] - Reconnection settings
//! - [`AgentCardConfig`] - Agent card configuration
//! - [`SkillConfig`] - Skill configuration

pub use crate::protocol::{
    A2AConfig, A2AIdempotencyConfig, A2APeer, A2ARateLimitConfig, A2AReconnectConfig,
    AgentCardConfig, SkillConfig,
};
