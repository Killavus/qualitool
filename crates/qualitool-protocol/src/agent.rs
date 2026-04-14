use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_hint: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub include_probes: Vec<String>,
    pub response_schema: serde_json::Value,
    pub constraints: AgentConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentConstraints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum InputMode {
    StdinJson,
    StdinPrompt,
    PromptFile,
    ArgsPrompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputMode {
    StdoutJson,
    StdoutJsonExtract,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_request_round_trip() {
        let req = AgentRequest {
            agent_hint: Some("default".into()),
            prompt: "Analyze architectural smells".into(),
            include_probes: vec!["git-history".into(), "dependency-graph".into()],
            response_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "smells": {"type": "array"}
                }
            }),
            constraints: AgentConstraints {
                max_tokens: Some(8000),
                timeout_seconds: Some(120),
                read_only: true,
            },
        };

        let json = serde_json::to_string(&req).unwrap();
        let deserialized: AgentRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, deserialized);
    }

    #[test]
    fn agent_request_without_hint() {
        let req = AgentRequest {
            agent_hint: None,
            prompt: "test".into(),
            include_probes: vec![],
            response_schema: serde_json::json!({}),
            constraints: AgentConstraints {
                max_tokens: None,
                timeout_seconds: None,
                read_only: false,
            },
        };

        let json = serde_json::to_string(&req).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("agent_hint").is_none());

        let deserialized: AgentRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, deserialized);
    }

    #[test]
    fn input_mode_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&InputMode::StdinJson).unwrap(),
            "\"stdin-json\""
        );
        assert_eq!(
            serde_json::to_string(&InputMode::PromptFile).unwrap(),
            "\"prompt-file\""
        );
        assert_eq!(
            serde_json::to_string(&InputMode::StdinPrompt).unwrap(),
            "\"stdin-prompt\""
        );
        assert_eq!(
            serde_json::to_string(&InputMode::ArgsPrompt).unwrap(),
            "\"args-prompt\""
        );
    }

    #[test]
    fn output_mode_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&OutputMode::StdoutJson).unwrap(),
            "\"stdout-json\""
        );
        assert_eq!(
            serde_json::to_string(&OutputMode::StdoutJsonExtract).unwrap(),
            "\"stdout-json-extract\""
        );
    }
}
