use schemars::schema_for;

use crate::agent::{AgentConstraints, AgentRequest, InputMode, OutputMode};
use crate::check::{CheckId, CheckOutput};
use crate::finding::{CodeLocation, Finding, FindingId, Severity};
use crate::jsonrpc::{
    CheckRunParams, CheckRunResult, ExtensionDescribeResult, HostAgentCompleteParams,
    HostAgentCompleteResult, HostLogParams, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    LogLevel, ProbeRunParams, ProbeRunResult,
};
use crate::manifest::{CheckManifest, ProbeManifest};

pub fn generate_schema() -> serde_json::Value {
    let mut defs = serde_json::Map::new();

    let types: Vec<(&str, serde_json::Value)> = vec![
        ("Finding", serde_json::to_value(schema_for!(Finding)).unwrap()),
        ("FindingId", serde_json::to_value(schema_for!(FindingId)).unwrap()),
        ("Severity", serde_json::to_value(schema_for!(Severity)).unwrap()),
        ("CodeLocation", serde_json::to_value(schema_for!(CodeLocation)).unwrap()),
        ("CheckId", serde_json::to_value(schema_for!(CheckId)).unwrap()),
        ("CheckOutput", serde_json::to_value(schema_for!(CheckOutput)).unwrap()),
        ("AgentRequest", serde_json::to_value(schema_for!(AgentRequest)).unwrap()),
        ("AgentConstraints", serde_json::to_value(schema_for!(AgentConstraints)).unwrap()),
        ("InputMode", serde_json::to_value(schema_for!(InputMode)).unwrap()),
        ("OutputMode", serde_json::to_value(schema_for!(OutputMode)).unwrap()),
        ("ProbeManifest", serde_json::to_value(schema_for!(ProbeManifest)).unwrap()),
        ("CheckManifest", serde_json::to_value(schema_for!(CheckManifest)).unwrap()),
        ("JsonRpcRequest", serde_json::to_value(schema_for!(JsonRpcRequest)).unwrap()),
        ("JsonRpcResponse", serde_json::to_value(schema_for!(JsonRpcResponse)).unwrap()),
        ("JsonRpcNotification", serde_json::to_value(schema_for!(JsonRpcNotification)).unwrap()),
        ("ExtensionDescribeResult", serde_json::to_value(schema_for!(ExtensionDescribeResult)).unwrap()),
        ("ProbeRunParams", serde_json::to_value(schema_for!(ProbeRunParams)).unwrap()),
        ("ProbeRunResult", serde_json::to_value(schema_for!(ProbeRunResult)).unwrap()),
        ("CheckRunParams", serde_json::to_value(schema_for!(CheckRunParams)).unwrap()),
        ("CheckRunResult", serde_json::to_value(schema_for!(CheckRunResult)).unwrap()),
        ("HostLogParams", serde_json::to_value(schema_for!(HostLogParams)).unwrap()),
        ("LogLevel", serde_json::to_value(schema_for!(LogLevel)).unwrap()),
        ("HostAgentCompleteParams", serde_json::to_value(schema_for!(HostAgentCompleteParams)).unwrap()),
        ("HostAgentCompleteResult", serde_json::to_value(schema_for!(HostAgentCompleteResult)).unwrap()),
    ];

    for (name, schema) in types {
        defs.insert(name.to_string(), schema);
    }

    serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "Qualitool Protocol",
        "description": "Wire types for the qualitool host-extension protocol",
        "version": crate::PROTOCOL_VERSION,
        "definitions": defs
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json_and_has_all_types() {
        let schema = generate_schema();

        let defs = schema["definitions"].as_object().unwrap();
        assert!(defs.contains_key("Finding"));
        assert!(defs.contains_key("Severity"));
        assert!(defs.contains_key("CodeLocation"));
        assert!(defs.contains_key("CheckOutput"));
        assert!(defs.contains_key("AgentRequest"));
        assert!(defs.contains_key("ProbeManifest"));
        assert!(defs.contains_key("CheckManifest"));
        assert!(defs.contains_key("JsonRpcRequest"));
        assert!(defs.contains_key("JsonRpcResponse"));
        assert!(defs.contains_key("ExtensionDescribeResult"));
        assert!(defs.contains_key("ProbeRunParams"));
        assert!(defs.contains_key("CheckRunParams"));
        assert!(defs.contains_key("HostLogParams"));
        assert!(defs.contains_key("HostAgentCompleteParams"));

        assert_eq!(schema["version"], crate::PROTOCOL_VERSION);
    }

    #[test]
    fn schema_serializes_to_valid_json_string() {
        let schema = generate_schema();
        let json_str = serde_json::to_string_pretty(&schema).unwrap();
        let _: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    }
}
