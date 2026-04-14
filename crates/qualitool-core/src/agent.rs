use std::collections::HashMap;

use qualitool_protocol::agent::AgentRequest;
use qualitool_protocol::finding::Finding;

/// Errors that can occur when routing a `CallAgent` effect.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// No agent router was configured but a check emitted `CallAgent`.
    #[error("no agent router configured")]
    NoRouter,

    /// The agent subprocess or network call failed.
    #[error("agent execution failed: {message}")]
    ExecutionFailed {
        message: String,
        #[source]
        source: Option<anyhow::Error>,
    },
}

/// Routes [`CheckOutput::CallAgent`](qualitool_protocol::check::CheckOutput::CallAgent)
/// effects to an external agent implementation.
///
/// Defined in `qualitool-core` so the dependency arrow stays clean:
/// `qualitool-core` never depends on `qualitool-agent`. The concrete
/// implementation that spawns subprocesses lives in a higher crate and
/// is injected via [`SchedulerBuilder::set_agent_router`](crate::scheduler::SchedulerBuilder::set_agent_router).
pub trait AgentRouter: Send + Sync {
    /// Invoke an agent with the given request and relevant probe outputs.
    ///
    /// `probe_outputs` contains only the probes listed in
    /// [`AgentRequest::include_probes`], keyed by probe name.
    /// The router is responsible for constructing the agent input envelope
    /// and parsing the agent's response into [`Finding`]s.
    fn route(
        &self,
        request: &AgentRequest,
        probe_outputs: &HashMap<String, serde_json::Value>,
    ) -> impl std::future::Future<Output = Result<Vec<Finding>, AgentError>> + Send;
}
