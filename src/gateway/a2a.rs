//! A2A Gateway Adapter
//!
//! Thin adapter that wires the generic `zeroclaw_a2a::gateway` implementation
//! into the main gateway by providing a concrete [`AgentDispatcher`].

use crate::gateway::AppState;
use axum::Router;
use zeroclaw_a2a::{build_a2a_routes, A2AGatewayState, AgentDispatcher};

/// Dispatcher that delegates to the main agent loop.
#[derive(Clone)]
struct GatewayDispatcher {
    state: AppState,
}

#[async_trait::async_trait]
impl AgentDispatcher for GatewayDispatcher {
    async fn dispatch(&self, message: &str) -> anyhow::Result<String> {
        crate::gateway::run_gateway_chat_with_tools(&self.state, message).await
    }
}

/// Build A2A protocol routes backed by the main gateway's agent loop.
///
/// Returns `None` if A2A is not configured or not enabled.
pub fn build_a2a_router(state: &AppState) -> Option<Router> {
    let config = state.config.lock();
    let a2a_config = config.channels_config.a2a.clone()?;
    if !a2a_config.enabled {
        return None;
    }

    let dispatcher = GatewayDispatcher {
        state: state.clone(),
    };

    let gateway_state = A2AGatewayState::new(a2a_config, dispatcher)
        .with_version(env!("CARGO_PKG_VERSION"));

    Some(build_a2a_routes(gateway_state))
}
