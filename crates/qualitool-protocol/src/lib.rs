pub const PROTOCOL_VERSION: &str = "0.1.0";

pub mod finding;
pub mod check;
pub mod agent;
pub mod manifest;
pub mod jsonrpc;
pub mod schema;

pub use finding::{CodeLocation, Finding, FindingId, Severity};
pub use check::{CheckId, CheckOutput};
pub use agent::{AgentConstraints, AgentRequest, InputMode, OutputMode};
pub use manifest::{CheckManifest, ProbeManifest};
pub use jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
