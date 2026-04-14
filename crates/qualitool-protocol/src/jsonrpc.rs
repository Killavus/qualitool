use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::check::CheckOutput;
use crate::manifest::{CheckManifest, ProbeManifest};
use crate::PROTOCOL_VERSION;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    pub id: JsonRpcId,
}

impl JsonRpcRequest {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>, id: JsonRpcId) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
            id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: JsonRpcId,
}

impl JsonRpcResponse {
    pub fn success(id: JsonRpcId, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(id: JsonRpcId, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            result: None,
            error: Some(error),
            id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// Standard JSON-RPC error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

// --- Method payload types ---

// extension.describe response
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExtensionDescribeResult {
    pub protocol_version: String,
    #[serde(default)]
    pub probes: Vec<ProbeManifest>,
    #[serde(default)]
    pub checks: Vec<CheckManifest>,
}

impl ExtensionDescribeResult {
    pub fn new(probes: Vec<ProbeManifest>, checks: Vec<CheckManifest>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.into(),
            probes,
            checks,
        }
    }
}

// probe.run request params
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProbeRunParams {
    pub probe_name: String,
    pub project_root: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub dependency_outputs: HashMap<String, serde_json::Value>,
}

// probe.run response result
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProbeRunResult {
    pub probe_name: String,
    pub output: serde_json::Value,
}

// check.run request params
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CheckRunParams {
    pub check_name: String,
    pub project_root: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub probe_outputs: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub check_outputs: HashMap<String, serde_json::Value>,
}

// check.run response result
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CheckRunResult {
    pub check_name: String,
    pub output: CheckOutput,
}

// host.log notification params
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostLogParams {
    pub level: LogLevel,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

// host.agent.complete request params
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostAgentCompleteParams {
    pub check_id: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_hint: Option<String>,
    #[serde(default)]
    pub probe_data: HashMap<String, serde_json::Value>,
    pub response_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub read_only: bool,
}

// host.agent.complete response result
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostAgentCompleteResult {
    pub response: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ProbeManifest;

    #[test]
    fn jsonrpc_request_round_trip() {
        let req = JsonRpcRequest::new(
            "probe.run",
            Some(serde_json::json!({"probe_name": "file-tree"})),
            JsonRpcId::Number(1),
        );

        let json = serde_json::to_string(&req).unwrap();
        let deserialized: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, deserialized);
        assert_eq!(deserialized.jsonrpc, "2.0");
    }

    #[test]
    fn jsonrpc_response_success_round_trip() {
        let resp = JsonRpcResponse::success(
            JsonRpcId::Number(1),
            serde_json::json!({"files": ["a.rs", "b.rs"]}),
        );

        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: JsonRpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, deserialized);
        assert!(deserialized.error.is_none());
    }

    #[test]
    fn jsonrpc_response_error_round_trip() {
        let resp = JsonRpcResponse::error(
            JsonRpcId::String("req-1".into()),
            JsonRpcError {
                code: METHOD_NOT_FOUND,
                message: "Method not found".into(),
                data: None,
            },
        );

        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: JsonRpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, deserialized);
        assert!(deserialized.result.is_none());
        assert_eq!(deserialized.error.as_ref().unwrap().code, -32601);
    }

    #[test]
    fn jsonrpc_id_number_and_string() {
        let num = JsonRpcId::Number(42);
        assert_eq!(serde_json::to_string(&num).unwrap(), "42");

        let s = JsonRpcId::String("abc".into());
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"abc\"");
    }

    #[test]
    fn jsonrpc_notification_round_trip() {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: "host.log".into(),
            params: Some(serde_json::json!({"level": "info", "message": "hello"})),
        };

        let json = serde_json::to_string(&notif).unwrap();
        let deserialized: JsonRpcNotification = serde_json::from_str(&json).unwrap();
        assert_eq!(notif, deserialized);
    }

    #[test]
    fn extension_describe_result_round_trip() {
        let result = ExtensionDescribeResult::new(
            vec![ProbeManifest {
                name: "file-tree".into(),
                version: "0.1.0".into(),
                description: None,
                input_schema: None,
                output_schema: None,
                dependencies: vec![],
                contains_source_code: false,
            }],
            vec![],
        );

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ExtensionDescribeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, deserialized);
        assert_eq!(deserialized.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn probe_run_params_round_trip() {
        let params = ProbeRunParams {
            probe_name: "file-tree".into(),
            project_root: "/tmp/project".into(),
            config: serde_json::json!({"includes": ["*.rs"]}),
            dependency_outputs: HashMap::new(),
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: ProbeRunParams = serde_json::from_str(&json).unwrap();
        assert_eq!(params, deserialized);
    }

    #[test]
    fn check_run_params_round_trip() {
        let mut probe_outputs = HashMap::new();
        probe_outputs.insert(
            "file-tree".into(),
            serde_json::json!({"files": ["a.rs"]}),
        );

        let params = CheckRunParams {
            check_name: "file-count".into(),
            project_root: "/tmp/project".into(),
            config: serde_json::json!({"threshold": 1000}),
            probe_outputs,
            check_outputs: HashMap::new(),
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: CheckRunParams = serde_json::from_str(&json).unwrap();
        assert_eq!(params, deserialized);
    }

    #[test]
    fn host_log_params_round_trip() {
        let params = HostLogParams {
            level: LogLevel::Warn,
            message: "Extension timeout approaching".into(),
            data: Some(serde_json::json!({"elapsed_ms": 55000})),
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: HostLogParams = serde_json::from_str(&json).unwrap();
        assert_eq!(params, deserialized);
    }

    #[test]
    fn host_agent_complete_round_trip() {
        let params = HostAgentCompleteParams {
            check_id: "arch-smell".into(),
            prompt: "Analyze".into(),
            agent_hint: None,
            probe_data: HashMap::new(),
            response_schema: serde_json::json!({"type": "object"}),
            max_tokens: Some(4000),
            timeout_seconds: None,
            read_only: true,
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: HostAgentCompleteParams = serde_json::from_str(&json).unwrap();
        assert_eq!(params, deserialized);

        let result = HostAgentCompleteResult {
            response: serde_json::json!({"findings": []}),
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: HostAgentCompleteResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, deserialized);
    }

    #[test]
    fn log_level_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&LogLevel::Trace).unwrap(),
            "\"trace\""
        );
        assert_eq!(
            serde_json::to_string(&LogLevel::Error).unwrap(),
            "\"error\""
        );
    }
}
