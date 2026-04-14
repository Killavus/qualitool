use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::AgentRequest;
use crate::finding::Finding;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CheckId(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CheckOutput {
    Findings { findings: Vec<Finding> },
    CallAgent { request: AgentRequest },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentConstraints;
    use crate::finding::{FindingId, Severity};

    #[test]
    fn check_output_findings_round_trip() {
        let output = CheckOutput::Findings {
            findings: vec![Finding {
                id: FindingId("f1".into()),
                check_id: "file-count".into(),
                severity: Severity::Low,
                title: "Few files".into(),
                summary: "Only 10 files".into(),
                location: None,
                tags: vec![],
                payload: serde_json::json!({"count": 10}),
            }],
        };

        let json = serde_json::to_string(&output).unwrap();
        let deserialized: CheckOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, deserialized);

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "findings");
    }

    #[test]
    fn check_output_call_agent_round_trip() {
        let output = CheckOutput::CallAgent {
            request: AgentRequest {
                agent_hint: Some("fast".into()),
                prompt: "Analyze this".into(),
                include_probes: vec!["git-history".into()],
                response_schema: serde_json::json!({"type": "object"}),
                constraints: AgentConstraints {
                    max_tokens: Some(8000),
                    timeout_seconds: None,
                    read_only: true,
                },
            },
        };

        let json = serde_json::to_string(&output).unwrap();
        let deserialized: CheckOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, deserialized);

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "call_agent");
    }
}
